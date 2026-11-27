//! RFC 8484 纯 IP 分层 DoH、Fake-IP 租约状态机、双栈治理与 Transport-to-DNS 精准反馈。
//!
//! - 全程采用纯 IP `POST {endpoint}` + `application/dns-message`（RFC 8484 二进制协议），解析 DoH 端点本身零 DNS 依赖；
//! - `PoisonProneOverseas`（`neoforged.net` / `modrinth.com` / `curseforge.com` / `forgecdn.net` / `github.com` 等）
//!   严格隔离在 `OverseasClean`（`1.0.0.1` / `1.1.1.1` / `9.9.9.9`），绝不回退国内 DNS 防缓存污染；
//! - 8s TUN 租约 + 20s `FakeIpBypass` 冷却：关闭 Clash TUN 后击穿 Windows `Dnscache` 残留 `198.18.x.y`；
//! - 每次主连接仅投放 1 个优选主 IP 并记 `last_served`，精准支持 `1-Rotate → 2-RST → SniBlocked(60s)` 闭环。

use std::collections::HashMap;
use std::error::Error as StdError;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use reqwest::Client;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::sync::watch;

use crate::config::ProxySettings;

use super::dns_wire::{self, DnsWireOutcome, TYPE_A, TYPE_AAAA};
use super::http::{HttpError, USER_AGENT};
use super::proxy_watch::{SystemProxySnapshot, apply_proxy_settings};

/// 海外干净纯 IP RFC 8484 DoH 节点池（P0 `1.0.0.1` 规避 Cloudflare `1.1.1.1` 部分地区 QoS 限流）。
const OVERSEAS_POOL: &[ProviderDef] = &[
    ProviderDef {
        id: "cf_secondary",
        endpoint: "https://1.0.0.1/dns-query",
    },
    ProviderDef {
        id: "cf_primary",
        endpoint: "https://1.1.1.1/dns-query",
    },
    ProviderDef {
        id: "quad9",
        endpoint: "https://9.9.9.9/dns-query",
    },
];

/// 国内纯 IP RFC 8484 DoH 节点池。
const DOMESTIC_POOL: &[ProviderDef] = &[
    ProviderDef {
        id: "dnspod",
        endpoint: "https://1.12.12.12/dns-query",
    },
    ProviderDef {
        id: "alidns",
        endpoint: "https://223.5.5.5/dns-query",
    },
];

const POISON_PRONE_OVERSEAS: &[&str] = &[
    "neoforged.net",
    "github.com",
    "githubusercontent.com",
    "modrinth.com",
    "curseforge.com",
    "forgecdn.net",
    "minecraftforge.net",
    "fabricmc.net",
    "quiltmc.org",
    "legacyfabric.net",
    "glass-launcher.net",
    "maven.org",
    "adoptium.net",
];

const GLOBAL_ANYCAST_OFFICIAL: &[&str] = &[
    "mojang.com",
    "minecraft.net",
    "minecraftservices.com",
    "microsoftonline.com",
    "xboxlive.com",
];

const DOMESTIC_OR_MIRROR: &[&str] = &["cn", "aliyuncs.com", "aliyun.com"];

const SYSTEM_PEEK_TIMEOUT: Duration = Duration::from_millis(400);
const QUERY_CONNECT_TIMEOUT: Duration = Duration::from_millis(1200);
const QUERY_TOTAL_TIMEOUT: Duration = Duration::from_millis(2200);
const HEDGE_DELAY: Duration = Duration::from_millis(180);
const TUN_LEASE_DURATION: Duration = Duration::from_secs(30);
const FAKE_IP_RECENT_OK_WINDOW: Duration = Duration::from_secs(5);
const FAKE_IP_BYPASS_DURATION: Duration = Duration::from_secs(20);
const SNI_BLOCKED_DURATION: Duration = Duration::from_secs(60);
const HOST_DEGRADED_DURATION: Duration = Duration::from_secs(120);
const FIRST_RST_WINDOW: Duration = Duration::from_secs(60);
const BAD_IP_COOLDOWN: Duration = Duration::from_secs(120);
const CERT_POISON_COOLDOWN: Duration = Duration::from_secs(600);
const IPV6_SUPPRESS_DURATION: Duration = Duration::from_secs(600);
const MIN_TTL_SECS: u32 = 30;
const SYSTEM_FALLBACK_TTL_SECS: u32 = 30;
const MAX_TTL_SECS: u32 = 600;
const MAX_CACHED_HOSTS: usize = 512;
const COOLDOWN_STEPS_SECS: [u64; 3] = [30, 60, 120];
const MAX_POP_DISCOVERY_PROVIDERS: usize = 3;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy)]
struct ProviderDef {
    id: &'static str,
    endpoint: &'static str,
}

#[derive(Debug, Clone)]
struct ProviderSpec {
    id: &'static str,
    endpoint: std::borrow::Cow<'static, str>,
}

impl From<ProviderDef> for ProviderSpec {
    fn from(def: ProviderDef) -> Self {
        Self {
            id: def.id,
            endpoint: std::borrow::Cow::Borrowed(def.endpoint),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainClass {
    PoisonProneOverseas,
    GlobalAnycastOfficial,
    DomesticOrMirror,
    OtherPublic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportDnsAction {
    RetryOnceSingleStream,
    SniBlockedSwitchSource,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectFailureKind {
    HandshakeResetOrEof,
    TlsCertMismatch,
    TimeoutOrUnreachable,
    Other,
}

#[derive(Debug, Clone)]
enum TunLeaseState {
    Normal,
    TunActive { lease_until: Instant },
    FakeIpBypass { until: Instant },
}

#[derive(Debug, Clone)]
struct ProviderHealth {
    ewma_rtt_ms: f64,
    consecutive_failures: u32,
    poison_strikes: u32,
    cooldown_until: Option<Instant>,
    cooldown_level: u8,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            ewma_rtt_ms: 120.0,
            consecutive_failures: 0,
            poison_strikes: 0,
            cooldown_until: None,
            cooldown_level: 0,
        }
    }
}

impl ProviderHealth {
    fn is_cooling(&self, now: Instant) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }

    fn score(&self) -> f64 {
        self.ewma_rtt_ms
            + 300.0 * f64::from(self.consecutive_failures)
            + 1000.0 * f64::from(self.poison_strikes)
    }

    fn record_ok(&mut self, rtt_ms: f64) {
        self.ewma_rtt_ms = 0.7 * self.ewma_rtt_ms + 0.3 * rtt_ms.clamp(5.0, 2200.0);
        self.consecutive_failures = 0;
        self.cooldown_until = None;
        self.cooldown_level = 0;
    }

    fn record_failure(&mut self, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        // 节点一旦超时或建连失败立即进入独立指数冷却（30s -> 60s -> 120s），绝不让死节点拖慢每次请求
        let idx = usize::from(self.cooldown_level).min(COOLDOWN_STEPS_SECS.len() - 1);
        self.cooldown_until = Some(now + Duration::from_secs(COOLDOWN_STEPS_SECS[idx]));
        self.cooldown_level = self.cooldown_level.saturating_add(1);
    }

    fn record_poison(&mut self, now: Instant) {
        self.poison_strikes = self.poison_strikes.saturating_add(1);
        self.cooldown_until = Some(now + Duration::from_secs(120));
    }
}

#[derive(Debug, Clone)]
struct CachedHost {
    ips: Vec<IpAddr>,
    provider_id: &'static str,
    expires: Instant,
}

#[derive(Debug, Clone)]
struct ServedRecord {
    ip: IpAddr,
    provider_id: Option<&'static str>,
}

type SingleflightPayload = Option<Result<Vec<IpAddr>, String>>;
type InFlightMap = HashMap<String, watch::Receiver<SingleflightPayload>>;

struct SingleflightGuard<'a> {
    map: &'a Mutex<InFlightMap>,
    host: String,
    tx: Option<watch::Sender<SingleflightPayload>>,
    done: bool,
}

impl Drop for SingleflightGuard<'_> {
    fn drop(&mut self) {
        if !self.done
            && let Some(tx) = self.tx.take()
        {
            let _ = tx.send(Some(Err("DoH 查询被取消".to_owned())));
        }
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.host);
    }
}

/// DoH 解析器。`Arc<Doh>` 同时服务下载引擎、代理热重载与设置页「测试 DoH」。
pub struct Doh {
    enabled: AtomicBool,
    metered: AtomicBool,
    doh_client: RwLock<Client>,
    custom_servers: Option<Vec<String>>,
    custom_doh_url: RwLock<Option<String>>,
    cache: Mutex<HashMap<String, CachedHost>>,
    in_flight: Mutex<InFlightMap>,
    providers: Mutex<HashMap<String, ProviderHealth>>,
    tun_state: Mutex<TunLeaseState>,
    last_fake_ip_ok: Mutex<Option<Instant>>,
    last_served: Mutex<HashMap<String, ServedRecord>>,
    bad_ips: Mutex<HashMap<(String, IpAddr), Instant>>,
    preferred_pops: Mutex<HashMap<String, (IpAddr, Instant)>>,
    demoted_pops: Mutex<HashMap<(String, IpAddr), Instant>>,
    first_rst: Mutex<HashMap<String, (IpAddr, Instant)>>,
    connect_failures: Mutex<HashMap<String, (u32, Instant)>>,
    sni_blocked_until: Mutex<HashMap<String, Instant>>,
    host_degraded_until: Mutex<HashMap<String, Instant>>,
    ipv6_failures: Mutex<u32>,
    ipv6_suppressed_until: Mutex<Option<Instant>>,
}

impl Doh {
    pub fn new(enabled: bool) -> Result<Self, HttpError> {
        Self::build_internal(enabled, None)
    }

    #[cfg(test)]
    fn with_servers(enabled: bool, servers: Vec<String>) -> Result<Self, HttpError> {
        Self::build_internal(enabled, Some(servers))
    }

    fn build_internal(
        enabled: bool,
        custom_servers: Option<Vec<String>>,
    ) -> Result<Self, HttpError> {
        let doh_client = build_doh_client(&ProxySettings::default(), None)?;
        Ok(Self {
            enabled: AtomicBool::new(enabled),
            metered: AtomicBool::new(false),
            doh_client: RwLock::new(doh_client),
            custom_servers,
            custom_doh_url: RwLock::new(None),
            cache: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            providers: Mutex::new(HashMap::new()),
            tun_state: Mutex::new(TunLeaseState::Normal),
            last_fake_ip_ok: Mutex::new(None),
            last_served: Mutex::new(HashMap::new()),
            bad_ips: Mutex::new(HashMap::new()),
            preferred_pops: Mutex::new(HashMap::new()),
            demoted_pops: Mutex::new(HashMap::new()),
            first_rst: Mutex::new(HashMap::new()),
            connect_failures: Mutex::new(HashMap::new()),
            sni_blocked_until: Mutex::new(HashMap::new()),
            host_degraded_until: Mutex::new(HashMap::new()),
            ipv6_failures: Mutex::new(0),
            ipv6_suppressed_until: Mutex::new(None),
        })
    }

    /// 同步内部 DoH HTTP 客户端的代理模式（如设为 `Direct` 时 DoH 也绝不走残留系统代理）。
    pub fn sync_proxy(&self, proxy: &ProxySettings) -> Result<(), HttpError> {
        self.sync_proxy_with_snapshot(proxy, None)
    }

    pub fn sync_proxy_with_snapshot(
        &self,
        proxy: &ProxySettings,
        sys_snap: Option<&SystemProxySnapshot>,
    ) -> Result<(), HttpError> {
        let next = build_doh_client(proxy, sys_snap)?;
        *self.doh_client.write().unwrap_or_else(|e| e.into_inner()) = next;
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            tracing::info!(enabled, "DoH 开关变了");
            self.on_network_epoch_change();
        }
    }

    pub fn set_custom_url(&self, url: Option<String>) {
        let changed = {
            let mut guard = self
                .custom_doh_url
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if *guard != url {
                *guard = url;
                true
            } else {
                false
            }
        };
        if changed {
            tracing::info!("自定义 DoH 地址已更新");
            self.on_network_epoch_change();
        }
    }

    /// 返回当前最健康 DoH 节点的简短摘要（供设置页状态条展示），未启用时返回 `None`。
    pub fn best_provider_summary(&self) -> Option<String> {
        if !self.is_enabled() {
            return None;
        }
        if self.is_tun_active() {
            return Some("代理接管 DNS".to_owned());
        }
        let now = Instant::now();
        let health = self.providers.lock().unwrap_or_else(|e| e.into_inner());
        let mut best: Option<(&str, f64)> = None;
        for (id, st) in health.iter() {
            if st.is_cooling(now) || st.consecutive_failures > 0 {
                continue;
            }
            if best.is_none_or(|(_, ms)| st.ewma_rtt_ms < ms) {
                best = Some((id.as_str(), st.ewma_rtt_ms));
            }
        }
        let (id, rtt) = best?;
        let label = match id {
            "user_custom" => "自定义 DoH",
            "cf_primary" | "cf_secondary" => "Cloudflare DoH",
            "quad9" => "Quad9 DoH",
            "dnspod" => "DNSPod DoH",
            "alidns" => "阿里 DoH",
            _ => "DoH",
        };
        Some(format!("{label} {}ms", rtt.round() as u32))
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_metered(&self, metered: bool) {
        self.metered.store(metered, Ordering::Relaxed);
    }

    pub fn is_metered(&self) -> bool {
        self.metered.load(Ordering::Relaxed)
    }

    pub fn is_tun_active(&self) -> bool {
        let mut state = self.tun_state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            TunLeaseState::TunActive { lease_until } if Instant::now() < lease_until => true,
            TunLeaseState::TunActive { .. } => {
                *state = TunLeaseState::Normal;
                false
            }
            _ => false,
        }
    }

    pub fn is_sni_blocked(&self, host: &str) -> bool {
        let key = normalize_host(host);
        let mut map = self
            .sni_blocked_until
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match map.get(&key).copied() {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                map.remove(&key);
                false
            }
            None => false,
        }
    }

    /// 主机是否处于 SNI 阻断或连续建连超时降级状态（供下载器与官方互备 CDN 秒切跳过）。
    pub fn is_host_degraded(&self, host: &str) -> bool {
        if self.is_sni_blocked(host) {
            return true;
        }
        let key = normalize_host(host);
        let mut map = self
            .host_degraded_until
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match map.get(&key).copied() {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                map.remove(&key);
                false
            }
            None => false,
        }
    }

    /// 注入官方多 `/24` 比测胜出的优选边缘 IP（严格经过 Gate 2 全局单播检查）。
    pub fn set_preferred_pop_ip(&self, host: &str, ip: IpAddr, ttl: Duration) -> bool {
        if !dns_wire::is_strictly_global_unicast(ip) {
            return false;
        }
        let key = normalize_host(host);
        let until = Instant::now() + ttl;
        self.preferred_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), (ip, until));
        self.demoted_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(key, ip));
        true
    }

    /// 清除当前主机的优选 POP IP，可选将其加入短暂降级名单（避免立即重选回慢边缘）。
    pub fn clear_preferred_pop_ip(&self, host: &str, demote: Option<(IpAddr, Duration)>) {
        let key = normalize_host(host);
        self.preferred_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        if let Some((ip, ttl)) = demote {
            self.demoted_pops
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((key, ip), Instant::now() + ttl);
        }
    }

    /// 读取当前有效（未过期、未拉黑、未降级）的优选 POP IP。
    pub fn preferred_pop_ip(&self, host: &str) -> Option<IpAddr> {
        let key = normalize_host(host);
        let now = Instant::now();
        let candidate = {
            let mut pref = self
                .preferred_pops
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match pref.get(&key).copied() {
                Some((ip, until)) if now < until => Some(ip),
                Some(_) => {
                    pref.remove(&key);
                    None
                }
                None => None,
            }
        }?;
        let bad = self
            .bad_ips
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(key.clone(), candidate))
            .is_some_and(|&u| now < u);
        let demoted = self
            .demoted_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(key.clone(), candidate))
            .is_some_and(|&u| now < u);
        if bad || demoted {
            self.preferred_pops
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            None
        } else {
            Some(candidate)
        }
    }

    /// 查询最近一次分发给该主机的 IP（用于边缘比测对比基线与回滚验证）。
    pub fn current_served_ip(&self, host: &str) -> Option<IpAddr> {
        let key = normalize_host(host);
        self.last_served
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .map(|r| r.ip)
    }

    fn evict_preferred_if(&self, key: &str, bad_ip: IpAddr) {
        let mut pref = self
            .preferred_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if pref.get(key).is_some_and(|&(ip, _)| ip == bad_ip) {
            pref.remove(key);
        }
    }

    /// 异步收集最多 `max_subnets` 个不同 `/24`（或 IPv6 `/48`）的干净公网边缘 IP（供 Phase 4 比测）。
    pub async fn discover_distinct_pop_ips(&self, host: &str, max_subnets: usize) -> Vec<IpAddr> {
        let key = normalize_host(host);
        if !self.is_enabled()
            || self.is_metered()
            || self.is_tun_active()
            || !needs_public_dns(&key)
        {
            return Vec::new();
        }
        let mut pool = Vec::new();
        if let Some((ips, _)) = self.cached(&key) {
            pool.extend(ips);
        }
        let providers: Vec<ProviderSpec> = self
            .candidate_providers(&key)
            .into_iter()
            .take(MAX_POP_DISCOVERY_PROVIDERS)
            .collect();
        if !providers.is_empty() {
            let futs = providers.iter().map(|p| self.query_wire(p, &key, TYPE_A));
            for (ips, _) in futures_util::future::join_all(futs)
                .await
                .into_iter()
                .flatten()
            {
                pool.extend(ips);
            }
        }
        let now = Instant::now();
        let bad = self
            .bad_ips
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let demoted = self
            .demoted_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        pool.retain(|ip| {
            dns_wire::is_strictly_global_unicast(*ip)
                && !bad.get(&(key.clone(), *ip)).is_some_and(|&u| now < u)
                && !demoted.get(&(key.clone(), *ip)).is_some_and(|&u| now < u)
        });
        super::edge_opt::dedupe_by_subnet(&pool, max_subnets)
    }

    /// 网络变更（代理开关、休眠唤醒、DoH 开关）时立刻清空陈旧缓存与冷却状态。
    pub fn on_network_epoch_change(&self) {
        *self.tun_state.lock().unwrap_or_else(|e| e.into_inner()) = TunLeaseState::Normal;
        *self
            .last_fake_ip_ok
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.last_served
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.bad_ips
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.preferred_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.demoted_pops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.first_rst
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.connect_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.sni_blocked_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.host_degraded_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.providers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *self.ipv6_failures.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        *self
            .ipv6_suppressed_until
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn doh_http(&self) -> Client {
        self.doh_client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn ipv6_suppressed(&self, now: Instant) -> bool {
        let mut guard = self
            .ipv6_suppressed_until
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match *guard {
            Some(until) if now < until => true,
            Some(_) => {
                *guard = None;
                false
            }
            None => false,
        }
    }

    fn cached(&self, host: &str) -> Option<(Vec<IpAddr>, &'static str)> {
        let now = Instant::now();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.get(host) {
            Some(entry) if now < entry.expires => Some((entry.ips.clone(), entry.provider_id)),
            Some(_) => {
                cache.remove(host);
                None
            }
            None => None,
        }
    }

    fn remember(&self, host: &str, ips: &[IpAddr], provider_id: &'static str, ttl: u32) {
        let now = Instant::now();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= MAX_CACHED_HOSTS {
            cache.retain(|_, e| now < e.expires);
            if cache.len() >= MAX_CACHED_HOSTS {
                cache.clear();
            }
        }
        cache.insert(
            host.to_owned(),
            CachedHost {
                ips: ips.to_vec(),
                provider_id,
                expires: now
                    + Duration::from_secs(u64::from(ttl.clamp(MIN_TTL_SECS, MAX_TTL_SECS))),
            },
        );
    }

    fn select_primary_ip(
        &self,
        host: &str,
        ips: &[IpAddr],
        provider_id: Option<&'static str>,
    ) -> IpAddr {
        if let Some(pref_ip) = self.preferred_pop_ip(host) {
            self.last_served
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    host.to_owned(),
                    ServedRecord {
                        ip: pref_ip,
                        provider_id,
                    },
                );
            return pref_ip;
        }
        let now = Instant::now();
        let ipv6_suppressed = self.ipv6_suppressed(now);
        let mut bad = self.bad_ips.lock().unwrap_or_else(|e| e.into_inner());
        bad.retain(|_, until| now < *until);
        let mut demoted = self.demoted_pops.lock().unwrap_or_else(|e| e.into_inner());
        demoted.retain(|_, until| now < *until);

        let is_bad = |ip: &IpAddr| bad.contains_key(&(host.to_owned(), *ip));
        let is_avoided = |ip: &IpAddr| is_bad(ip) || demoted.contains_key(&(host.to_owned(), *ip));
        let picked = ips
            .iter()
            .copied()
            .find(|ip| ip.is_ipv4() && !is_avoided(ip))
            .or_else(|| {
                (!ipv6_suppressed)
                    .then(|| {
                        ips.iter()
                            .copied()
                            .find(|ip| ip.is_ipv6() && !is_avoided(ip))
                    })
                    .flatten()
            })
            .or_else(|| ips.iter().copied().find(|ip| ip.is_ipv4() && !is_bad(ip)))
            .or_else(|| ips.iter().copied().find(|ip| !is_bad(ip)))
            .unwrap_or(ips[0]);

        drop(demoted);
        drop(bad);
        self.last_served
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                host.to_owned(),
                ServedRecord {
                    ip: picked,
                    provider_id,
                },
            );
        picked
    }

    fn candidate_providers(&self, host: &str) -> Vec<ProviderSpec> {
        if let Some(custom) = &self.custom_servers {
            // 测试用自定义端点
            return custom
                .iter()
                .map(|url| ProviderSpec {
                    id: "custom",
                    endpoint: std::borrow::Cow::Owned(url.clone()),
                })
                .collect();
        }
        let now = Instant::now();
        let health = self.providers.lock().unwrap_or_else(|e| e.into_inner());
        let custom_user = self
            .custom_doh_url
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|_| !health.get("user_custom").is_some_and(|h| h.is_cooling(now)))
            .map(|url| ProviderSpec {
                id: "user_custom",
                endpoint: std::borrow::Cow::Owned(url),
            });

        let sort_pool = |pool: &[ProviderDef]| -> Vec<ProviderSpec> {
            let mut active: Vec<ProviderDef> = pool
                .iter()
                .copied()
                .filter(|p| !health.get(p.id).is_some_and(|h| h.is_cooling(now)))
                .collect();
            active.sort_by(|a, b| {
                let sa = health.get(a.id).map_or(120.0, ProviderHealth::score);
                let sb = health.get(b.id).map_or(120.0, ProviderHealth::score);
                sa.total_cmp(&sb)
            });
            active.into_iter().map(ProviderSpec::from).collect()
        };

        let mut out = Vec::new();
        if let Some(u) = custom_user {
            out.push(u);
        }
        match classify_domain(host) {
            DomainClass::PoisonProneOverseas => {
                // 污染高危域严禁回退国内 DNS；若海外节点全部处于冷却，必须尊重冷却直接返回空（0ms 跳过）
                out.extend(sort_pool(OVERSEAS_POOL));
            }
            DomainClass::GlobalAnycastOfficial => {
                let mut list = sort_pool(OVERSEAS_POOL);
                if list.is_empty() && out.is_empty() {
                    // 海外节点全冷却时仅允许尝试 DNSPod（且仍过 Bogon 校验）
                    list = sort_pool(&DOMESTIC_POOL[..1]);
                }
                out.extend(list);
            }
            DomainClass::DomesticOrMirror => {
                out.extend(sort_pool(DOMESTIC_POOL));
                out.extend(sort_pool(OVERSEAS_POOL));
            }
            DomainClass::OtherPublic => {
                out.extend(sort_pool(OVERSEAS_POOL));
                out.extend(sort_pool(DOMESTIC_POOL));
            }
        }
        out
    }

    /// 按 RFC 8484 二进制查询单个节点，自动更新其 EWMA 延迟与冷却状态。
    async fn query_wire(
        &self,
        provider: &ProviderSpec,
        host: &str,
        qtype: u16,
    ) -> Result<(Vec<IpAddr>, u32), QueryErr> {
        let wire_req = dns_wire::build_query(0x0000, host, qtype).ok_or(QueryErr::InvalidHost)?;
        let started = Instant::now();
        let resp = self
            .doh_http()
            .post(provider.endpoint.as_ref())
            .header(reqwest::header::CONTENT_TYPE, "application/dns-message")
            .header(reqwest::header::ACCEPT, "application/dns-message")
            .body(wire_req)
            .send()
            .await
            .map_err(|e| {
                self.mark_provider_failure(provider.id);
                QueryErr::Transport(describe(&e))
            })?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            self.mark_provider_failure(provider.id);
            return Err(QueryErr::Transport(format!("服务返回 {code}")));
        }
        let bytes = resp.bytes().await.map_err(|e| {
            self.mark_provider_failure(provider.id);
            QueryErr::Transport(describe(&e))
        })?;
        let rtt_ms = started.elapsed().as_secs_f64() * 1000.0;
        match dns_wire::parse_ip_response(&bytes, host, qtype) {
            DnsWireOutcome::Records { ips, min_ttl } => {
                self.providers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(provider.id.to_owned())
                    .or_default()
                    .record_ok(rtt_ms);
                Ok((ips, min_ttl))
            }
            DnsWireOutcome::NoData { nxdomain } => {
                self.providers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(provider.id.to_owned())
                    .or_default()
                    .record_ok(rtt_ms);
                Err(QueryErr::NoRecords { nxdomain })
            }
            DnsWireOutcome::PoisonedOrBogon => {
                self.providers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(provider.id.to_owned())
                    .or_default()
                    .record_poison(Instant::now());
                Err(QueryErr::Poisoned)
            }
            DnsWireOutcome::Malformed | DnsWireOutcome::ServerFailure => {
                self.mark_provider_failure(provider.id);
                Err(QueryErr::Transport("DNS 报文异常".to_owned()))
            }
        }
    }

    fn mark_provider_failure(&self, id: &str) {
        self.providers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_owned())
            .or_default()
            .record_failure(Instant::now());
    }

    async fn resolve_via_providers(&self, host: &str) -> Option<(Vec<IpAddr>, &'static str, u32)> {
        let candidates = self.candidate_providers(host);
        if candidates.is_empty() {
            return None;
        }
        let suppress_v6 = self.ipv6_suppressed(Instant::now());

        if self.is_metered() || candidates.len() == 1 {
            for provider in candidates {
                if let Some(found) = self.query_provider_dual(&provider, host, suppress_v6).await {
                    return Some((found.0, provider.id, found.1));
                }
            }
            return None;
        }

        // 180ms 阶梯竞速：先发最优 P0，若 180ms 未返立刻拉起 P1 竞速
        let p0 = candidates[0].clone();
        let p1 = candidates[1].clone();
        let f0 = self.query_provider_dual(&p0, host, suppress_v6);
        tokio::pin!(f0);

        tokio::select! {
            res0 = &mut f0 => {
                if let Some((ips, ttl)) = res0 {
                    return Some((ips, p0.id, ttl));
                }
            }
            () = tokio::time::sleep(HEDGE_DELAY) => {
                let f1 = self.query_provider_dual(&p1, host, suppress_v6);
                tokio::pin!(f1);
                tokio::select! {
                    res0 = &mut f0 => {
                        if let Some((ips, ttl)) = res0 {
                            return Some((ips, p0.id, ttl));
                        }
                        if let Some((ips, ttl)) = f1.await {
                            return Some((ips, p1.id, ttl));
                        }
                    }
                    res1 = &mut f1 => {
                        if let Some((ips, ttl)) = res1 {
                            return Some((ips, p1.id, ttl));
                        }
                        if let Some((ips, ttl)) = f0.await {
                            return Some((ips, p0.id, ttl));
                        }
                    }
                }
            }
        }

        for provider in &candidates[2..] {
            if let Some(found) = self.query_provider_dual(provider, host, suppress_v6).await {
                return Some((found.0, provider.id, found.1));
            }
        }
        None
    }

    /// 先查 `TYPE_A`，仅明确 `NoData { nxdomain: false }` 且未抑制 IPv6 时查 `TYPE_AAAA`。
    async fn query_provider_dual(
        &self,
        provider: &ProviderSpec,
        host: &str,
        suppress_v6: bool,
    ) -> Option<(Vec<IpAddr>, u32)> {
        match self.query_wire(provider, host, TYPE_A).await {
            Ok((ips, ttl)) => Some((ips, ttl)),
            Err(QueryErr::NoRecords { nxdomain: false }) if !suppress_v6 => {
                self.query_wire(provider, host, TYPE_AAAA).await.ok()
            }
            Err(_) => None,
        }
    }

    async fn resolve_with_singleflight(
        &self,
        host: &str,
    ) -> Result<(Vec<IpAddr>, &'static str), String> {
        enum Role {
            Leader(watch::Sender<SingleflightPayload>),
            Follower(watch::Receiver<SingleflightPayload>),
        }

        let role = {
            let mut map = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(rx) = map.get(host) {
                Role::Follower(rx.clone())
            } else {
                let (tx, rx) = watch::channel(None);
                map.insert(host.to_owned(), rx);
                Role::Leader(tx)
            }
        };

        match role {
            Role::Follower(mut rx) => loop {
                if let Some(res) = rx.borrow().clone() {
                    return res.map(|ips| (ips, "singleflight"));
                }
                if rx.changed().await.is_err() {
                    return Err("DoH 查询未返回".to_owned());
                }
            },
            Role::Leader(tx) => {
                let mut guard = SingleflightGuard {
                    map: &self.in_flight,
                    host: host.to_owned(),
                    tx: Some(tx),
                    done: false,
                };
                let outcome = match self.resolve_via_providers(host).await {
                    Some((ips, provider_id, ttl)) => {
                        self.remember(host, &ips, provider_id, ttl);
                        Ok((ips, provider_id))
                    }
                    None => {
                        // Leader 统一回退一次系统 DNS 并短缓存 30s，避免 24 并发 follower 重复刷日志与重查
                        match self.system_lookup_prioritized(host).await {
                            Ok(addrs) if !addrs.is_empty() => {
                                let ips: Vec<IpAddr> = addrs.iter().map(SocketAddr::ip).collect();
                                if !ips.iter().all(|ip| is_fake_ip(*ip)) {
                                    tracing::debug!(
                                        host,
                                        "DoH 节点冷却或未命中，已回退系统 DNS（IPv4 优先，缓存 30s）"
                                    );
                                    self.remember(host, &ips, "system", SYSTEM_FALLBACK_TTL_SECS);
                                }
                                Ok((ips, "system"))
                            }
                            _ => Err("DoH 与系统 DNS 均不可用".to_owned()),
                        }
                    }
                };
                guard.done = true;
                if let Some(tx) = guard.tx.take() {
                    let _ = tx.send(Some(
                        outcome
                            .as_ref()
                            .map(|(ips, _)| ips.clone())
                            .map_err(Clone::clone),
                    ));
                }
                outcome
            }
        }
    }

    async fn lookup(self: Arc<Self>, host: String) -> Result<Vec<SocketAddr>, BoxError> {
        let host = normalize_host(&host);
        if !self.is_enabled() || !needs_public_dns(&host) {
            return self.system_lookup_prioritized(&host).await;
        }

        // 先检查 Fake-IP / TUN 租约与旁路冷却状态机
        let now = Instant::now();
        let tun_action = {
            let mut state = self.tun_state.lock().unwrap_or_else(|e| e.into_inner());
            match *state {
                TunLeaseState::TunActive { lease_until } if now < lease_until => 1, // direct TUN
                TunLeaseState::TunActive { .. } => {
                    *state = TunLeaseState::Normal;
                    0
                }
                TunLeaseState::FakeIpBypass { until } if now < until => 2, // bypass system_peek -> DoH
                TunLeaseState::FakeIpBypass { .. } => {
                    *state = TunLeaseState::Normal;
                    0
                }
                TunLeaseState::Normal => 0,
            }
        };

        if tun_action == 1
            && let Ok(addrs) = self.system_lookup_prioritized(&host).await
        {
            if let Some(first) = addrs.first() {
                self.last_served
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        host.clone(),
                        ServedRecord {
                            ip: first.ip(),
                            provider_id: None,
                        },
                    );
            }
            return Ok(addrs);
        }

        if tun_action != 1
            && let Some(pref_ip) = self.preferred_pop_ip(&host)
        {
            self.last_served
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    host.clone(),
                    ServedRecord {
                        ip: pref_ip,
                        provider_id: None,
                    },
                );
            return filter_rebinding_addrs(&host, vec![SocketAddr::new(pref_ip, 0)], false)
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from);
        }

        if let Some((ips, provider_id)) = self.cached(&host) {
            let primary = self.select_primary_ip(&host, &ips, Some(provider_id));
            return filter_rebinding_addrs(
                &host,
                vec![SocketAddr::new(primary, 0)],
                provider_id == "system",
            )
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from);
        }

        if tun_action == 0
            && let Ok(Ok(addrs)) =
                tokio::time::timeout(SYSTEM_PEEK_TIMEOUT, system_lookup(&host)).await
            && !addrs.is_empty()
            && addrs.iter().all(|a| is_fake_ip(a.ip()))
        {
            tracing::info!(
                host,
                "检测到代理软件 Fake-IP（198.18.x.y）：授予 30s TUN 租约交由代理接管"
            );
            *self.tun_state.lock().unwrap_or_else(|e| e.into_inner()) = TunLeaseState::TunActive {
                lease_until: Instant::now() + TUN_LEASE_DURATION,
            };
            self.last_served
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    host.clone(),
                    ServedRecord {
                        ip: addrs[0].ip(),
                        provider_id: None,
                    },
                );
            return Ok(addrs);
        }

        if let Ok((ips, provider_id)) = self.resolve_with_singleflight(&host).await
            && !ips.is_empty()
        {
            let provider =
                (provider_id != "system" && provider_id != "singleflight").then_some(provider_id);
            let primary = self.select_primary_ip(&host, &ips, provider);
            return filter_rebinding_addrs(
                &host,
                vec![SocketAddr::new(primary, 0)],
                provider_id == "system",
            )
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from);
        }

        self.system_lookup_prioritized(&host).await
    }

    async fn system_lookup_prioritized(&self, host: &str) -> Result<Vec<SocketAddr>, BoxError> {
        let mut addrs = system_lookup(host).await?;
        let now = Instant::now();
        // 系统 DNS 返回全 Fake-IP 且不在 FakeIpBypass 冷却期时，立即激活 TUN 租约防止后续再卡 DoH
        let in_bypass = {
            let mut state = self.tun_state.lock().unwrap_or_else(|e| e.into_inner());
            let bypass = matches!(*state, TunLeaseState::FakeIpBypass { until } if now < until);
            if !bypass && !addrs.is_empty() && addrs.iter().all(|a| is_fake_ip(a.ip())) {
                *state = TunLeaseState::TunActive {
                    lease_until: now + TUN_LEASE_DURATION,
                };
            }
            bypass
        };
        addrs = filter_rebinding_addrs(host, addrs, !in_bypass)
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
        let suppress_v6 = self.ipv6_suppressed(now);
        if suppress_v6 && addrs.iter().any(SocketAddr::is_ipv4) {
            addrs.retain(SocketAddr::is_ipv4);
        } else {
            // 重排为 IPv4 在前，消除 Windows RFC 6724 偏好坏 IPv6 的 10s 黑洞长尾
            addrs.sort_by_key(|a| u8::from(a.is_ipv6()));
        }
        if let Some(first) = addrs.first() {
            self.last_served
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    host.to_owned(),
                    ServedRecord {
                        ip: first.ip(),
                        provider_id: None,
                    },
                );
        }
        Ok(addrs)
    }

    /// 传输建连成功滑动续期 TUN 租约并清理临时 RST / 降级状态。
    pub fn report_connect_ok(&self, host: &str) {
        let key = normalize_host(host);
        let now = Instant::now();
        self.first_rst
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        self.connect_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        self.host_degraded_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        let served = self
            .last_served
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        if let Some(record) = served {
            if record.ip.is_ipv4() {
                *self.ipv6_failures.lock().unwrap_or_else(|e| e.into_inner()) = 0;
            }
            if is_fake_ip(record.ip) {
                *self
                    .last_fake_ip_ok
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(now);
                let mut tun = self.tun_state.lock().unwrap_or_else(|e| e.into_inner());
                if matches!(*tun, TunLeaseState::TunActive { .. }) {
                    *tun = TunLeaseState::TunActive {
                        lease_until: now + TUN_LEASE_DURATION,
                    };
                }
            }
        }
    }

    /// 传输建连/握手失败归因闭环（由 `reqwest::Error` 错误链驱动）。
    pub fn report_connect_failure(&self, host: &str, err: &reqwest::Error) -> TransportDnsAction {
        let kind = classify_connect_error_kind(err);
        self.report_connect_failure_kind(host, kind)
    }

    /// 可测试的纯状态机版本，不依赖造假 `reqwest::Error`。
    pub fn report_connect_failure_kind(
        &self,
        host: &str,
        kind: ConnectFailureKind,
    ) -> TransportDnsAction {
        let key = normalize_host(host);
        let now = Instant::now();
        let served = self
            .last_served
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();

        // 1) Fake-IP 建连失败：区分「TUN 刚关闭导致的残留 Fake-IP」与「TUN 存活但单个远端主机不可达」
        if let Some(ref record) = served
            && is_fake_ip(record.ip)
        {
            let recent_tun_ok = self
                .last_fake_ip_ok
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|at| now.duration_since(at) <= FAKE_IP_RECENT_OK_WINDOW);
            if matches!(kind, ConnectFailureKind::TimeoutOrUnreachable) && !recent_tun_ok {
                tracing::warn!(host = %key, ip = %record.ip, "Fake-IP 建连失败，切入 20s DoH 旁路冷却");
                *self.tun_state.lock().unwrap_or_else(|e| e.into_inner()) =
                    TunLeaseState::FakeIpBypass {
                        until: now + FAKE_IP_BYPASS_DURATION,
                    };
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
                return TransportDnsAction::RetryOnceSingleStream;
            }
            // TUN 仍在工作（仅该目标主机握手被重置或远端超时）：仅降级该主机，绝不破坏全局 TUN 租约
            if self.record_host_connect_failure(&key, now) >= 2 {
                self.host_degraded_until
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, now + HOST_DEGRADED_DURATION);
                return TransportDnsAction::SniBlockedSwitchSource;
            }
            return TransportDnsAction::RetryOnceSingleStream;
        }

        // 2) IPv6 连接黑洞熔断
        if let Some(ref record) = served
            && record.ip.is_ipv6()
            && matches!(
                kind,
                ConnectFailureKind::TimeoutOrUnreachable | ConnectFailureKind::HandshakeResetOrEof
            )
        {
            let mut fails = self.ipv6_failures.lock().unwrap_or_else(|e| e.into_inner());
            *fails = fails.saturating_add(1);
            if *fails >= 2 {
                *self
                    .ipv6_suppressed_until
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(now + IPV6_SUPPRESS_DURATION);
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
                return TransportDnsAction::RetryOnceSingleStream;
            }
        }

        match kind {
            // 3) GFW TLS SNI 10054 RST 闭环：`1-Rotate → 2-RST → SniBlocked(60s)`（绝不误扣 DoH 节点分）
            ConnectFailureKind::HandshakeResetOrEof => {
                if let Some(ref record) = served {
                    self.bad_ips
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert((key.clone(), record.ip), now + BAD_IP_COOLDOWN);
                    self.evict_preferred_if(&key, record.ip);
                }
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);

                let mut rst = self.first_rst.lock().unwrap_or_else(|e| e.into_inner());
                let cur_ip = served.as_ref().map(|r| r.ip);
                match (rst.get(&key).copied(), cur_ip) {
                    (Some((prev_ip, first_at)), Some(ip))
                        if prev_ip != ip && now.duration_since(first_at) <= FIRST_RST_WINDOW =>
                    {
                        rst.remove(&key);
                        self.sni_blocked_until
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key.clone(), now + SNI_BLOCKED_DURATION);
                        self.host_degraded_until
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key, now + HOST_DEGRADED_DURATION);
                        TransportDnsAction::SniBlockedSwitchSource
                    }
                    (_, Some(ip)) => {
                        rst.insert(key, (ip, now));
                        TransportDnsAction::RetryOnceSingleStream
                    }
                    _ => {
                        if self.record_host_connect_failure(&key, now) >= 2 {
                            self.host_degraded_until
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(key, now + HOST_DEGRADED_DURATION);
                            TransportDnsAction::SniBlockedSwitchSource
                        } else {
                            TransportDnsAction::RetryOnceSingleStream
                        }
                    }
                }
            }
            // 4) 证书名/签发者错配：高置信污染 IP，拉黑 600s 并对对应 DoH 节点加污染罚分
            ConnectFailureKind::TlsCertMismatch => {
                if let Some(ref record) = served {
                    self.bad_ips
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert((key.clone(), record.ip), now + CERT_POISON_COOLDOWN);
                    self.evict_preferred_if(&key, record.ip);
                    if let Some(pid) = record.provider_id {
                        self.providers
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .entry(pid.to_owned())
                            .or_default()
                            .record_poison(now);
                    }
                }
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
                TransportDnsAction::RetryOnceSingleStream
            }
            // 5) 连续建连超时/黑洞 IP（如被污染或静默丢 SYN 的源）：2 次即标主机降级 120s 秒切官方互备 CDN
            ConnectFailureKind::TimeoutOrUnreachable => {
                if let Some(ref record) = served {
                    self.bad_ips
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert((key.clone(), record.ip), now + BAD_IP_COOLDOWN);
                    self.evict_preferred_if(&key, record.ip);
                }
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
                if self.record_host_connect_failure(&key, now) >= 2 {
                    self.host_degraded_until
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, now + HOST_DEGRADED_DURATION);
                    TransportDnsAction::SniBlockedSwitchSource
                } else {
                    TransportDnsAction::RetryOnceSingleStream
                }
            }
            ConnectFailureKind::Other => TransportDnsAction::None,
        }
    }

    fn record_host_connect_failure(&self, key: &str, now: Instant) -> u32 {
        let mut map = self
            .connect_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(key.to_owned()).or_insert((0, now));
        if now.duration_since(entry.1) > FIRST_RST_WINDOW {
            *entry = (1, now);
        } else {
            entry.0 = entry.0.saturating_add(1);
            entry.1 = now;
        }
        entry.0
    }
}

/// 「测试 DoH」按钮的一行结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DnsProbe {
    pub name: String,
    pub ms: Option<f64>,
    pub addresses: Vec<String>,
    pub error: Option<String>,
}

/// 测试时问的域名（游戏下载最先用到的那个）。
pub const TEST_HOST: &str = "piston-meta.mojang.com";
const SHOWN_ADDRESSES: usize = 3;

impl Doh {
    /// 每个内置 RFC 8484 DoH 节点与系统 DNS 各探测一次（不用缓存，开关关着也测）。
    pub async fn test(&self) -> Vec<DnsProbe> {
        let mut defs: Vec<ProviderSpec> = match &self.custom_servers {
            Some(servers) => servers
                .iter()
                .map(|url| ProviderSpec {
                    id: "custom",
                    endpoint: std::borrow::Cow::Owned(url.clone()),
                })
                .collect(),
            None => OVERSEAS_POOL
                .iter()
                .chain(DOMESTIC_POOL.iter())
                .copied()
                .map(ProviderSpec::from)
                .collect(),
        };
        if self.custom_servers.is_none()
            && let Some(custom_url) = self
                .custom_doh_url
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        {
            defs.insert(
                0,
                ProviderSpec {
                    id: "user_custom",
                    endpoint: std::borrow::Cow::Owned(custom_url),
                },
            );
        }

        let doh_futures = defs.into_iter().map(|def| async move {
            let started = Instant::now();
            let result = self.query_wire(&def, TEST_HOST, TYPE_A).await;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            let name = server_label(def.endpoint.as_ref());
            match result {
                Ok((ips, _)) => probe_ok(name, ms, ips.iter().map(IpAddr::to_string)),
                Err(QueryErr::NoRecords { .. }) => probe_failed(name, "没有这个域名的地址"),
                Err(QueryErr::Poisoned) => probe_failed(name, "返回了异常/保留地址"),
                Err(QueryErr::Transport(reason)) => probe_failed(name, &reason),
                Err(QueryErr::InvalidHost) => probe_failed(name, "域名格式无效"),
            }
        });

        let system_future = async {
            let started = Instant::now();
            let result = system_lookup(TEST_HOST).await;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            match result {
                Ok(addrs) => probe_ok(
                    if addrs.iter().all(|a| is_fake_ip(a.ip())) {
                        "系统 DNS（代理接管）".to_owned()
                    } else {
                        "系统 DNS".to_owned()
                    },
                    ms,
                    addrs.iter().map(|a| a.ip().to_string()),
                ),
                Err(error) => probe_failed("系统 DNS".to_owned(), &describe(error.as_ref())),
            }
        };

        let (mut probes, system) =
            tokio::join!(futures_util::future::join_all(doh_futures), system_future);
        probes.push(system);
        probes
    }
}

enum QueryErr {
    InvalidHost,
    NoRecords {
        #[allow(dead_code)]
        nxdomain: bool,
    },
    Poisoned,
    Transport(String),
}

fn build_doh_client(
    proxy: &ProxySettings,
    sys_snap: Option<&SystemProxySnapshot>,
) -> Result<Client, HttpError> {
    let builder = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(QUERY_CONNECT_TIMEOUT)
        .timeout(QUERY_TOTAL_TIMEOUT)
        .http2_keep_alive_interval(Duration::from_secs(20))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none());
    let builder = apply_proxy_settings(builder, proxy, sys_snap)?;
    builder.build().map_err(HttpError::Build)
}

fn probe_ok(name: String, ms: f64, addresses: impl Iterator<Item = String>) -> DnsProbe {
    DnsProbe {
        name,
        ms: Some(ms),
        addresses: addresses.take(SHOWN_ADDRESSES).collect(),
        error: None,
    }
}

fn probe_failed(name: String, error: &str) -> DnsProbe {
    DnsProbe {
        name,
        ms: None,
        addresses: Vec::new(),
        error: Some(error.to_owned()),
    }
}

fn server_label(server: &str) -> String {
    let host = super::url_host(server).unwrap_or_else(|| server.to_owned());
    match host.as_str() {
        "223.5.5.5" | "223.6.6.6" => "阿里 DoH (223.5.5.5)".to_owned(),
        "1.12.12.12" => "DNSPod DoH (1.12.12.12)".to_owned(),
        "1.0.0.1" => "Cloudflare DoH (1.0.0.1)".to_owned(),
        "1.1.1.1" => "Cloudflare DoH (1.1.1.1)".to_owned(),
        "9.9.9.9" => "Quad9 DoH (9.9.9.9)".to_owned(),
        other => format!("DoH {other}"),
    }
}

fn describe(error: &(dyn std::error::Error + Send + Sync + 'static)) -> String {
    match error.downcast_ref::<reqwest::Error>() {
        Some(e) if e.is_timeout() => "超时".to_owned(),
        Some(e) if e.is_connect() => "连不上".to_owned(),
        Some(e) if e.is_status() => format!(
            "服务返回 {}",
            e.status()
                .map_or_else(String::new, |s| s.as_u16().to_string())
        ),
        _ => "出错了".to_owned(),
    }
}

fn classify_connect_error_kind(err: &reqwest::Error) -> ConnectFailureKind {
    let mut cur: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(node) = cur {
        if let Some(io_err) = node.downcast_ref::<io::Error>() {
            if matches!(
                io_err.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
            ) || matches!(io_err.raw_os_error(), Some(10054))
            {
                return ConnectFailureKind::HandshakeResetOrEof;
            }
            if io_err.get_ref().is_some_and(|inner| {
                let s = format!("{inner:?}");
                s.contains("InvalidCertificate")
                    || s.contains("UnknownIssuer")
                    || s.contains("Expired")
            }) {
                return ConnectFailureKind::TlsCertMismatch;
            }
        }
        cur = node.source();
    }
    if err.is_timeout() || err.is_connect() {
        ConnectFailureKind::TimeoutOrUnreachable
    } else {
        ConnectFailureKind::Other
    }
}

#[derive(Clone)]
pub(crate) struct DohResolver(pub(crate) Arc<Doh>);

impl Resolve for DohResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let doh = self.0.clone();
        let host = normalize_host(name.as_str());
        Box::pin(async move {
            doh.lookup(host)
                .await
                .map(|addrs| Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// 启动器统一用的 HTTP 客户端，域名先问 DoH（默认走系统代理策略）。
pub fn build_client_with_doh(doh: Arc<Doh>) -> Result<Client, HttpError> {
    build_client_with_proxy(doh, &ProxySettings::default())
}

/// 带 DoH 与代理设置的 HTTP 客户端（返回兼容 `h2_small` 主客户端）。
pub fn build_client_with_proxy(doh: Arc<Doh>, proxy: &ProxySettings) -> Result<Client, HttpError> {
    super::http::ClientBundle::build(doh, proxy, None).map(|bundle| bundle.h2_small)
}

pub fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

pub fn is_fake_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18)
}

fn matches_domain_suffix(host: &str, suffix: &str) -> bool {
    host == suffix
        || host
            .strip_suffix(suffix)
            .is_some_and(|rest| rest.ends_with('.'))
}

pub fn classify_domain(host: &str) -> DomainClass {
    let lower = normalize_host(host);
    if POISON_PRONE_OVERSEAS
        .iter()
        .any(|s| matches_domain_suffix(&lower, s))
    {
        DomainClass::PoisonProneOverseas
    } else if GLOBAL_ANYCAST_OFFICIAL
        .iter()
        .any(|s| matches_domain_suffix(&lower, s))
    {
        DomainClass::GlobalAnycastOfficial
    } else if DOMESTIC_OR_MIRROR
        .iter()
        .any(|s| matches_domain_suffix(&lower, s))
    {
        DomainClass::DomesticOrMirror
    } else {
        DomainClass::OtherPublic
    }
}

/// 公网域名才问 DoH：`localhost`、没有点的内网名字、`.local` / `.lan` 这类局域网名字用系统 DNS。
fn needs_public_dns(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() || !host.contains('.') {
        return false;
    }
    let lower = normalize_host(host);
    ![".local", ".lan", ".home", ".internal", ".localhost"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// Gate 2（防 DNS Rebinding）：公网域名解析结果中只保留严格全球单播 IP
/// （当 `allow_fake_ip = true` 时额外放行代理 TUN 的 `198.18.0.0/15`）。
pub fn filter_rebinding_addrs(
    host: &str,
    addrs: Vec<SocketAddr>,
    allow_fake_ip: bool,
) -> Result<Vec<SocketAddr>, String> {
    if !needs_public_dns(host) {
        return Ok(addrs);
    }
    let filtered: Vec<SocketAddr> = addrs
        .into_iter()
        .filter(|addr| {
            let ip = addr.ip();
            dns_wire::is_strictly_global_unicast(ip) || (allow_fake_ip && is_fake_ip(ip))
        })
        .collect();
    if filtered.is_empty() {
        return Err(format!(
            "DNS 解析返回非公网地址（已拦截潜在 DNS 重绑定）：{host}"
        ));
    }
    Ok(filtered)
}

async fn system_lookup(host: &str) -> Result<Vec<SocketAddr>, BoxError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
    if addrs.is_empty() {
        return Err(format!("{host} 解析不到地址").into());
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poison_prone_overseas_hosts_strictly_exclude_domestic_dns() {
        let doh = Doh::new(true).unwrap();
        for host in [
            "maven.neoforged.net",
            "cdn.modrinth.com",
            "edge.forgecdn.net",
            "api.curseforge.com",
        ] {
            let candidates = doh.candidate_providers(host);
            assert!(
                candidates
                    .iter()
                    .all(|p| { matches!(p.id, "cf_secondary" | "cf_primary" | "quad9") }),
                "{host} candidates leaked domestic DNS"
            );
        }
        let domestic = doh.candidate_providers("mirrors.tuna.tsinghua.edu.cn");
        assert!(matches!(domestic[0].id, "dnspod" | "alidns"));
    }

    #[test]
    fn tun_lease_fake_ip_bypass_and_epoch_reset_work() {
        let doh = Doh::new(true).unwrap();
        *doh.tun_state.lock().unwrap() = TunLeaseState::TunActive {
            lease_until: Instant::now() + Duration::from_secs(8),
        };
        assert!(doh.is_tun_active());
        doh.last_served.lock().unwrap().insert(
            "piston-meta.mojang.com".to_owned(),
            ServedRecord {
                ip: "198.18.0.55".parse().unwrap(),
                provider_id: None,
            },
        );
        let action = doh.report_connect_failure_kind(
            "piston-meta.mojang.com",
            ConnectFailureKind::TimeoutOrUnreachable,
        );
        assert_eq!(action, TransportDnsAction::RetryOnceSingleStream);
        assert!(!doh.is_tun_active());
        assert!(matches!(
            *doh.tun_state.lock().unwrap(),
            TunLeaseState::FakeIpBypass { .. }
        ));

        doh.on_network_epoch_change();
        assert!(matches!(
            *doh.tun_state.lock().unwrap(),
            TunLeaseState::Normal
        ));
    }

    #[test]
    fn two_rst_on_distinct_ips_marks_sni_blocked_without_blaming_doh() {
        let doh = Doh::new(true).unwrap();
        doh.last_served.lock().unwrap().insert(
            "maven.neoforged.net".to_owned(),
            ServedRecord {
                ip: "185.199.108.1".parse().unwrap(),
                provider_id: Some("cf_secondary"),
            },
        );
        assert_eq!(
            doh.report_connect_failure_kind(
                "maven.neoforged.net",
                ConnectFailureKind::HandshakeResetOrEof,
            ),
            TransportDnsAction::RetryOnceSingleStream
        );
        assert!(!doh.is_sni_blocked("maven.neoforged.net"));

        doh.last_served.lock().unwrap().insert(
            "maven.neoforged.net".to_owned(),
            ServedRecord {
                ip: "185.199.109.1".parse().unwrap(),
                provider_id: Some("cf_primary"),
            },
        );
        assert_eq!(
            doh.report_connect_failure_kind(
                "maven.neoforged.net",
                ConnectFailureKind::HandshakeResetOrEof,
            ),
            TransportDnsAction::SniBlockedSwitchSource
        );
        assert!(doh.is_sni_blocked("maven.neoforged.net"));

        let health = doh.providers.lock().unwrap();
        assert!(!health.contains_key("cf_secondary"));
        assert!(!health.contains_key("cf_primary"));
    }

    #[test]
    fn ipv6_suppressed_after_two_failures() {
        let doh = Doh::new(true).unwrap();
        doh.last_served.lock().unwrap().insert(
            "cdn.modrinth.com".to_owned(),
            ServedRecord {
                ip: "2606:4700::1".parse().unwrap(),
                provider_id: Some("cf_secondary"),
            },
        );
        doh.report_connect_failure_kind(
            "cdn.modrinth.com",
            ConnectFailureKind::TimeoutOrUnreachable,
        );
        assert!(!doh.ipv6_suppressed(Instant::now()));
        doh.report_connect_failure_kind(
            "cdn.modrinth.com",
            ConnectFailureKind::TimeoutOrUnreachable,
        );
        assert!(doh.ipv6_suppressed(Instant::now()));
    }

    #[test]
    fn local_names_skip_doh() {
        assert!(needs_public_dns("piston-meta.mojang.com"));
        assert!(!needs_public_dns("localhost"));
        assert!(!needs_public_dns("nas"));
        assert!(!needs_public_dns("printer.local"));
        assert!(!needs_public_dns("app.localhost"));
        assert!(!needs_public_dns("127.0.0.1"));
    }

    #[test]
    fn cache_follows_ttl() {
        let doh = Doh::with_servers(true, Vec::new()).unwrap();
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        doh.remember("a.com", &[ip], "cf_secondary", 9);
        assert_eq!(
            doh.cached("a.com"),
            Some((vec![ip], "cf_secondary")),
            "TTL 太短按 30 秒记"
        );
        doh.cache.lock().unwrap().get_mut("a.com").unwrap().expires = Instant::now();
        assert_eq!(doh.cached("a.com"), None, "过期就不用了");
    }

    #[test]
    fn proxy_fake_ips_are_recognised() {
        for ip in ["198.18.3.1", "198.19.255.1"] {
            assert!(is_fake_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["198.17.0.1", "198.20.0.1", "150.171.110.100"] {
            assert!(!is_fake_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn servers_have_readable_names() {
        assert_eq!(
            server_label("https://223.5.5.5/dns-query"),
            "阿里 DoH (223.5.5.5)"
        );
        assert_eq!(
            server_label("https://1.0.0.1/dns-query"),
            "Cloudflare DoH (1.0.0.1)"
        );
        assert_eq!(server_label("https://dns.example/q"), "DoH dns.example");
    }

    #[tokio::test]
    async fn test_lists_every_server_and_the_system() {
        let doh =
            Doh::with_servers(false, vec!["https://127.0.0.1:9/dns-query".to_owned()]).unwrap();
        let probes = doh.test().await;
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].name, "DoH 127.0.0.1");
        assert!(probes[0].ms.is_none());
        assert!(
            matches!(probes[0].error.as_deref(), Some("连不上" | "超时")),
            "{:?}",
            probes[0].error
        );
        assert!(probes[1].name.starts_with("系统 DNS"), "{}", probes[1].name);
    }

    #[test]
    fn cooling_overseas_pool_returns_empty_without_retrying_dead_nodes() {
        let doh = Doh::new(true).unwrap();
        for id in ["cf_secondary", "cf_primary", "quad9"] {
            doh.mark_provider_failure(id);
        }
        assert!(
            doh.candidate_providers("maven.neoforged.net").is_empty(),
            "全员冷却时必须 0ms 跳过，绝不反复超时重试"
        );
    }

    #[test]
    fn two_connect_timeouts_mark_host_degraded_and_reset_on_ok() {
        let doh = Doh::new(true).unwrap();
        assert_eq!(
            doh.report_connect_failure_kind(
                "maven.neoforged.net",
                ConnectFailureKind::TimeoutOrUnreachable,
            ),
            TransportDnsAction::RetryOnceSingleStream
        );
        assert!(!doh.is_host_degraded("maven.neoforged.net"));
        assert_eq!(
            doh.report_connect_failure_kind(
                "maven.neoforged.net",
                ConnectFailureKind::TimeoutOrUnreachable,
            ),
            TransportDnsAction::SniBlockedSwitchSource
        );
        assert!(doh.is_host_degraded("maven.neoforged.net"));

        doh.report_connect_ok("maven.neoforged.net");
        assert!(!doh.is_host_degraded("maven.neoforged.net"));
    }

    #[test]
    fn active_tun_is_preserved_when_single_host_rsts_or_tun_recently_succeeded() {
        let doh = Doh::new(true).unwrap();
        *doh.tun_state.lock().unwrap() = TunLeaseState::TunActive {
            lease_until: Instant::now() + Duration::from_secs(30),
        };
        doh.last_served.lock().unwrap().insert(
            "piston-meta.mojang.com".to_owned(),
            ServedRecord {
                ip: "198.18.0.10".parse().unwrap(),
                provider_id: None,
            },
        );
        doh.report_connect_ok("piston-meta.mojang.com");

        doh.last_served.lock().unwrap().insert(
            "maven.neoforged.net".to_owned(),
            ServedRecord {
                ip: "198.18.0.99".parse().unwrap(),
                provider_id: None,
            },
        );
        doh.report_connect_failure_kind(
            "maven.neoforged.net",
            ConnectFailureKind::TimeoutOrUnreachable,
        );
        assert!(
            doh.is_tun_active(),
            "其他主机刚通过 TUN 建连成功，单主机超时不应误杀全局 TUN 租约"
        );
        let action = doh.report_connect_failure_kind(
            "maven.neoforged.net",
            ConnectFailureKind::TimeoutOrUnreachable,
        );
        assert_eq!(action, TransportDnsAction::SniBlockedSwitchSource);
        assert!(doh.is_host_degraded("maven.neoforged.net"));
        assert!(doh.is_tun_active());
    }

    #[test]
    fn blocks_dns_rebinding_private_and_metadata_ips() {
        let evil_addrs: Vec<SocketAddr> = [
            "169.254.169.254:443",
            "127.0.0.1:443",
            "[::1]:443",
            "[::ffff:127.0.0.1]:443",
            "10.0.0.8:443",
        ]
        .into_iter()
        .map(|s| s.parse().unwrap())
        .collect();
        assert!(filter_rebinding_addrs("rebind.example.com", evil_addrs, false).is_err());

        let fake_ip_addrs = vec!["198.18.0.42:443".parse().unwrap()];
        assert!(
            filter_rebinding_addrs("cdn.modrinth.com", fake_ip_addrs.clone(), false).is_err(),
            "直连模式必须拦住 Fake-IP / 保留段"
        );
        assert_eq!(
            filter_rebinding_addrs("cdn.modrinth.com", fake_ip_addrs, true).unwrap(),
            vec!["198.18.0.42:443".parse::<SocketAddr>().unwrap()]
        );

        let mixed = vec![
            "169.254.169.254:443".parse().unwrap(),
            "150.171.110.100:443".parse().unwrap(),
        ];
        assert_eq!(
            filter_rebinding_addrs("piston-meta.mojang.com", mixed, false).unwrap(),
            vec!["150.171.110.100:443".parse::<SocketAddr>().unwrap()]
        );
    }

    #[test]
    fn preferred_pop_ip_takes_priority_and_clears_on_failure_and_epoch() {
        let doh = Doh::new(true).unwrap();
        let old_ip: IpAddr = "151.101.1.10".parse().unwrap();
        let best_ip: IpAddr = "151.101.65.10".parse().unwrap();
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();

        // Gate 2：优选 POP 拒绝私网/回环 IP
        assert!(!doh.set_preferred_pop_ip("cdn.modrinth.com", loopback, Duration::from_secs(60)));
        assert!(doh.set_preferred_pop_ip("cdn.modrinth.com", best_ip, Duration::from_secs(60)));

        let picked =
            doh.select_primary_ip("cdn.modrinth.com", &[old_ip, best_ip], Some("cf_primary"));
        assert_eq!(picked, best_ip);
        assert_eq!(doh.current_served_ip("cdn.modrinth.com"), Some(best_ip));

        // 建连失败立即撤回优选 IP
        let _ = doh.report_connect_failure_kind(
            "cdn.modrinth.com",
            ConnectFailureKind::TimeoutOrUnreachable,
        );
        assert_eq!(doh.preferred_pop_ip("cdn.modrinth.com"), None);

        // 重新设置后，网络代际切换瞬间清空
        let third_ip: IpAddr = "151.101.129.10".parse().unwrap();
        assert!(doh.set_preferred_pop_ip("cdn.modrinth.com", third_ip, Duration::from_secs(60)));
        assert_eq!(doh.preferred_pop_ip("cdn.modrinth.com"), Some(third_ip));
        doh.on_network_epoch_change();
        assert_eq!(doh.preferred_pop_ip("cdn.modrinth.com"), None);
    }
}
