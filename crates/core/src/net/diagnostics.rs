//! 网络连通性诊断（D26a）：并行探测 6 个核心服务端点的连通状态与响应耗时。
//! 只做极轻的 HEAD/GET 探测，不携带任何账号凭据或用户标识。

use std::time::{Duration, Instant};

use futures_util::future::join_all;
use reqwest::Client;
use serde::Serialize;

const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum EndpointStatus {
    /// 正常连通（收到 2xx / 3xx 响应）
    Ok,
    /// 可到达但服务端返回受限/鉴权或错误状态码（如 401/403/5xx）
    Restricted,
    /// 无法连接或请求超时
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct EndpointProbe {
    /// 稳定标识，前端按它查本地化显示名
    pub id: String,
    pub host: String,
    pub status: EndpointStatus,
    pub http_status: Option<u16>,
    pub latency_ms: Option<u32>,
    pub error: Option<String>,
}

struct Target {
    id: &'static str,
    host: &'static str,
    url: &'static str,
    use_get: bool,
}

const TARGETS: &[Target] = &[
    Target {
        id: "microsoftAuth",
        host: "login.microsoftonline.com",
        url: "https://login.microsoftonline.com/consumers/v2.0/.well-known/openid-configuration",
        use_get: false,
    },
    Target {
        id: "mojang",
        host: "piston-meta.mojang.com",
        url: "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
        use_get: false,
    },
    Target {
        id: "bmclapi",
        host: "bmclapi2.bangbang93.com",
        url: "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json",
        use_get: false,
    },
    Target {
        id: "modrinth",
        host: "api.modrinth.com",
        url: "https://api.modrinth.com/v2/tag/loader",
        use_get: false,
    },
    Target {
        id: "curseforge",
        host: "api.curseforge.com",
        url: "https://api.curseforge.com/v1/games",
        use_get: false,
    },
    Target {
        id: "mcim",
        host: "mod.mcimirror.top",
        url: "https://mod.mcimirror.top/",
        use_get: true,
    },
];

/// 并行探测 6 个关键服务端点的连通性与延迟。
pub async fn probe_endpoints(client: &Client) -> Vec<EndpointProbe> {
    let futures = TARGETS
        .iter()
        .map(|target| probe_one(client, target.id, target.host, target.url, target.use_get));
    join_all(futures).await
}

async fn probe_one(
    client: &Client,
    id: &str,
    host: &str,
    url: &str,
    use_get: bool,
) -> EndpointProbe {
    let started = Instant::now();
    let req = if use_get {
        client.get(url)
    } else {
        client.head(url)
    };
    let response = match req.timeout(PROBE_TIMEOUT).send().await {
        Ok(resp) if !use_get && resp.status().as_u16() == 405 => {
            // 部分边缘节点不支持 HEAD，自动回退一次轻 GET 复核
            client.get(url).timeout(PROBE_TIMEOUT).send().await
        }
        other => other,
    };
    let elapsed = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);

    match response {
        Ok(resp) => {
            let code = resp.status().as_u16();
            // CurseForge 无 Key 访问返回 403 但代表网络层完全通畅
            let ok = resp.status().is_success()
                || resp.status().is_redirection()
                || (id == "curseforge" && code == 403);
            EndpointProbe {
                id: id.to_owned(),
                host: host.to_owned(),
                status: if ok {
                    EndpointStatus::Ok
                } else {
                    EndpointStatus::Restricted
                },
                http_status: Some(code),
                latency_ms: Some(elapsed),
                error: None,
            }
        }
        Err(err) => {
            let reason = if err.is_timeout() {
                "timeout".to_owned()
            } else if err.is_connect() {
                "connect".to_owned()
            } else {
                crate::redact::redact(&err.to_string()).into_owned()
            };
            EndpointProbe {
                id: id.to_owned(),
                host: host.to_owned(),
                status: EndpointStatus::Failed,
                http_status: None,
                latency_ms: None,
                error: Some(reason),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::net::http::client_builder;

    #[tokio::test]
    async fn probes_local_endpoint_and_handles_head_405_fallback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let reply = if req.starts_with("HEAD ") {
                    "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                };
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });

        let client = client_builder().no_proxy().build().unwrap();
        let result = probe_one(
            &client,
            "mojang",
            "127.0.0.1",
            &format!("http://{addr}/test"),
            false,
        )
        .await;
        assert_eq!(result.status, EndpointStatus::Ok);
        assert_eq!(result.http_status, Some(200));
        assert!(result.latency_ms.is_some());
    }
}
