//! 查我的世界服务器的 SRV 记录（`_minecraft._tcp.<域名>`），和游戏自己一样：
//! 只在地址里没写端口时查，查到就连记录里的主机和端口。
//!
//! 问的是电脑自己设置的 DNS 服务器（Windows 从注册表读，别的系统读 `/etc/resolv.conf`），
//! 不问第三方。按 RFC 1035 的公开格式自己拼一个最小的 DNS 查询，够读 SRV 就行。
//! 读到的 DNS 服务器里可能有断开的网卡留下的旧地址：一起问，谁先答用谁。

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::task::JoinSet;

/// 整个 SRV 查询最多等多久（查不到就按默认端口连，不能让状态一直转圈）
const TIMEOUT: Duration = Duration::from_millis(1500);
/// 最多同时问几个 DNS 服务器
const MAX_SERVERS: usize = 4;
const TYPE_SRV: u16 = 33;

/// SRV 记录指向的地方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvTarget {
    pub host: String,
    pub port: u16,
}

/// 查 `_minecraft._tcp.<host>`。没有记录、查不到、超时都返回 `None`（按默认端口连）。
pub async fn lookup(host: &str) -> Option<SrvTarget> {
    let servers = dns_servers();
    if servers.is_empty() {
        tracing::debug!("没读到 DNS 服务器，不查 SRV");
        return None;
    }
    let name = format!("_minecraft._tcp.{}", host.trim_end_matches('.'));
    let id = query_id();
    let query = build_query(id, &name)?;
    let mut tasks = JoinSet::new();
    for server in servers.into_iter().take(MAX_SERVERS) {
        let query = query.clone();
        tasks.spawn(async move { ask(server, &query, id).await });
    }
    let found = tokio::time::timeout(TIMEOUT, async {
        while let Some(done) = tasks.join_next().await {
            // 有一个服务器明确答了（有或者没有）就用它
            if let Ok(Some(records)) = done {
                return Some(records);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()?;
    pick(found)
}

/// 问一个 DNS 服务器。没答、答的不对返回 `None`；答了「没有」返回空列表。
async fn ask(server: IpAddr, query: &[u8], id: u16) -> Option<Vec<Srv>> {
    let bind: SocketAddr = if server.is_ipv4() {
        "0.0.0.0:0".parse().ok()?
    } else {
        "[::]:0".parse().ok()?
    };
    let socket = UdpSocket::bind(bind).await.ok()?;
    socket.connect(SocketAddr::new(server, 53)).await.ok()?;
    socket.send(query).await.ok()?;
    let mut buf = [0u8; 1500];
    // 同一个口子上可能收到别的（迟到的）包：对不上编号的丢掉再等
    for _ in 0..4 {
        let len = socket.recv(&mut buf).await.ok()?;
        if let Some(records) = parse_response(id, &buf[..len]) {
            return Some(records);
        }
    }
    None
}

fn query_id() -> u16 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    u16::try_from(nanos % 65_536).unwrap_or(0x5a5a)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Srv {
    priority: u16,
    weight: u16,
    port: u16,
    target: String,
}

/// 优先级数字小的先用，同样优先级的挑权重大的；目标是「.」表示这个域名不提供服务。
fn pick(mut records: Vec<Srv>) -> Option<SrvTarget> {
    records.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.weight.cmp(&a.weight)));
    let best = records.into_iter().next()?;
    let host = best.target.trim_end_matches('.').to_owned();
    (!host.is_empty() && best.port != 0).then_some(SrvTarget {
        host,
        port: best.port,
    })
}

/// 拼一个「请递归查 SRV」的查询。域名不是 ASCII、标签太长时返回 `None`。
fn build_query(id: u16, name: &str) -> Option<Vec<u8>> {
    if !name.is_ascii() || name.len() > 253 {
        return None;
    }
    let mut out = Vec::with_capacity(name.len() + 18);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00]); // 只开「要递归」
    out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // 一个问题
    for label in name.trim_end_matches('.').split('.') {
        let len = u8::try_from(label.len())
            .ok()
            .filter(|&n| (1..=63).contains(&n))?;
        out.push(len);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&TYPE_SRV.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // IN
    Some(out)
}

fn u16_at(msg: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*msg.get(pos)?, *msg.get(pos + 1)?]))
}

/// 跳过一个名字（可能是压缩指针），返回名字后面的位置。
fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *msg.get(pos)?;
        match len {
            0 => return Some(pos + 1),
            l if l & 0xC0 == 0xC0 => return Some(pos + 2),
            l if l & 0xC0 != 0 => return None,
            l => pos += 1 + usize::from(l),
        }
    }
}

/// 读一个名字（跟着压缩指针走，最多跳 16 次防止死循环）。
fn read_name(msg: &[u8], mut pos: usize) -> Option<String> {
    let mut labels: Vec<String> = Vec::new();
    for _ in 0..64 {
        let len = *msg.get(pos)?;
        if len == 0 {
            return Some(labels.join("."));
        }
        if len & 0xC0 == 0xC0 {
            let pointer = usize::from(u16_at(msg, pos)? & 0x3FFF);
            if pointer >= pos {
                return None; // 只许往前指
            }
            pos = pointer;
            continue;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let start = pos + 1;
        let label = msg.get(start..start + usize::from(len))?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        pos = start + usize::from(len);
    }
    None
}

/// 读回答：编号对不上、不是回答时返回 `None`；域名不存在、没有记录时返回空列表。
fn parse_response(id: u16, msg: &[u8]) -> Option<Vec<Srv>> {
    if u16_at(msg, 0)? != id {
        return None;
    }
    let flags = u16_at(msg, 2)?;
    if flags & 0x8000 == 0 {
        return None;
    }
    match flags & 0x000F {
        0 => {}
        3 => return Some(Vec::new()), // 没有这个域名
        _ => return None,             // 服务器出错：等别的服务器
    }
    let questions = u16_at(msg, 4)?;
    let answers = u16_at(msg, 6)?;
    let mut pos = 12;
    for _ in 0..questions {
        pos = skip_name(msg, pos)? + 4;
    }
    let mut records = Vec::new();
    for _ in 0..answers {
        pos = skip_name(msg, pos)?;
        let kind = u16_at(msg, pos)?;
        let len = usize::from(u16_at(msg, pos + 8)?);
        let data = pos + 10;
        if data + len > msg.len() {
            return None;
        }
        if kind == TYPE_SRV && len >= 7 {
            records.push(Srv {
                priority: u16_at(msg, data)?,
                weight: u16_at(msg, data + 2)?,
                port: u16_at(msg, data + 4)?,
                target: read_name(msg, data + 6)?,
            });
        }
        pos = data + len;
    }
    Some(records)
}

/// 电脑设置的 DNS 服务器（去重，手动设的在前）。
fn dns_servers() -> Vec<IpAddr> {
    let mut found = Vec::new();
    for text in raw_dns_servers() {
        for part in text.split([',', ' ', ';']) {
            if let Ok(ip) = part.trim().parse::<IpAddr>()
                && !ip.is_unspecified()
                && !found.contains(&ip)
            {
                found.push(ip);
            }
        }
    }
    found
}

#[cfg(windows)]
fn raw_dns_servers() -> Vec<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut manual = Vec::new();
    let mut dhcp = Vec::new();
    for base in [
        r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
        r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters",
    ] {
        let Ok(params) = hklm.open_subkey(base) else {
            continue;
        };
        let mut keys = vec![params];
        if let Ok(interfaces) = hklm.open_subkey(format!(r"{base}\Interfaces")) {
            keys.extend(
                interfaces
                    .enum_keys()
                    .flatten()
                    .filter_map(|name| interfaces.open_subkey(name).ok()),
            );
        }
        for key in keys {
            if let Ok(value) = key.get_value::<String, _>("NameServer")
                && !value.trim().is_empty()
            {
                manual.push(value);
            }
            if let Ok(value) = key.get_value::<String, _>("DhcpNameServer")
                && !value.trim().is_empty()
            {
                dhcp.push(value);
            }
        }
    }
    manual.extend(dhcp);
    manual
}

#[cfg(not(windows))]
fn raw_dns_servers() -> Vec<String> {
    std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .map(|rest| rest.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一个回答：问题照抄，后面两条 SRV（第二条的目标用压缩指针指回问题里的域名）
    fn response(id: u16, rcode: u8) -> Vec<u8> {
        let query = build_query(id, "_minecraft._tcp.example.com").unwrap();
        let mut msg = query.clone();
        msg[2] = 0x81;
        msg[3] = 0x80 | rcode;
        msg[7] = 2; // 两条回答
        let question_name = 12u16;
        for (priority, weight, port, target) in [
            (10u16, 5u16, 25570u16, Some("play.example.net")),
            (5, 1, 25600, None),
        ] {
            msg.extend_from_slice(&(0xC000 | question_name).to_be_bytes());
            msg.extend_from_slice(&TYPE_SRV.to_be_bytes());
            msg.extend_from_slice(&1u16.to_be_bytes());
            msg.extend_from_slice(&300u32.to_be_bytes());
            let mut data = Vec::new();
            data.extend_from_slice(&priority.to_be_bytes());
            data.extend_from_slice(&weight.to_be_bytes());
            data.extend_from_slice(&port.to_be_bytes());
            match target {
                Some(name) => {
                    for label in name.split('.') {
                        data.push(u8::try_from(label.len()).unwrap());
                        data.extend_from_slice(label.as_bytes());
                    }
                    data.push(0);
                }
                // 指向问题里「example.com」那一段（跳过 _minecraft、_tcp 两个标签）
                None => data.extend_from_slice(&(0xC000 | (question_name + 16)).to_be_bytes()),
            }
            msg.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
            msg.extend_from_slice(&data);
        }
        msg
    }

    #[test]
    fn builds_a_plain_srv_query() {
        let query = build_query(0x1234, "_minecraft._tcp.mc.example.com.").unwrap();
        assert_eq!(&query[..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(query[12], 10);
        assert_eq!(&query[13..23], b"_minecraft");
        assert_eq!(&query[query.len() - 4..], &[0, 33, 0, 1]);
        assert!(build_query(1, "中文.example.com").is_none());
        assert!(build_query(1, &format!("{}.com", "a".repeat(64))).is_none());
        assert!(build_query(1, "a..com").is_none());
    }

    #[test]
    fn reads_records_and_picks_the_best() {
        let records = parse_response(7, &response(7, 0)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].target, "play.example.net");
        assert_eq!(records[1].target, "example.com", "压缩指针");
        assert_eq!(
            pick(records),
            Some(SrvTarget {
                host: "example.com".to_owned(),
                port: 25600
            }),
            "优先级数字小的先用"
        );
        assert_eq!(parse_response(8, &response(7, 0)), None, "编号对不上");
        assert_eq!(
            parse_response(7, &response(7, 3)),
            Some(Vec::new()),
            "没有这个域名"
        );
        assert_eq!(parse_response(7, &response(7, 2)), None, "服务器出错等别的");
        let mut cut = response(7, 0);
        cut.truncate(cut.len() - 3);
        assert_eq!(parse_response(7, &cut), None, "被截断的不读");
    }

    #[test]
    fn a_dot_target_means_no_service() {
        let none = Srv {
            priority: 0,
            weight: 0,
            port: 25565,
            target: String::new(),
        };
        assert_eq!(pick(vec![none]), None);
        assert_eq!(pick(Vec::new()), None);
    }

    #[test]
    fn pointer_loops_are_rejected() {
        let msg = [0u8, 0, 0xC0, 2];
        assert_eq!(read_name(&msg, 2), None);
    }
}
