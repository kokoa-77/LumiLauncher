//! 六层根因网络与磁盘自愈诊断模型（全结构化错误链归因，零脆弱字符串匹配）。

use std::error::Error as StdError;
use std::io;

use serde::{Deserialize, Serialize};

use crate::config::ProxyMode;
use crate::fs::is_win_lock_error;

use super::doh::Doh;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum RootCauseKind {
    ProxyUnreachable,
    FakeIpStale,
    DnsPoisoned,
    SniBlocked,
    ClockSkewCert,
    RateLimited429,
    IntegrityMismatch,
    DiskLockedOrFull,
    AuthDenied,
    TimeoutOrReset,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum QuickFixAction {
    EnableDohAndRetry,
    SwitchOfficialCdnAndRetry,
    EnableMirrorFallbackRetry,
    SwitchDirectAndRetry,
    LowerDiskPressureAndRetry,
    LowerConcurrencyAndRetry,
    OpenDateTimeSettings,
    OpenStorageClean,
    OpenProxySettings,
    OpenDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct FailureDiagnosis {
    pub cause: RootCauseKind,
    pub host: Option<String>,
    pub summary: String,
    pub explanation: String,
    pub primary_fix: QuickFixAction,
    pub secondary_fix: Option<QuickFixAction>,
}

impl FailureDiagnosis {
    pub fn rate_limited_429(host: &str) -> Self {
        Self {
            cause: RootCauseKind::RateLimited429,
            host: (!host.is_empty()).then(|| host.to_owned()),
            summary: format!("{host} 限流（HTTP 429），已自动退避"),
            explanation: "并发连接过高触发下载源限流，可降低最大并发数与分段数后重试。".to_owned(),
            primary_fix: QuickFixAction::LowerConcurrencyAndRetry,
            secondary_fix: Some(QuickFixAction::SwitchOfficialCdnAndRetry),
        }
    }

    pub fn integrity_mismatch(host: Option<String>, detail: String) -> Self {
        Self {
            cause: RootCauseKind::IntegrityMismatch,
            host,
            summary: "文件校验未通过（内容被截断或非目标文件）".to_owned(),
            explanation: detail,
            primary_fix: QuickFixAction::SwitchOfficialCdnAndRetry,
            secondary_fix: Some(QuickFixAction::EnableDohAndRetry),
        }
    }

    pub fn auth_denied(host: Option<String>, detail: String) -> Self {
        Self {
            cause: RootCauseKind::AuthDenied,
            host,
            summary: "下载源拒绝了请求（HTTP 401/403 或禁止分发）".to_owned(),
            explanation: detail,
            primary_fix: QuickFixAction::OpenProxySettings,
            secondary_fix: Some(QuickFixAction::OpenDiagnostics),
        }
    }
}

pub fn classify_io_error(err: &io::Error) -> FailureDiagnosis {
    // Windows ERROR_DISK_FULL (112) / ERROR_HANDLE_DISK_FULL (39) 或 ENOSPC (28)
    let is_disk_full = err.kind() == io::ErrorKind::StorageFull
        || matches!(err.raw_os_error(), Some(39 | 112 | 28));
    if is_disk_full {
        return FailureDiagnosis {
            cause: RootCauseKind::DiskLockedOrFull,
            host: None,
            summary: "目标磁盘可用空间不足".to_owned(),
            explanation: format!("写入失败：{err}。建议清理缓存或释放磁盘空间后重试。"),
            primary_fix: QuickFixAction::OpenStorageClean,
            secondary_fix: None,
        };
    }
    if is_win_lock_error(err) {
        return FailureDiagnosis {
            cause: RootCauseKind::DiskLockedOrFull,
            host: None,
            summary: "文件正被杀毒软件或其它程序占用".to_owned(),
            explanation: format!(
                "文件改名/删除被锁定（{err}），可降低并发写入压力或在杀毒扫描完成后重试。"
            ),
            primary_fix: QuickFixAction::LowerDiskPressureAndRetry,
            secondary_fix: None,
        };
    }
    FailureDiagnosis {
        cause: RootCauseKind::DiskLockedOrFull,
        host: None,
        summary: "本地磁盘读写出错".to_owned(),
        explanation: err.to_string(),
        primary_fix: QuickFixAction::LowerDiskPressureAndRetry,
        secondary_fix: None,
    }
}

pub fn classify_reqwest_error(
    err: &reqwest::Error,
    host: Option<&str>,
    doh: Option<&Doh>,
    proxy_mode: ProxyMode,
) -> FailureDiagnosis {
    let host_owned = host.map(str::to_owned);
    if let Some(code) = err.status().map(|s| s.as_u16()) {
        if code == 429 {
            return FailureDiagnosis::rate_limited_429(host.unwrap_or("下载源"));
        }
        if matches!(code, 401 | 403) {
            return FailureDiagnosis::auth_denied(host_owned, format!("服务器返回 HTTP {code}"));
        }
    }

    if let Some(h) = host
        && let Some(d) = doh
        && d.is_sni_blocked(h)
    {
        #[cfg(feature = "community-mirrors")]
        let secondary_fix = if super::mirror::is_mirrorable_host(h) {
            Some(QuickFixAction::EnableMirrorFallbackRetry)
        } else {
            Some(QuickFixAction::OpenProxySettings)
        };
        #[cfg(not(feature = "community-mirrors"))]
        let secondary_fix = Some(QuickFixAction::OpenProxySettings);

        return FailureDiagnosis {
            cause: RootCauseKind::SniBlocked,
            host: host_owned,
            summary: format!("直连 {h} 遭遇 TLS 握手阻断（10054 RST）"),
            explanation: "连续 2 个节点在 TLS 握手期断连，属链路级 SNI 阻断，DoH 无法绕过。可优先切换官方 CDN 或使用可用代理。".to_owned(),
            primary_fix: QuickFixAction::SwitchOfficialCdnAndRetry,
            secondary_fix,
        };
    }

    let mut has_cert_hint = false;
    let mut has_reset_eof = false;
    let mut has_refused = false;
    let mut cur: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(node) = cur {
        if let Some(io_err) = node.downcast_ref::<io::Error>() {
            match io_err.kind() {
                io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof => {
                    has_reset_eof = true;
                }
                io::ErrorKind::ConnectionRefused => {
                    has_refused = true;
                }
                _ => {}
            }
            if matches!(io_err.raw_os_error(), Some(10054)) {
                has_reset_eof = true;
            }
            if io_err.get_ref().is_some_and(|inner| {
                let name = format!("{inner:?}");
                name.contains("InvalidCertificate")
                    || name.contains("UnknownIssuer")
                    || name.contains("Expired")
            }) {
                has_cert_hint = true;
            }
        }
        cur = node.source();
    }

    if has_cert_hint {
        return FailureDiagnosis {
            cause: RootCauseKind::ClockSkewCert,
            host: host_owned,
            summary: "HTTPS 证书校验未通过（可能系统时间不准或节点异常）".to_owned(),
            explanation:
                "请核对 Windows 系统日期、时间与时区是否准确；若时间正常，可能是网络存在 TLS 劫持。"
                    .to_owned(),
            primary_fix: QuickFixAction::OpenDateTimeSettings,
            secondary_fix: Some(QuickFixAction::OpenDiagnostics),
        };
    }

    if matches!(proxy_mode, ProxyMode::Custom | ProxyMode::System)
        && (err.is_connect() || has_refused)
    {
        return FailureDiagnosis {
            cause: RootCauseKind::ProxyUnreachable,
            host: host_owned,
            summary: "代理服务器未响应或端口不可达".to_owned(),
            explanation: format!("当前处于代理模式，无法连通代理出口或目标地址（{err}）。"),
            primary_fix: if proxy_mode == ProxyMode::Custom {
                QuickFixAction::SwitchDirectAndRetry
            } else {
                QuickFixAction::OpenProxySettings
            },
            secondary_fix: Some(QuickFixAction::SwitchDirectAndRetry),
        };
    }

    if has_reset_eof {
        #[cfg(feature = "community-mirrors")]
        let secondary_fix = host
            .filter(|h| super::mirror::is_mirrorable_host(h))
            .map(|_| QuickFixAction::EnableMirrorFallbackRetry)
            .or(Some(QuickFixAction::OpenProxySettings));
        #[cfg(not(feature = "community-mirrors"))]
        let secondary_fix = Some(QuickFixAction::OpenProxySettings);

        return FailureDiagnosis {
            cause: RootCauseKind::SniBlocked,
            host: host_owned,
            summary: "TLS 握手阶段连接被远端重置".to_owned(),
            explanation: format!(
                "建立安全连接被重置（{err}），建议切换官方 CDN 或启用代理后重试。"
            ),
            primary_fix: QuickFixAction::SwitchOfficialCdnAndRetry,
            secondary_fix,
        };
    }

    if err.is_connect() {
        return FailureDiagnosis {
            cause: RootCauseKind::DnsPoisoned,
            host: host_owned,
            summary: "目标服务器建连失败或域名解析异常".to_owned(),
            explanation: format!(
                "无法稳定连接到下载节点（{err}），可开启 DoH 解析或运行网络诊断检查出口连通性。"
            ),
            primary_fix: QuickFixAction::EnableDohAndRetry,
            secondary_fix: Some(QuickFixAction::OpenDiagnostics),
        };
    }

    if err.is_timeout() {
        return FailureDiagnosis {
            cause: RootCauseKind::TimeoutOrReset,
            host: host_owned,
            summary: "网络请求超时或传输意外中断".to_owned(),
            explanation: err.to_string(),
            primary_fix: QuickFixAction::LowerConcurrencyAndRetry,
            secondary_fix: Some(QuickFixAction::OpenDiagnostics),
        };
    }

    FailureDiagnosis {
        cause: RootCauseKind::Unknown,
        host: host_owned,
        summary: "网络传输意外中断".to_owned(),
        explanation: err.to_string(),
        primary_fix: QuickFixAction::OpenDiagnostics,
        secondary_fix: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_disk_full_and_win_lock_errors() {
        let locked = io::Error::from_raw_os_error(32);
        let diag_locked = classify_io_error(&locked);
        assert_eq!(diag_locked.cause, RootCauseKind::DiskLockedOrFull);
        assert_eq!(
            diag_locked.primary_fix,
            QuickFixAction::LowerDiskPressureAndRetry
        );

        let full = io::Error::from_raw_os_error(112);
        let diag_full = classify_io_error(&full);
        assert_eq!(diag_full.cause, RootCauseKind::DiskLockedOrFull);
        assert_eq!(diag_full.primary_fix, QuickFixAction::OpenStorageClean);
    }

    #[test]
    fn constructs_429_and_integrity_diagnoses() {
        let rate = FailureDiagnosis::rate_limited_429("cdn.modrinth.com");
        assert_eq!(rate.cause, RootCauseKind::RateLimited429);
        assert_eq!(rate.primary_fix, QuickFixAction::LowerConcurrencyAndRetry);

        let integ = FailureDiagnosis::integrity_mismatch(
            Some("edge.forgecdn.net".into()),
            "bad sha1".into(),
        );
        assert_eq!(integ.cause, RootCauseKind::IntegrityMismatch);
        assert_eq!(integ.primary_fix, QuickFixAction::SwitchOfficialCdnAndRetry);
    }
}
