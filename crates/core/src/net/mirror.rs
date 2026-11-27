//! 零信任合规多源织网（仅 `--features community-mirrors` 启用）。
//!
//! - **Manifest-Anchored Zero-Trust**：仅采信官方清单锚定的 SHA-1 与精确字节数；无强哈希物品
//!   严禁走任何第三方镜像；认证/皮肤、CurseForge 与官方元数据主清单永不镜像；
//! - **零探针被动 EWMA 分桶调度**：按 `(AssetClass, SizeBucket)` 统计真实连接延迟与吞吐；
//!   计费网络与限速模式下探索率强制为 `0%`；
//! - **BMCLAPI 302 最优停止重摇与双向熔断**：对 `Location` 节点提取 `host:port` 维护 600s 篡改冷却，
//!   单文件最多重摇 2 次、全局重摇上限 `4 req/s`，主站连续异常自动收敛回源官方。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;

use crate::config::{DownloadSettings, DownloadSourcePolicy};

use super::doh::Doh;
use super::download::{DownloadItem, HostHeader};
use super::http::{
    MAX_REDIRECTS, RedirectSendError, USER_AGENT, expand_official_fallbacks,
    send_following_safe_redirects, validate_redirect_target,
};
use super::url_host;

const BMCLAPI_ROOT: &str = "https://bmclapi2.bangbang93.com";
const MCIM_CDN_ROOT: &str = "https://mod.mcimirror.top";
const PUBLIC_MAVEN_ROOT: &str = "https://maven.aliyun.com/repository/public";

const SOURCE_INTEGRITY_COOLDOWN: Duration = Duration::from_secs(600);
const NODE_FAIL_COOLDOWN: Duration = Duration::from_secs(180);
const NODE_WINDOW: Duration = Duration::from_secs(30);
const TAMPER_WINDOW: Duration = Duration::from_secs(1800);
const CIRCUIT_STEPS_SECS: [u64; 3] = [60, 180, 600];
const EWMA_ALPHA: f64 = 0.30;

pub const MAX_REROLLS: u32 = 2;
const MAX_REROLLS_PER_SEC: usize = 4;
const SLOW_MIRROR_MIN_FILE_BYTES: u64 = 2 * 1024 * 1024;
const SLOW_MIRROR_MIN_ELAPSED_SECS: f64 = 3.0;
const SLOW_MIRROR_FLOOR_BPS: f64 = 128.0 * 1024.0;
const HEALTHY_ALT_MIN_BPS: f64 = 512.0 * 1024.0;

const NEVER_MIRRORED_HOSTS: &[&str] = &[
    "login.microsoftonline.com",
    "xboxlive.com",
    "api.minecraftservices.com",
    "textures.minecraft.net",
    "authserver.mojang.com",
    "sessionserver.mojang.com",
    "api.curseforge.com",
    "edge.forgecdn.net",
    "mediafilez.forgecdn.net",
    "piston-meta.mojang.com",
    "api.mclo.gs",
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetClass {
    MojangObject,
    MojangAsset,
    MojangLibrary,
    NeoForge,
    Forge,
    Fabric,
    Quilt,
    MavenCentral,
    Modrinth,
    OfficialOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SizeBucket {
    Small,
    Medium,
    Large,
}

impl SizeBucket {
    pub fn from_size(size: Option<u64>) -> Self {
        match size.unwrap_or(1024 * 1024) {
            0..=262_144 => Self::Small,
            262_145..=4_194_304 => Self::Medium,
            _ => Self::Large,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceId {
    Official,
    OfficialAlt,
    Bmclapi,
    PublicMaven,
    CustomMirror,
    McimModrinth,
}

impl SourceId {
    pub fn is_mirror(self) -> bool {
        !matches!(self, Self::Official | Self::OfficialAlt)
    }

    pub fn is_bmclapi_like(self) -> bool {
        matches!(self, Self::Bmclapi | Self::CustomMirror)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Official => "官方源",
            Self::OfficialAlt => "官方互备 CDN",
            Self::Bmclapi => "社区镜像源",
            Self::PublicMaven => "公共 Maven 镜像",
            Self::CustomMirror => "自定义镜像源",
            Self::McimModrinth => "Modrinth 应急镜像",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateUrl {
    pub url: String,
    pub source: SourceId,
    pub asset_class: AssetClass,
    pub is_bmclapi_like: bool,
}

#[derive(Debug, Clone)]
struct BucketStat {
    ewma_ttfb_ms: f64,
    ewma_bps: f64,
    ewma_success: f64,
    samples: u32,
}

impl Default for BucketStat {
    fn default() -> Self {
        Self {
            ewma_ttfb_ms: 150.0,
            ewma_bps: 2.0 * 1024.0 * 1024.0,
            ewma_success: 0.95,
            samples: 0,
        }
    }
}

impl BucketStat {
    fn cost(&self, expected_size: u64) -> f64 {
        let size = (expected_size.max(65_536)) as f64;
        let latency_term = self.ewma_ttfb_ms / 1000.0;
        let transfer_term = size / self.ewma_bps.max(65_536.0);
        (latency_term + transfer_term) / self.ewma_success.clamp(0.15, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitState {
    Closed,
    Open { until: Instant, level: u8 },
    HalfOpen { in_flight: bool, level: u8 },
}

#[derive(Debug)]
struct SourceCircuit {
    state: CircuitState,
    consecutive_failures: u32,
    recent_node_failures: VecDeque<(String, Instant)>,
    recent_tamper_nodes: VecDeque<(String, Instant)>,
}

impl Default for SourceCircuit {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            recent_node_failures: VecDeque::new(),
            recent_tamper_nodes: VecDeque::new(),
        }
    }
}

#[derive(Debug)]
struct NodeBan {
    until: Instant,
    failures: u32,
}

#[derive(Debug, Default)]
struct RouterInner {
    buckets: HashMap<(SourceId, AssetClass, SizeBucket), BucketStat>,
    circuits: HashMap<SourceId, SourceCircuit>,
    node_bans: HashMap<String, NodeBan>,
    reroll_stamps: VecDeque<Instant>,
}

/// 半开探针并发与异常退出的 RAII 守卫（未结算直接 Drop 时自动释放 `HalfOpen` 占用，避免永久锁死）。
pub struct MirrorAttemptGuard {
    router: Arc<MirrorRouter>,
    source: SourceId,
    settled: AtomicBool,
}

impl MirrorAttemptGuard {
    pub fn mark_settled(&self) {
        self.settled.store(true, Ordering::Release);
    }
}

impl Drop for MirrorAttemptGuard {
    fn drop(&mut self) {
        if !self.settled.load(Ordering::Acquire) && self.source.is_mirror() {
            self.router.release_unsettled_half_open(self.source);
        }
    }
}

/// 零探针被动 EWMA 多源路由与熔断器。
#[derive(Debug, Default)]
pub struct MirrorRouter {
    inner: Mutex<RouterInner>,
    explore_seq: AtomicU64,
}

impl MirrorRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_begin_attempt(self: &Arc<Self>, source: SourceId) -> Option<MirrorAttemptGuard> {
        if !source.is_mirror() {
            return Some(MirrorAttemptGuard {
                router: self.clone(),
                source,
                settled: AtomicBool::new(true),
            });
        }
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let circuit = inner.circuits.entry(source).or_default();
        let allowed = match circuit.state {
            CircuitState::Closed => true,
            CircuitState::Open { until, level } => {
                if now >= until {
                    circuit.state = CircuitState::HalfOpen {
                        in_flight: true,
                        level,
                    };
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen { in_flight, level } => {
                if in_flight {
                    false
                } else {
                    circuit.state = CircuitState::HalfOpen {
                        in_flight: true,
                        level,
                    };
                    true
                }
            }
        };
        if allowed {
            Some(MirrorAttemptGuard {
                router: self.clone(),
                source,
                settled: AtomicBool::new(false),
            })
        } else {
            None
        }
    }

    fn release_unsettled_half_open(&self, source: SourceId) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(circuit) = inner.circuits.get_mut(&source)
            && let CircuitState::HalfOpen { level, .. } = circuit.state
        {
            circuit.state = CircuitState::HalfOpen {
                in_flight: false,
                level,
            };
        }
    }

    pub fn on_network_epoch_change(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.circuits.clear();
        inner.node_bans.clear();
        inner.reroll_stamps.clear();
    }

    pub fn plan_candidates(
        &self,
        item: &DownloadItem,
        cfg: &DownloadSettings,
        doh: Option<&Doh>,
        metered: bool,
        speed_limited: bool,
    ) -> Vec<CandidateUrl> {
        let official_urls = expand_official_fallbacks(&item.urls, doh);
        let official_candidates: Vec<CandidateUrl> = official_urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                let class = classify_url(url);
                let source = if i == 0 {
                    SourceId::Official
                } else {
                    SourceId::OfficialAlt
                };
                CandidateUrl {
                    url: url.clone(),
                    source,
                    asset_class: class,
                    is_bmclapi_like: false,
                }
            })
            .collect();

        if cfg.source_policy == DownloadSourcePolicy::OfficialOnly || !has_verified_anchor(item) {
            return official_candidates;
        }

        let now = Instant::now();
        let size_bucket = SizeBucket::from_size(item.size);
        let expected_size = item.size.unwrap_or(1024 * 1024);

        let mut primary_mirrors: Vec<CandidateUrl> = Vec::new();
        let mut modrinth_fallbacks: Vec<CandidateUrl> = Vec::new();
        let custom_root = cfg.custom_mirror_root.as_deref();

        for base in &official_urls {
            let (class, mapped) =
                map_official_to_mirrors(base, custom_root, cfg.modrinth_fallback_mirror);
            let _ = class;
            for cand in mapped {
                if cand.source == SourceId::McimModrinth {
                    push_unique(&mut modrinth_fallbacks, cand);
                } else {
                    push_unique(&mut primary_mirrors, cand);
                }
            }
        }

        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let healthy_mirrors: Vec<CandidateUrl> = primary_mirrors
            .into_iter()
            .filter(|c| is_source_eligible(&inner.circuits, c.source, now))
            .collect();
        let healthy_modrinth_fallbacks: Vec<CandidateUrl> = modrinth_fallbacks
            .into_iter()
            .filter(|c| is_source_eligible(&inner.circuits, c.source, now))
            .collect();

        let (mut official_healthy, official_degraded): (Vec<_>, Vec<_>) =
            official_candidates.into_iter().partition(|c| {
                !doh.is_some_and(|d| url_host(&c.url).is_some_and(|h| d.is_host_degraded(&h)))
            });

        let mut ordered = match cfg.source_policy {
            DownloadSourcePolicy::OfficialOnly => official_healthy,
            DownloadSourcePolicy::MirrorFirst => {
                let mut pool = healthy_mirrors;
                pool.sort_by(|a, b| {
                    let ca = bucket_cost(
                        &inner.buckets,
                        a.source,
                        a.asset_class,
                        size_bucket,
                        expected_size,
                    );
                    let cb = bucket_cost(
                        &inner.buckets,
                        b.source,
                        b.asset_class,
                        size_bucket,
                        expected_size,
                    );
                    ca.total_cmp(&cb)
                });
                pool.extend(official_healthy);
                pool
            }
            DownloadSourcePolicy::Auto => {
                official_healthy.extend(healthy_mirrors);
                official_healthy.sort_by(|a, b| {
                    let ca = bucket_cost(
                        &inner.buckets,
                        a.source,
                        a.asset_class,
                        size_bucket,
                        expected_size,
                    );
                    let cb = bucket_cost(
                        &inner.buckets,
                        b.source,
                        b.asset_class,
                        size_bucket,
                        expected_size,
                    );
                    ca.total_cmp(&cb)
                });
                // 非计费、无限速时保持 5% 探索次优源；计费/限速时严格 0%
                if !metered
                    && !speed_limited
                    && official_healthy.len() >= 2
                    && self
                        .explore_seq
                        .fetch_add(1, Ordering::Relaxed)
                        .is_multiple_of(20)
                {
                    official_healthy.swap(0, 1);
                }
                official_healthy
            }
        };

        for deg in official_degraded {
            push_unique(&mut ordered, deg);
        }
        for fallback in healthy_modrinth_fallbacks {
            push_unique(&mut ordered, fallback);
        }
        ordered
    }

    pub fn is_node_banned(&self, node_key: &str) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some(ban) = inner.node_bans.get(node_key) {
            if now < ban.until {
                return true;
            }
            inner.node_bans.remove(node_key);
        }
        false
    }

    pub fn try_acquire_reroll_quota(&self) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while inner
            .reroll_stamps
            .front()
            .is_some_and(|&t| now.duration_since(t) >= Duration::from_secs(1))
        {
            inner.reroll_stamps.pop_front();
        }
        if inner.reroll_stamps.len() >= MAX_REROLLS_PER_SEC {
            return false;
        }
        inner.reroll_stamps.push_back(now);
        true
    }

    pub fn record_success(
        &self,
        source: SourceId,
        asset_class: AssetClass,
        size: Option<u64>,
        ttfb_ms: f64,
        bytes: u64,
        elapsed_secs: f64,
    ) {
        let bucket = SizeBucket::from_size(size);
        let bps = if elapsed_secs > 0.005 {
            bytes as f64 / elapsed_secs
        } else {
            2.0 * 1024.0 * 1024.0
        };
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let stat = inner
            .buckets
            .entry((source, asset_class, bucket))
            .or_default();
        if stat.samples == 0 {
            stat.ewma_ttfb_ms = ttfb_ms.clamp(1.0, 10_000.0);
            stat.ewma_bps = bps.max(32.0 * 1024.0);
            stat.ewma_success = 1.0;
        } else {
            stat.ewma_ttfb_ms =
                EWMA_ALPHA * ttfb_ms.clamp(1.0, 10_000.0) + (1.0 - EWMA_ALPHA) * stat.ewma_ttfb_ms;
            stat.ewma_bps =
                EWMA_ALPHA * bps.max(32.0 * 1024.0) + (1.0 - EWMA_ALPHA) * stat.ewma_bps;
            stat.ewma_success = EWMA_ALPHA * 1.0 + (1.0 - EWMA_ALPHA) * stat.ewma_success;
        }
        stat.samples = stat.samples.saturating_add(1);

        if let Some(circuit) = inner.circuits.get_mut(&source) {
            circuit.consecutive_failures = 0;
            if matches!(circuit.state, CircuitState::HalfOpen { .. }) {
                circuit.state = CircuitState::Closed;
            }
        }
    }

    pub fn record_failure(&self, source: SourceId, node_key: Option<&str>) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for (&(src, _, _), stat) in &mut inner.buckets {
            if src == source {
                stat.ewma_success *= 1.0 - EWMA_ALPHA;
            }
        }

        if let Some(key) = node_key {
            let entry = inner.node_bans.entry(key.to_owned()).or_insert(NodeBan {
                until: now,
                failures: 0,
            });
            entry.failures = entry.failures.saturating_add(1);
            if entry.failures >= 2 {
                entry.until = now + NODE_FAIL_COOLDOWN;
            }
        }

        if !source.is_mirror() {
            return;
        }
        let circuit = inner.circuits.entry(source).or_default();
        circuit.consecutive_failures = circuit.consecutive_failures.saturating_add(1);
        if let Some(key) = node_key {
            while circuit
                .recent_node_failures
                .front()
                .is_some_and(|(_, t)| now.duration_since(*t) > NODE_WINDOW)
            {
                circuit.recent_node_failures.pop_front();
            }
            circuit
                .recent_node_failures
                .push_back((key.to_owned(), now));
        }
        let distinct_failed_nodes: HashSet<&str> = circuit
            .recent_node_failures
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        let should_open = circuit.consecutive_failures >= 5
            || distinct_failed_nodes.len() >= 3
            || matches!(circuit.state, CircuitState::HalfOpen { .. });
        if should_open {
            open_circuit(circuit, now, None);
        }
    }

    pub fn record_integrity_violation(&self, source: SourceId, node_key: Option<&str>) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = node_key {
            inner.node_bans.insert(
                key.to_owned(),
                NodeBan {
                    until: now + SOURCE_INTEGRITY_COOLDOWN,
                    failures: 2,
                },
            );
        }
        if !source.is_mirror() {
            return;
        }
        let circuit = inner.circuits.entry(source).or_default();
        if let Some(key) = node_key {
            while circuit
                .recent_tamper_nodes
                .front()
                .is_some_and(|(_, t)| now.duration_since(*t) > TAMPER_WINDOW)
            {
                circuit.recent_tamper_nodes.pop_front();
            }
            circuit.recent_tamper_nodes.push_back((key.to_owned(), now));
            let distinct_tamper_nodes: HashSet<&str> = circuit
                .recent_tamper_nodes
                .iter()
                .map(|(k, _)| k.as_str())
                .collect();
            if distinct_tamper_nodes.len() >= 2 {
                open_circuit(circuit, now, Some(SOURCE_INTEGRITY_COOLDOWN));
            }
        } else {
            open_circuit(circuit, now, Some(SOURCE_INTEGRITY_COOLDOWN));
        }
    }

    pub fn should_reroll_slow_mirror(
        &self,
        source: SourceId,
        asset_class: AssetClass,
        size: Option<u64>,
        elapsed_secs: f64,
        cur_bps: f64,
    ) -> bool {
        if !source.is_bmclapi_like()
            || size.unwrap_or(0) <= SLOW_MIRROR_MIN_FILE_BYTES
            || elapsed_secs < SLOW_MIRROR_MIN_ELAPSED_SECS
            || cur_bps >= SLOW_MIRROR_FLOOR_BPS
        {
            return false;
        }
        let bucket = SizeBucket::from_size(size);
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .buckets
            .get(&(SourceId::Official, asset_class, bucket))
            .is_none_or(|stat| stat.samples == 0 || stat.ewma_bps >= HEALTHY_ALT_MIN_BPS)
    }
}

pub fn matches_domain_suffix(host: &str, suffix: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h == suffix
        || h.strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

pub fn is_never_mirrored_host(host: &str) -> bool {
    NEVER_MIRRORED_HOSTS
        .iter()
        .any(|blocked| matches_domain_suffix(host, blocked))
}

pub fn is_mirrorable_host(host: &str) -> bool {
    [
        "piston-data.mojang.com",
        "launchermeta.mojang.com",
        "launcher.mojang.com",
        "resources.download.minecraft.net",
        "libraries.minecraft.net",
        "maven.neoforged.net",
        "neoforged.forgecdn.net",
        "maven.minecraftforge.net",
        "maven.fabricmc.net",
        "maven.quiltmc.org",
        "repo1.maven.org",
        "cdn.modrinth.com",
    ]
    .iter()
    .any(|allowed| matches_domain_suffix(host, allowed))
}

pub fn has_verified_anchor(item: &DownloadItem) -> bool {
    item.sha1.as_deref().is_some_and(|raw| {
        let trimmed = raw.trim();
        !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_hexdigit())
    })
}

pub fn extract_node_key(url: &reqwest::Url) -> String {
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let port = url.port_or_known_default().unwrap_or(443);
    format!("{host}:{port}")
}

/// 从 URL 字符串提取与 302 黑名单同口径的 `host:port` 节点标识（保留非 443 端口）。
pub(crate) fn node_key_from_url_str(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok().map(|u| extract_node_key(&u))
}

/// 带 BMCLAPI 302 节点黑名单剔除与最优停止重摇的安全请求发送。
pub(crate) async fn send_following_safe_redirects_with_mirror(
    client: &Client,
    initial_url: &str,
    range_header: Option<&str>,
    host_header: Option<&HostHeader>,
    is_bmclapi_like: bool,
    router: &MirrorRouter,
) -> Result<(reqwest::Response, String, Option<String>), RedirectSendError> {
    if !is_bmclapi_like {
        let (resp, final_url) =
            send_following_safe_redirects(client, initial_url, range_header, host_header).await?;
        let node = node_key_from_url_str(&final_url);
        return Ok((resp, final_url, node));
    }

    let mut current_url = initial_url.to_owned();
    let mut current = reqwest::Url::parse(&current_url)
        .map_err(|e| RedirectSendError::Protocol(format!("无效下载地址 {current_url}：{e}")))?;
    let mut rerolls = 0u32;
    let mut selected_node: Option<String> = None;

    for _step in 0..=(MAX_REDIRECTS + MAX_REROLLS as usize) {
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
        if !resp.status().is_redirection() {
            let final_node = selected_node.or_else(|| Some(extract_node_key(&current)));
            return Ok((resp, current_url, final_node));
        }

        let Some(location) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        else {
            let final_node = selected_node.or_else(|| Some(extract_node_key(&current)));
            return Ok((resp, current_url, final_node));
        };

        let next = current.join(location).map_err(|e| {
            RedirectSendError::Protocol(format!("重定向地址无效（{location}）：{e}"))
        })?;
        validate_redirect_target(&current, &next).map_err(RedirectSendError::Protocol)?;

        let node_key = extract_node_key(&next);
        if router.is_node_banned(&node_key) {
            if rerolls < MAX_REROLLS && router.try_acquire_reroll_quota() {
                rerolls += 1;
                selected_node = None;
                current_url = initial_url.to_owned();
                current = reqwest::Url::parse(&current_url).map_err(|e| {
                    RedirectSendError::Protocol(format!("无效下载地址 {current_url}：{e}"))
                })?;
                continue;
            }
            return Err(RedirectSendError::Protocol(
                "镜像节点处于冷却期且已达重摇上限，直接回源官方".to_owned(),
            ));
        }
        selected_node = Some(node_key);
        current_url = next.to_string();
        current = next;
    }

    Err(RedirectSendError::Protocol("重定向次数过多".to_owned()))
}

fn classify_url(url: &str) -> AssetClass {
    let Some(host) = url_host(url) else {
        return AssetClass::OfficialOnly;
    };
    if is_never_mirrored_host(&host) {
        return AssetClass::OfficialOnly;
    }
    if url.starts_with("https://piston-data.mojang.com/")
        || url.starts_with("https://launchermeta.mojang.com/")
        || url.starts_with("https://launcher.mojang.com/")
    {
        AssetClass::MojangObject
    } else if url.starts_with("https://resources.download.minecraft.net/") {
        AssetClass::MojangAsset
    } else if url.starts_with("https://libraries.minecraft.net/") {
        AssetClass::MojangLibrary
    } else if url.starts_with("https://maven.neoforged.net/releases/")
        || url.starts_with("https://neoforged.forgecdn.net/releases/")
    {
        AssetClass::NeoForge
    } else if url.starts_with("https://maven.minecraftforge.net/") {
        AssetClass::Forge
    } else if url.starts_with("https://maven.fabricmc.net/") {
        AssetClass::Fabric
    } else if url.starts_with("https://maven.quiltmc.org/repository/release/") {
        AssetClass::Quilt
    } else if url.starts_with("https://repo1.maven.org/maven2/") {
        AssetClass::MavenCentral
    } else if url.starts_with("https://cdn.modrinth.com/") {
        AssetClass::Modrinth
    } else {
        AssetClass::OfficialOnly
    }
}

fn map_official_to_mirrors(
    url: &str,
    custom_root: Option<&str>,
    modrinth_fallback: bool,
) -> (AssetClass, Vec<CandidateUrl>) {
    let class = classify_url(url);
    if class == AssetClass::OfficialOnly {
        return (class, Vec::new());
    }

    let custom_clean = custom_root.map(|r| r.trim_end_matches('/'));
    let mut out = Vec::new();
    let push_bmclapi_pair = |out: &mut Vec<CandidateUrl>, suffix: &str| {
        let clean = suffix.trim_start_matches('/');
        out.push(CandidateUrl {
            url: format!("{BMCLAPI_ROOT}/{clean}"),
            source: SourceId::Bmclapi,
            asset_class: class,
            is_bmclapi_like: true,
        });
        if let Some(root) = custom_clean {
            out.push(CandidateUrl {
                url: format!("{root}/{clean}"),
                source: SourceId::CustomMirror,
                asset_class: class,
                is_bmclapi_like: true,
            });
        }
    };

    if let Some(rest) = url
        .strip_prefix("https://piston-data.mojang.com/")
        .or_else(|| url.strip_prefix("https://launchermeta.mojang.com/"))
        .or_else(|| url.strip_prefix("https://launcher.mojang.com/"))
    {
        push_bmclapi_pair(&mut out, rest);
    } else if let Some(rest) = url.strip_prefix("https://resources.download.minecraft.net/") {
        push_bmclapi_pair(&mut out, &format!("assets/{rest}"));
    } else if let Some(rest) = url
        .strip_prefix("https://libraries.minecraft.net/")
        .or_else(|| url.strip_prefix("https://maven.neoforged.net/releases/"))
        .or_else(|| url.strip_prefix("https://neoforged.forgecdn.net/releases/"))
        .or_else(|| url.strip_prefix("https://maven.minecraftforge.net/"))
        .or_else(|| url.strip_prefix("https://maven.fabricmc.net/"))
        .or_else(|| url.strip_prefix("https://maven.quiltmc.org/repository/release/"))
    {
        push_bmclapi_pair(&mut out, &format!("maven/{rest}"));
    } else if let Some(rest) = url.strip_prefix("https://repo1.maven.org/maven2/") {
        push_bmclapi_pair(&mut out, &format!("maven/{rest}"));
        out.push(CandidateUrl {
            url: format!("{PUBLIC_MAVEN_ROOT}/{rest}"),
            source: SourceId::PublicMaven,
            asset_class: class,
            is_bmclapi_like: false,
        });
    } else if let Some(rest) = url.strip_prefix("https://cdn.modrinth.com/")
        && modrinth_fallback
    {
        out.push(CandidateUrl {
            url: format!("{MCIM_CDN_ROOT}/{rest}"),
            source: SourceId::McimModrinth,
            asset_class: class,
            is_bmclapi_like: false,
        });
    }

    (class, out)
}

fn push_unique(vec: &mut Vec<CandidateUrl>, cand: CandidateUrl) {
    if !vec.iter().any(|existing| existing.url == cand.url) {
        vec.push(cand);
    }
}

fn bucket_cost(
    buckets: &HashMap<(SourceId, AssetClass, SizeBucket), BucketStat>,
    source: SourceId,
    class: AssetClass,
    size_bucket: SizeBucket,
    expected_size: u64,
) -> f64 {
    buckets.get(&(source, class, size_bucket)).map_or_else(
        || BucketStat::default().cost(expected_size),
        |s| s.cost(expected_size),
    )
}

fn is_source_eligible(
    circuits: &HashMap<SourceId, SourceCircuit>,
    source: SourceId,
    now: Instant,
) -> bool {
    if !source.is_mirror() {
        return true;
    }
    let Some(circuit) = circuits.get(&source) else {
        return true;
    };
    match circuit.state {
        CircuitState::Closed => true,
        CircuitState::Open { until, .. } => now >= until,
        CircuitState::HalfOpen { in_flight, .. } => !in_flight,
    }
}

fn open_circuit(circuit: &mut SourceCircuit, now: Instant, forced_cooldown: Option<Duration>) {
    let next_level = match circuit.state {
        CircuitState::Open { level, .. } | CircuitState::HalfOpen { level, .. } => {
            level.saturating_add(1)
        }
        CircuitState::Closed => 0,
    };
    let idx = usize::from(next_level).min(CIRCUIT_STEPS_SECS.len() - 1);
    let dur = forced_cooldown.unwrap_or_else(|| Duration::from_secs(CIRCUIT_STEPS_SECS[idx]));
    circuit.state = CircuitState::Open {
        until: now + dur,
        level: next_level,
    };
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;

    fn test_item(url: &str, sha1: Option<&str>, size: Option<u64>) -> DownloadItem {
        DownloadItem {
            urls: vec![url.to_owned()],
            path: PathBuf::from("artifact.jar"),
            sha1: sha1.map(str::to_owned),
            size,
        }
    }

    #[test]
    fn never_mirrors_auth_curseforge_or_unhashed_items() {
        let router = MirrorRouter::new();
        let cfg = DownloadSettings {
            source_policy: DownloadSourcePolicy::MirrorFirst,
            modrinth_fallback_mirror: true,
            custom_mirror_root: Some("https://mirror.example.com".to_owned()),
            ..Default::default()
        };

        for url in [
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            "https://user.auth.xboxlive.com/user/authenticate",
            "https://api.minecraftservices.com/minecraft/profile",
            "https://textures.minecraft.net/texture/abc",
            "https://edge.forgecdn.net/files/1234/567/mod.jar",
            "https://mediafilez.forgecdn.net/files/1234/567/mod.jar",
            "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
        ] {
            let item = test_item(
                url,
                Some("0123456789abcdef0123456789abcdef01234567"),
                Some(100_000),
            );
            let cands = router.plan_candidates(&item, &cfg, None, false, false);
            assert!(
                cands.iter().all(|c| !c.source.is_mirror()),
                "forbidden host {url} generated mirror candidates: {cands:?}"
            );
        }

        // 无强哈希资产必须拒走一切镜像
        let unhashed = test_item(
            "https://libraries.minecraft.net/com/mojang/authlib/6.0.55/authlib-6.0.55.jar",
            None,
            Some(120_000),
        );
        let cands = router.plan_candidates(&unhashed, &cfg, None, false, false);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].source, SourceId::Official);
    }

    #[test]
    fn rewrites_mojang_loaders_custom_and_modrinth_fallback_correctly() {
        let router = MirrorRouter::new();
        let mut cfg = DownloadSettings {
            source_policy: DownloadSourcePolicy::MirrorFirst,
            custom_mirror_root: Some("https://mirror.example.com".to_owned()),
            modrinth_fallback_mirror: false,
            ..Default::default()
        };

        let sha = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let nf = test_item(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.1/neoforge-21.1.1.jar",
            sha,
            Some(3_000_000),
        );
        let nf_urls: Vec<String> = router
            .plan_candidates(&nf, &cfg, None, false, false)
            .into_iter()
            .map(|c| c.url)
            .collect();
        assert!(nf_urls.contains(&format!(
            "{BMCLAPI_ROOT}/maven/net/neoforged/neoforge/21.1.1/neoforge-21.1.1.jar"
        )));
        assert!(nf_urls.contains(&"https://mirror.example.com/maven/net/neoforged/neoforge/21.1.1/neoforge-21.1.1.jar".to_owned()));
        assert!(nf_urls.contains(&"https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.1/neoforge-21.1.1.jar".to_owned()));
        assert!(nf_urls.contains(&"https://neoforged.forgecdn.net/releases/net/neoforged/neoforge/21.1.1/neoforge-21.1.1.jar".to_owned()));

        let mod_item = test_item(
            "https://cdn.modrinth.com/data/AANobbMI/versions/abc/sodium.jar",
            sha,
            Some(900_000),
        );
        let without_mcim = router.plan_candidates(&mod_item, &cfg, None, false, false);
        assert!(
            without_mcim
                .iter()
                .all(|c| c.source != SourceId::McimModrinth)
        );

        cfg.modrinth_fallback_mirror = true;
        let with_mcim = router.plan_candidates(&mod_item, &cfg, None, false, false);
        assert_eq!(with_mcim[0].source, SourceId::Official);
        assert_eq!(with_mcim.last().unwrap().source, SourceId::McimModrinth);
    }

    #[test]
    fn integrity_violation_cools_node_for_600s_and_opens_circuit_on_two_nodes() {
        let router = MirrorRouter::new();
        router.record_integrity_violation(SourceId::Bmclapi, Some("bad1.node:443"));
        assert!(router.is_node_banned("bad1.node:443"));

        // 单节点篡改尚未打开主站熔断，但跨 2 个不同节点篡改必须立即熔断主站 600s
        let cfg = DownloadSettings {
            source_policy: DownloadSourcePolicy::MirrorFirst,
            ..Default::default()
        };
        let item = test_item(
            "https://libraries.minecraft.net/com/google/guava/guava.jar",
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            Some(500_000),
        );
        let cands1 = router.plan_candidates(&item, &cfg, None, false, false);
        assert!(cands1.iter().any(|c| c.source == SourceId::Bmclapi));

        router.record_integrity_violation(SourceId::Bmclapi, Some("bad2.node:443"));
        let cands2 = router.plan_candidates(&item, &cfg, None, false, false);
        assert!(cands2.iter().all(|c| c.source != SourceId::Bmclapi));

        // 代理 epoch 变更立即复位熔断与节点黑名单
        router.on_network_epoch_change();
        assert!(!router.is_node_banned("bad1.node:443"));
        let cands3 = router.plan_candidates(&item, &cfg, None, false, false);
        assert!(cands3.iter().any(|c| c.source == SourceId::Bmclapi));
    }

    #[test]
    fn reroll_rate_limit_and_slow_mirror_threshold_behave_accurately() {
        let router = MirrorRouter::new();
        for _ in 0..MAX_REROLLS_PER_SEC {
            assert!(router.try_acquire_reroll_quota());
        }
        assert!(
            !router.try_acquire_reroll_quota(),
            "超过每秒 4 次必须拦截重摇"
        );

        router.record_success(
            SourceId::Official,
            AssetClass::MojangObject,
            Some(10 * 1024 * 1024),
            80.0,
            10 * 1024 * 1024,
            1.0,
        );
        assert!(router.should_reroll_slow_mirror(
            SourceId::Bmclapi,
            AssetClass::MojangObject,
            Some(10 * 1024 * 1024),
            3.5,
            64.0 * 1024.0,
        ));
        assert!(!router.should_reroll_slow_mirror(
            SourceId::Bmclapi,
            AssetClass::MojangObject,
            Some(512 * 1024),
            5.0,
            32.0 * 1024.0,
        ));
    }

    #[test]
    fn ewma_bucket_scheduling_and_zero_exploration_when_metered_or_limited() {
        let router = MirrorRouter::new();
        let cfg = DownloadSettings {
            source_policy: DownloadSourcePolicy::Auto,
            ..Default::default()
        };
        let item = test_item(
            "https://libraries.minecraft.net/net/sf/jopt-simple/jopt.jar",
            Some("cccccccccccccccccccccccccccccccccccccccc"),
            Some(128 * 1024),
        );

        // 让镜像源在同一分桶显著更快
        router.record_success(
            SourceId::Official,
            AssetClass::MojangLibrary,
            item.size,
            400.0,
            128 * 1024,
            1.2,
        );
        router.record_success(
            SourceId::Bmclapi,
            AssetClass::MojangLibrary,
            item.size,
            25.0,
            128 * 1024,
            0.05,
        );

        // 计费模式下连续 40 次计划都不得触发探索换序
        for _ in 0..40 {
            let cands = router.plan_candidates(&item, &cfg, None, true, false);
            assert_eq!(cands[0].source, SourceId::Bmclapi);
        }
    }

    #[tokio::test]
    async fn bmclapi_302_reroll_skips_banned_node_and_hits_healthy_node() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrd};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let good_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good_addr = good_listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = good_listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ngood")
                        .await;
                });
            }
        });

        let bad_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bad_addr = bad_listener.local_addr().unwrap();
        let bad_hits = Arc::new(AtomicUsize::new(0));
        let bad_hits_srv = bad_hits.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = bad_listener.accept().await {
                bad_hits_srv.fetch_add(1, AtomicOrd::AcqRel);
                let _ = sock.shutdown().await;
            }
        });

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_srv = calls.clone();
        let coord_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let coord_addr = coord_listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = coord_listener.accept().await {
                let n = calls_srv.fetch_add(1, AtomicOrd::AcqRel);
                tokio::spawn(async move {
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let target = if n == 0 { bad_addr } else { good_addr };
                    let resp = format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{target}/artifact.jar\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });

        let router = MirrorRouter::new();
        router.record_integrity_violation(SourceId::Bmclapi, Some(&bad_addr.to_string()));
        assert!(router.is_node_banned(&bad_addr.to_string()));

        let client = crate::net::http::client_builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let (resp, final_url, node_key) = send_following_safe_redirects_with_mirror(
            &client,
            &format!("http://{coord_addr}/maven/test.jar"),
            None,
            None,
            true,
            &router,
        )
        .await
        .unwrap();

        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "good");
        assert_eq!(final_url, format!("http://{good_addr}/artifact.jar"));
        assert_eq!(node_key.as_deref(), Some(good_addr.to_string().as_str()));
        assert_eq!(
            bad_hits.load(AtomicOrd::Acquire),
            0,
            "已拉黑节点绝不能被实际建连"
        );
        assert_eq!(
            calls.load(AtomicOrd::Acquire),
            2,
            "应向主站重摇 1 次并锁定健康节点"
        );
    }

    #[test]
    fn half_open_probe_recovers_when_unattempted_or_cancelled() {
        let router = Arc::new(MirrorRouter::new());
        let cfg = DownloadSettings {
            source_policy: DownloadSourcePolicy::Auto,
            ..Default::default()
        };
        let item = test_item(
            "https://libraries.minecraft.net/com/google/guava/guava.jar",
            Some("dddddddddddddddddddddddddddddddddddddddd"),
            Some(500_000),
        );

        // 模拟 BMCLAPI 处于已到期的 Open 熔断态
        {
            let mut inner = router.inner.lock().unwrap();
            inner.circuits.insert(
                SourceId::Bmclapi,
                SourceCircuit {
                    state: CircuitState::Open {
                        until: Instant::now() - Duration::from_secs(1),
                        level: 0,
                    },
                    consecutive_failures: 5,
                    recent_node_failures: VecDeque::new(),
                    recent_tamper_nodes: VecDeque::new(),
                },
            );
        }

        // 1) plan_candidates 只读检查，即使未触发尝试，连续多次规划都不会卡死或消耗 HalfOpen 名额
        for _ in 0..3 {
            let cands = router.plan_candidates(&item, &cfg, None, false, false);
            assert!(cands.iter().any(|c| c.source == SourceId::Bmclapi));
        }

        // 2) try_begin_attempt 占住唯一的 HalfOpen 探针槽位，期间并发第 2 个请求被拦截
        let guard1 = router.try_begin_attempt(SourceId::Bmclapi);
        assert!(guard1.is_some());
        assert!(
            router.try_begin_attempt(SourceId::Bmclapi).is_none(),
            "HalfOpen 仅允许放行 1 个真实试探请求"
        );

        // 3) 若 guard1 未结算即 Drop（如暂停/取消/跳过），必须自动复位 in_flight=false，避免永久锁死
        drop(guard1);
        let guard2 = router.try_begin_attempt(SourceId::Bmclapi);
        assert!(guard2.is_some(), "未结算 Drop 后必须恢复允许下一次试探");
    }

    #[test]
    fn node_key_preserves_non_default_ports() {
        assert_eq!(
            node_key_from_url_str("https://edge.openbmclapi.link:9443/files/a"),
            Some("edge.openbmclapi.link:9443".to_owned())
        );
        assert_eq!(
            node_key_from_url_str("https://edge.openbmclapi.link/files/a"),
            Some("edge.openbmclapi.link:443".to_owned())
        );

        let router = MirrorRouter::new();
        router.record_failure(SourceId::Bmclapi, Some("edge.openbmclapi.link:9443"));
        router.record_failure(SourceId::Bmclapi, Some("edge.openbmclapi.link:9443"));
        assert!(router.is_node_banned("edge.openbmclapi.link:9443"));
        assert!(!router.is_node_banned("edge.openbmclapi.link:443"));
    }
}
