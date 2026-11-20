//! 服务器状态：按公开的「服务器列表 Ping」协议问一下服务器（和游戏「多人游戏」列表里一样），
//! 拿在线人数、延迟、版本、简介和图标。
//!
//! 地址没写端口时先查 SRV 记录（[`super::srv`]），查不到用 25565。整个过程最多几秒，
//! 连不上就说清楚是哪种（找不到地址 / 拒绝连接 / 超时 / 回答看不懂）。

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::srv;

pub const DEFAULT_PORT: u16 = 25565;
/// 握手里写的协议号：问状态时服务器不看它，写一个新版本的
const PROTOCOL: i32 = 767;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// 连上以后问状态、测延迟一共最多等多久
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);
/// 回答最大多大（带图标的一般几十 KB）
const MAX_PACKET: usize = 1 << 20;
/// 图标最大多大（64×64 的 PNG 转成文字）
const MAX_FAVICON: usize = 128 * 1024;
const FAVICON_PREFIX: &str = "data:image/png;base64,";
/// 简介最多留多少字
const MAX_MOTD: usize = 200;

/// 一个服务器现在的样子。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ServerStatus {
    #[serde(rename_all = "camelCase")]
    Online {
        /// 来回一趟多少毫秒
        latency_ms: u32,
        players_online: u32,
        players_max: u32,
        /// 服务器自己写的版本（「Paper 1.21.1」「1.8-1.21」）
        version: Option<String>,
        /// 简介（去掉了颜色代码，最多两行）
        motd: String,
        /// 服务器图标（`data:image/png;base64,...`）
        favicon: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Offline { reason: OfflineReason },
}

/// 连不上的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum OfflineReason {
    /// 地址写得不对
    BadAddress,
    /// 找不到这个地址（域名不存在、没网）
    NotFound,
    /// 服务器没开（拒绝连接、到不了）
    Refused,
    /// 等太久没回应
    Timeout,
    /// 回答看不懂（太老的版本、不是我的世界服务器）
    BadReply,
}

/// 问一下服务器。
pub async fn ping(address: &str) -> ServerStatus {
    match ping_inner(address).await {
        Ok(status) => status,
        Err(reason) => {
            tracing::debug!(server = %address, ?reason, "服务器连不上");
            ServerStatus::Offline { reason }
        }
    }
}

async fn ping_inner(address: &str) -> Result<ServerStatus, OfflineReason> {
    let (host, port) = split_address(address).ok_or(OfflineReason::BadAddress)?;
    let (host, port) = match port {
        Some(port) => (host, port),
        None if host.parse::<IpAddr>().is_ok() => (host, DEFAULT_PORT),
        None => match srv::lookup(&host).await {
            Some(target) => (target.host, target.port),
            None => (host, DEFAULT_PORT),
        },
    };
    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        RESOLVE_TIMEOUT,
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| OfflineReason::NotFound)?
    .map_err(|_| OfflineReason::NotFound)?
    .collect();
    let mut last = OfflineReason::NotFound;
    // 一个域名可能有好几个地址（IPv4 / IPv6）：前两个都试一下
    for addr in addrs.into_iter().take(2) {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(mut stream)) => {
                let _ = stream.set_nodelay(true);
                return tokio::time::timeout(EXCHANGE_TIMEOUT, exchange(&mut stream, &host, port))
                    .await
                    .map_err(|_| OfflineReason::Timeout)?;
            }
            Ok(Err(_)) => last = OfflineReason::Refused,
            Err(_) => last = OfflineReason::Timeout,
        }
    }
    Err(last)
}

/// 拆出主机和端口：`host`、`host:port`、`[v6]`、`[v6]:port`、不带括号的 IPv6。
fn split_address(address: &str) -> Option<(String, Option<u16>)> {
    let address = address.trim();
    if address.is_empty() || address.len() > 255 || address.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(rest) = address.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?.parse().ok()?),
        };
        return (!host.is_empty()).then(|| (host.to_owned(), port));
    }
    match address.split_once(':') {
        // 不止一个冒号：不带括号的 IPv6
        Some((_, rest)) if rest.contains(':') => Some((address.to_owned(), None)),
        Some((host, port)) => {
            let port: u16 = port.parse().ok().filter(|&p| p != 0)?;
            (!host.is_empty()).then(|| (host.to_owned(), Some(port)))
        }
        None => Some((address.to_owned(), None)),
    }
}

fn write_varint(out: &mut Vec<u8>, value: i32) {
    let mut value = u32::from_ne_bytes(value.to_ne_bytes());
    loop {
        let byte = u8::try_from(value & 0x7F).unwrap_or(0);
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    write_varint(out, i32::try_from(text.len()).unwrap_or(0));
    out.extend_from_slice(text.as_bytes());
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    write_varint(&mut out, i32::try_from(body.len()).unwrap_or(0));
    out.extend_from_slice(body);
    out
}

async fn read_varint(stream: &mut TcpStream) -> Result<u32, OfflineReason> {
    let mut value = 0u32;
    for shift in 0..5 {
        // 一个字都没回就断开：服务器没开（开着代理时代理会先接下连接，连不上再断开）
        let byte = stream.read_u8().await.map_err(|_| {
            if shift == 0 {
                OfflineReason::Refused
            } else {
                OfflineReason::BadReply
            }
        })?;
        value |= u32::from(byte & 0x7F) << (7 * shift);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(OfflineReason::BadReply)
}

/// 从字节里读一个 VarInt，返回值和剩下的部分
fn take_varint(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let mut value = 0u32;
    for (i, byte) in bytes.iter().take(5).enumerate() {
        value |= u32::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, &bytes[i + 1..]));
        }
    }
    None
}

async fn read_packet(stream: &mut TcpStream) -> Result<Vec<u8>, OfflineReason> {
    let len = usize::try_from(read_varint(stream).await?).map_err(|_| OfflineReason::BadReply)?;
    if len == 0 || len > MAX_PACKET {
        return Err(OfflineReason::BadReply);
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|_| OfflineReason::BadReply)?;
    Ok(body)
}

async fn exchange(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
) -> Result<ServerStatus, OfflineReason> {
    let mut handshake = Vec::new();
    write_varint(&mut handshake, 0x00);
    write_varint(&mut handshake, PROTOCOL);
    write_string(&mut handshake, host);
    handshake.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake, 1); // 下一步：问状态
    let mut out = frame(&handshake);
    out.extend_from_slice(&frame(&[0x00]));
    let asked = Instant::now();
    stream
        .write_all(&out)
        .await
        .map_err(|_| OfflineReason::Refused)?;

    let packet = read_packet(stream).await?;
    let status_latency = asked.elapsed();
    let (id, rest) = take_varint(&packet).ok_or(OfflineReason::BadReply)?;
    let (len, rest) = take_varint(rest).ok_or(OfflineReason::BadReply)?;
    let json = usize::try_from(len)
        .ok()
        .and_then(|len| rest.get(..len))
        .filter(|_| id == 0x00)
        .ok_or(OfflineReason::BadReply)?;
    let reply: Value = serde_json::from_slice(json).map_err(|_| OfflineReason::BadReply)?;

    // 再测一次来回（和游戏一样用 Ping / Pong）；服务器不答 Pong 就用问状态那一趟的时间
    let latency = match measure_pong(stream).await {
        Some(latency) => latency,
        None => status_latency,
    };
    Ok(status_from(&reply, latency))
}

async fn measure_pong(stream: &mut TcpStream) -> Option<Duration> {
    let payload = crate::now_millis().to_be_bytes();
    let mut body = vec![0x01];
    body.extend_from_slice(&payload);
    let started = Instant::now();
    stream.write_all(&frame(&body)).await.ok()?;
    let packet = tokio::time::timeout(Duration::from_secs(2), read_packet(stream))
        .await
        .ok()?
        .ok()?;
    (packet.first() == Some(&0x01) && packet.get(1..9) == Some(&payload[..]))
        .then(|| started.elapsed())
}

fn status_from(reply: &Value, latency: Duration) -> ServerStatus {
    let count = |key: &str| {
        reply
            .pointer(&format!("/players/{key}"))
            .and_then(Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let version = reply
        .pointer("/version/name")
        .and_then(Value::as_str)
        .map(strip_codes)
        .map(|v| v.trim().chars().take(64).collect::<String>())
        .filter(|v| !v.is_empty());
    let motd = reply
        .get("description")
        .map(|d| tidy_motd(&plain_text(d)))
        .unwrap_or_default();
    ServerStatus::Online {
        latency_ms: u32::try_from(latency.as_millis()).unwrap_or(u32::MAX),
        players_online: count("online"),
        players_max: count("max"),
        version,
        motd,
        favicon: reply
            .get("favicon")
            .and_then(Value::as_str)
            .and_then(clean_favicon),
    }
}

/// 简介可能是文字，也可能是带 `text` / `extra` 的聊天组件：只取文字
fn plain_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(plain_text).collect(),
        Value::Object(map) => {
            let mut out = map
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if let Some(Value::Array(extra)) = map.get("extra") {
                for item in extra {
                    out.push_str(&plain_text(item));
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// 去掉 `§a` 这类颜色、格式代码
fn strip_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// 去掉颜色代码，每行去掉首尾空白，最多两行、200 个字
fn tidy_motd(text: &str) -> String {
    strip_codes(text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(MAX_MOTD)
        .collect()
}

/// 只收 PNG 图标，去掉老服务器塞进去的换行，字符只能是 base64 的
fn clean_favicon(raw: &str) -> Option<String> {
    let data: String = raw.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    let body = data.strip_prefix(FAVICON_PREFIX)?;
    let ok = !body.is_empty()
        && data.len() <= MAX_FAVICON
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b));
    ok.then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn splits_addresses_like_the_game() {
        let s = split_address;
        assert_eq!(s("mc.example.com"), Some(("mc.example.com".into(), None)));
        assert_eq!(
            s(" mc.example.com:25570 "),
            Some(("mc.example.com".into(), Some(25570)))
        );
        assert_eq!(s("[::1]:25566"), Some(("::1".into(), Some(25566))));
        assert_eq!(s("[::1]"), Some(("::1".into(), None)));
        assert_eq!(s("2001:db8::1"), Some(("2001:db8::1".into(), None)));
        assert_eq!(s("host:0"), None);
        assert_eq!(s("host:99999"), None);
        assert_eq!(s(":25565"), None);
        assert_eq!(s("a b"), None);
        assert_eq!(s(""), None);
    }

    #[test]
    fn varints_round_trip() {
        for value in [0, 1, 127, 128, 255, 25565, 2_097_151, i32::MAX, -1] {
            let mut out = Vec::new();
            write_varint(&mut out, value);
            let (read, rest) = take_varint(&out).unwrap();
            assert_eq!(read, u32::from_ne_bytes(value.to_ne_bytes()));
            assert!(rest.is_empty());
        }
        assert_eq!(take_varint(&[0x80, 0x80]), None, "没读完");
    }

    #[test]
    fn reads_text_components_and_strips_codes() {
        let reply: Value = serde_json::from_str(
            r#"{"version":{"name":"§cPaper 1.21.1","protocol":767},
                "players":{"max":100,"online":12},
                "description":{"text":"  §a欢迎来到","extra":[{"text":"§l生存服"},"\n  第二行  ","\n第三行"]},
                "favicon":"data:image/png;base64,iVBO\nRw0K"}"#,
        )
        .unwrap();
        let ServerStatus::Online {
            latency_ms,
            players_online,
            players_max,
            version,
            motd,
            favicon,
        } = status_from(&reply, Duration::from_millis(42))
        else {
            panic!("应该是在线");
        };
        assert_eq!((latency_ms, players_online, players_max), (42, 12, 100));
        assert_eq!(version.as_deref(), Some("Paper 1.21.1"));
        assert_eq!(motd, "欢迎来到生存服\n第二行");
        assert_eq!(favicon.as_deref(), Some("data:image/png;base64,iVBORw0K"));

        let plain: Value = serde_json::from_str(r#"{"description":"§6A Server"}"#).unwrap();
        let ServerStatus::Online {
            players_max,
            motd,
            version,
            favicon,
            ..
        } = status_from(&plain, Duration::ZERO)
        else {
            panic!("应该是在线");
        };
        assert_eq!((players_max, motd.as_str()), (0, "A Server"));
        assert_eq!((version, favicon), (None, None));
    }

    #[test]
    fn rejects_odd_favicons() {
        assert_eq!(clean_favicon("data:image/svg+xml;base64,PHN2Zz4="), None);
        assert_eq!(clean_favicon("data:image/png;base64,\"><script>"), None);
        assert_eq!(clean_favicon("data:image/png;base64,"), None);
        let huge = format!("{FAVICON_PREFIX}{}", "A".repeat(MAX_FAVICON));
        assert_eq!(clean_favicon(&huge), None);
    }

    /// 一个假的服务器：读握手和请求，回一段 JSON，再答 Pong
    async fn fake_server(json: &'static str, answer_pong: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let handshake = read_packet(&mut stream).await.unwrap();
            let (id, rest) = take_varint(&handshake).unwrap();
            assert_eq!(id, 0);
            let (_, rest) = take_varint(rest).unwrap();
            let (len, rest) = take_varint(rest).unwrap();
            let host = std::str::from_utf8(&rest[..len as usize]).unwrap();
            assert_eq!(host, "127.0.0.1");
            assert_eq!(rest[len as usize + 2], 1, "下一步是问状态");
            assert_eq!(read_packet(&mut stream).await.unwrap(), vec![0x00]);
            let mut body = Vec::new();
            write_varint(&mut body, 0);
            write_string(&mut body, json);
            stream.write_all(&frame(&body)).await.unwrap();
            if answer_pong {
                let ping = read_packet(&mut stream).await.unwrap();
                stream.write_all(&frame(&ping)).await.unwrap();
            }
            // 不答 Pong 的：拖一会儿再关
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        port
    }

    #[tokio::test]
    async fn pings_a_running_server() {
        let port = fake_server(
            r#"{"version":{"name":"1.21.1"},"players":{"max":20,"online":3},"description":"hi"}"#,
            true,
        )
        .await;
        let status = ping(&format!("127.0.0.1:{port}")).await;
        let ServerStatus::Online {
            players_online,
            players_max,
            version,
            motd,
            ..
        } = status
        else {
            panic!("应该是在线：{status:?}");
        };
        assert_eq!((players_online, players_max), (3, 20));
        assert_eq!(version.as_deref(), Some("1.21.1"));
        assert_eq!(motd, "hi");
    }

    #[tokio::test]
    async fn still_online_without_pong() {
        let port = fake_server(r#"{"players":{"max":1,"online":0}}"#, false).await;
        assert!(matches!(
            ping(&format!("127.0.0.1:{port}")).await,
            ServerStatus::Online { players_max: 1, .. }
        ));
    }

    #[tokio::test]
    async fn closed_and_garbage_servers_are_offline() {
        // 先占一个端口再放掉：上面没人听
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        assert_eq!(
            ping(&format!("127.0.0.1:{port}")).await,
            ServerStatus::Offline {
                reason: OfflineReason::Refused
            }
        );
        assert_eq!(
            ping("bad address").await,
            ServerStatus::Offline {
                reason: OfflineReason::BadAddress
            }
        );

        // 接下连接马上断开（开着代理时连不上的服务器就是这样）
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream);
        });
        assert_eq!(
            ping(&format!("127.0.0.1:{port}")).await,
            ServerStatus::Offline {
                reason: OfflineReason::Refused
            }
        );

        // 回的不是我的世界的协议
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let _ = stream.read(&mut buf).await;
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            // 马上关掉的话 Windows 会发重置，把没读的回答也丢掉：等一会儿再关
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        assert_eq!(
            ping(&format!("127.0.0.1:{port}")).await,
            ServerStatus::Offline {
                reason: OfflineReason::BadReply
            }
        );
    }
}
