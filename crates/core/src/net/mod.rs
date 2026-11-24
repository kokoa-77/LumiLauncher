//! 网络层：统一的 HTTP 客户端、并发校验下载引擎、DoH 与连通性诊断，以及查询 Minecraft 服务器状态（`status`、`srv`）。
//!
//! 所有元数据与文件下载均直连官方源（Mojang、各加载器官方仓库、Modrinth、CurseForge）。

pub mod diagnostics;
pub mod doh;
pub mod download;
pub mod http;
pub mod mclogs;
pub mod srv;
pub mod status;

pub use diagnostics::{EndpointProbe, EndpointStatus, probe_endpoints};
pub use doh::{DnsProbe, Doh, build_client_with_doh, build_client_with_proxy};
pub use download::{
    DENIED_PREFIX, DownloadError, DownloadItem, DownloadProgress, DownloadReport, Downloader,
    FileFailure, HostHeader, sha1_file,
};
pub use http::{HttpError, build_client, get_bytes, get_json};
pub use mclogs::{MclogsError, MclogsUpload, sanitize_for_upload, upload_log};

/// `https://` 地址的主机名（小写，去掉用户名和端口）。不是 https 的返回 `None`。
pub fn url_host(url: &str) -> Option<String> {
    let scheme = url.get(..8)?;
    if !scheme.eq_ignore_ascii_case("https://") {
        return None;
    }
    let rest = &url[8..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// 网址参数编码（只保留字母数字和 `-_.~`，其余按 UTF-8 字节转义）
pub(crate) fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 把哈希结果转成小写十六进制字符串。
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // 写入 String 不会失败
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_https_hosts_only() {
        assert_eq!(
            url_host("https://cdn.modrinth.com/data/x").as_deref(),
            Some("cdn.modrinth.com")
        );
        assert_eq!(
            url_host("HTTPS://User@Edge.ForgeCDN.net:443/a?b").as_deref(),
            Some("edge.forgecdn.net")
        );
        assert_eq!(url_host("http://cdn.modrinth.com/x"), None);
        assert_eq!(url_host("https:///x"), None);
        assert_eq!(url_host("ftp://x"), None);
    }
}
