//! 零依赖 RFC 1035 / RFC 8484 DNS 二进制编解码与 Bogon / 污染 IP 过滤。
//!
//! 供 [`super::doh`]（RFC 8484 `application/dns-message`）与 [`super::srv`]（UDP SRV 查询）共用。

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const TYPE_A: u16 = 1;
pub const TYPE_CNAME: u16 = 5;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_SRV: u16 = 33;

const CLASS_IN: u16 = 1;
const MAX_POINTER_JUMPS: usize = 16;
const MAX_LABELS: usize = 128;
const MAX_NAME_LEN: usize = 253;
const MAX_CNAME_HOPS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsWireOutcome {
    Records { ips: Vec<IpAddr>, min_ttl: u32 },
    NoData { nxdomain: bool },
    PoisonedOrBogon,
    Malformed,
    ServerFailure,
}

pub fn u16_at(msg: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*msg.get(pos)?, *msg.get(pos + 1)?]))
}

pub fn u32_at(msg: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *msg.get(pos)?,
        *msg.get(pos + 1)?,
        *msg.get(pos + 2)?,
        *msg.get(pos + 3)?,
    ]))
}

/// 构造单个标准递归 DNS 查询包（RFC 1035 / RFC 8484 §4.1 推荐 `id = 0x0000`）。
pub fn build_query(id: u16, name: &str, qtype: u16) -> Option<Vec<u8>> {
    let trimmed = name.trim_end_matches('.');
    if trimmed.is_empty() || !trimmed.is_ascii() || trimmed.len() > MAX_NAME_LEN {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() + 18);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00]); // QR=0, Opcode=0, RD=1
    out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // QDCOUNT=1, AN=0, NS=0, AR=0
    for label in trimmed.split('.') {
        let len = u8::try_from(label.len())
            .ok()
            .filter(|&n| (1..=63).contains(&n))?;
        out.push(len);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    Some(out)
}

/// 跳过报文中位置 `pos` 处的域名（含压缩指针），返回域名结束后的偏移。
pub fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    for _ in 0..=MAX_LABELS {
        let len = *msg.get(pos)?;
        match len {
            0 => return Some(pos + 1),
            l if l & 0xC0 == 0xC0 => {
                // 确保指针第二个字节也在包内
                let _ = msg.get(pos + 1)?;
                return Some(pos + 2);
            }
            l if l & 0xC0 != 0 => return None,
            l => {
                let next = pos.checked_add(1 + usize::from(l))?;
                if next > msg.len() {
                    return None;
                }
                pos = next;
            }
        }
    }
    None
}

/// 解析 DNS 压缩/非压缩域名：严格只允许向前跳指针（防成环）、限制 16 跳和总长 <= 253。
pub fn read_name(msg: &[u8], mut pos: usize) -> Option<String> {
    let mut labels: Vec<String> = Vec::new();
    let mut jumps = 0usize;
    let mut total_len = 0usize;
    let mut min_pointer_floor = msg.len();

    for _ in 0..MAX_LABELS {
        let len = *msg.get(pos)?;
        if len == 0 {
            return Some(labels.join("."));
        }
        if len & 0xC0 == 0xC0 {
            let pointer = usize::from(u16_at(msg, pos)? & 0x3FFF);
            if pointer >= pos || pointer >= min_pointer_floor {
                return None;
            }
            jumps += 1;
            if jumps > MAX_POINTER_JUMPS {
                return None;
            }
            min_pointer_floor = pointer;
            pos = pointer;
            continue;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let label_len = usize::from(len);
        let start = pos + 1;
        let end = start.checked_add(label_len)?;
        let raw_label = msg.get(start..end)?;
        if !raw_label.is_ascii() {
            return None;
        }
        let added = label_len + usize::from(!labels.is_empty());
        total_len += added;
        if total_len > MAX_NAME_LEN {
            return None;
        }
        labels.push(String::from_utf8_lossy(raw_label).into_owned());
        pos = end;
    }
    None
}

/// 判断公网域名返回的 IP 是否为合法全球单播地址（解包 IPv4-mapped IPv6 后一并检查）。
pub fn is_strictly_global_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_global_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_global_v4(mapped);
            }
            is_global_v6(v6)
        }
    }
}

pub fn is_bogon_or_poisoned(ip: IpAddr) -> bool {
    !is_strictly_global_unicast(ip)
}

fn is_global_v4(v4: Ipv4Addr) -> bool {
    let [a, b, c, _] = v4.octets();
    // 0.0.0.0/8 (本网)
    if a == 0 {
        return false;
    }
    // 10.0.0.0/8 (私网)
    if a == 10 {
        return false;
    }
    // 100.64.0.0/10 (CGNAT)
    if a == 100 && (64..=127).contains(&b) {
        return false;
    }
    // 127.0.0.0/8 (回环)
    if a == 127 {
        return false;
    }
    // 169.254.0.0/16 (链路本地)
    if a == 169 && b == 254 {
        return false;
    }
    // 172.16.0.0/12 (私网)
    if a == 172 && (16..=31).contains(&b) {
        return false;
    }
    // 192.0.0.0/24 (IETF 保留), 192.0.2.0/24 (TEST-NET-1), 192.168.0.0/16 (私网)
    if a == 192 && ((b == 0 && (c == 0 || c == 2)) || b == 168) {
        return false;
    }
    // 198.18.0.0/15 (Benchmark / Clash Fake-IP) & 198.51.100.0/24 (TEST-NET-2)
    if a == 198 && ((b & 0xFE) == 18 || (b == 51 && c == 100)) {
        return false;
    }
    // 203.0.113.0/24 (TEST-NET-3)
    if a == 203 && b == 0 && c == 113 {
        return false;
    }
    // 224.0.0.0/4 (组播) & 240.0.0.0/4 (保留 / 广播)
    if a >= 224 {
        return false;
    }
    true
}

fn is_global_v6(v6: Ipv6Addr) -> bool {
    let seg = v6.segments();
    // :: (未指定) & ::1 (回环)
    if v6.is_unspecified() || v6.is_loopback() {
        return false;
    }
    // fc00::/7 (ULA)
    if (seg[0] & 0xFE00) == 0xFC00 {
        return false;
    }
    // fe80::/10 (链路本地)
    if (seg[0] & 0xFFC0) == 0xFE80 {
        return false;
    }
    // ff00::/8 (组播)
    if (seg[0] & 0xFF00) == 0xFF00 {
        return false;
    }
    // 2001:db8::/32 (文档保留)
    if seg[0] == 0x2001 && seg[1] == 0x0DB8 {
        return false;
    }
    // 仅放行常规公网 2000::/3 全球单播段（排除上面 2001:db8::/32 及非全球实验段）
    (seg[0] & 0xE000) == 0x2000
}

#[derive(Debug)]
struct ParsedAnswer {
    owner: String,
    rtype: u16,
    ttl: u32,
    cname: Option<String>,
    ip: Option<IpAddr>,
}

/// 解析 RFC 8484 二进制响应：
/// 1. 强校 header：`QR=1, Opcode=0, TC=0, QDCOUNT=1`
/// 2. 跳过 Question 段，逐条解析 Answer 段
/// 3. 沿 `qname` 完整跟随 CNAME 链（最多 8 跳防环），仅采信链上 `qtype` 地址记录
/// 4. 对所有命中 IP 做严格 Bogon / 污染 IP 过滤
pub fn parse_ip_response(msg: &[u8], qname: &str, qtype: u16) -> DnsWireOutcome {
    if msg.len() < 12 || !matches!(qtype, TYPE_A | TYPE_AAAA) {
        return DnsWireOutcome::Malformed;
    }
    let Some(flags) = u16_at(msg, 2) else {
        return DnsWireOutcome::Malformed;
    };
    let is_response = (flags & 0x8000) != 0;
    let opcode = (flags & 0x7800) >> 11;
    let truncated = (flags & 0x0200) != 0;
    let rcode = flags & 0x000F;
    let (Some(qdcount), Some(ancount)) = (u16_at(msg, 4), u16_at(msg, 6)) else {
        return DnsWireOutcome::Malformed;
    };

    if !is_response || opcode != 0 || truncated || qdcount != 1 {
        return DnsWireOutcome::Malformed;
    }
    match rcode {
        0 => {}
        3 => return DnsWireOutcome::NoData { nxdomain: true },
        _ => return DnsWireOutcome::ServerFailure,
    }

    let mut pos = 12usize;
    for _ in 0..qdcount {
        let Some(next) = skip_name(msg, pos) else {
            return DnsWireOutcome::Malformed;
        };
        if next + 4 > msg.len() {
            return DnsWireOutcome::Malformed;
        }
        pos = next + 4;
    }

    let mut answers = Vec::with_capacity(usize::from(ancount.min(32)));
    for _ in 0..ancount {
        let Some(owner) = read_name(msg, pos) else {
            return DnsWireOutcome::Malformed;
        };
        let Some(after_name) = skip_name(msg, pos) else {
            return DnsWireOutcome::Malformed;
        };
        let (Some(rtype), Some(rclass), Some(ttl), Some(rdlength)) = (
            u16_at(msg, after_name),
            u16_at(msg, after_name + 2),
            u32_at(msg, after_name + 4),
            u16_at(msg, after_name + 8),
        ) else {
            return DnsWireOutcome::Malformed;
        };
        let rdata_start = after_name + 10;
        let rdata_end = rdata_start + usize::from(rdlength);
        if rdata_end > msg.len() {
            return DnsWireOutcome::Malformed;
        }
        pos = rdata_end;

        if rclass != CLASS_IN {
            continue;
        }
        let owner = owner.trim_end_matches('.').to_ascii_lowercase();
        if rtype == TYPE_CNAME {
            let Some(target) = read_name(msg, rdata_start) else {
                return DnsWireOutcome::Malformed;
            };
            answers.push(ParsedAnswer {
                owner,
                rtype,
                ttl,
                cname: Some(target.trim_end_matches('.').to_ascii_lowercase()),
                ip: None,
            });
        } else if rtype == TYPE_A {
            if rdlength != 4 {
                return DnsWireOutcome::Malformed;
            }
            let rdata = &msg[rdata_start..rdata_end];
            let ip = IpAddr::V4(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]));
            answers.push(ParsedAnswer {
                owner,
                rtype,
                ttl,
                cname: None,
                ip: Some(ip),
            });
        } else if rtype == TYPE_AAAA {
            if rdlength != 16 {
                return DnsWireOutcome::Malformed;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&msg[rdata_start..rdata_end]);
            let ip = IpAddr::V6(Ipv6Addr::from(octets));
            answers.push(ParsedAnswer {
                owner,
                rtype,
                ttl,
                cname: None,
                ip: Some(ip),
            });
        }
    }

    // 从查询域名出发严格跟随 CNAME 链，绝不采信链外夹带记录
    let mut current = qname.trim_end_matches('.').to_ascii_lowercase();
    let mut seen_targets = HashSet::new();
    seen_targets.insert(current.clone());

    let mut min_ttl = u32::MAX;
    let mut ips = Vec::new();
    for _ in 0..=MAX_CNAME_HOPS {
        let mut next_cname: Option<(&str, u32)> = None;
        for ans in &answers {
            if ans.owner != current {
                continue;
            }
            if ans.rtype == qtype
                && let Some(ip) = ans.ip
            {
                if is_bogon_or_poisoned(ip) {
                    return DnsWireOutcome::PoisonedOrBogon;
                }
                if !ips.contains(&ip) {
                    ips.push(ip);
                }
                min_ttl = min_ttl.min(ans.ttl);
            } else if ans.rtype == TYPE_CNAME
                && next_cname.is_none()
                && let Some(target) = ans.cname.as_deref()
            {
                next_cname = Some((target, ans.ttl));
            }
        }
        if !ips.is_empty() {
            return DnsWireOutcome::Records { ips, min_ttl };
        }
        let Some((target, ttl)) = next_cname else {
            break;
        };
        if !seen_targets.insert(target.to_owned()) {
            return DnsWireOutcome::Malformed;
        }
        min_ttl = min_ttl.min(ttl);
        current = target.to_owned();
    }

    DnsWireOutcome::NoData { nxdomain: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_labels(name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for label in name.trim_end_matches('.').split('.') {
            out.push(u8::try_from(label.len()).unwrap());
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out
    }

    fn build_test_response(
        qname: &str,
        qtype: u16,
        flags: u16,
        answers: Vec<(&str, u16, u32, Vec<u8>)>,
    ) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&flags.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT=1
        msg.extend_from_slice(&u16::try_from(answers.len()).unwrap().to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&encode_labels(qname));
        msg.extend_from_slice(&qtype.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        for (owner, rtype, ttl, rdata) in answers {
            msg.extend_from_slice(&encode_labels(owner));
            msg.extend_from_slice(&rtype.to_be_bytes());
            msg.extend_from_slice(&CLASS_IN.to_be_bytes());
            msg.extend_from_slice(&ttl.to_be_bytes());
            msg.extend_from_slice(&u16::try_from(rdata.len()).unwrap().to_be_bytes());
            msg.extend_from_slice(&rdata);
        }
        msg
    }

    #[test]
    fn follows_cname_chain_and_ignores_out_of_chain_records() {
        let wire = build_test_response(
            "piston-meta.mojang.com",
            TYPE_A,
            0x8180,
            vec![
                (
                    "unrelated.evil.com",
                    TYPE_A,
                    60,
                    vec![1, 2, 3, 4], // 不在链上，必须忽略
                ),
                (
                    "piston-meta.mojang.com",
                    TYPE_CNAME,
                    300,
                    encode_labels("cdn.azureedge.net"),
                ),
                ("cdn.azureedge.net", TYPE_A, 45, vec![150, 171, 110, 100]),
            ],
        );
        assert_eq!(
            parse_ip_response(&wire, "piston-meta.mojang.com", TYPE_A),
            DnsWireOutcome::Records {
                ips: vec!["150.171.110.100".parse().unwrap()],
                min_ttl: 45,
            }
        );
    }

    #[test]
    fn parses_ipv6_and_nxdomain_and_tc() {
        let aaaa = build_test_response(
            "example.com",
            TYPE_AAAA,
            0x8180,
            vec![(
                "example.com",
                TYPE_AAAA,
                120,
                "2606:4700::1"
                    .parse::<Ipv6Addr>()
                    .unwrap()
                    .octets()
                    .to_vec(),
            )],
        );
        assert_eq!(
            parse_ip_response(&aaaa, "example.com", TYPE_AAAA),
            DnsWireOutcome::Records {
                ips: vec!["2606:4700::1".parse().unwrap()],
                min_ttl: 120,
            }
        );

        let nx = build_test_response("no.example.com", TYPE_A, 0x8183, vec![]);
        assert_eq!(
            parse_ip_response(&nx, "no.example.com", TYPE_A),
            DnsWireOutcome::NoData { nxdomain: true }
        );

        let tc = build_test_response("example.com", TYPE_A, 0x8380, vec![]);
        assert_eq!(
            parse_ip_response(&tc, "example.com", TYPE_A),
            DnsWireOutcome::Malformed
        );
    }

    #[test]
    fn rejects_bogon_fake_ip_and_mapped_loopback() {
        for bad in [
            vec![127, 0, 0, 1],
            vec![10, 0, 0, 1],
            vec![198, 18, 0, 42],
            vec![169, 254, 169, 254],
            vec![172, 16, 0, 1],
            vec![192, 168, 1, 1],
            vec![100, 64, 0, 1],
            vec![0, 0, 0, 0],
        ] {
            let wire = build_test_response(
                "maven.neoforged.net",
                TYPE_A,
                0x8180,
                vec![("maven.neoforged.net", TYPE_A, 60, bad.clone())],
            );
            assert_eq!(
                parse_ip_response(&wire, "maven.neoforged.net", TYPE_A),
                DnsWireOutcome::PoisonedOrBogon,
                "bogon IPv4 {bad:?} must be rejected"
            );
        }

        let mapped_loopback = "::ffff:127.0.0.1"
            .parse::<Ipv6Addr>()
            .unwrap()
            .octets()
            .to_vec();
        let wire_v6 = build_test_response(
            "maven.neoforged.net",
            TYPE_AAAA,
            0x8180,
            vec![("maven.neoforged.net", TYPE_AAAA, 60, mapped_loopback)],
        );
        assert_eq!(
            parse_ip_response(&wire_v6, "maven.neoforged.net", TYPE_AAAA),
            DnsWireOutcome::PoisonedOrBogon
        );
    }

    #[test]
    fn rejects_self_and_forward_compression_loops() {
        let msg = [0u8, 0, 0xC0, 2];
        assert_eq!(read_name(&msg, 2), None);

        // 后向高地址跳转也不能指向未访问后的位置
        let forward_msg = [0xC0, 4, 0, 0, 0];
        assert_eq!(read_name(&forward_msg, 0), None);
    }
}
