//! 系统代理热感知、多协议 Windows 代理字符串解析与休眠唤醒断连检测（全零 `unsafe`）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{ProxyMode, ProxySettings};

use super::doh::Doh;
use super::http::HttpError;

/// 挂钟相对单调时钟漂移超过该阈值时判定经历系统休眠/唤醒，需丢弃旧连接池。
const SLEEP_WAKE_DRIFT_SECS: i128 = 12;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemProxySnapshot {
    pub enabled: bool,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub auto_config_url: Option<String>,
}

impl SystemProxySnapshot {
    /// Windows 未开启静态代理（`ProxyEnable == 0`），仅设置了 PAC 自动配置脚本地址。
    /// `reqwest` 原生不执行 PAC 脚本，实际连接会走直连，界面需诚实提示。
    pub fn has_pac_only(&self) -> bool {
        !self.enabled && self.auto_config_url.is_some()
    }
}

/// 规范化代理条目；`socks` / `socks5` 强制升级为 `socks5h://` 防止本地 DNS 泄露。
fn normalize_proxy_url(raw: &str, default_scheme: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains(['\r', '\n', '\0']) {
        return None;
    }
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        let lower = scheme.to_ascii_lowercase();
        let target_scheme = match lower.as_str() {
            "socks" | "socks5" => "socks5h",
            "socks5h" | "socks4" | "socks4a" | "http" | "https" => lower.as_str(),
            _ => return None,
        };
        if rest.trim().is_empty() {
            return None;
        }
        return Some(format!("{target_scheme}://{}", rest.trim()));
    }
    Some(format!("{default_scheme}://{trimmed}"))
}

/// 解析 Windows 注册表 `ProxyServer` 字符串：
/// - 单值：`127.0.0.1:7890` → `all_proxy = Some("http://127.0.0.1:7890")`
/// - 多协议分号分隔：`http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7891`
pub fn parse_windows_proxy_server(raw: &str) -> (Option<String>, Option<String>, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, None, None);
    }
    if !trimmed.contains('=') {
        return (None, None, normalize_proxy_url(trimmed, "http"));
    }
    let mut http_proxy = None;
    let mut https_proxy = None;
    let mut all_proxy = None;
    for part in trimmed.split(';') {
        let part = part.trim();
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "http" => http_proxy = normalize_proxy_url(val, "http"),
            "https" => https_proxy = normalize_proxy_url(val, "http"),
            "socks" | "socks5" | "socks5h" => all_proxy = normalize_proxy_url(val, "socks5h"),
            _ => {}
        }
    }
    (http_proxy, https_proxy, all_proxy)
}

fn normalize_proxy_override(raw: &str) -> Option<String> {
    let mut entries = Vec::new();
    for item in raw.split([';', ',']) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if item.eq_ignore_ascii_case("<local>") {
            for local in ["localhost", "127.0.0.1", "::1"] {
                if !entries.iter().any(|e: &String| e == local) {
                    entries.push(local.to_owned());
                }
            }
        } else {
            entries.push(item.to_owned());
        }
    }
    (!entries.is_empty()).then(|| entries.join(","))
}

#[cfg(windows)]
pub fn read_system_proxy() -> SystemProxySnapshot {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(settings) =
        hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
    else {
        return SystemProxySnapshot::default();
    };
    let enabled = settings.get_value::<u32, _>("ProxyEnable").unwrap_or(0) != 0;
    let proxy_server = settings
        .get_value::<String, _>("ProxyServer")
        .unwrap_or_default();
    let proxy_override = settings
        .get_value::<String, _>("ProxyOverride")
        .unwrap_or_default();
    let auto_config_url = settings
        .get_value::<String, _>("AutoConfigURL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let (http_proxy, https_proxy, all_proxy) = if enabled {
        parse_windows_proxy_server(&proxy_server)
    } else {
        (None, None, None)
    };
    let no_proxy = if enabled {
        normalize_proxy_override(&proxy_override)
    } else {
        None
    };

    SystemProxySnapshot {
        enabled,
        http_proxy,
        https_proxy,
        all_proxy,
        no_proxy,
        auto_config_url,
    }
}

#[cfg(not(windows))]
pub fn read_system_proxy() -> SystemProxySnapshot {
    let read_var = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let http_proxy = read_var(&["HTTP_PROXY", "http_proxy"]);
    let https_proxy = read_var(&["HTTPS_PROXY", "https_proxy"]);
    let all_proxy = read_var(&["ALL_PROXY", "all_proxy"]);
    let no_proxy = read_var(&["NO_PROXY", "no_proxy"]);
    let enabled = http_proxy.is_some() || https_proxy.is_some() || all_proxy.is_some();
    SystemProxySnapshot {
        enabled,
        http_proxy,
        https_proxy,
        all_proxy,
        no_proxy,
        auto_config_url: None,
    }
}

/// 将 `ProxySettings` 与系统代理快照统一应用到 `reqwest::ClientBuilder`（客户端池与 DoH 客户端共用）。
pub fn apply_proxy_settings(
    builder: reqwest::ClientBuilder,
    proxy: &ProxySettings,
    sys_snap: Option<&SystemProxySnapshot>,
) -> Result<reqwest::ClientBuilder, HttpError> {
    match proxy.mode {
        ProxyMode::Direct => Ok(builder.no_proxy()),
        ProxyMode::Custom => {
            let Some(url) = proxy.custom_url() else {
                return Err(HttpError::InvalidProxy(format!(
                    "{}:{}",
                    proxy.host, proxy.port
                )));
            };
            let parsed = reqwest::Proxy::all(&url).map_err(HttpError::Build)?;
            Ok(builder.proxy(parsed))
        }
        ProxyMode::System => {
            let owned;
            let snap = match sys_snap {
                Some(s) => s,
                None => {
                    owned = read_system_proxy();
                    &owned
                }
            };
            apply_system_snapshot(builder, snap)
        }
    }
}

fn apply_system_snapshot(
    mut builder: reqwest::ClientBuilder,
    snap: &SystemProxySnapshot,
) -> Result<reqwest::ClientBuilder, HttpError> {
    if !snap.enabled {
        // 未启用静态系统代理时显式禁代理避免环境变量残留；PAC 暂不支持，外层会如实标注直连提示
        return Ok(builder.no_proxy());
    }

    let has_explicit =
        snap.http_proxy.is_some() || snap.https_proxy.is_some() || snap.all_proxy.is_some();
    if !has_explicit {
        return Ok(builder);
    }

    let no_proxy = snap
        .no_proxy
        .as_deref()
        .and_then(reqwest::NoProxy::from_string);
    // 先清理默认 sysproxy，再挂载精确多协议代理避免重复覆盖
    builder = builder.no_proxy();
    if let Some(url) = &snap.http_proxy {
        let p = reqwest::Proxy::http(url)
            .map_err(HttpError::Build)?
            .no_proxy(no_proxy.clone());
        builder = builder.proxy(p);
    }
    if let Some(url) = &snap.https_proxy {
        let p = reqwest::Proxy::https(url)
            .map_err(HttpError::Build)?
            .no_proxy(no_proxy.clone());
        builder = builder.proxy(p);
    }
    if let Some(url) = &snap.all_proxy {
        let p = reqwest::Proxy::all(url)
            .map_err(HttpError::Build)?
            .no_proxy(no_proxy);
        builder = builder.proxy(p);
    }
    Ok(builder)
}

#[derive(Debug)]
struct SleepWakeDetector {
    last_instant: Instant,
    last_wall: SystemTime,
}

impl SleepWakeDetector {
    fn new() -> Self {
        Self {
            last_instant: Instant::now(),
            last_wall: SystemTime::now(),
        }
    }

    fn check_with(&mut self, now_instant: Instant, now_wall: SystemTime) -> bool {
        let mono_ms = i128::try_from(
            now_instant
                .saturating_duration_since(self.last_instant)
                .as_millis(),
        )
        .unwrap_or(i128::MAX);
        let wall_ms = match now_wall.duration_since(self.last_wall) {
            Ok(d) => i128::try_from(d.as_millis()).unwrap_or(i128::MAX),
            Err(e) => -i128::try_from(e.duration().as_millis()).unwrap_or(i128::MAX),
        };
        self.last_instant = now_instant;
        self.last_wall = now_wall;
        (wall_ms - mono_ms).abs() > SLEEP_WAKE_DRIFT_SECS * 1000
    }

    fn check(&mut self) -> bool {
        self.check_with(Instant::now(), SystemTime::now())
    }
}

/// 监听系统代理热变更与系统休眠唤醒的统一状态守卫。
pub struct ProxyWatcher {
    epoch: AtomicU64,
    snapshot: RwLock<SystemProxySnapshot>,
    sleep: Mutex<SleepWakeDetector>,
}

impl ProxyWatcher {
    pub fn new() -> Self {
        Self {
            epoch: AtomicU64::new(1),
            snapshot: RwLock::new(read_system_proxy()),
            sleep: Mutex::new(SleepWakeDetector::new()),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub fn snapshot(&self) -> SystemProxySnapshot {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn bump_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 轮询一次系统代理与休眠唤醒；有变更返回 `Some((new_epoch, snapshot, woke_from_sleep))`。
    pub fn poll(&self, user_proxy: &ProxySettings) -> Option<(u64, SystemProxySnapshot, bool)> {
        let woke = self.sleep.lock().unwrap_or_else(|e| e.into_inner()).check();
        let next_snap = read_system_proxy();
        let proxy_changed = {
            let mut guard = self.snapshot.write().unwrap_or_else(|e| e.into_inner());
            if *guard != next_snap {
                *guard = next_snap.clone();
                matches!(user_proxy.mode, ProxyMode::System)
            } else {
                false
            }
        };
        if woke || proxy_changed {
            let epoch = self.bump_epoch();
            return Some((epoch, next_snap, woke));
        }
        None
    }

    /// 当前是否处于代理/TUN 接管环境下（用于爬坡退守门限）。
    pub fn is_proxied_env(&self, user_proxy: &ProxySettings, doh: &Doh) -> bool {
        match user_proxy.mode {
            ProxyMode::Custom => true,
            ProxyMode::Direct => doh.is_tun_active(),
            ProxyMode::System => self.snapshot().enabled || doh.is_tun_active(),
        }
    }
}

impl Default for ProxyWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// 供 `network-profiles.json` 与日志脱敏引用的代理粗分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyKindTag {
    Direct,
    SystemProxy,
    TunFakeIp,
    CustomHttp,
    CustomSocks5,
}

pub fn classify_proxy_tag(
    user_proxy: &ProxySettings,
    snap: &SystemProxySnapshot,
    tun_active: bool,
) -> ProxyKindTag {
    use crate::config::ProxyScheme;
    match user_proxy.mode {
        ProxyMode::Custom => match user_proxy.scheme {
            ProxyScheme::Http => ProxyKindTag::CustomHttp,
            ProxyScheme::Socks5 => ProxyKindTag::CustomSocks5,
        },
        ProxyMode::Direct => {
            if tun_active {
                ProxyKindTag::TunFakeIp
            } else {
                ProxyKindTag::Direct
            }
        }
        ProxyMode::System => {
            if snap.enabled {
                ProxyKindTag::SystemProxy
            } else if tun_active {
                ProxyKindTag::TunFakeIp
            } else {
                ProxyKindTag::Direct
            }
        }
    }
}

pub fn ping_sleep_drift_tolerance() -> Duration {
    Duration::from_secs(u64::try_from(SLEEP_WAKE_DRIFT_SECS).unwrap_or(12))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_single_and_multi_protocol_proxy_strings() {
        assert_eq!(
            parse_windows_proxy_server("127.0.0.1:7890"),
            (None, None, Some("http://127.0.0.1:7890".to_owned()))
        );
        assert_eq!(
            parse_windows_proxy_server(
                "http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7891"
            ),
            (
                Some("http://127.0.0.1:7890".to_owned()),
                Some("http://127.0.0.1:7890".to_owned()),
                Some("socks5h://127.0.0.1:7891".to_owned()),
            )
        );
        assert_eq!(
            parse_windows_proxy_server("socks=socks5://[::1]:1080"),
            (None, None, Some("socks5h://[::1]:1080".to_owned()))
        );
    }

    #[test]
    fn normalizes_windows_proxy_override_with_local_token() {
        assert_eq!(
            normalize_proxy_override("localhost;10.*;<local>").as_deref(),
            Some("localhost,10.*,127.0.0.1,::1")
        );
    }

    #[test]
    fn detects_wall_clock_sleep_wake_drift() {
        let base_mono = Instant::now();
        let base_wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut detector = SleepWakeDetector {
            last_instant: base_mono,
            last_wall: base_wall,
        };
        assert!(!detector.check_with(
            base_mono + Duration::from_secs(3),
            base_wall + Duration::from_secs(3),
        ));
        assert!(detector.check_with(
            base_mono + Duration::from_secs(6),
            base_wall + Duration::from_secs(30),
        ));
    }
}
