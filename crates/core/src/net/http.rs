//! 统一的 HTTP 客户端：所有请求都带同一个 User-Agent、同样的超时和连接池。

use std::time::Duration;

use reqwest::Client;
use serde::de::DeserializeOwned;

/// 所有对外请求统一使用的客户端标识（「名字/版本」）。
/// 2026-11-15 定名「微光 / Lumi」，以前是代号 `UnnamedLauncher`
pub const USER_AGENT: &str = concat!("LumiLauncher/", env!("CARGO_PKG_VERSION"));

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

/// 创建启动器统一使用的 HTTP 客户端（会跟随 Windows 系统代理）。
/// 应用里用的是 [`super::build_client_with_doh`]（多了 DoH），命令行工具和测试用这个。
pub fn build_client() -> Result<Client, HttpError> {
    client_builder().build().map_err(HttpError::Build)
}

/// 两种客户端共用的设置
pub(super) fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(32)
}

/// 按顺序尝试候选地址，返回第一个成功的响应内容。
pub async fn get_bytes(client: &Client, urls: &[String]) -> Result<Vec<u8>, HttpError> {
    let mut last = String::from("没有可用的地址");
    for url in urls {
        match get_one(client, url).await {
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
    let request_error = |source| HttpError::Request {
        url: url.to_owned(),
        source,
    };
    let response = client.get(url).send().await.map_err(request_error)?;
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

    /// 所有请求都带 `名字/版本` 格式的 User-Agent
    #[test]
    fn user_agent_is_name_slash_version() {
        let (name, version) = USER_AGENT.split_once('/').unwrap();
        assert!(!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(
            version.split('.').count() == 3 && version.split('.').all(|p| p.parse::<u32>().is_ok())
        );
    }

    /// 本机小服务器：记下每个请求的路径和 UA；`/old` 302 跳到 `/file`（验证重定向保持 UA）
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

    /// 查元数据、下文件、跳转以后的请求，UA 都一样（MCLF-CN/docs#2：不能有遗漏）
    #[tokio::test]
    async fn every_request_carries_the_same_user_agent() {
        let (base, seen) = recording_server().await;
        // 和应用里同一套设置；测试连本机，绕开系统代理
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
}
