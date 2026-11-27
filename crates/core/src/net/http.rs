//! 统一的 HTTP 客户端三池架构、自管安全重定向与官方互备源矩阵。

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::config::ProxySettings;

use super::dns_wire::is_strictly_global_unicast;
use super::doh::{Doh, DohResolver};
use super::download::HostHeader;
use super::proxy_watch::{SystemProxySnapshot, apply_proxy_settings};
use super::url_host;

/// 所有对外请求统一使用的客户端标识（「名字/版本」）。
/// 2026-11-15 定名「微光 / Lumi」，以前是代号 `UnnamedLauncher`
pub const USER_AGENT: &str = concat!("LumiLauncher/", env!("CARGO_PKG_VERSION"));

pub const MAX_REDIRECTS: usize = 5;

const NEOFORGE_PRIMARY_PREFIX: &str = "https://maven.neoforged.net/releases/";
const NEOFORGE_OFFICIAL_CDN_PREFIX: &str = "https://neoforged.forgecdn.net/releases/";
const CF_EDGE_PRIMARY: &str = "https://edge.forgecdn.net/";
const CF_EDGE_ALT: &str = "https://mediafilez.forgecdn.net/";

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("无法创建网络客户端")]
    Build(#[source] reqwest::Error),
    #[error("自定义代理地址无效：{0}")]
    InvalidProxy(String),
    #[error("网络请求失败：{url}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("服务器返回 {status}：{url}")]
    Status { url: String, status: u16 },
    #[error("返回的数据格式不对：{url}")]
    Decode {
        url: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("所有下载源都失败了（最后一个错误：{last}）")]
    AllSourcesFailed { last: String },
}

/// 三池分层 HTTP 客户端集合（同一网络代际内共享，支持热切换）：
/// - `h2_small`：元数据/API/通用请求（H2 2MiB/16MiB 窗口 + 自带安全重定向，拒绝对 HTTPS 降级 HTTP）
/// - `h2_download`：H2 小文件下载专池（禁自动重定向，由下载器自管逐跳剥离敏感头）
/// - `h1_segmented`：HTTP/1.1 独立物理 TCP 连接池（杜绝 H2 队头阻塞与流控塌缩）
/// - `spare`：无空闲复用逃生池（用于 STALL/RST 孤立单连接抢救）
#[derive(Clone)]
pub struct ClientBundle {
    pub h2_small: Client,
    pub h2_download: Client,
    pub h2_shards: [Client; 4],
    pub h1_segmented: Client,
    pub spare: Client,
}

impl ClientBundle {
    pub fn next_h2_download(&self, index: usize) -> Client {
        self.h2_shards[index % self.h2_shards.len()].clone()
    }

    pub fn build(
        doh: Arc<Doh>,
        proxy: &ProxySettings,
        sys_snap: Option<&SystemProxySnapshot>,
    ) -> Result<Self, HttpError> {
        let apply = |builder: reqwest::ClientBuilder| -> Result<reqwest::ClientBuilder, HttpError> {
            apply_proxy_settings(
                builder.dns_resolver(DohResolver(doh.clone())),
                proxy,
                sys_snap,
            )
        };

        let safe_redirect = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(std::io::Error::other("重定向次数过多"));
            }
            if let Some(prev) = attempt.previous().last() {
                if let Err(msg) = validate_redirect_target(prev, attempt.url()) {
                    return attempt.error(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        msg,
                    ));
                }
                // 严防 CurseForge API 跨源重定向泄露 `x-api-key`
                if prev
                    .host_str()
                    .is_some_and(|h| h.eq_ignore_ascii_case("api.curseforge.com"))
                {
                    let next_allowed = attempt.url().host_str().is_some_and(|h| {
                        h.eq_ignore_ascii_case("api.curseforge.com")
                            || h.eq_ignore_ascii_case("edge.forgecdn.net")
                            || h.eq_ignore_ascii_case("mediafilez.forgecdn.net")
                    });
                    if !next_allowed {
                        return attempt.error(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "拒绝 CurseForge 接口跨源重定向",
                        ));
                    }
                }
            }
            attempt.follow()
        });

        let build_h2_pool = |redirect: reqwest::redirect::Policy| -> Result<Client, HttpError> {
            apply(
                client_builder()
                    .http2_initial_stream_window_size(Some(2 * 1024 * 1024))
                    .http2_initial_connection_window_size(Some(16 * 1024 * 1024))
                    .http2_adaptive_window(true)
                    .http2_keep_alive_interval(Some(Duration::from_secs(15)))
                    .http2_keep_alive_timeout(Duration::from_secs(10))
                    .tcp_nodelay(true)
                    .redirect(redirect),
            )?
            .build()
            .map_err(HttpError::Build)
        };

        let h2_small = build_h2_pool(safe_redirect)?;
        let h2_shards: [Client; 4] = [
            build_h2_pool(reqwest::redirect::Policy::none())?,
            build_h2_pool(reqwest::redirect::Policy::none())?,
            build_h2_pool(reqwest::redirect::Policy::none())?,
            build_h2_pool(reqwest::redirect::Policy::none())?,
        ];
        let h2_download = h2_shards[0].clone();

        let h1_segmented = apply(
            client_builder()
                .http1_only()
                .pool_max_idle_per_host(64)
                .pool_idle_timeout(Some(Duration::from_secs(60)))
                .tcp_nodelay(true)
                .redirect(reqwest::redirect::Policy::none()),
        )?
        .build()
        .map_err(HttpError::Build)?;

        let spare = apply(
            client_builder()
                .http1_only()
                .pool_max_idle_per_host(0)
                .tcp_nodelay(true)
                .redirect(reqwest::redirect::Policy::none()),
        )?
        .build()
        .map_err(HttpError::Build)?;

        Ok(Self {
            h2_small,
            h2_download,
            h2_shards,
            h1_segmented,
            spare,
        })
    }
}

/// 纯官方互备 CDN 地址扩展（100% 官方域名，不含任何第三方社区镜像）：
/// - `maven.neoforged.net/releases/...` ↔ `neoforged.forgecdn.net/releases/...`
/// - `mediafilez.forgecdn.net/...` ↔ `edge.forgecdn.net/...`
///
/// 若主域名处于 `SniBlocked` 冷却，将官方互备 CDN 排在最前。
pub fn official_fallback_urls(url: &str, doh: Option<&Doh>) -> Vec<String> {
    if let Some(rest) = url.strip_prefix(NEOFORGE_PRIMARY_PREFIX) {
        let alt = format!("{NEOFORGE_OFFICIAL_CDN_PREFIX}{rest}");
        return if doh.is_some_and(|d| d.is_host_degraded("maven.neoforged.net")) {
            vec![alt, url.to_owned()]
        } else {
            vec![url.to_owned(), alt]
        };
    }
    if let Some(rest) = url.strip_prefix(NEOFORGE_OFFICIAL_CDN_PREFIX) {
        let primary = format!("{NEOFORGE_PRIMARY_PREFIX}{rest}");
        return vec![url.to_owned(), primary];
    }
    if let Some(rest) = url.strip_prefix(CF_EDGE_ALT) {
        let primary = format!("{CF_EDGE_PRIMARY}{rest}");
        return if doh.is_some_and(|d| d.is_host_degraded("mediafilez.forgecdn.net")) {
            vec![primary, url.to_owned()]
        } else {
            vec![url.to_owned(), primary]
        };
    }
    if let Some(rest) = url.strip_prefix(CF_EDGE_PRIMARY) {
        let alt = format!("{CF_EDGE_ALT}{rest}");
        return if doh.is_some_and(|d| d.is_host_degraded("edge.forgecdn.net")) {
            vec![alt, url.to_owned()]
        } else {
            vec![url.to_owned(), alt]
        };
    }
    vec![url.to_owned()]
}

pub fn expand_official_fallbacks(urls: &[String], doh: Option<&Doh>) -> Vec<String> {
    let mut out = Vec::with_capacity(urls.len() * 2);
    for url in urls {
        for candidate in official_fallback_urls(url, doh) {
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
}

fn is_loopback_origin(url: &reqwest::Url) -> bool {
    let Some(raw) = url.host_str() else {
        return false;
    };
    let host = raw.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
            IpAddr::V4(v4) => v4.is_loopback(),
        })
}

/// Gate 3（防 302 SSRF 跳内网 / HTTPS 降级 HTTP / 凭据注入）：供全局重定向策略与自管多跳下载共用。
pub(crate) fn validate_redirect_target(
    origin: &reqwest::Url,
    next: &reqwest::Url,
) -> Result<(), String> {
    if !matches!(next.scheme(), "http" | "https") {
        return Err(format!("不支持的重定向协议：{}", next.scheme()));
    }
    if origin.scheme().eq_ignore_ascii_case("https") && next.scheme().eq_ignore_ascii_case("http") {
        return Err(format!(
            "拒绝从 HTTPS 降级重定向到 HTTP：{}",
            url_host(next.as_str()).unwrap_or_else(|| next.scheme().to_owned())
        ));
    }
    if !next.username().is_empty() || next.password().is_some() {
        return Err("拒绝包含凭据字段的重定向地址".to_owned());
    }
    let Some(raw_host) = next.host_str() else {
        return Err("重定向目标缺少主机名".to_owned());
    };
    let host = raw_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let origin_loopback = is_loopback_origin(origin);

    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_strictly_global_unicast(ip) {
            let next_loopback = match ip {
                IpAddr::V4(v4) => v4.is_loopback(),
                IpAddr::V6(v6) => {
                    v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
                }
            };
            if !(origin_loopback && next_loopback) {
                return Err(format!("拒绝重定向到内网或保留地址：{host}"));
            }
        }
        return Ok(());
    }

    let is_local_name = host == "localhost"
        || !host.contains('.')
        || [
            ".localhost",
            ".local",
            ".lan",
            ".internal",
            ".arpa",
            ".home",
        ]
        .iter()
        .any(|s| host.ends_with(s));
    if is_local_name {
        let allowed_loopback_name =
            origin_loopback && (host == "localhost" || host.ends_with(".localhost"));
        if !allowed_loopback_name {
            return Err(format!("拒绝重定向到本地内部域名：{host}"));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum RedirectSendError {
    Request { url: String, source: reqwest::Error },
    Protocol(String),
}

/// 自管安全重定向发送（用于 `Policy::none()` 下载池）：
/// - 最多 5 跳；拒绝 HTTPS → HTTP 协议降级与跳内网 SSRF；
/// - 严格按 `HostHeader::applies_to` 决定是否附带 `x-api-key` 等敏感头，跨源不泄露凭据。
pub(crate) async fn send_following_safe_redirects(
    client: &Client,
    initial_url: &str,
    range_header: Option<&str>,
    host_header: Option<&HostHeader>,
) -> Result<(reqwest::Response, String), RedirectSendError> {
    let mut current_url = initial_url.to_owned();
    let mut current = reqwest::Url::parse(&current_url)
        .map_err(|e| RedirectSendError::Protocol(format!("无效下载地址 {current_url}：{e}")))?;

    for _hop in 0..=MAX_REDIRECTS {
        let mut req = client
            .get(&current_url)
            .header(reqwest::header::USER_AGENT, USER_AGENT);
        if let Some(range) = range_header {
            req = req.header(reqwest::header::RANGE, range);
        }
        if let Some(auth) = host_header.filter(|h| h.applies_to(&current_url)) {
            req = req.header(auth.name, auth.value.expose().as_str());
        }

        let resp = req
            .send()
            .await
            .map_err(|source| RedirectSendError::Request {
                url: current_url.clone(),
                source,
            })?;
        let status = resp.status();
        if !status.is_redirection() {
            return Ok((resp, current_url));
        }
        let Some(location) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        else {
            return Ok((resp, current_url));
        };

        let next = current.join(location).map_err(|e| {
            RedirectSendError::Protocol(format!("重定向地址无效（{location}）：{e}"))
        })?;
        validate_redirect_target(&current, &next).map_err(RedirectSendError::Protocol)?;
        current_url = next.to_string();
        current = next;
    }

    Err(RedirectSendError::Protocol("重定向次数过多".to_owned()))
}

/// 创建启动器统一使用的 HTTP 客户端（会跟随 Windows 系统代理）。
/// 应用里用的是 [`super::build_client_with_doh`]（多了 DoH），命令行工具和测试用这个。
pub fn build_client() -> Result<Client, HttpError> {
    client_builder().build().map_err(HttpError::Build)
}

/// 专用于 Microsoft / Xbox / Mojang 认证与官方皮肤披风请求的 HTTP 客户端：
/// 严格启用 DoH + 用户代理设置 + `https_only` + `Policy::none()`（禁止自动重定向携带凭据）。
pub fn build_auth_client(
    doh: Arc<Doh>,
    proxy: &ProxySettings,
    sys_snap: Option<&SystemProxySnapshot>,
) -> Result<Client, HttpError> {
    apply_proxy_settings(
        Client::builder()
            .user_agent(USER_AGENT)
            .dns_resolver(DohResolver(doh))
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15)),
        proxy,
        sys_snap,
    )?
    .build()
    .map_err(HttpError::Build)
}

/// 两种客户端共用的基础设置
pub(super) fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(32)
}

/// 按顺序尝试候选地址，返回第一个成功的响应内容。
pub async fn get_bytes(client: &Client, urls: &[String]) -> Result<Vec<u8>, HttpError> {
    get_bytes_with_doh(client, urls, None).await
}

/// 按顺序尝试候选地址，并向可选 [`Doh`] 回传建连成功/失败状态以联动主机降级与官方互备 CDN 秒切。
pub async fn get_bytes_with_doh(
    client: &Client,
    urls: &[String],
    doh: Option<&Doh>,
) -> Result<Vec<u8>, HttpError> {
    let mut last = String::from("没有可用的地址");
    for url in urls {
        match get_one_with_doh(client, url, doh).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                tracing::warn!(url, error = %error, "请求失败，尝试下一个下载源");
                last = error.to_string();
            }
        }
    }
    Err(HttpError::AllSourcesFailed { last })
}

/// 按顺序尝试候选地址，把第一个成功的响应解析成 JSON。
/// 某个源返回了格式错误的内容时也会换下一个源。
pub async fn get_json<T: DeserializeOwned>(
    client: &Client,
    urls: &[String],
) -> Result<T, HttpError> {
    let mut last = String::from("没有可用的地址");
    for url in urls {
        let parsed = get_one(client, url).await.and_then(|bytes| {
            serde_json::from_slice(&bytes).map_err(|source| HttpError::Decode {
                url: url.clone(),
                source,
            })
        });
        match parsed {
            Ok(value) => return Ok(value),
            Err(error) => {
                tracing::warn!(url, error = %error, "请求或解析失败，尝试下一个下载源");
                last = error.to_string();
            }
        }
    }
    Err(HttpError::AllSourcesFailed { last })
}

async fn get_one(client: &Client, url: &str) -> Result<Vec<u8>, HttpError> {
    get_one_with_doh(client, url, None).await
}

async fn get_one_with_doh(
    client: &Client,
    url: &str,
    doh: Option<&Doh>,
) -> Result<Vec<u8>, HttpError> {
    let request_error = |source| HttpError::Request {
        url: url.to_owned(),
        source,
    };
    let response = match client.get(url).send().await {
        Ok(resp) => {
            if let Some(d) = doh
                && let Some(host) = url_host(url)
            {
                d.report_connect_ok(&host);
            }
            resp
        }
        Err(source) => {
            if let Some(d) = doh
                && let Some(host) = url_host(url)
            {
                let _ = d.report_connect_failure(&host, &source);
            }
            return Err(request_error(source));
        }
    };
    let status = response.status();
    if !status.is_success() {
        return Err(HttpError::Status {
            url: url.to_owned(),
            status: status.as_u16(),
        });
    }
    let bytes = response.bytes().await.map_err(request_error)?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::net::{DownloadItem, Downloader};

    #[test]
    fn user_agent_is_name_slash_version() {
        let (name, version) = USER_AGENT.split_once('/').unwrap();
        assert!(!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(
            version.split('.').count() == 3 && version.split('.').all(|p| p.parse::<u32>().is_ok())
        );
    }

    #[test]
    fn expands_neoforge_and_curseforge_official_cdn_fallbacks() {
        let nf = official_fallback_urls(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml",
            None,
        );
        assert_eq!(
            nf,
            vec![
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml",
                "https://neoforged.forgecdn.net/releases/net/neoforged/neoforge/maven-metadata.xml",
            ]
        );

        let doh = Doh::new(true).unwrap();
        doh.report_connect_failure_kind(
            "maven.neoforged.net",
            super::super::doh::ConnectFailureKind::HandshakeResetOrEof,
        );
        let cf = official_fallback_urls(
            "https://mediafilez.forgecdn.net/files/1/2/a.jar",
            Some(&doh),
        );
        assert_eq!(
            cf,
            vec![
                "https://mediafilez.forgecdn.net/files/1/2/a.jar",
                "https://edge.forgecdn.net/files/1/2/a.jar",
            ]
        );
    }

    async fn recording_server() -> (String, Arc<Mutex<Vec<(String, String)>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let agent = request
                        .lines()
                        .find_map(|l| {
                            l.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                                .map(|(_, v)| v.trim().to_owned())
                        })
                        .unwrap_or_default();
                    log.lock().unwrap().push((path.clone(), agent));
                    let response = match path.as_str() {
                        "/old" => "HTTP/1.1 302 Found\r\nLocation: /file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                        "/meta" => "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_owned(),
                        _ => "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_owned(),
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (base, seen)
    }

    #[tokio::test]
    async fn every_request_carries_the_same_user_agent() {
        let (base, seen) = recording_server().await;
        let client = client_builder().no_proxy().build().unwrap();

        let _: serde_json::Value = get_json(&client, &[format!("{base}/meta")]).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        Downloader::new(client, 2)
            .download_all(
                &[DownloadItem {
                    urls: vec![format!("{base}/old")],
                    path: dir.path().join("a.txt"),
                    sha1: None,
                    size: Some(5),
                }],
                &CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        let seen = seen.lock().unwrap().clone();
        let paths: Vec<&str> = seen.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["/meta", "/old", "/file"]);
        for (path, agent) in &seen {
            assert_eq!(agent, USER_AGENT, "{path} 的 UA 不对");
        }
    }

    #[test]
    fn redirect_gate_blocks_ssrf_and_downgrade() {
        let public_https = reqwest::Url::parse("https://cdn.modrinth.com/data/a.jar").unwrap();
        let loopback_http = reqwest::Url::parse("http://127.0.0.1:8080/old").unwrap();

        // 1) 公网跳内网 / 回环 / 云元数据必须全拒
        for evil in [
            "https://127.0.0.1/admin",
            "https://169.254.169.254/latest/meta-data",
            "https://10.0.0.1/secret",
            "https://192.168.1.1/router",
            "https://[::ffff:127.0.0.1]/admin",
            "https://localhost/internal",
            "https://printer.local/status",
            "http://cdn.modrinth.com/downgrade.jar",
            "https://user:pass@cdn.modrinth.com/leak.jar",
            "file:///C:/Windows/win.ini",
        ] {
            let next = reqwest::Url::parse(evil).unwrap();
            assert!(
                validate_redirect_target(&public_https, &next).is_err(),
                "{evil} 必须被 Redirect SSRF 门拦截"
            );
        }

        // 2) 回环测试服务不允许跳向 169.254.169.254，但允许同机回环跳转
        let metadata = reqwest::Url::parse("http://169.254.169.254/latest").unwrap();
        assert!(validate_redirect_target(&loopback_http, &metadata).is_err());

        let same_loopback = reqwest::Url::parse("http://127.0.0.1:8080/file").unwrap();
        assert!(validate_redirect_target(&loopback_http, &same_loopback).is_ok());

        // 3) 合法公网 HTTPS 跳转放行
        let good_cdn = reqwest::Url::parse("https://fastly.example.net/data/a.jar").unwrap();
        assert!(validate_redirect_target(&public_https, &good_cdn).is_ok());
    }
}
