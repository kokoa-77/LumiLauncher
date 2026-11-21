//! 日志手动上传至 mclo.gs。
//!
//! 严禁自动上传！必须由用户点击确认后调用；上传前强制经过 [`crate::redact::redact`] 脱敏，
//! 并额外抹除本机 Windows 用户目录名，保护玩家隐私与账号凭据。

use std::sync::LazyLock;

use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const MCLOGS_ENDPOINT: &str = "https://api.mclo.gs/1/log";
/// 上传最大字符数（mclo.gs 单条建议控制在 2 MiB 内，超出只保留尾部最新日志）
const MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MclogsUpload {
    pub url: String,
    pub raw: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MclogsError {
    #[error("日志内容为空，无法上传")]
    Empty,
    #[error("网络请求失败：{0}")]
    Http(#[from] reqwest::Error),
    #[error("mclo.gs 服务返回异常：{0}")]
    Service(String),
}

#[derive(Deserialize)]
struct MclogsResponse {
    success: bool,
    url: Option<String>,
    raw: Option<String>,
    error: Option<String>,
}

#[allow(clippy::expect_used)] // 常量正则，单元测试覆盖
static USER_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)([A-Z]:\\Users\\)([^\\/\r\n"'\s]+)"#).expect("用户路径脱敏正则必须合法")
});

/// 脱敏并裁剪待上传日志：去令牌、去 JWT、隐藏 Windows 用户名路径，超出 2 MiB 保留尾部最新片段。
pub fn sanitize_for_upload(raw: &str) -> String {
    let redacted = crate::redact::redact(raw);
    let masked = USER_PATH_RE.replace_all(&redacted, "${1}<user>");
    let text = masked.trim();
    if text.len() <= MAX_UPLOAD_BYTES {
        return text.to_owned();
    }
    // 截断时对齐 UTF-8 字符边界，保留尾部最新日志
    let mut start = text.len().saturating_sub(MAX_UPLOAD_BYTES);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "...(trimmed earlier lines to fit 2 MiB upload limit)...\n{}",
        &text[start..]
    )
}

/// 将脱敏后的日志手动上传至 mclo.gs，返回可分享的网页链接与纯文本链接。
pub async fn upload_log(client: &Client, raw_log: &str) -> Result<MclogsUpload, MclogsError> {
    let sanitized = sanitize_for_upload(raw_log);
    if sanitized.is_empty() {
        return Err(MclogsError::Empty);
    }
    let encoded = format!("content={}", super::url_encode(&sanitized));
    let response = client
        .post(MCLOGS_ENDPOINT)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(encoded)
        .send()
        .await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    let parsed: MclogsResponse = serde_json::from_slice(&bytes)
        .map_err(|e| MclogsError::Service(format!("解析返回数据失败：{e}")))?;
    if !status.is_success() || !parsed.success {
        return Err(MclogsError::Service(
            parsed.error.unwrap_or_else(|| format!("HTTP {status}")),
        ));
    }
    let url = parsed
        .url
        .ok_or_else(|| MclogsError::Service("未返回日志链接".into()))?;
    let raw = parsed.raw.unwrap_or_else(|| url.clone());
    Ok(MclogsUpload { url, raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_tokens_and_windows_user_paths() {
        let input = r#"Running from C:\Users\Alice\AppData\Roaming\.minecraft with --accessToken secret123 and Authorization: Bearer tok999"#;
        let out = sanitize_for_upload(input);
        assert!(out.contains(r"C:\Users\<user>\AppData"));
        assert!(!out.contains("Alice"));
        assert!(!out.contains("secret123"));
        assert!(!out.contains("tok999"));
    }
}
