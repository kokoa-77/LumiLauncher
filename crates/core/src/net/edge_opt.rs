//! 官方 CDN 多 `/24` 边缘异步优选与受控批尾小文件对冲（Phase 4，100% 官方域、零探针浪费约束）。
//!
//! - **官方边缘比测（`OfficialEdgeOptimizer`）**：仅在直连 + DoH 开启 + 非计费 + 非限速 + 非 SNI 阻断下，
//!   检测到官方 CDN 明显退化时，最多从已通过 `DomainClass` 隔离的 DoH 节点收集 `<= 3` 个不同 `/24`（或 IPv6 `/48`）
//!   候选 POP，每候选做 `256 KiB` Range GET 比测（全局滑动 `<= 8 MiB/h` 硬预算）；
//!   若胜出 POP 显著优于当前节点（`>= 1.25x` 且 `>= 512 KiB/s`），注入 `Doh` 优选 IP 20 分钟；
//!   后续持续观测吞吐窗口，连续 2 个窗口低于切换前基线 `80%` 即刻自动回滚并降级该 POP 10 分钟。
//! - **批尾小文件对冲（`BatchTailHedger`）**：严格禁止在计费网络或限速模式下运行；仅当整批文件数 `>= 4`、
//!   剩余文件数 `<= 4` 且目标已知大小 `<= 2 MiB` 时启用，在自适应延迟 `clamp(2.5 * EWMA, 800ms..=3000ms)` 后
//!   于主请求仍未收齐 `75%` 时使用 `ClientBundle::spare` 新连接发起受控对冲，每批最多 `4` 次对冲。

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;

use super::dns_wire::is_strictly_global_unicast;
use super::doh::Doh;
use super::download::{DownloadEngine, looks_like_html_payload};
use super::http::USER_AGENT;
use super::telemetry::{TelemetryEventKind, TelemetryHub};

/// 允许执行官方多 `/24` POP 优选的 100% 官方 CDN 域名白名单（绝不含认证/皮肤/API 域名）。
pub const OFFICIAL_CDN_POP_HOSTS: &[&str] = &[
    "piston-data.mojang.com",
    "libraries.minecraft.net",
    "resources.download.minecraft.net",
    "cdn.modrinth.com",
    "neoforged.forgecdn.net",
    "edge.forgecdn.net",
    "mediafilez.forgecdn.net",
];

pub const HOURLY_PROBE_BUDGET_BYTES: u64 = 8 * 1024 * 1024; // 8 MiB/h 硬上限
pub const PROBE_SLICE_BYTES: u64 = 256 * 1024; // 256 KiB/候选
pub const MAX_CANDIDATE_SUBNETS: usize = 3;
pub const HOST_PROBE_COOLDOWN: Duration = Duration::from_secs(300);
pub const PREFERRED_POP_TTL: Duration = Duration::from_secs(1200);
pub const DEMOTED_POP_TTL: Duration = Duration::from_secs(600);
pub const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const PROBE_TOTAL_TIMEOUT: Duration = Duration::from_millis(3500);

pub const DEGRADED_ABS_BPS: f64 = 3.5 * 1024.0 * 1024.0;
pub const MIN_WINNER_BPS: f64 = 512.0 * 1024.0;
pub const MIN_IMPROVEMENT_RATIO: f64 = 1.25;
pub const ROLLBACK_THRESHOLD_RATIO: f64 = 0.80;
pub const ROLLBACK_CONSECUTIVE_WINDOWS: u8 = 2;
const PROMOTION_WARMUP_SECS: f64 = 1.5;

pub const TAIL_HEDGE_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const TAIL_HEDGE_MIN_BATCH_FILES: u64 = 4;
pub const TAIL_HEDGE_MAX_REMAINING: u64 = 4;
pub const TAIL_HEDGE_MAX_PER_BATCH: u32 = 4;
pub const TAIL_HEDGE_MIN_DELAY_MS: f64 = 800.0;
pub const TAIL_HEDGE_MAX_DELAY_MS: f64 = 3000.0;
const TAIL_HEDGE_DEFAULT_DELAY_MS: f64 = 1500.0;
const HOUR_SECS: Duration = Duration::from_secs(3600);

/// 判断域名是否属于 Phase 4 允许官方多 POP 优选的资产分发域名。
pub fn is_official_cdn_pop_host(host: &str) -> bool {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    OFFICIAL_CDN_POP_HOSTS.iter().any(|&h| h == lower)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SubnetKey {
    V4([u8; 3]),
    V6([u8; 6]),
}

fn subnet_key(ip: IpAddr) -> SubnetKey {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            SubnetKey::V4([o[0], o[1], o[2]])
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                let o = v4.octets();
                SubnetKey::V4([o[0], o[1], o[2]])
            } else {
                let o = v6.octets();
                SubnetKey::V6([o[0], o[1], o[2], o[3], o[4], o[5]])
            }
        }
    }
}

/// 按 IPv4 `/24`（或 IPv6 `/48`）对全球单播 IP 去重，最多返回 `max_count` 个代表节点。
pub fn dedupe_by_subnet(ips: &[IpAddr], max_count: usize) -> Vec<IpAddr> {
    if max_count == 0 {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(max_count.min(ips.len()));
    for &ip in ips {
        if !is_strictly_global_unicast(ip) {
            continue;
        }
        if seen.insert(subnet_key(ip)) {
            out.push(ip);
            if out.len() >= max_count {
                break;
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct HourlyByteBudget {
    entries: VecDeque<(Instant, u64)>,
}

impl HourlyByteBudget {
    fn prune(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) >= HOUR_SECS)
        {
            self.entries.pop_front();
        }
    }

    fn used_bytes(&mut self, now: Instant) -> u64 {
        self.prune(now);
        self.entries.iter().map(|(_, b)| *b).sum()
    }

    fn remaining_bytes(&mut self, now: Instant) -> u64 {
        HOURLY_PROBE_BUDGET_BYTES.saturating_sub(self.used_bytes(now))
    }

    fn try_reserve_candidates(
        &mut self,
        requested: usize,
        slice_bytes: u64,
        now: Instant,
    ) -> usize {
        if slice_bytes == 0 || requested == 0 {
            return 0;
        }
        let rem = self.remaining_bytes(now);
        let affordable = usize::try_from(rem / slice_bytes).unwrap_or(usize::MAX);
        let granted = requested.min(affordable);
        if granted > 0 {
            self.entries.push_back((now, slice_bytes * granted as u64));
        }
        granted
    }
}

#[derive(Debug, Clone)]
struct ActivePopSelection {
    selected_ip: IpAddr,
    baseline_bps: f64,
    promoted_at: Instant,
    underperform_windows: u8,
    verified_good_windows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopWindowOutcome {
    NoActiveSelection,
    WarmingUp,
    Healthy,
    Underperforming,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeProbeSample {
    pub ip: IpAddr,
    pub bps: f64,
    pub ttfb_ms: f64,
    pub bytes: u64,
}

#[derive(Debug, Default)]
struct OptimizerState {
    budget: HourlyByteBudget,
    last_probe_at: HashMap<String, Instant>,
    in_flight_hosts: HashSet<String>,
    active_selections: HashMap<String, ActivePopSelection>,
}

/// 官方 CDN 多 `/24` 边缘异步比测与两窗口 `< 80%` 自动回滚管理器。
#[derive(Debug, Default)]
pub struct OfficialEdgeOptimizer {
    state: Mutex<OptimizerState>,
}

impl OfficialEdgeOptimizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 网络代际变更时清除历史探测冷却与优选绑定（保留 1 小时字节预算以防绕过总量限制）。
    pub fn on_network_epoch_change(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.last_probe_at.clear();
        st.active_selections.clear();
        st.in_flight_hosts.clear();
    }

    pub fn remaining_hourly_budget_bytes(&self, now: Instant) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .budget
            .remaining_bytes(now)
    }

    pub fn used_hourly_budget_bytes(&self, now: Instant) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .budget
            .used_bytes(now)
    }

    pub fn try_reserve_probe_candidates(&self, requested: usize, now: Instant) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .budget
            .try_reserve_candidates(requested, PROBE_SLICE_BYTES, now)
    }

    pub fn has_active_selection(&self, host: &str) -> bool {
        let key = normalize_host(host);
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_selections
            .contains_key(&key)
    }

    pub fn active_selected_ip(&self, host: &str) -> Option<IpAddr> {
        let key = normalize_host(host);
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_selections
            .get(&key)
            .map(|s| s.selected_ip)
    }

    /// 评估当前正处于优选状态的官方 POP 在最新观测窗口的表现：
    /// 连续 2 个窗口 `< 80%` 切换前基线时自动回滚并降级该 POP 10 分钟。
    pub fn evaluate_active_pop_window(
        &self,
        host: &str,
        observed_bps: f64,
        now: Instant,
        doh: &Doh,
        telemetry: &TelemetryHub,
    ) -> PopWindowOutcome {
        if observed_bps <= 0.0 || !observed_bps.is_finite() {
            return if self.has_active_selection(host) {
                PopWindowOutcome::WarmingUp
            } else {
                PopWindowOutcome::NoActiveSelection
            };
        }
        let key = normalize_host(host);
        let active_doh_pref = doh.preferred_pop_ip(&key);
        let rollback_target = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(sel) = st.active_selections.get(&key)
                && (now.saturating_duration_since(sel.promoted_at) >= PREFERRED_POP_TTL
                    || active_doh_pref != Some(sel.selected_ip))
            {
                st.active_selections.remove(&key);
                return PopWindowOutcome::NoActiveSelection;
            }
            let Some(sel) = st.active_selections.get_mut(&key) else {
                return PopWindowOutcome::NoActiveSelection;
            };
            if now.saturating_duration_since(sel.promoted_at).as_secs_f64() < PROMOTION_WARMUP_SECS
            {
                return PopWindowOutcome::WarmingUp;
            }
            if observed_bps < sel.baseline_bps * ROLLBACK_THRESHOLD_RATIO {
                sel.underperform_windows = sel.underperform_windows.saturating_add(1);
                if sel.underperform_windows >= ROLLBACK_CONSECUTIVE_WINDOWS {
                    let removed = st.active_selections.remove(&key);
                    st.last_probe_at.insert(key.clone(), now);
                    removed.map(|r| r.selected_ip)
                } else {
                    return PopWindowOutcome::Underperforming;
                }
            } else {
                sel.underperform_windows = 0;
                sel.verified_good_windows = sel.verified_good_windows.saturating_add(1);
                return PopWindowOutcome::Healthy;
            }
        };

        if let Some(bad_ip) = rollback_target {
            doh.clear_preferred_pop_ip(&key, Some((bad_ip, DEMOTED_POP_TTL)));
            telemetry.push_event(
                TelemetryEventKind::RampRollback,
                "官方 CDN 新边缘节点连续两窗口低于预期，已自动回滚",
            );
            PopWindowOutcome::RolledBack
        } else {
            PopWindowOutcome::Healthy
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn should_trigger_probe(
        &self,
        host: &str,
        observed_bps: f64,
        historical_peak_bps: Option<f64>,
        metered: bool,
        speed_limit_kib: u32,
        proxied_or_tun: bool,
        doh_enabled: bool,
        sni_blocked: bool,
        now: Instant,
    ) -> bool {
        if metered
            || speed_limit_kib > 0
            || proxied_or_tun
            || !doh_enabled
            || sni_blocked
            || !is_official_cdn_pop_host(host)
            || observed_bps <= 0.0
            || !observed_bps.is_finite()
        {
            return false;
        }
        let degraded = observed_bps < DEGRADED_ABS_BPS
            || historical_peak_bps
                .is_some_and(|peak| peak >= 4.0 * 1024.0 * 1024.0 && observed_bps < peak * 0.65);
        if !degraded {
            return false;
        }

        let key = normalize_host(host);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st
            .active_selections
            .get(&key)
            .is_some_and(|sel| now.saturating_duration_since(sel.promoted_at) >= PREFERRED_POP_TTL)
        {
            st.active_selections.remove(&key);
        }
        if st.active_selections.contains_key(&key) || st.in_flight_hosts.contains(&key) {
            return false;
        }
        if st
            .last_probe_at
            .get(&key)
            .is_some_and(|&at| now.saturating_duration_since(at) < HOST_PROBE_COOLDOWN)
        {
            return false;
        }
        st.budget.remaining_bytes(now) >= PROBE_SLICE_BYTES * 2
    }

    /// 评估探测样本；若存在显著更快且健康的官方 `/24` POP，则注入 `Doh` 并登记活跃优选记录。
    #[allow(clippy::too_many_arguments)]
    pub fn apply_probe_results(
        &self,
        host: &str,
        current_ip: Option<IpAddr>,
        observed_bps: f64,
        samples: &[EdgeProbeSample],
        now: Instant,
        doh: &Doh,
        telemetry: &TelemetryHub,
    ) -> Option<IpAddr> {
        let key = normalize_host(host);
        let current_probe_bps = current_ip
            .and_then(|cur| samples.iter().find(|s| s.ip == cur).map(|s| s.bps))
            .unwrap_or(0.0);
        let baseline = current_probe_bps.max(observed_bps).max(64.0 * 1024.0);

        let best = samples
            .iter()
            .filter(|s| s.bytes > 0 && s.bps.is_finite() && s.bps >= MIN_WINNER_BPS)
            .filter(|s| current_ip.is_none_or(|cur| cur != s.ip))
            .max_by(|a, b| a.bps.total_cmp(&b.bps))?;

        if best.bps < baseline * MIN_IMPROVEMENT_RATIO {
            return None;
        }

        if !doh.set_preferred_pop_ip(&key, best.ip, PREFERRED_POP_TTL) {
            return None;
        }

        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.last_probe_at.insert(key.clone(), now);
            st.active_selections.insert(
                key,
                ActivePopSelection {
                    selected_ip: best.ip,
                    baseline_bps: baseline.max(MIN_WINNER_BPS),
                    promoted_at: now,
                    underperform_windows: 0,
                    verified_good_windows: 0,
                },
            );
        }
        telemetry.push_event(
            TelemetryEventKind::RouteSwitched,
            "检测到更快的官方 CDN 边缘节点，已自动优选切换",
        );
        Some(best.ip)
    }

    /// 异步旁路触发多 `/24` POP 比测（仅直连 + DoH + 非计费 + 非限速 + 官方 CDN 退化时触发，不阻塞主下载路径）。
    pub fn maybe_spawn_probe(
        self: &Arc<Self>,
        engine: &Arc<DownloadEngine>,
        host: &str,
        probe_url: &str,
        observed_bps: f64,
        historical_peak_bps: Option<f64>,
    ) {
        if !probe_url
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
        {
            return;
        }
        let now = Instant::now();
        let key = normalize_host(host);
        let doh_enabled = engine.doh().is_enabled();
        let metered = engine.is_metered();
        let speed_limit = engine.effective_speed_limit_kib();
        let proxied_or_tun = engine.is_proxied_env();
        let sni_blocked = engine.doh().is_sni_blocked(&key);

        let doh_pref = engine.doh().preferred_pop_ip(&key);
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(sel) = st.active_selections.get(&key)
                && doh_pref != Some(sel.selected_ip)
            {
                st.active_selections.remove(&key);
            }
        }

        if !self.should_trigger_probe(
            &key,
            observed_bps,
            historical_peak_bps,
            metered,
            speed_limit,
            proxied_or_tun,
            doh_enabled,
            sni_blocked,
            now,
        ) {
            return;
        }

        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !st.in_flight_hosts.insert(key.clone()) {
                return;
            }
            st.last_probe_at.insert(key.clone(), now);
        }

        let optimizer = self.clone();
        let engine_arc = engine.clone();
        let probe_url_owned = probe_url.to_owned();
        tokio::spawn(async move {
            struct ProbeFlightGuard {
                opt: Arc<OfficialEdgeOptimizer>,
                host: String,
            }
            impl Drop for ProbeFlightGuard {
                fn drop(&mut self) {
                    self.opt
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .in_flight_hosts
                        .remove(&self.host);
                }
            }
            let _guard = ProbeFlightGuard {
                opt: optimizer.clone(),
                host: key.clone(),
            };

            let start_epoch = engine_arc.proxy_watcher().epoch();
            let candidates = engine_arc
                .doh()
                .discover_distinct_pop_ips(&key, MAX_CANDIDATE_SUBNETS)
                .await;
            if candidates.len() < 2 {
                return;
            }
            if engine_arc.is_metered()
                || engine_arc.effective_speed_limit_kib() > 0
                || engine_arc.is_proxied_env()
                || engine_arc.proxy_watcher().epoch() != start_epoch
            {
                return;
            }

            let granted = optimizer.try_reserve_probe_candidates(candidates.len(), Instant::now());
            if granted < 2 {
                return;
            }
            let selected_candidates: Vec<IpAddr> = candidates.into_iter().take(granted).collect();
            let probe_futs = selected_candidates
                .into_iter()
                .map(|ip| probe_single_pop_ip(key.clone(), ip, probe_url_owned.clone()));
            let results: Vec<Option<EdgeProbeSample>> =
                futures_util::future::join_all(probe_futs).await;

            if engine_arc.is_metered()
                || engine_arc.effective_speed_limit_kib() > 0
                || engine_arc.is_proxied_env()
                || engine_arc.proxy_watcher().epoch() != start_epoch
            {
                return;
            }

            let samples: Vec<EdgeProbeSample> = results.into_iter().flatten().collect();
            if samples.is_empty() {
                return;
            }
            let current_ip = engine_arc.doh().current_served_ip(&key);
            let _ = optimizer.apply_probe_results(
                &key,
                current_ip,
                observed_bps,
                &samples,
                Instant::now(),
                engine_arc.doh(),
                engine_arc.telemetry(),
            );
        });
    }

    /// 便捷入口：在单文件完成或窗口更新时兼顾评估已选 POP 回滚与触发新探测。
    pub fn on_window_throughput(
        self: &Arc<Self>,
        engine: &Arc<DownloadEngine>,
        host: &str,
        probe_url: &str,
        observed_bps: f64,
        historical_peak_bps: Option<f64>,
    ) {
        if host.is_empty() || !is_official_cdn_pop_host(host) {
            return;
        }
        let outcome = self.evaluate_active_pop_window(
            host,
            observed_bps,
            Instant::now(),
            engine.doh(),
            engine.telemetry(),
        );
        if matches!(outcome, PopWindowOutcome::NoActiveSelection) {
            self.maybe_spawn_probe(engine, host, probe_url, observed_bps, historical_peak_bps);
        }
    }
}

async fn probe_single_pop_ip(host: String, ip: IpAddr, url: String) -> Option<EdgeProbeSample> {
    let parsed = reqwest::Url::parse(&url).ok()?;
    if !parsed.scheme().eq_ignore_ascii_case("https") {
        return None;
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(PROBE_CONNECT_TIMEOUT)
        .timeout(PROBE_TOTAL_TIMEOUT)
        .resolve(&host, SocketAddr::new(ip, port))
        .build()
        .ok()?;

    let range = format!("bytes=0-{}", PROBE_SLICE_BYTES - 1);
    let started = Instant::now();
    let resp = client
        .get(&url)
        .header(reqwest::header::RANGE, &range)
        .send()
        .await
        .ok()?;
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || !status.is_success() {
        return None;
    }
    if resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| {
            ct.trim_start()
                .to_ascii_lowercase()
                .starts_with("text/html")
        })
    {
        return None;
    }
    let ttfb_ms = started.elapsed().as_secs_f64() * 1000.0;

    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut total_bytes = 0u64;
    let mut sniff = Vec::with_capacity(64);
    while let Some(chunk_res) = stream.next().await {
        let Ok(chunk) = chunk_res else {
            break;
        };
        if sniff.len() < 64 {
            let need = (64 - sniff.len()).min(chunk.len());
            sniff.extend_from_slice(&chunk[..need]);
            if looks_like_html_payload(&sniff) {
                return None;
            }
        }
        total_bytes = total_bytes.saturating_add(chunk.len() as u64);
        if total_bytes >= PROBE_SLICE_BYTES {
            break;
        }
    }
    if total_bytes < 16 * 1024 {
        return None;
    }
    let elapsed_secs = started.elapsed().as_secs_f64().max(0.01);
    let bps = total_bytes as f64 / elapsed_secs;
    Some(EdgeProbeSample {
        ip,
        bps,
        ttfb_ms,
        bytes: total_bytes,
    })
}

/// 非计费/非限速态下的批尾 `<= 2 MiB` 小文件受控对冲协调器（仅用于批尾长尾收尾）。
#[derive(Debug)]
pub struct BatchTailHedger {
    total_files: u64,
    small_latency_ewma_ms: Mutex<Option<f64>>,
    hedges_launched: AtomicU32,
    hedges_won: AtomicU32,
    delay_override: Option<Duration>,
}

impl BatchTailHedger {
    pub fn new(total_files: u64, delay_override: Option<Duration>) -> Self {
        Self {
            total_files,
            small_latency_ewma_ms: Mutex::new(None),
            hedges_launched: AtomicU32::new(0),
            hedges_won: AtomicU32::new(0),
            delay_override,
        }
    }

    pub fn record_small_completion_ms(&self, elapsed_ms: f64) {
        if !elapsed_ms.is_finite() || elapsed_ms <= 0.0 {
            return;
        }
        let clamped = elapsed_ms.clamp(10.0, 10_000.0);
        let mut guard = self
            .small_latency_ewma_ms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(match *guard {
            Some(prev) => 0.25 * clamped + 0.75 * prev,
            None => clamped,
        });
    }

    /// 判断当前小文件在批次里是否启用对冲计时（计费网络、限速或 SNI 阻断严格返回 `None`，0% 冗余流量）。
    pub fn eligible_delay(
        &self,
        metered: bool,
        speed_limit_kib: u32,
        file_size: Option<u64>,
        sni_blocked: bool,
    ) -> Option<Duration> {
        if metered || speed_limit_kib > 0 || sni_blocked {
            return None;
        }
        let size = file_size?;
        if size == 0 || size > TAIL_HEDGE_MAX_FILE_BYTES {
            return None;
        }
        if self.total_files < TAIL_HEDGE_MIN_BATCH_FILES {
            return None;
        }
        if self.hedges_launched.load(Ordering::Acquire) >= TAIL_HEDGE_MAX_PER_BATCH {
            return None;
        }
        if let Some(over) = self.delay_override {
            return Some(over);
        }
        let ewma = self
            .small_latency_ewma_ms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(TAIL_HEDGE_DEFAULT_DELAY_MS / 2.5);
        let delay_ms = (ewma * 2.5).clamp(TAIL_HEDGE_MIN_DELAY_MS, TAIL_HEDGE_MAX_DELAY_MS);
        Some(Duration::from_millis(delay_ms.round() as u64))
    }

    /// 计时器到期时判定当前是否已进入“批尾收尾窗口”（已有 >= 2 文件完成且剩余 <= 4）。
    pub fn is_tail_window(&self, done_files: u64) -> bool {
        if self.total_files < TAIL_HEDGE_MIN_BATCH_FILES || done_files < 2 {
            return false;
        }
        let remaining = self.total_files.saturating_sub(done_files);
        remaining > 0 && remaining <= TAIL_HEDGE_MAX_REMAINING
    }

    pub fn try_claim_hedge_slot(&self) -> bool {
        let mut cur = self.hedges_launched.load(Ordering::Acquire);
        while cur < TAIL_HEDGE_MAX_PER_BATCH {
            match self.hedges_launched.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
        false
    }

    pub fn record_hedge_win(&self) {
        self.hedges_won.fetch_add(1, Ordering::Relaxed);
    }

    pub fn hedges_launched(&self) -> u32 {
        self.hedges_launched.load(Ordering::Acquire)
    }

    pub fn hedges_won(&self) -> u32 {
        self.hedges_won.load(Ordering::Acquire)
    }
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_8mib_hourly_budget_and_distinct_slash24() {
        let opt = OfficialEdgeOptimizer::new();
        let t0 = Instant::now();

        let ips: Vec<IpAddr> = vec![
            "127.0.0.1".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
            "151.101.1.10".parse().unwrap(),
            "151.101.1.25".parse().unwrap(), // 同 /24 去重
            "151.101.65.10".parse().unwrap(),
            "151.101.129.10".parse().unwrap(),
            "151.101.193.10".parse().unwrap(),
        ];
        let deduped = dedupe_by_subnet(&ips, MAX_CANDIDATE_SUBNETS);
        assert_eq!(deduped.len(), 3);
        assert_eq!(deduped[0], "151.101.1.10".parse::<IpAddr>().unwrap());
        assert_eq!(deduped[1], "151.101.65.10".parse::<IpAddr>().unwrap());
        assert_eq!(deduped[2], "151.101.129.10".parse::<IpAddr>().unwrap());

        assert_eq!(
            opt.remaining_hourly_budget_bytes(t0),
            HOURLY_PROBE_BUDGET_BYTES
        );
        // 8 MiB / 256 KiB = 32 个探测名额
        let mut total_granted = 0usize;
        for _ in 0..10 {
            total_granted += opt.try_reserve_probe_candidates(3, t0);
        }
        assert_eq!(total_granted, 30);
        assert_eq!(opt.try_reserve_probe_candidates(3, t0), 2);
        assert_eq!(opt.try_reserve_probe_candidates(1, t0), 0);
        assert_eq!(opt.used_hourly_budget_bytes(t0), HOURLY_PROBE_BUDGET_BYTES);
        assert_eq!(opt.remaining_hourly_budget_bytes(t0), 0);

        // 超过 1 小时滑动窗口后预算恢复
        let t_later = t0 + Duration::from_secs(3601);
        assert_eq!(
            opt.remaining_hourly_budget_bytes(t_later),
            HOURLY_PROBE_BUDGET_BYTES
        );
        assert_eq!(opt.try_reserve_probe_candidates(2, t_later), 2);
    }

    #[test]
    fn zero_probes_when_metered_throttled_or_proxied() {
        let opt = OfficialEdgeOptimizer::new();
        let now = Instant::now();
        let slow_bps = 1.2 * 1024.0 * 1024.0;

        assert!(opt.should_trigger_probe(
            "cdn.modrinth.com",
            slow_bps,
            None,
            false,
            0,
            false,
            true,
            false,
            now
        ));
        assert!(!opt.should_trigger_probe(
            "cdn.modrinth.com",
            slow_bps,
            None,
            true, // metered
            0,
            false,
            true,
            false,
            now
        ));
        assert!(!opt.should_trigger_probe(
            "cdn.modrinth.com",
            slow_bps,
            None,
            false,
            2048, // speed limit
            false,
            true,
            false,
            now
        ));
        assert!(!opt.should_trigger_probe(
            "cdn.modrinth.com",
            slow_bps,
            None,
            false,
            0,
            true, // proxied or TUN
            true,
            false,
            now
        ));
        assert!(!opt.should_trigger_probe(
            "cdn.modrinth.com",
            slow_bps,
            None,
            false,
            0,
            false,
            false, // DoH disabled
            false,
            now
        ));
        assert!(!opt.should_trigger_probe(
            "login.microsoftonline.com", // 绝非 CDN 探测域
            slow_bps,
            None,
            false,
            0,
            false,
            true,
            false,
            now
        ));
        assert_eq!(opt.used_hourly_budget_bytes(now), 0);
    }

    #[test]
    fn promotes_faster_pop_and_rolls_back_after_two_slow_windows() {
        let opt = OfficialEdgeOptimizer::new();
        let doh = Doh::new(true).unwrap();
        let telemetry = TelemetryHub::new();
        let t0 = Instant::now();

        let old_ip: IpAddr = "151.101.1.10".parse().unwrap();
        let new_ip: IpAddr = "151.101.65.10".parse().unwrap();
        let baseline_bps = 2.0 * 1024.0 * 1024.0; // 2 MiB/s

        let promoted = opt.apply_probe_results(
            "cdn.modrinth.com",
            Some(old_ip),
            baseline_bps,
            &[
                EdgeProbeSample {
                    ip: old_ip,
                    bps: baseline_bps,
                    ttfb_ms: 80.0,
                    bytes: PROBE_SLICE_BYTES,
                },
                EdgeProbeSample {
                    ip: new_ip,
                    bps: 6.0 * 1024.0 * 1024.0,
                    ttfb_ms: 35.0,
                    bytes: PROBE_SLICE_BYTES,
                },
            ],
            t0,
            &doh,
            &telemetry,
        );
        assert_eq!(promoted, Some(new_ip));
        assert_eq!(doh.preferred_pop_ip("cdn.modrinth.com"), Some(new_ip));
        assert!(opt.has_active_selection("cdn.modrinth.com"));

        // 窗口低于 80%（1.4 < 1.6 MiB/s）一次：只记 Underperforming，不立即回滚
        assert_eq!(
            opt.evaluate_active_pop_window(
                "cdn.modrinth.com",
                1.4 * 1024.0 * 1024.0,
                t0 + Duration::from_secs(2),
                &doh,
                &telemetry,
            ),
            PopWindowOutcome::Underperforming
        );
        assert_eq!(doh.preferred_pop_ip("cdn.modrinth.com"), Some(new_ip));

        // 窗口恢复到 >= 80%：连续慢窗口计数清零
        assert_eq!(
            opt.evaluate_active_pop_window(
                "cdn.modrinth.com",
                3.0 * 1024.0 * 1024.0,
                t0 + Duration::from_secs(4),
                &doh,
                &telemetry,
            ),
            PopWindowOutcome::Healthy
        );

        // 再次连续两个窗口 < 80%：触发自动回滚并降级 new_ip，发射 RampRollback 事件
        assert_eq!(
            opt.evaluate_active_pop_window(
                "cdn.modrinth.com",
                1.2 * 1024.0 * 1024.0,
                t0 + Duration::from_secs(6),
                &doh,
                &telemetry,
            ),
            PopWindowOutcome::Underperforming
        );
        assert_eq!(
            opt.evaluate_active_pop_window(
                "cdn.modrinth.com",
                1.1 * 1024.0 * 1024.0,
                t0 + Duration::from_secs(8),
                &doh,
                &telemetry,
            ),
            PopWindowOutcome::RolledBack
        );
        assert_eq!(doh.preferred_pop_ip("cdn.modrinth.com"), None);
        assert!(!opt.has_active_selection("cdn.modrinth.com"));

        let snap = telemetry.snapshot(
            super::super::telemetry::ActiveRouteKind::Direct,
            "直连".to_owned(),
            1,
            64,
            super::super::throttle::DiskMediaKind::Ssd,
            12,
            3.0,
            false,
            false,
            0,
            None,
        );
        assert!(
            snap.recent_events
                .iter()
                .any(|e| e.kind == TelemetryEventKind::RouteSwitched)
        );
        assert!(
            snap.recent_events
                .iter()
                .any(|e| e.kind == TelemetryEventKind::RampRollback)
        );
    }

    #[test]
    fn batch_tail_hedger_bounds_size_window_and_metered() {
        let hedger = BatchTailHedger::new(6, None);
        hedger.record_small_completion_ms(200.0);
        // 文件过大 (> 2 MiB) 禁止
        assert_eq!(
            hedger.eligible_delay(false, 0, Some(3 * 1024 * 1024), false),
            None
        );
        // 尚在前半批 (done=1, rem=5) 不可触发批尾窗口
        assert!(!hedger.is_tail_window(1));
        assert!(hedger.is_tail_window(4));
        // 计费 / 限速 / SNI 阻断禁止
        assert_eq!(
            hedger.eligible_delay(true, 0, Some(100 * 1024), false),
            None
        );
        assert_eq!(
            hedger.eligible_delay(false, 1024, Some(100 * 1024), false),
            None
        );
        assert_eq!(
            hedger.eligible_delay(false, 0, Some(100 * 1024), true),
            None
        );
        // 批尾正常命中且受 800ms..=3000ms 钳位
        let delay = hedger
            .eligible_delay(false, 0, Some(100 * 1024), false)
            .unwrap();
        assert_eq!(delay, Duration::from_millis(800));
        for _ in 0..TAIL_HEDGE_MAX_PER_BATCH {
            assert!(hedger.try_claim_hedge_slot());
        }
        assert!(!hedger.try_claim_hedge_slot());
        assert_eq!(
            hedger.eligible_delay(false, 0, Some(100 * 1024), false),
            None
        );
    }

    #[test]
    fn stale_active_selection_clears_after_ttl_or_doh_eviction() {
        let opt = OfficialEdgeOptimizer::new();
        let doh = Doh::new(true).unwrap();
        let telemetry = TelemetryHub::new();
        let t0 = Instant::now();

        let old_ip: IpAddr = "151.101.1.10".parse().unwrap();
        let new_ip: IpAddr = "151.101.65.10".parse().unwrap();
        let promoted = opt.apply_probe_results(
            "cdn.modrinth.com",
            Some(old_ip),
            1.5 * 1024.0 * 1024.0,
            &[EdgeProbeSample {
                ip: new_ip,
                bps: 5.0 * 1024.0 * 1024.0,
                ttfb_ms: 40.0,
                bytes: PROBE_SLICE_BYTES,
            }],
            t0,
            &doh,
            &telemetry,
        );
        assert_eq!(promoted, Some(new_ip));
        assert!(opt.has_active_selection("cdn.modrinth.com"));

        // 1) 当 Doh 建连失败将优选 IP 撤回后，评估窗口立即清理 stale selection
        doh.clear_preferred_pop_ip("cdn.modrinth.com", None);
        assert_eq!(
            opt.evaluate_active_pop_window(
                "cdn.modrinth.com",
                1.0 * 1024.0 * 1024.0,
                t0 + Duration::from_secs(2),
                &doh,
                &telemetry,
            ),
            PopWindowOutcome::NoActiveSelection
        );
        assert!(!opt.has_active_selection("cdn.modrinth.com"));

        // 2) 重新优选后若超过 PREFERRED_POP_TTL (20min)，也自动清理并放开后续探测
        let t1 = t0 + Duration::from_secs(400);
        let promoted2 = opt.apply_probe_results(
            "cdn.modrinth.com",
            Some(old_ip),
            1.5 * 1024.0 * 1024.0,
            &[EdgeProbeSample {
                ip: new_ip,
                bps: 5.0 * 1024.0 * 1024.0,
                ttfb_ms: 40.0,
                bytes: PROBE_SLICE_BYTES,
            }],
            t1,
            &doh,
            &telemetry,
        );
        assert_eq!(promoted2, Some(new_ip));
        let t_expired = t1 + PREFERRED_POP_TTL + Duration::from_secs(1);
        assert_eq!(
            opt.evaluate_active_pop_window(
                "cdn.modrinth.com",
                1.0 * 1024.0 * 1024.0,
                t_expired,
                &doh,
                &telemetry,
            ),
            PopWindowOutcome::NoActiveSelection
        );
        assert!(!opt.has_active_selection("cdn.modrinth.com"));
        assert!(opt.should_trigger_probe(
            "cdn.modrinth.com",
            1.0 * 1024.0 * 1024.0,
            None,
            false,
            0,
            false,
            true,
            false,
            t_expired,
        ));
    }
}
