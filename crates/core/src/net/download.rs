//! 下载引擎：全局优先级公平信号量、内存快车道与溢出写盘、活体 SHA-1、早截断结构验尸与自愈重试。
//!
//! - 全局单例 [`DownloadEngine`]：按 [`DownloadPriority`] 保留至少 25%（≥4）快速通道槽位给交互任务；
//! - 磁盘写作与改名：`<= 1 MiB` 走 64 MiB 内存快车道算完哈希一次落盘（超限无缝溢出到 `.part`），大文件
//!   使用 `256 KiB BufWriter` 并联算活体 SHA-1（磁盘总 I/O 直降 50%），改名/删除走 6 步 Defender 退避；
//! - 安全防护：HTTP 302 逐跳自管（禁 HTTPS→HTTP 降级、不跨源泄露 `HostHeader`）、HTML 劫持首包截断、
//!   无强哈希文件执行 ZIP EOCD / JSON / XML 微秒级结构验尸、429 `Retry-After` 退避、停滞看门狗退避、
//!   以及向 [`Doh`] 回传传输握手结果触发官方互备 CDN 秒切。

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use reqwest::{Client, StatusCode, header};
use sha1::{Digest, Sha1};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::sync::{Mutex as AsyncMutex, Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::config::{
    ConcurrencyMode, DiskWriteMode, DownloadSettings, MeteredPolicy, ProxyMode, ProxySettings,
};
use crate::fs::{remove_with_win_retry, rename_with_win_retry};
use crate::jobs::PauseControl;
use crate::redact::Secret;

use super::diagnosis::{
    FailureDiagnosis, RootCauseKind, classify_io_error, classify_reqwest_error,
};
use super::doh::{Doh, TransportDnsAction};
use super::edge_opt::{BatchTailHedger, OfficialEdgeOptimizer, TAIL_HEDGE_MAX_FILE_BYTES};
use super::http::{
    ClientBundle, HttpError, RedirectSendError, expand_official_fallbacks,
    send_following_safe_redirects_with_if_range,
};
use super::hydrator::LocalHydrator;
use super::metrics::{
    ActiveRouteKind, TransferMetricEventKind, TransferMetrics, TransferMetricsHub,
};
use super::profile::NetworkProfileStore;
use super::proxy_watch::ProxyWatcher;
use super::proxy_watch::SystemProxySnapshot;
use super::segmented::{
    self, SegmentedOutcome, can_segment, has_resumable_artifacts, header_str, parse_content_range,
};
use super::throttle::{DiskMediaDetector, DiskMediaKind, TokenBucket};
use super::{to_hex, url_host};

const ATTEMPTS_PER_URL: u32 = 2;
/// 带了凭据还被拒绝（401 / 403）时失败原因的开头，上层据此提示「API Key 无效或被撤销」
pub const DENIED_PREFIX: &str = "下载源拒绝了启动器的凭据";
pub(crate) const SIZE_MISMATCH: &str = "大小不对：";
pub(crate) const SHA1_MISMATCH: &str = "SHA-1 校验失败：";
pub(crate) const STRUCTURE_MISMATCH: &str = "文件结构异常：";
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

const SMALL_FILE_THRESHOLD: usize = 1024 * 1024; // 1 MiB
const WRITE_BUF_BYTES: usize = 256 * 1024; // 256 KiB
const PROGRESS_TICK_MS: u64 = 100; // 10 Hz 进度合并节流
const DEFAULT_NET_SLOTS: usize = 64;
const BATCH_NET_SLOTS: usize = 48; // 75% 配额，保证 ≥16 槽给 Interactive
const DISK_PERMIT_POOL: u32 = 12;
const MEM_BUDGET_MIB: usize = 64;
const STALL_TIMEOUTS_SECS: &[u64] = &[10, 15, 20];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPriority {
    Interactive,
    Batch,
}

/// 全局下载引擎单例：统一托管网络/磁盘/内存资源预算、令牌桶限速、遥测、指纹记忆、本地水合与三池 HTTP 客户端。
pub struct DownloadEngine {
    bundle: RwLock<ClientBundle>,
    user_proxy: RwLock<ProxySettings>,
    runtime_cfg: RwLock<DownloadSettings>,
    doh: Arc<Doh>,
    proxy_watcher: Arc<ProxyWatcher>,
    net_total: RwLock<Arc<Semaphore>>,
    net_batch: RwLock<Arc<Semaphore>>,
    disk_pool: RwLock<(Arc<Semaphore>, u32)>,
    max_net_permits: AtomicU32,
    disk_ewma_ms: Mutex<f64>,
    last_backpressure_event: Mutex<Option<Instant>>,
    mem_budget: Arc<Semaphore>,
    path_locks: Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>,
    throttle: Arc<TokenBucket>,
    metrics: Arc<TransferMetricsHub>,
    disk_detector: Arc<DiskMediaDetector>,
    profiles: Arc<NetworkProfileStore>,
    hydrator: Arc<LocalHydrator>,
    edge_optimizer: Arc<OfficialEdgeOptimizer>,
    game_root: RwLock<PathBuf>,
    metered: AtomicBool,
    h2_shard_rr: AtomicUsize,
    tail_hedges_launched: AtomicU32,
    tail_hedges_won: AtomicU32,
    #[cfg(test)]
    tail_hedge_delay_override: Mutex<Option<Duration>>,
}

impl DownloadEngine {
    pub fn new(
        doh: Arc<Doh>,
        proxy_watcher: Arc<ProxyWatcher>,
        proxy: &ProxySettings,
    ) -> Result<Self, HttpError> {
        let snap = proxy_watcher.snapshot();
        let bundle = ClientBundle::build(doh.clone(), proxy, Some(&snap))?;
        let cfg = DownloadSettings {
            proxy: proxy.clone(),
            ..DownloadSettings::default()
        };
        Ok(Self {
            bundle: RwLock::new(bundle),
            user_proxy: RwLock::new(proxy.clone()),
            runtime_cfg: RwLock::new(cfg),
            doh,
            proxy_watcher,
            net_total: RwLock::new(Arc::new(Semaphore::new(DEFAULT_NET_SLOTS))),
            net_batch: RwLock::new(Arc::new(Semaphore::new(BATCH_NET_SLOTS))),
            disk_pool: RwLock::new((
                Arc::new(Semaphore::new(
                    usize::try_from(DISK_PERMIT_POOL).unwrap_or(12),
                )),
                DISK_PERMIT_POOL,
            )),
            max_net_permits: AtomicU32::new(DEFAULT_NET_SLOTS as u32),
            disk_ewma_ms: Mutex::new(5.0),
            last_backpressure_event: Mutex::new(None),
            mem_budget: Arc::new(Semaphore::new(MEM_BUDGET_MIB)),
            path_locks: Mutex::new(HashMap::new()),
            throttle: Arc::new(TokenBucket::new(0)),
            metrics: Arc::new(TransferMetricsHub::new()),
            disk_detector: Arc::new(DiskMediaDetector::new()),
            profiles: Arc::new(NetworkProfileStore::in_memory()),
            hydrator: Arc::new(LocalHydrator::in_memory(std::env::temp_dir())),
            edge_optimizer: Arc::new(OfficialEdgeOptimizer::new()),
            game_root: RwLock::new(std::env::temp_dir()),
            metered: AtomicBool::new(false),
            h2_shard_rr: AtomicUsize::new(0),
            tail_hedges_launched: AtomicU32::new(0),
            tail_hedges_won: AtomicU32::new(0),
            #[cfg(test)]
            tail_hedge_delay_override: Mutex::new(None),
        })
    }

    pub fn new_with_paths(
        doh: Arc<Doh>,
        proxy_watcher: Arc<ProxyWatcher>,
        proxy: &ProxySettings,
        data_dir: PathBuf,
        game_root: PathBuf,
    ) -> Result<Self, HttpError> {
        let snap = proxy_watcher.snapshot();
        let bundle = ClientBundle::build(doh.clone(), proxy, Some(&snap))?;
        let cfg = DownloadSettings {
            proxy: proxy.clone(),
            ..DownloadSettings::default()
        };
        Ok(Self {
            bundle: RwLock::new(bundle),
            user_proxy: RwLock::new(proxy.clone()),
            runtime_cfg: RwLock::new(cfg),
            doh,
            proxy_watcher,
            net_total: RwLock::new(Arc::new(Semaphore::new(DEFAULT_NET_SLOTS))),
            net_batch: RwLock::new(Arc::new(Semaphore::new(BATCH_NET_SLOTS))),
            disk_pool: RwLock::new((
                Arc::new(Semaphore::new(
                    usize::try_from(DISK_PERMIT_POOL).unwrap_or(12),
                )),
                DISK_PERMIT_POOL,
            )),
            max_net_permits: AtomicU32::new(DEFAULT_NET_SLOTS as u32),
            disk_ewma_ms: Mutex::new(5.0),
            last_backpressure_event: Mutex::new(None),
            mem_budget: Arc::new(Semaphore::new(MEM_BUDGET_MIB)),
            path_locks: Mutex::new(HashMap::new()),
            throttle: Arc::new(TokenBucket::new(0)),
            metrics: Arc::new(TransferMetricsHub::new()),
            disk_detector: Arc::new(DiskMediaDetector::new()),
            profiles: Arc::new(NetworkProfileStore::open(
                data_dir.join("network-profiles.json"),
            )),
            hydrator: Arc::new(LocalHydrator::new(game_root.clone(), Some(data_dir))),
            edge_optimizer: Arc::new(OfficialEdgeOptimizer::new()),
            game_root: RwLock::new(game_root),
            metered: AtomicBool::new(false),
            h2_shard_rr: AtomicUsize::new(0),
            tail_hedges_launched: AtomicU32::new(0),
            tail_hedges_won: AtomicU32::new(0),
            #[cfg(test)]
            tail_hedge_delay_override: Mutex::new(None),
        })
    }

    /// 热应用设置页更改的下载/限速/并发/分段/写入/DoH 参数。
    pub fn apply_settings(&self, cfg: &DownloadSettings) {
        self.doh.set_enabled(cfg.doh_enabled);
        self.doh.set_custom_url(cfg.custom_doh_url.clone());

        let new_net = match cfg.concurrency_mode {
            ConcurrencyMode::Auto => DEFAULT_NET_SLOTS as u32,
            ConcurrencyMode::Manual => cfg.max_concurrency.clamp(1, 128),
        };
        if self.max_net_permits.swap(new_net, Ordering::AcqRel) != new_net {
            let total = usize::try_from(new_net).unwrap_or(DEFAULT_NET_SLOTS).max(1);
            let reserved = total.min(4).max(total / 4);
            let batch = total.saturating_sub(reserved).max(1);
            *self.net_total.write().unwrap_or_else(|e| e.into_inner()) =
                Arc::new(Semaphore::new(total));
            *self.net_batch.write().unwrap_or_else(|e| e.into_inner()) =
                Arc::new(Semaphore::new(batch));
        }

        let disk_kind = self.disk_kind_for_game_root();
        let new_disk = match cfg.disk_write_mode {
            DiskWriteMode::Auto => match disk_kind {
                DiskMediaKind::Ssd => 12,
                DiskMediaKind::Hdd => 4,
                DiskMediaKind::Unknown => 10,
            },
            DiskWriteMode::Manual => u32::from(cfg.max_disk_writers.clamp(1, 32)),
        };
        {
            let mut pool = self.disk_pool.write().unwrap_or_else(|e| e.into_inner());
            if pool.1 != new_disk {
                *pool = (
                    Arc::new(Semaphore::new(
                        usize::try_from(new_disk).unwrap_or(12).max(1),
                    )),
                    new_disk,
                );
            }
        }

        *self.runtime_cfg.write().unwrap_or_else(|e| e.into_inner()) = cfg.clone();
        self.sync_effective_speed_limit();
    }

    pub fn set_metered(&self, metered: bool) {
        let changed = self.metered.swap(metered, Ordering::AcqRel) != metered;
        self.doh.set_metered(metered);
        if changed {
            self.sync_effective_speed_limit();
        }
    }

    pub fn is_metered(&self) -> bool {
        self.metered.load(Ordering::Acquire)
    }

    pub fn set_game_root(&self, game_root: PathBuf, external_roots: Vec<PathBuf>) {
        *self.game_root.write().unwrap_or_else(|e| e.into_inner()) = game_root.clone();
        self.hydrator.set_roots(game_root, external_roots);
        let cfg = self.settings_snapshot();
        self.apply_settings(&cfg);
    }

    pub fn set_external_roots(&self, external_roots: Vec<PathBuf>) {
        self.hydrator.set_external_roots(external_roots);
    }

    pub fn settings_snapshot(&self) -> DownloadSettings {
        self.runtime_cfg
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn effective_speed_limit_kib(&self) -> u32 {
        let cfg = self.runtime_cfg.read().unwrap_or_else(|e| e.into_inner());
        let user = cfg.speed_limit_kib_per_sec;
        if self.is_metered() && cfg.metered_policy == MeteredPolicy::Throttle {
            let metered_lim = cfg.metered_speed_limit_kib_per_sec.max(128);
            if user == 0 {
                metered_lim
            } else {
                user.min(metered_lim)
            }
        } else {
            user
        }
    }

    fn sync_effective_speed_limit(&self) {
        self.throttle
            .set_limit_kib(self.effective_speed_limit_kib());
    }

    pub fn local_reuse_enabled(&self) -> bool {
        self.runtime_cfg
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .local_reuse_enabled
    }

    pub fn throttle(&self) -> &Arc<TokenBucket> {
        &self.throttle
    }

    pub fn metrics(&self) -> &Arc<TransferMetricsHub> {
        &self.metrics
    }

    pub fn profiles(&self) -> &Arc<NetworkProfileStore> {
        &self.profiles
    }

    pub fn hydrator(&self) -> &Arc<LocalHydrator> {
        &self.hydrator
    }

    pub fn edge_optimizer(&self) -> &Arc<OfficialEdgeOptimizer> {
        &self.edge_optimizer
    }

    pub fn tail_hedges_launched(&self) -> u32 {
        self.tail_hedges_launched.load(Ordering::Acquire)
    }

    pub fn tail_hedges_won(&self) -> u32 {
        self.tail_hedges_won.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn set_tail_hedge_delay_override(&self, delay: Option<Duration>) {
        *self
            .tail_hedge_delay_override
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = delay;
    }

    pub fn max_net_permits(&self) -> u32 {
        self.max_net_permits.load(Ordering::Acquire)
    }

    pub(crate) fn net_total_sem(&self) -> Arc<Semaphore> {
        self.net_total
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn net_batch_sem(&self) -> Arc<Semaphore> {
        self.net_batch
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_proxied_env(&self) -> bool {
        let proxy = self
            .user_proxy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.proxy_watcher.is_proxied_env(&proxy, &self.doh)
    }

    pub fn disk_kind_for_path(&self, path: &Path) -> DiskMediaKind {
        self.disk_detector.detect(path)
    }

    pub fn disk_kind_for_game_root(&self) -> DiskMediaKind {
        let root = self
            .game_root
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.disk_detector.detect(&root)
    }

    pub fn disk_ewma_ms(&self) -> f64 {
        *self.disk_ewma_ms.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn runtime_metrics(&self) -> TransferMetrics {
        let proxy = self
            .user_proxy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let snap = self.proxy_watcher.snapshot();
        let tun_active = self.doh.is_tun_active();
        let (route_kind, route_label) = match proxy.mode {
            ProxyMode::Direct => {
                if tun_active {
                    (ActiveRouteKind::TunFakeIp, "TUN 代理接管".to_owned())
                } else {
                    (ActiveRouteKind::Direct, "直连".to_owned())
                }
            }
            ProxyMode::Custom => (
                ActiveRouteKind::CustomProxy,
                format!("自定义代理 :{}", proxy.port),
            ),
            ProxyMode::System => {
                if snap.enabled {
                    (ActiveRouteKind::SystemProxy, "系统代理".to_owned())
                } else if tun_active {
                    (ActiveRouteKind::TunFakeIp, "TUN 代理接管".to_owned())
                } else if snap.has_pac_only() {
                    (
                        ActiveRouteKind::Direct,
                        "直连（检测到系统 PAC，暂不支持自动脚本）".to_owned(),
                    )
                } else {
                    (ActiveRouteKind::Direct, "直连".to_owned())
                }
            }
        };

        let metered = self.is_metered();
        let cfg = self.settings_snapshot();
        let metered_throttled = metered && cfg.metered_policy == MeteredPolicy::Throttle;
        let limit_kib = self.effective_speed_limit_kib();
        let disk_kind = self.disk_kind_for_game_root();
        let disk_limit = self.disk_pool.read().unwrap_or_else(|e| e.into_inner()).1;
        let disk_ewma = self.disk_ewma_ms();
        let epoch = u32::try_from(self.proxy_watcher.epoch()).unwrap_or(u32::MAX);

        self.metrics.snapshot(
            route_kind,
            route_label,
            epoch,
            self.max_net_permits(),
            disk_kind,
            disk_limit,
            disk_ewma,
            metered,
            metered_throttled,
            limit_kib,
            self.doh.best_provider_summary(),
        )
    }

    /// 代理设置或系统网络代际变化时热重建全部三池客户端，返回兼容主 `Client`。
    pub fn rebuild_clients(
        &self,
        proxy: &ProxySettings,
        sys_snap: Option<&SystemProxySnapshot>,
    ) -> Result<Client, HttpError> {
        let bundle = ClientBundle::build(self.doh.clone(), proxy, sys_snap)?;
        let h2 = bundle.h2_small.clone();
        *self.user_proxy.write().unwrap_or_else(|e| e.into_inner()) = proxy.clone();
        *self.bundle.write().unwrap_or_else(|e| e.into_inner()) = bundle;
        self.edge_optimizer.on_network_epoch_change();
        Ok(h2)
    }

    pub fn proxy_mode(&self) -> crate::config::ProxyMode {
        self.user_proxy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .mode
    }

    /// 建连失败时 0ms 现场复查一次系统代理（不等后台轮询），若代理刚开/刚关立刻热重建三池。
    pub fn refresh_proxy_if_changed(&self) -> Option<Client> {
        let proxy = self
            .user_proxy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let (epoch, snap, woke) = self.proxy_watcher.poll(&proxy)?;
        tracing::info!(
            epoch,
            woke,
            sys_enabled = snap.enabled,
            "建连失败触发即时代理复查：检测到网络变更，立即热重建连接池"
        );
        self.metrics.push_event(
            TransferMetricEventKind::RouteSwitched,
            "检测到系统代理/网络变化，已热切换连接池",
        );
        let _ = self.doh.sync_proxy_with_snapshot(&proxy, Some(&snap));
        self.doh.on_network_epoch_change();
        self.rebuild_clients(&proxy, Some(&snap)).ok()
    }

    pub fn h2_client(&self) -> Client {
        self.bundle
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .h2_small
            .clone()
    }

    pub fn next_h2_shard(&self) -> usize {
        self.h2_shard_rr.fetch_add(1, Ordering::Relaxed)
    }

    pub fn bundle(&self) -> ClientBundle {
        self.bundle
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn doh(&self) -> &Arc<Doh> {
        &self.doh
    }

    pub fn proxy_watcher(&self) -> &Arc<ProxyWatcher> {
        &self.proxy_watcher
    }

    pub(crate) async fn acquire_net(
        &self,
        priority: DownloadPriority,
        cancel: &CancellationToken,
    ) -> Option<NetPermit> {
        let batch_sem = self.net_batch_sem();
        let total_sem = self.net_total_sem();
        let batch_permit = if priority == DownloadPriority::Batch {
            tokio::select! {
                res = batch_sem.acquire_owned() => Some(res.ok()?),
                () = cancel.cancelled() => return None,
            }
        } else {
            None
        };
        let total_permit = tokio::select! {
            res = total_sem.acquire_owned() => res.ok()?,
            () = cancel.cancelled() => return None,
        };
        Some(NetPermit {
            _batch: batch_permit,
            _total: total_permit,
        })
    }

    pub(crate) fn try_acquire_net(&self, priority: DownloadPriority) -> Option<NetPermit> {
        let batch_permit = if priority == DownloadPriority::Batch {
            Some(self.net_batch_sem().try_acquire_owned().ok()?)
        } else {
            None
        };
        let total_permit = self.net_total_sem().try_acquire_owned().ok()?;
        Some(NetPermit {
            _batch: batch_permit,
            _total: total_permit,
        })
    }

    pub(crate) async fn acquire_disk(
        &self,
        cancel: &CancellationToken,
    ) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let ewma = *self.disk_ewma_ms.lock().unwrap_or_else(|e| e.into_inner());
        let (disk_sem, limit) = {
            let pool = self.disk_pool.read().unwrap_or_else(|e| e.into_inner());
            (pool.0.clone(), pool.1.max(1))
        };
        let slow_target = 2u32.min(limit);
        let cost = if ewma >= 50.0 {
            (limit / slow_target).clamp(1, limit)
        } else {
            1
        };
        tokio::select! {
            res = disk_sem.acquire_many_owned(cost) => res.ok(),
            () = cancel.cancelled() => None,
        }
    }

    pub(crate) fn record_disk_write_ms(&self, elapsed_ms: f64, bytes: u64) {
        // 大文件合并/哈希按每 MiB 归一化，避免 SSD 150ms 合并 50 MiB 被误判为慢盘卡顿
        let effective_ms = if bytes > 1_048_576 {
            elapsed_ms / (bytes as f64 / 1_048_576.0)
        } else {
            elapsed_ms
        };
        let mut ewma = self.disk_ewma_ms.lock().unwrap_or_else(|e| e.into_inner());
        let next = 0.7 * *ewma + 0.3 * effective_ms.clamp(0.1, 2000.0);
        let crossed = *ewma < 50.0 && next >= 50.0;
        *ewma = next;
        drop(ewma);
        if crossed {
            let mut last = self
                .last_backpressure_event
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let should_emit = last.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
            if should_emit {
                *last = Some(Instant::now());
                drop(last);
                self.metrics.push_event(
                    TransferMetricEventKind::DiskBackpressure,
                    "磁盘写入延迟升高，自动降档并发保护流畅度",
                );
            }
        }
    }

    pub(crate) fn lock_for_path(&self, target: &Path) -> Arc<AsyncMutex<()>> {
        let normalized = canonical_or_normalized(target);
        let mut locks = self.path_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.retain(|_, weak| weak.strong_count() > 0);
        let slot = locks.entry(normalized).or_default();
        if let Some(existing) = slot.upgrade() {
            return existing;
        }
        let fresh = Arc::new(AsyncMutex::new(()));
        *slot = Arc::downgrade(&fresh);
        fresh
    }
}

pub(crate) struct NetPermit {
    _batch: Option<tokio::sync::OwnedSemaphorePermit>,
    _total: tokio::sync::OwnedSemaphorePermit,
}

impl NetPermit {
    pub(crate) fn new(
        batch: Option<tokio::sync::OwnedSemaphorePermit>,
        total: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            _batch: batch,
            _total: total,
        }
    }
}

fn canonical_or_normalized(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) => {
            let base = fs_canonicalize_sync(parent).unwrap_or_else(|| parent.to_path_buf());
            match path.file_name() {
                Some(name) => base.join(name),
                None => base,
            }
        }
        None => path.to_path_buf(),
    }
}

fn fs_canonicalize_sync(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// 一个要下载的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadItem {
    /// 候选地址，按尝试顺序排列
    pub urls: Vec<String>,
    /// 保存位置（调用方负责保证它在允许的目录里）
    pub path: PathBuf,
    /// 期望的 SHA-1（十六进制）。没有时只校验大小与结构
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub done_files: u64,
    pub total_files: u64,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub received_bytes: u64,
    pub fallbacks: u64,
    pub reused_bytes: u64,
    pub reused_files: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadReport {
    pub downloaded: u64,
    pub skipped: u64,
    pub bytes_received: u64,
    pub reused_bytes: u64,
    pub reused_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFailure {
    pub path: PathBuf,
    pub reason: String,
    pub item: Option<DownloadItem>,
    pub cause: RootCauseKind,
    pub diagnosis: Option<FailureDiagnosis>,
    pub attempts: u32,
    pub last_http_status: Option<u16>,
    pub has_resumable_part: bool,
}

impl FileFailure {
    pub fn simple(path: PathBuf, reason: String) -> Self {
        Self {
            path,
            reason,
            item: None,
            cause: RootCauseKind::Unknown,
            diagnosis: None,
            attempts: 1,
            last_http_status: None,
            has_resumable_part: false,
        }
    }

    pub fn is_integrity_mismatch(&self) -> bool {
        self.reason.starts_with(SIZE_MISMATCH)
            || self.reason.starts_with(SHA1_MISMATCH)
            || self.reason.starts_with(STRUCTURE_MISMATCH)
    }
}

/// 后端部分失败回执条目（保留 `DownloadItem` 仅存后端内存，绝不直接序列化路径给前端）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedDownloadEntry {
    pub item: DownloadItem,
    pub cause: RootCauseKind,
    pub reason: String,
    pub attempts: u32,
    pub last_http_status: Option<u16>,
    pub has_resumable_part: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialFailureReceipt {
    pub job_id: String,
    pub step_label: String,
    pub created_at_ms: u64,
    pub total_files: u64,
    pub succeeded_files: u64,
    pub reused_files: u64,
    pub failed_entries: Vec<FailedDownloadEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("下载已取消")]
    Cancelled,
    #[error("有 {} 个文件下载失败", .0.len())]
    Failed(Vec<FileFailure>),
}

enum FileResult {
    Downloaded,
    Skipped,
    Failed(Box<FileFailure>),
    Cancelled,
}

enum AttemptError {
    Retry {
        reason: String,
        delay_override: Option<Duration>,
        use_spare_next: bool,
        stall_bump: bool,
        connect_failed: bool,
        diagnosis: Option<FailureDiagnosis>,
        http_status: Option<u16>,
    },
    NextUrl {
        reason: String,
        diagnosis: Option<FailureDiagnosis>,
        http_status: Option<u16>,
    },
    Paused,
    Cancelled,
}

impl AttemptError {
    fn retry(reason: impl Into<String>) -> Self {
        Self::Retry {
            reason: reason.into(),
            delay_override: None,
            use_spare_next: false,
            stall_bump: false,
            connect_failed: false,
            diagnosis: None,
            http_status: None,
        }
    }

    fn next_url(reason: impl Into<String>) -> Self {
        Self::NextUrl {
            reason: reason.into(),
            diagnosis: None,
            http_status: None,
        }
    }
}

pub(crate) struct Counters {
    pub(crate) total_files: u64,
    pub(crate) total_bytes: u64,
    pub(crate) done_files: AtomicU64,
    pub(crate) done_bytes: AtomicU64,
    pub(crate) received_bytes: AtomicU64,
    pub(crate) live_received: AtomicU64,
    pub(crate) fallbacks: AtomicU64,
    pub(crate) reused_bytes: AtomicU64,
    pub(crate) reused_files: AtomicU64,
    pub(crate) dirty: AtomicBool,
}

impl Counters {
    pub(crate) fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    fn snapshot(&self) -> DownloadProgress {
        let done_bytes = self.done_bytes.load(Ordering::Relaxed);
        DownloadProgress {
            done_files: self.done_files.load(Ordering::Relaxed),
            total_files: self.total_files,
            done_bytes,
            total_bytes: self.total_bytes.max(done_bytes),
            received_bytes: self.live_received.load(Ordering::Relaxed),
            fallbacks: self.fallbacks.load(Ordering::Relaxed),
            reused_bytes: self.reused_bytes.load(Ordering::Relaxed),
            reused_files: self.reused_files.load(Ordering::Relaxed),
        }
    }
}

pub(crate) struct Credit<'a> {
    pub(crate) counters: &'a Counters,
    bytes: u64,
}

impl Credit<'_> {
    pub(crate) fn add(&mut self, bytes: u64) {
        if bytes > 0 {
            self.counters.done_bytes.fetch_add(bytes, Ordering::Relaxed);
            self.bytes += bytes;
            self.counters.mark_dirty();
        }
    }

    /// 记录由并发分段 worker 直接累加到 `counters.done_bytes` 的字节数，保证回滚零漏算。
    pub(crate) fn account_external(&mut self, bytes: u64) {
        self.bytes = self.bytes.saturating_add(bytes);
    }

    /// 无回弹差值对齐到 `actual`：不清空再加，彻底消除暂停恢复时的进度条回弹与完成态偏差。
    pub(crate) fn sync_to(&mut self, actual: u64) {
        if actual > self.bytes {
            let delta = actual - self.bytes;
            self.counters.done_bytes.fetch_add(delta, Ordering::Relaxed);
            self.bytes = actual;
            self.counters.mark_dirty();
        } else if actual < self.bytes {
            let delta = self.bytes - actual;
            self.counters.done_bytes.fetch_sub(delta, Ordering::Relaxed);
            self.bytes = actual;
            self.counters.mark_dirty();
        }
    }

    pub(crate) fn rollback(&mut self) {
        if self.bytes > 0 {
            self.counters
                .done_bytes
                .fetch_sub(self.bytes, Ordering::Relaxed);
            self.bytes = 0;
            self.counters.mark_dirty();
        }
    }
}

/// 只发给指定域名的请求头（例如 CurseForge 的 `x-api-key`）。
#[derive(Clone)]
pub struct HostHeader {
    pub hosts: &'static [&'static str],
    pub name: &'static str,
    pub value: Secret<String>,
}

impl HostHeader {
    pub(crate) fn applies_to(&self, url: &str) -> bool {
        url_host(url).is_some_and(|host| self.hosts.contains(&host.as_str()))
    }
}

pub struct Downloader {
    client: Client,
    engine: Option<Arc<DownloadEngine>>,
    priority: DownloadPriority,
    concurrency: usize,
    quick_verify: bool,
    host_header: Option<HostHeader>,
    pause: Option<PauseControl>,
}

impl Downloader {
    pub fn new(client: Client, concurrency: usize) -> Self {
        Self {
            client,
            engine: None,
            priority: DownloadPriority::Batch,
            concurrency: concurrency.max(1),
            quick_verify: false,
            host_header: None,
            pause: None,
        }
    }

    pub fn from_engine(
        engine: Arc<DownloadEngine>,
        priority: DownloadPriority,
        concurrency: usize,
    ) -> Self {
        Self {
            client: engine.h2_client(),
            engine: Some(engine),
            priority,
            concurrency: concurrency.max(1),
            quick_verify: false,
            host_header: None,
            pause: None,
        }
    }

    pub fn with_host_header(mut self, header: Option<HostHeader>) -> Self {
        self.host_header = header;
        self
    }

    pub fn with_pause(mut self, pause: PauseControl) -> Self {
        self.pause = Some(pause);
        self
    }

    pub fn quick_verify(mut self, enabled: bool) -> Self {
        self.quick_verify = enabled;
        self
    }

    /// 并发下载一批文件；采用 10 Hz `ProgressTicker` 防止 50 MB/s 下回调锁争抢。
    pub async fn download_all<F>(
        &self,
        items: &[DownloadItem],
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<DownloadReport, DownloadError>
    where
        F: Fn(DownloadProgress) + Send + Sync,
    {
        let counters = Arc::new(Counters {
            total_files: items.len() as u64,
            total_bytes: items.iter().filter_map(|i| i.size).sum(),
            done_files: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            received_bytes: AtomicU64::new(0),
            live_received: AtomicU64::new(0),
            fallbacks: AtomicU64::new(0),
            reused_bytes: AtomicU64::new(0),
            reused_files: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        });
        let done_notify = Notify::new();
        let all_finished = AtomicBool::new(false);
        #[cfg(test)]
        let hedge_override = self.engine.as_ref().and_then(|e| {
            *e.tail_hedge_delay_override
                .lock()
                .unwrap_or_else(|err| err.into_inner())
        });
        #[cfg(not(test))]
        let hedge_override = None;
        let tail_hedger = Arc::new(BatchTailHedger::new(items.len() as u64, hedge_override));

        if let Some(engine) = &self.engine {
            engine.metrics().set_diagnosis(None);
            if !engine.metrics().is_active() {
                engine.metrics().reset_batch_reuse();
            }
        }

        // 先送一次 t=0 初始快照，让上层 Rate 立刻建立起点时间戳（避免高速短爆发来不及出速度）
        if !items.is_empty() {
            on_progress(counters.snapshot());
        }

        let counters_w = counters.clone();
        let tail_hedger_w = tail_hedger.clone();
        let workers_fut = async {
            // 大文件优先（LPT）：先拉起大 Mod/JAR 与小文件并行填充尾部，防止收尾只剩单个大文件慢拖
            let mut order: Vec<usize> = (0..items.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(items[i].size.unwrap_or(0)));
            let batch_concurrency = match &self.engine {
                Some(e) if e.settings_snapshot().concurrency_mode == ConcurrencyMode::Auto => self
                    .concurrency
                    .max(usize::try_from(e.max_net_permits()).unwrap_or(self.concurrency)),
                _ => self.concurrency,
            };
            let tasks = order
                .into_iter()
                .map(|idx| self.download_one(&items[idx], cancel, &counters_w, &tail_hedger_w));
            let results: Vec<FileResult> = stream::iter(tasks)
                .buffer_unordered(batch_concurrency)
                .collect()
                .await;
            all_finished.store(true, Ordering::Release);
            done_notify.notify_waiters();
            results
        };

        let counters_t = counters.clone();
        let ticker_fut = async {
            let mut interval = tokio::time::interval(Duration::from_millis(PROGRESS_TICK_MS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = done_notify.notified() => break,
                    () = cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if all_finished.load(Ordering::Acquire) {
                            break;
                        }
                        if counters_t.dirty.swap(false, Ordering::AcqRel) {
                            on_progress(counters_t.snapshot());
                        }
                    }
                }
            }
        };

        let (results, ()) = tokio::join!(workers_fut, ticker_fut);
        // 结束前无条件同步最后一次精准快照
        on_progress(counters.snapshot());

        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }

        let mut report = DownloadReport {
            bytes_received: counters.received_bytes.load(Ordering::Relaxed),
            reused_bytes: counters.reused_bytes.load(Ordering::Relaxed),
            reused_files: counters.reused_files.load(Ordering::Relaxed),
            ..DownloadReport::default()
        };
        let mut failures = Vec::new();
        for result in results {
            match result {
                FileResult::Downloaded => report.downloaded += 1,
                FileResult::Skipped => report.skipped += 1,
                FileResult::Failed(failure) => failures.push(*failure),
                FileResult::Cancelled => return Err(DownloadError::Cancelled),
            }
        }
        if failures.is_empty() {
            Ok(report)
        } else {
            Err(DownloadError::Failed(failures))
        }
    }
}

#[derive(Clone)]
struct PlannedCandidate {
    url: String,
}

async fn send_attempt_request(
    client: &Client,
    cand: &PlannedCandidate,
    range: Option<&str>,
    if_range: Option<&str>,
    host_header: Option<&HostHeader>,
    engine: Option<&DownloadEngine>,
) -> Result<(reqwest::Response, String), RedirectSendError> {
    let _ = engine;
    send_following_safe_redirects_with_if_range(client, &cand.url, range, if_range, host_header)
        .await
}

impl Downloader {
    fn plan_download_candidates(&self, item: &DownloadItem) -> Vec<PlannedCandidate> {
        let doh = self.engine.as_ref().map(|e| e.doh().as_ref());
        let mut urls = expand_official_fallbacks(&item.urls, doh);
        if let Some(d) = doh {
            urls.sort_by_key(|u| u8::from(url_host(u).is_some_and(|h| d.is_host_degraded(&h))));
        }
        urls.into_iter()
            .map(|url| PlannedCandidate { url })
            .collect()
    }

    async fn download_one(
        &self,
        item: &DownloadItem,
        cancel: &CancellationToken,
        counters: &Arc<Counters>,
        tail_hedger: &Arc<BatchTailHedger>,
    ) -> FileResult {
        if let Some(p) = &self.pause
            && !p.wait_resumed(cancel).await
        {
            return FileResult::Cancelled;
        }
        if cancel.is_cancelled() {
            return FileResult::Cancelled;
        }
        // 全程按目标路径加锁，彻底防止同路径并发下载共用 `.part` 互相踩坏（并在存在时直接命中复用）
        let path_lock = self.engine.as_ref().map(|e| e.lock_for_path(&item.path));
        let _path_guard = match path_lock.as_ref() {
            Some(m) => Some(m.lock().await),
            None => None,
        };
        if cancel.is_cancelled() {
            return FileResult::Cancelled;
        }
        if is_valid_existing(item, self.quick_verify).await {
            counters.done_files.fetch_add(1, Ordering::Relaxed);
            if let Some(size) = item.size {
                counters.done_bytes.fetch_add(size, Ordering::Relaxed);
            }
            counters.mark_dirty();
            return FileResult::Skipped;
        }

        // 零字节跨启动器本地内容水合（仅强哈希文件，100% 校验后物化，不计入网络速度）
        if let Some(engine) = &self.engine
            && engine.local_reuse_enabled()
            && item.sha1.is_some()
            && let Some(reused_bytes) = engine.hydrator().try_hydrate(item, cancel).await
        {
            counters.done_files.fetch_add(1, Ordering::Relaxed);
            counters
                .done_bytes
                .fetch_add(reused_bytes, Ordering::Relaxed);
            counters
                .reused_bytes
                .fetch_add(reused_bytes, Ordering::Relaxed);
            counters.reused_files.fetch_add(1, Ordering::Relaxed);
            engine.metrics().record_reuse(reused_bytes, 1);
            counters.mark_dirty();
            return FileResult::Skipped;
        }

        let doh = self.engine.as_ref().map(|e| e.doh().as_ref());
        let mut candidate_urls = self.plan_download_candidates(item);
        let mut last_reason = String::from("没有可用的下载地址");
        let mut last_diag: Option<FailureDiagnosis> = None;
        let mut last_http_status: Option<u16> = None;
        let mut total_attempts = 0u32;
        let mut stall_strikes = 0usize;
        let mut tried_degraded_deferred: Vec<PlannedCandidate> = Vec::new();
        let mut index = 0usize;

        while index < candidate_urls.len() {
            let cand = candidate_urls[index].clone();
            let remaining_healthy = candidate_urls[index + 1..].iter().any(|c| {
                doh.is_none_or(|d| url_host(&c.url).is_none_or(|h| !d.is_host_degraded(&h)))
            });
            // 运行中途若兄弟任务刚刚将当前主机标为降级，且后续还有健康备选源：立刻跳过本源直奔健康源
            if remaining_healthy
                && doh.is_some_and(|d| url_host(&cand.url).is_some_and(|h| d.is_host_degraded(&h)))
            {
                tried_degraded_deferred.push(cand);
                index += 1;
                continue;
            }
            let has_other_candidate =
                index + 1 < candidate_urls.len() || !tried_degraded_deferred.is_empty();

            // 大文件 (>=4 MiB) 满足六重准入门控时优先走 Zero-HEAD 净增益分段引擎
            if let Some(engine) = &self.engine
                && can_segment(
                    item,
                    engine.settings_snapshot().segment_mode,
                    engine.disk_kind_for_path(&item.path),
                    engine.disk_ewma_ms(),
                    url_host(&cand.url).is_some_and(|h| engine.doh().is_sni_blocked(&h)),
                    engine.effective_speed_limit_kib(),
                )
            {
                let mut seg_attempt = 0u32;
                let mut seg_fall_to_next_url = false;
                let mut seg_fall_to_single_stream = false;
                let mut credit = Credit { counters, bytes: 0 };
                while seg_attempt < ATTEMPTS_PER_URL {
                    total_attempts += 1;
                    match segmented::download_segmented(
                        &cand.url,
                        item,
                        engine,
                        self.priority,
                        self.host_header.as_ref(),
                        cancel,
                        self.pause.as_ref(),
                        &mut credit,
                        counters,
                    )
                    .await
                    {
                        SegmentedOutcome::Done { received } => {
                            if let Some(expected) = item.size {
                                credit.sync_to(expected);
                            }
                            counters.done_files.fetch_add(1, Ordering::Relaxed);
                            if index > 0 {
                                counters.fallbacks.fetch_add(1, Ordering::Relaxed);
                                engine.metrics().record_fallback();
                            }
                            counters
                                .received_bytes
                                .fetch_add(received, Ordering::Relaxed);
                            counters.mark_dirty();
                            return FileResult::Downloaded;
                        }
                        SegmentedOutcome::Paused => {
                            if let Some(p) = &self.pause
                                && !p.wait_resumed(cancel).await
                            {
                                credit.rollback();
                                return FileResult::Cancelled;
                            }
                            // 保持暂停期间已完成字节，恢复时由 sync_to 平滑衔接，零回弹不双计
                            continue;
                        }
                        SegmentedOutcome::Cancelled => {
                            credit.rollback();
                            return FileResult::Cancelled;
                        }
                        SegmentedOutcome::NextUrl(reason) => {
                            credit.rollback();
                            last_reason = reason;
                            seg_fall_to_next_url = true;
                            break;
                        }
                        SegmentedOutcome::Retry {
                            reason,
                            delay_override,
                            connect_failed,
                        } => {
                            credit.rollback();
                            last_reason = reason;
                            seg_attempt += 1;
                            if connect_failed && has_other_candidate {
                                seg_fall_to_next_url = true;
                                break;
                            }
                            if seg_attempt >= ATTEMPTS_PER_URL {
                                // 若分段异常（例如非 Range 异常），让单流引擎最后兜底试一次
                                seg_fall_to_single_stream = !has_other_candidate;
                                break;
                            }
                            let delay = delay_override.unwrap_or(RETRY_BASE_DELAY);
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                () = cancel.cancelled() => return FileResult::Cancelled,
                            }
                        }
                    }
                }
                if seg_fall_to_next_url {
                    index += 1;
                    if index == candidate_urls.len() && !tried_degraded_deferred.is_empty() {
                        candidate_urls.append(&mut tried_degraded_deferred);
                    }
                    continue;
                }
                if !seg_fall_to_single_stream {
                    index += 1;
                    if index == candidate_urls.len() && !tried_degraded_deferred.is_empty() {
                        candidate_urls.append(&mut tried_degraded_deferred);
                    }
                    continue;
                }
                // 分段失败切入单流前清理 `.part.s*` 与 `.part.meta`，防止大文件孤儿分片长期占盘
                segmented::clean_segment_artifacts(&item.path, cancel).await;
            }

            let mut use_spare = false;
            let mut attempt = 0u32;
            let mut credit = Credit { counters, bytes: 0 };
            while attempt < ATTEMPTS_PER_URL {
                total_attempts += 1;
                let attempt_started = Instant::now();
                let host_sni_blocked =
                    url_host(&cand.url).is_some_and(|h| doh.is_some_and(|d| d.is_sni_blocked(&h)));
                let hedge_delay = if attempt == 0 && !use_spare {
                    self.engine.as_ref().and_then(|eng| {
                        tail_hedger.eligible_delay(
                            eng.is_metered(),
                            eng.effective_speed_limit_kib(),
                            item.size,
                            host_sni_blocked,
                        )
                    })
                } else {
                    None
                };

                let attempt_outcome = if let Some(delay) = hedge_delay {
                    match self
                        .attempt_hedged_small(
                            &cand,
                            item,
                            cancel,
                            counters,
                            &mut credit,
                            delay,
                            tail_hedger,
                        )
                        .await
                    {
                        Some(res) => res,
                        None => {
                            self.attempt(
                                &cand,
                                item,
                                cancel,
                                &mut credit,
                                use_spare,
                                stall_strikes,
                                attempt == 0 && has_other_candidate,
                            )
                            .await
                        }
                    }
                } else {
                    self.attempt(
                        &cand,
                        item,
                        cancel,
                        &mut credit,
                        use_spare,
                        stall_strikes,
                        attempt == 0 && has_other_candidate,
                    )
                    .await
                };

                match attempt_outcome {
                    Ok(received) => {
                        let elapsed_secs = attempt_started.elapsed().as_secs_f64();
                        if item.size.is_some_and(|s| s <= TAIL_HEDGE_MAX_FILE_BYTES) {
                            tail_hedger.record_small_completion_ms(elapsed_secs * 1000.0);
                        }
                        if received >= 512 * 1024
                            && elapsed_secs >= 1.5
                            && let Some(engine) = &self.engine
                            && let Some(host) = url_host(&cand.url)
                        {
                            engine.edge_optimizer().on_window_throughput(
                                engine,
                                &host,
                                &cand.url,
                                received as f64 / elapsed_secs,
                                None,
                            );
                        }
                        if let Some(expected) = item.size {
                            credit.sync_to(expected);
                        }
                        counters.done_files.fetch_add(1, Ordering::Relaxed);
                        if index > 0 {
                            counters.fallbacks.fetch_add(1, Ordering::Relaxed);
                            if let Some(e) = &self.engine {
                                e.metrics().record_fallback();
                            }
                        }
                        counters
                            .received_bytes
                            .fetch_add(received, Ordering::Relaxed);
                        counters.mark_dirty();
                        tracing::debug!(path = %item.path.display(), url = %cand.url, "下载完成");
                        return FileResult::Downloaded;
                    }
                    Err(AttemptError::Paused) => {
                        if let Some(p) = &self.pause
                            && !p.wait_resumed(cancel).await
                        {
                            credit.rollback();
                            return FileResult::Cancelled;
                        }
                        // 暂停恢复后保留已写入 .part 的进度，由 attempt 差值对齐零回弹
                        continue;
                    }
                    Err(AttemptError::Cancelled) => {
                        credit.rollback();
                        return FileResult::Cancelled;
                    }
                    Err(AttemptError::NextUrl {
                        reason,
                        diagnosis,
                        http_status,
                    }) => {
                        credit.rollback();
                        tracing::debug!(url = %cand.url, reason, "换下一个下载地址");
                        last_reason = reason;
                        if diagnosis.is_some() {
                            last_diag = diagnosis;
                        }
                        if http_status.is_some() {
                            last_http_status = http_status;
                        }
                        break;
                    }
                    Err(AttemptError::Retry {
                        reason,
                        delay_override,
                        use_spare_next,
                        stall_bump,
                        connect_failed,
                        diagnosis,
                        http_status,
                    }) => {
                        credit.rollback();
                        tracing::debug!(url = %cand.url, attempt, reason, "下载失败，准备重试");
                        last_reason = reason;
                        if diagnosis.is_some() {
                            last_diag = diagnosis;
                        }
                        if http_status.is_some() {
                            last_http_status = http_status;
                        }
                        use_spare = use_spare_next;
                        if stall_bump {
                            stall_strikes = stall_strikes.saturating_add(1);
                        }
                        // 有备选源且当前源连 TCP/TLS 都建连失败时，绝不在死源上浪费第二次 5s 超时，立刻切换备选源
                        if connect_failed && has_other_candidate {
                            break;
                        }
                        attempt += 1;
                        if attempt < ATTEMPTS_PER_URL {
                            let delay = delay_override
                                .unwrap_or_else(|| RETRY_BASE_DELAY * 2u32.pow(attempt - 1));
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                () = cancel.cancelled() => return FileResult::Cancelled,
                            }
                        }
                    }
                }
            }
            index += 1;
            if index == candidate_urls.len() && !tried_degraded_deferred.is_empty() {
                candidate_urls.append(&mut tried_degraded_deferred);
            }
        }

        if let (Some(engine), Some(diag)) = (&self.engine, last_diag.as_ref()) {
            engine.metrics().set_diagnosis(Some(diag.clone()));
        }
        let cause = last_diag
            .as_ref()
            .map_or(RootCauseKind::Unknown, |d| d.cause);
        let resumable = part_path(&item.path).is_file() || has_resumable_artifacts(&item.path);
        tracing::warn!(path = %item.path.display(), reason = %last_reason, "文件下载失败");
        FileResult::Failed(Box::new(FileFailure {
            path: item.path.clone(),
            reason: last_reason,
            item: Some(item.clone()),
            cause,
            diagnosis: last_diag,
            attempts: total_attempts.max(1),
            last_http_status,
            has_resumable_part: resumable,
        }))
    }

    fn select_client(&self, item: &DownloadItem, use_spare: bool) -> Client {
        let Some(engine) = &self.engine else {
            return self.client.clone();
        };
        let bundle = engine.bundle();
        if use_spare {
            bundle.spare
        } else if item.size.is_some_and(|s| s <= SMALL_FILE_THRESHOLD as u64) {
            bundle.next_h2_download(engine.next_h2_shard())
        } else {
            bundle.h1_segmented
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn attempt(
        &self,
        cand: &PlannedCandidate,
        item: &DownloadItem,
        cancel: &CancellationToken,
        credit: &mut Credit<'_>,
        use_spare: bool,
        stall_strikes: usize,
        allow_slow_reroll: bool,
    ) -> Result<u64, AttemptError> {
        let url = cand.url.as_str();
        let part = part_path(&item.path);
        let etag_sidecar = part_etag_path(&item.path);
        if let Some(parent) = item.path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AttemptError::retry(format!("无法创建目录：{e}")))?;
        }

        let partial_size = fs::metadata(&part).await.ok().map(|m| m.len());
        let mut resume_from = partial_size.unwrap_or(0);
        if let Some(size) = item.size {
            if partial_size == Some(size) {
                credit.sync_to(resume_from);
                finish_file(&part, item, None, self.engine.as_deref(), cancel).await?;
                return Ok(0);
            }
            if resume_from > size {
                remove_quietly(&part, cancel).await;
                remove_quietly(&etag_sidecar, cancel).await;
                resume_from = 0;
            }
        }
        let (saved_etag, saved_lm) = if resume_from > 0 {
            read_part_validators(&etag_sidecar).await
        } else {
            (None, None)
        };
        let if_range_val = saved_etag.clone().or_else(|| saved_lm.clone());

        if let Some(p) = &self.pause
            && p.is_paused()
        {
            return Err(AttemptError::Paused);
        }

        let _transfer_guard = self.engine.as_ref().map(|e| e.metrics().begin_transfer());
        let _net_permit = if let Some(engine) = &self.engine {
            Some(
                engine
                    .acquire_net(self.priority, cancel)
                    .await
                    .ok_or(AttemptError::Cancelled)?,
            )
        } else {
            None
        };
        let _h2_guard = self.engine.as_ref().map(|e| e.metrics().begin_h2_stream());

        let client = self.select_client(item, use_spare);
        let range_val = (resume_from > 0).then(|| format!("bytes={resume_from}-"));
        let req_host = url_host(url);
        let req_started = Instant::now();

        let send_fut = send_attempt_request(
            &client,
            cand,
            range_val.as_deref(),
            if_range_val.as_deref(),
            self.host_header.as_ref(),
            self.engine.as_deref(),
        );
        let (response, final_url) = tokio::select! {
            res = send_fut => match res {
                Ok(ok) => ok,
                Err(err) => {
                    return Err(classify_send_request_error(
                        err,
                        req_host.as_deref(),
                        cand,
                        self.engine.as_deref(),
                    ));
                }
            },
            () = async {
                if let Some(p) = &self.pause {
                    p.paused_signal().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => return Err(AttemptError::Paused),
            () = cancel.cancelled() => return Err(AttemptError::Cancelled),
        };

        let _ = (req_started, allow_slow_reroll);

        if let Some(engine) = &self.engine
            && let Some(h) = url_host(&final_url).or(req_host.clone())
        {
            engine.doh().report_connect_ok(&h);
        }

        let status = response.status();
        let code = status.as_u16();
        let authorized = self.host_header.as_ref().filter(|h| h.applies_to(url));

        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = parse_retry_after(response.headers());
            let host_str = req_host.as_deref().unwrap_or("下载源");
            if let Some(e) = &self.engine {
                e.metrics().push_event(
                    TransferMetricEventKind::RateLimitCooldown,
                    format!("{host_str} 触发 429 限流，等待退避"),
                );
            }
            return Err(AttemptError::Retry {
                reason: format!("服务器返回 {status}"),
                delay_override: Some(wait),
                use_spare_next: false,
                stall_bump: false,
                connect_failed: false,
                diagnosis: Some(FailureDiagnosis::rate_limited_429(host_str)),
                http_status: Some(code),
            });
        }
        if status == StatusCode::RANGE_NOT_SATISFIABLE {
            remove_quietly(&part, cancel).await;
            return Err(AttemptError::retry("续传位置无效，改为从头下载"));
        }
        if !(status == StatusCode::PARTIAL_CONTENT && resume_from > 0) && !status.is_success() {
            if is_retryable(status) {
                return Err(AttemptError::Retry {
                    reason: format!("服务器返回 {status}"),
                    delay_override: None,
                    use_spare_next: false,
                    stall_bump: false,
                    connect_failed: false,
                    diagnosis: None,
                    http_status: Some(code),
                });
            }
            if authorized.is_some()
                && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            {
                let msg = format!("{DENIED_PREFIX}（服务器返回 {status}）");
                return Err(AttemptError::NextUrl {
                    diagnosis: Some(FailureDiagnosis::auth_denied(req_host.clone(), msg.clone())),
                    reason: msg,
                    http_status: Some(code),
                });
            }
            let msg = format!("服务器返回 {status}");
            let diag = matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                .then(|| FailureDiagnosis::auth_denied(req_host.clone(), msg.clone()));
            return Err(AttemptError::NextUrl {
                reason: msg,
                diagnosis: diag,
                http_status: Some(code),
            });
        }

        let resuming = status == StatusCode::PARTIAL_CONTENT && resume_from > 0;
        let resp_etag = header_str(response.headers(), header::ETAG);
        let resp_lm = header_str(response.headers(), header::LAST_MODIFIED);
        if resuming {
            let range_header = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range);
            let range_ok = matches!(
                range_header,
                Some((start, _, total)) if start == resume_from && total.is_none_or(|t| item.size.is_none_or(|exp| t == exp))
            );
            let validator_ok = validator_matches(
                saved_etag.as_deref(),
                saved_lm.as_deref(),
                resp_etag.as_deref(),
                resp_lm.as_deref(),
            );
            if !range_ok || !validator_ok {
                remove_quietly(&part, cancel).await;
                remove_quietly(&etag_sidecar, cancel).await;
                return Err(AttemptError::retry(
                    "续传范围或远端 ETag 校验不一致，重置从头下载",
                ));
            }
        } else {
            resume_from = 0;
            remove_quietly(&etag_sidecar, cancel).await;
        }

        // 首包 HTML 劫持防线（响应头判定）
        let is_binary = is_binary_target(&item.path);
        if is_binary
            && response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| {
                    ct.trim_start()
                        .to_ascii_lowercase()
                        .starts_with("text/html")
                })
        {
            return Err(AttemptError::next_url(format!(
                "{STRUCTURE_MISMATCH}下载源返回了 HTML 网页而非目标文件"
            )));
        }

        let server_content_len = response.content_length();
        let expected_total = item
            .size
            .or_else(|| server_content_len.map(|len| resume_from.saturating_add(len)));

        // RAM 快车道决策：无续传 + 小文件 + 64 MiB 内存预算（预算满时自动退回流式写盘）
        let mem_permit = if !resuming && item.size.is_none_or(|s| s <= SMALL_FILE_THRESHOLD as u64)
        {
            match &self.engine {
                Some(engine) => engine.mem_budget.clone().try_acquire_owned().ok(),
                None => None,
            }
        } else {
            None
        };
        let use_ram_initially = !resuming
            && item.size.is_none_or(|s| s <= SMALL_FILE_THRESHOLD as u64)
            && (self.engine.is_none() || mem_permit.is_some());

        enum Sink {
            Ram(Vec<u8>, Option<tokio::sync::OwnedSemaphorePermit>),
            Disk(BufWriter<fs::File>),
        }

        impl Sink {
            async fn flush_for_resume(self, part: &Path, engine: Option<&DownloadEngine>) {
                match self {
                    Self::Ram(buf, _permit) => {
                        if let Some(e) = engine {
                            e.metrics().sub_mem_bytes(buf.len());
                        }
                        if !buf.is_empty() {
                            let _ = fs::write(part, &buf).await;
                        }
                    }
                    Self::Disk(mut writer) => {
                        let _ = writer.flush().await;
                    }
                }
            }
        }

        let mut sink = if use_ram_initially {
            credit.sync_to(0);
            Sink::Ram(
                Vec::with_capacity(item.size.unwrap_or(0).min(64 * 1024) as usize),
                mem_permit,
            )
        } else if resuming {
            credit.sync_to(resume_from);
            let file = fs::OpenOptions::new()
                .append(true)
                .open(&part)
                .await
                .map_err(|e| AttemptError::retry(format!("无法写入临时文件：{e}")))?;
            Sink::Disk(BufWriter::with_capacity(WRITE_BUF_BYTES, file))
        } else {
            credit.sync_to(0);
            let file = fs::File::create(&part)
                .await
                .map_err(|e| AttemptError::retry(format!("无法写入临时文件：{e}")))?;
            Sink::Disk(BufWriter::with_capacity(WRITE_BUF_BYTES, file))
        };

        let mut live_hasher = (!resuming && item.sha1.is_some()).then(Sha1::new);
        let mut written = resume_from;
        let mut sniff_buf = Vec::with_capacity(512);
        let mut sniff_checked = !is_binary || resuming;
        let stall_timeout = Duration::from_secs(
            STALL_TIMEOUTS_SECS[stall_strikes.min(STALL_TIMEOUTS_SECS.len() - 1)],
        );

        let mut body = response.bytes_stream();
        loop {
            let next = tokio::select! {
                step = tokio::time::timeout(stall_timeout, body.next()) => match step {
                    Ok(item_next) => item_next,
                    Err(_) => {
                        sink.flush_for_resume(&part, self.engine.as_deref()).await;
                        save_part_validators(&etag_sidecar, resp_etag.as_deref(), resp_lm.as_deref()).await;
                        return Err(AttemptError::Retry {
                            reason: "接收数据长时间停滞，准备重连".to_owned(),
                            delay_override: Some(Duration::from_millis(200)),
                            use_spare_next: true,
                            stall_bump: true,
                            connect_failed: false,
                            diagnosis: None,
                            http_status: None,
                        });
                    }
                },
                () = async {
                    if let Some(p) = &self.pause {
                        p.paused_signal().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    sink.flush_for_resume(&part, self.engine.as_deref()).await;
                    save_part_validators(&etag_sidecar, resp_etag.as_deref(), resp_lm.as_deref()).await;
                    return Err(AttemptError::Paused);
                }
                () = cancel.cancelled() => {
                    sink.flush_for_resume(&part, self.engine.as_deref()).await;
                    save_part_validators(&etag_sidecar, resp_etag.as_deref(), resp_lm.as_deref()).await;
                    return Err(AttemptError::Cancelled);
                }
            };

            match next {
                None => break,
                Some(Err(error)) => {
                    sink.flush_for_resume(&part, self.engine.as_deref()).await;
                    save_part_validators(&etag_sidecar, resp_etag.as_deref(), resp_lm.as_deref())
                        .await;
                    return Err(AttemptError::retry(format!("下载中断：{error}")));
                }
                Some(Ok(chunk)) => {
                    if let Some(engine) = &self.engine
                        && !engine.throttle().consume(chunk.len(), cancel).await
                    {
                        sink.flush_for_resume(&part, self.engine.as_deref()).await;
                        save_part_validators(
                            &etag_sidecar,
                            resp_etag.as_deref(),
                            resp_lm.as_deref(),
                        )
                        .await;
                        return Err(AttemptError::Cancelled);
                    }
                    if !sniff_checked && sniff_buf.len() < 512 {
                        let need = (512 - sniff_buf.len()).min(chunk.len());
                        sniff_buf.extend_from_slice(&chunk[..need]);
                        if looks_like_html_payload(&sniff_buf) {
                            drop(sink);
                            remove_quietly(&part, cancel).await;
                            remove_quietly(&etag_sidecar, cancel).await;
                            let reason = format!("{STRUCTURE_MISMATCH}下载源返回了 HTML 劫持页面");
                            return Err(AttemptError::NextUrl {
                                diagnosis: Some(FailureDiagnosis::integrity_mismatch(
                                    req_host.clone(),
                                    reason.clone(),
                                )),
                                reason,
                                http_status: None,
                            });
                        }
                        if sniff_buf.len() >= 16 {
                            sniff_checked = true;
                        }
                    }

                    if let Some(hasher) = live_hasher.as_mut() {
                        hasher.update(&chunk);
                    }

                    // RAM 超限 1 MiB 无缝溢出到磁盘 `.part`
                    sink = match sink {
                        Sink::Ram(mut buf, mem_p) => {
                            if buf.len().saturating_add(chunk.len()) <= SMALL_FILE_THRESHOLD {
                                if let Some(engine) = &self.engine {
                                    engine.metrics().add_mem_bytes(chunk.len());
                                }
                                buf.extend_from_slice(&chunk);
                                Sink::Ram(buf, mem_p)
                            } else {
                                if let Some(engine) = &self.engine {
                                    engine.metrics().sub_mem_bytes(buf.len());
                                }
                                let file = fs::File::create(&part).await.map_err(|e| {
                                    AttemptError::retry(format!("无法写入临时文件：{e}"))
                                })?;
                                let mut writer = BufWriter::with_capacity(WRITE_BUF_BYTES, file);
                                writer
                                    .write_all(&buf)
                                    .await
                                    .map_err(|e| AttemptError::retry(format!("写入失败：{e}")))?;
                                writer
                                    .write_all(&chunk)
                                    .await
                                    .map_err(|e| AttemptError::retry(format!("写入失败：{e}")))?;
                                drop(mem_p);
                                Sink::Disk(writer)
                            }
                        }
                        Sink::Disk(mut writer) => {
                            writer
                                .write_all(&chunk)
                                .await
                                .map_err(|e| AttemptError::retry(format!("写入失败：{e}")))?;
                            Sink::Disk(writer)
                        }
                    };

                    let len = chunk.len() as u64;
                    written += len;
                    credit.add(len);
                    credit
                        .counters
                        .live_received
                        .fetch_add(len, Ordering::Relaxed);
                    if let Some(engine) = &self.engine {
                        engine.metrics().record_rx(len);
                    }
                    if item.size.is_some_and(|size| written > size) {
                        drop(sink);
                        remove_quietly(&part, cancel).await;
                        remove_quietly(&etag_sidecar, cancel).await;
                        let reason = "收到的内容比预期大，可能不是正确的文件".to_owned();
                        return Err(AttemptError::retry(reason));
                    }
                }
            }
        }

        let live_sha1 = live_hasher.map(|h| to_hex(&h.finalize()));

        match sink {
            Sink::Ram(buf, _mem_p) => {
                let ram_len = buf.len();
                let cleanup_ram = || {
                    if let Some(engine) = &self.engine {
                        engine.metrics().sub_mem_bytes(ram_len);
                    }
                };
                if !sniff_checked && looks_like_html_payload(&buf) {
                    cleanup_ram();
                    let reason = format!("{STRUCTURE_MISMATCH}下载源返回了 HTML 劫持页面");
                    return Err(AttemptError::NextUrl {
                        diagnosis: Some(FailureDiagnosis::integrity_mismatch(
                            req_host.clone(),
                            reason.clone(),
                        )),
                        reason,
                        http_status: None,
                    });
                }
                if let Some(expected) = expected_total
                    && buf.len() as u64 != expected
                {
                    cleanup_ram();
                    let reason = format!(
                        "{SIZE_MISMATCH}应为 {expected} 字节，实际 {} 字节",
                        buf.len()
                    );
                    return Err(AttemptError::retry(reason));
                }
                if let (Some(expected), Some(actual)) = (&item.sha1, &live_sha1)
                    && !actual.eq_ignore_ascii_case(expected)
                {
                    cleanup_ram();
                    let reason = format!("{SHA1_MISMATCH}应为 {expected}，实际 {actual}");
                    return Err(AttemptError::retry(reason));
                }
                if item.sha1.is_none()
                    && let Err(reason) = verify_structure_bytes(&buf, &item.path)
                {
                    cleanup_ram();
                    return Err(AttemptError::retry(reason));
                }

                cleanup_ram();
                commit_ram_buffer_to_disk(item, &part, &buf, self.engine.as_deref(), cancel)
                    .await?;
            }
            Sink::Disk(mut writer) => {
                writer
                    .flush()
                    .await
                    .map_err(|e| AttemptError::retry(format!("写入失败：{e}")))?;
                drop(writer);
                if let Some(expected) = expected_total
                    && written != expected
                {
                    remove_quietly(&part, cancel).await;
                    remove_quietly(&etag_sidecar, cancel).await;
                    let reason =
                        format!("{SIZE_MISMATCH}应为 {expected} 字节，实际 {written} 字节");
                    return Err(AttemptError::retry(reason));
                }
                finish_file(
                    &part,
                    item,
                    live_sha1.as_deref(),
                    self.engine.as_deref(),
                    cancel,
                )
                .await?;
            }
        }

        let received = written - resume_from;
        Ok(received)
    }

    #[allow(clippy::too_many_arguments)]
    async fn attempt_hedged_small(
        &self,
        cand: &PlannedCandidate,
        item: &DownloadItem,
        cancel: &CancellationToken,
        counters: &Arc<Counters>,
        credit: &mut Credit<'_>,
        hedge_delay: Duration,
        tail_hedger: &Arc<BatchTailHedger>,
    ) -> Option<Result<u64, AttemptError>> {
        let engine = self.engine.as_ref()?;
        let size = item.size?;
        if size == 0 || size > TAIL_HEDGE_MAX_FILE_BYTES {
            return None;
        }
        let part = part_path(&item.path);
        if fs::metadata(&part).await.is_ok() {
            return None;
        }
        let permits_needed =
            u32::try_from(size.div_ceil(SMALL_FILE_THRESHOLD as u64).clamp(1, 2)).unwrap_or(1);
        let mem_permit = engine
            .mem_budget
            .clone()
            .try_acquire_many_owned(permits_needed)
            .ok()?;

        if let Some(parent) = item.path.parent()
            && let Err(e) = fs::create_dir_all(parent).await
        {
            return Some(Err(AttemptError::retry(format!("无法创建目录：{e}"))));
        }
        if let Some(p) = &self.pause
            && p.is_paused()
        {
            return Some(Err(AttemptError::Paused));
        }
        if cancel.is_cancelled() {
            return Some(Err(AttemptError::Cancelled));
        }

        let _transfer_guard = engine.metrics().begin_transfer();
        let _net_permit = match engine.acquire_net(self.priority, cancel).await {
            Some(p) => p,
            None => return Some(Err(AttemptError::Cancelled)),
        };
        let bundle = engine.bundle();
        let primary_client = bundle.next_h2_download(engine.next_h2_shard());
        let primary_rx = Arc::new(AtomicU64::new(0));
        let mut p_fut = Box::pin(self.fetch_small_into_ram(
            primary_client,
            cand,
            item,
            size,
            cancel,
            counters,
            &primary_rx,
        ));

        let mut wait_dur = hedge_delay;
        let winner_res = loop {
            let timer = tokio::time::sleep(wait_dur);
            tokio::pin!(timer);

            let first_outcome = tokio::select! {
                res = &mut p_fut => Some(res),
                () = &mut timer => None,
                () = async {
                    if let Some(p) = &self.pause {
                        p.paused_signal().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => return Some(Err(AttemptError::Paused)),
                () = cancel.cancelled() => return Some(Err(AttemptError::Cancelled)),
            };

            if let Some(res) = first_outcome {
                break res;
            }

            let sni_blocked = url_host(&cand.url).is_some_and(|h| engine.doh().is_sni_blocked(&h));
            if engine.is_metered()
                || engine.effective_speed_limit_kib() > 0
                || sni_blocked
                || primary_rx.load(Ordering::Acquire) >= size.saturating_mul(3) / 4
            {
                break tokio::select! {
                    res = &mut p_fut => res,
                    () = async {
                        if let Some(p) = &self.pause {
                            p.paused_signal().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => Err(AttemptError::Paused),
                    () = cancel.cancelled() => Err(AttemptError::Cancelled),
                };
            }

            let done_now = counters.done_files.load(Ordering::Relaxed);
            if !tail_hedger.is_tail_window(done_now) {
                wait_dur = Duration::from_millis(200);
                continue;
            }

            let Some(hedge_permit) = engine.try_acquire_net(self.priority) else {
                break tokio::select! {
                    res = &mut p_fut => res,
                    () = async {
                        if let Some(p) = &self.pause {
                            p.paused_signal().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => Err(AttemptError::Paused),
                    () = cancel.cancelled() => Err(AttemptError::Cancelled),
                };
            };

            if !tail_hedger.try_claim_hedge_slot() {
                drop(hedge_permit);
                break tokio::select! {
                    res = &mut p_fut => res,
                    () = async {
                        if let Some(p) = &self.pause {
                            p.paused_signal().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => Err(AttemptError::Paused),
                    () = cancel.cancelled() => Err(AttemptError::Cancelled),
                };
            }

            engine.tail_hedges_launched.fetch_add(1, Ordering::Relaxed);
            let hedge_rx = Arc::new(AtomicU64::new(0));
            let mut h_fut = Box::pin(self.fetch_small_into_ram(
                bundle.spare,
                cand,
                item,
                size,
                cancel,
                counters,
                &hedge_rx,
            ));
            enum Branch {
                Primary(Result<RamFetchResult, AttemptError>),
                Hedge(Result<RamFetchResult, AttemptError>),
            }
            let first = tokio::select! {
                res = &mut p_fut => Branch::Primary(res),
                res = &mut h_fut => Branch::Hedge(res),
                () = async {
                    if let Some(p) = &self.pause {
                        p.paused_signal().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => return Some(Err(AttemptError::Paused)),
                () = cancel.cancelled() => return Some(Err(AttemptError::Cancelled)),
            };
            let race_res = match first {
                Branch::Primary(Ok(ok)) => Ok(ok),
                Branch::Hedge(Ok(ok)) => {
                    tail_hedger.record_hedge_win();
                    engine.tail_hedges_won.fetch_add(1, Ordering::Relaxed);
                    engine.metrics().push_event(
                        TransferMetricEventKind::WatchdogRecovery,
                        "批尾小文件对冲命中，已绕过慢连接",
                    );
                    Ok(ok)
                }
                Branch::Primary(Err(err)) => {
                    if matches!(err, AttemptError::Paused | AttemptError::Cancelled) {
                        Err(err)
                    } else {
                        let second = tokio::select! {
                            res = &mut h_fut => res,
                            () = async {
                                if let Some(p) = &self.pause {
                                    p.paused_signal().await;
                                } else {
                                    std::future::pending::<()>().await;
                                }
                            } => return Some(Err(AttemptError::Paused)),
                            () = cancel.cancelled() => return Some(Err(AttemptError::Cancelled)),
                        };
                        match second {
                            Ok(ok) => {
                                tail_hedger.record_hedge_win();
                                engine.tail_hedges_won.fetch_add(1, Ordering::Relaxed);
                                engine.metrics().push_event(
                                    TransferMetricEventKind::WatchdogRecovery,
                                    "批尾小文件对冲命中，已绕过慢连接",
                                );
                                Ok(ok)
                            }
                            Err(_) => Err(err),
                        }
                    }
                }
                Branch::Hedge(Err(err)) => {
                    if matches!(err, AttemptError::Paused | AttemptError::Cancelled) {
                        Err(err)
                    } else {
                        tokio::select! {
                            res = &mut p_fut => res,
                            () = async {
                                if let Some(p) = &self.pause {
                                    p.paused_signal().await;
                                } else {
                                    std::future::pending::<()>().await;
                                }
                            } => return Some(Err(AttemptError::Paused)),
                            () = cancel.cancelled() => return Some(Err(AttemptError::Cancelled)),
                        }
                    }
                }
            };
            drop(hedge_permit);
            break race_res;
        };

        drop(mem_permit);
        let fetched = match winner_res {
            Ok(ok) => ok,
            Err(e) => return Some(Err(e)),
        };
        let ram_len = fetched.bytes.len() as u64;
        if let Err(e) =
            commit_ram_buffer_to_disk(item, &part, &fetched.bytes, Some(engine), cancel).await
        {
            return Some(Err(e));
        }
        credit.sync_to(ram_len);
        let _ = (&fetched.ttfb_ms, &fetched.elapsed_secs);
        Some(Ok(fetched.received))
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_small_into_ram(
        &self,
        client: Client,
        cand: &PlannedCandidate,
        item: &DownloadItem,
        expected_size: u64,
        cancel: &CancellationToken,
        counters: &Arc<Counters>,
        branch_rx: &Arc<AtomicU64>,
    ) -> Result<RamFetchResult, AttemptError> {
        struct RamMeter<'a>(usize, Option<&'a DownloadEngine>);
        impl Drop for RamMeter<'_> {
            fn drop(&mut self) {
                if self.0 > 0
                    && let Some(e) = self.1
                {
                    e.metrics().sub_mem_bytes(self.0);
                }
            }
        }

        let url = cand.url.as_str();
        let req_host = url_host(url);
        let req_started = Instant::now();
        let _h2_guard = self.engine.as_ref().map(|e| e.metrics().begin_h2_stream());

        let (response, final_url) = send_attempt_request(
            &client,
            cand,
            None,
            None,
            self.host_header.as_ref(),
            self.engine.as_deref(),
        )
        .await
        .map_err(|err| {
            classify_send_request_error(err, req_host.as_deref(), cand, self.engine.as_deref())
        })?;

        let ttfb_ms = req_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(engine) = &self.engine
            && let Some(h) = url_host(&final_url).or(req_host.clone())
        {
            engine.doh().report_connect_ok(&h);
        }

        let status = response.status();
        let code = status.as_u16();
        let authorized = self.host_header.as_ref().filter(|h| h.applies_to(url));
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = parse_retry_after(response.headers());
            let host_str = req_host.as_deref().unwrap_or("下载源");
            if let Some(e) = &self.engine {
                e.metrics().push_event(
                    TransferMetricEventKind::RateLimitCooldown,
                    format!("{host_str} 触发 429 限流，等待退避"),
                );
            }
            return Err(AttemptError::Retry {
                reason: format!("服务器返回 {status}"),
                delay_override: Some(wait),
                use_spare_next: false,
                stall_bump: false,
                connect_failed: false,
                diagnosis: Some(FailureDiagnosis::rate_limited_429(host_str)),
                http_status: Some(code),
            });
        }
        if !status.is_success() {
            if is_retryable(status) {
                return Err(AttemptError::Retry {
                    reason: format!("服务器返回 {status}"),
                    delay_override: None,
                    use_spare_next: false,
                    stall_bump: false,
                    connect_failed: false,
                    diagnosis: None,
                    http_status: Some(code),
                });
            }
            if authorized.is_some()
                && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            {
                let msg = format!("{DENIED_PREFIX}（服务器返回 {status}）");
                return Err(AttemptError::NextUrl {
                    diagnosis: Some(FailureDiagnosis::auth_denied(req_host.clone(), msg.clone())),
                    reason: msg,
                    http_status: Some(code),
                });
            }
            let msg = format!("服务器返回 {status}");
            let diag = matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                .then(|| FailureDiagnosis::auth_denied(req_host.clone(), msg.clone()));
            return Err(AttemptError::NextUrl {
                reason: msg,
                diagnosis: diag,
                http_status: Some(code),
            });
        }

        let is_binary = is_binary_target(&item.path);
        if is_binary
            && response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| {
                    ct.trim_start()
                        .to_ascii_lowercase()
                        .starts_with("text/html")
                })
        {
            return Err(AttemptError::next_url(format!(
                "{STRUCTURE_MISMATCH}下载源返回了 HTML 网页而非目标文件"
            )));
        }

        let stream_started = Instant::now();
        let mut buf = Vec::with_capacity(expected_size.min(256 * 1024) as usize);
        let mut ram_meter = RamMeter(0, self.engine.as_deref());
        let mut hasher = item.sha1.is_some().then(Sha1::new);
        let mut body = response.bytes_stream();
        let stall_timeout = Duration::from_secs(STALL_TIMEOUTS_SECS[0]);

        while let Some(step) = tokio::time::timeout(stall_timeout, body.next())
            .await
            .map_err(|_| AttemptError::Retry {
                reason: "接收数据长时间停滞，准备重连".to_owned(),
                delay_override: Some(Duration::from_millis(200)),
                use_spare_next: true,
                stall_bump: true,
                connect_failed: false,
                diagnosis: None,
                http_status: None,
            })?
        {
            let chunk = step.map_err(|e| AttemptError::retry(format!("下载中断：{e}")))?;
            if let Some(engine) = &self.engine
                && !engine.throttle().consume(chunk.len(), cancel).await
            {
                return Err(AttemptError::Cancelled);
            }
            if buf.len().saturating_add(chunk.len()) as u64 > expected_size {
                return Err(AttemptError::retry(
                    "收到的内容比预期大，可能不是正确的文件",
                ));
            }
            if let Some(h) = hasher.as_mut() {
                h.update(&chunk);
            }
            buf.extend_from_slice(&chunk);
            ram_meter.0 += chunk.len();
            let len_u64 = chunk.len() as u64;
            branch_rx.fetch_add(len_u64, Ordering::Release);
            counters.live_received.fetch_add(len_u64, Ordering::Relaxed);
            if let Some(engine) = &self.engine {
                engine.metrics().add_mem_bytes(chunk.len());
                engine.metrics().record_rx(len_u64);
            }
        }

        if is_binary && looks_like_html_payload(&buf) {
            return Err(AttemptError::next_url(format!(
                "{STRUCTURE_MISMATCH}下载源返回了 HTML 劫持页面"
            )));
        }
        if buf.len() as u64 != expected_size {
            return Err(AttemptError::retry(format!(
                "{SIZE_MISMATCH}应为 {expected_size} 字节，实际 {} 字节",
                buf.len()
            )));
        }
        if let (Some(expected), Some(h)) = (&item.sha1, hasher) {
            let actual = to_hex(&h.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(AttemptError::retry(format!(
                    "{SHA1_MISMATCH}应为 {expected}，实际 {actual}"
                )));
            }
        } else if item.sha1.is_none()
            && let Err(reason) = verify_structure_bytes(&buf, &item.path)
        {
            return Err(AttemptError::retry(reason));
        }

        let received = buf.len() as u64;
        let elapsed_secs = stream_started.elapsed().as_secs_f64();
        drop(ram_meter);
        Ok(RamFetchResult {
            bytes: buf,
            ttfb_ms,
            received,
            elapsed_secs,
        })
    }
}

struct RamFetchResult {
    bytes: Vec<u8>,
    ttfb_ms: f64,
    received: u64,
    elapsed_secs: f64,
}

fn classify_send_request_error(
    err: RedirectSendError,
    req_host: Option<&str>,
    cand: &PlannedCandidate,
    engine: Option<&DownloadEngine>,
) -> AttemptError {
    match err {
        RedirectSendError::Protocol(msg) => {
            let _ = cand;
            AttemptError::next_url(msg)
        }
        RedirectSendError::Request {
            url: failed_url,
            source,
        } => {
            let failed_host = url_host(&failed_url).or_else(|| req_host.map(str::to_owned));
            if let Some(eng) = engine {
                // 快切代理 0ms 自愈：建连出错立刻复查一次系统代理，若刚变更则用新池瞬间重试
                if source.is_connect() && eng.refresh_proxy_if_changed().is_some() {
                    return AttemptError::Retry {
                        reason: "检测到系统代理刚刚切换，已重建连接并重试".to_owned(),
                        delay_override: Some(Duration::from_millis(30)),
                        use_spare_next: true,
                        stall_bump: false,
                        connect_failed: false,
                        diagnosis: None,
                        http_status: None,
                    };
                }
                if let Some(h) = failed_host.as_deref() {
                    match eng.doh().report_connect_failure(h, &source) {
                        TransportDnsAction::SniBlockedSwitchSource => {
                            let diag = classify_reqwest_error(
                                &source,
                                Some(h),
                                Some(eng.doh()),
                                eng.proxy_mode(),
                            );
                            eng.metrics().push_event(
                                TransferMetricEventKind::SniBypass,
                                format!("{h} 握手重置，自动切换备选源"),
                            );
                            return AttemptError::NextUrl {
                                reason: format!("{}（已切换备选源）", diag.summary),
                                diagnosis: Some(diag),
                                http_status: None,
                            };
                        }
                        TransportDnsAction::RetryOnceSingleStream => {
                            let diag = classify_reqwest_error(
                                &source,
                                Some(h),
                                Some(eng.doh()),
                                eng.proxy_mode(),
                            );
                            return AttemptError::Retry {
                                reason: diag.summary.clone(),
                                delay_override: Some(Duration::from_millis(80)),
                                use_spare_next: true,
                                stall_bump: false,
                                connect_failed: true,
                                diagnosis: Some(diag),
                                http_status: None,
                            };
                        }
                        TransportDnsAction::None => {}
                    }
                }
                let diag = classify_reqwest_error(
                    &source,
                    failed_host.as_deref(),
                    Some(eng.doh()),
                    eng.proxy_mode(),
                );
                return AttemptError::Retry {
                    reason: diag.summary.clone(),
                    delay_override: None,
                    use_spare_next: false,
                    stall_bump: false,
                    connect_failed: true,
                    diagnosis: Some(diag),
                    http_status: None,
                };
            }
            AttemptError::Retry {
                reason: format!("连接失败：{source}"),
                delay_override: None,
                use_spare_next: false,
                stall_bump: false,
                connect_failed: true,
                diagnosis: None,
                http_status: None,
            }
        }
    }
}

async fn commit_ram_buffer_to_disk(
    item: &DownloadItem,
    part: &Path,
    buf: &[u8],
    engine: Option<&DownloadEngine>,
    cancel: &CancellationToken,
) -> Result<(), AttemptError> {
    let mut disk_guard = engine.map(|e| e.metrics().begin_disk_write());
    let _disk_permit = if let Some(eng) = engine {
        Some(
            eng.acquire_disk(cancel)
                .await
                .ok_or(AttemptError::Cancelled)?,
        )
    } else {
        None
    };
    if let Some(g) = disk_guard.as_mut() {
        g.mark_active();
    }

    let started = Instant::now();
    let mut file = fs::File::create(part)
        .await
        .map_err(|e| AttemptError::retry(format!("无法保存临时文件：{e}")))?;
    file.write_all(buf)
        .await
        .map_err(|e| AttemptError::retry(format!("写入文件失败：{e}")))?;
    file.flush()
        .await
        .map_err(|e| AttemptError::retry(format!("刷新文件失败：{e}")))?;
    drop(file);
    if let Err(err) = rename_with_win_retry(part, &item.path, cancel).await {
        remove_quietly(part, cancel).await;
        if err.kind() == io::ErrorKind::Interrupted {
            return Err(AttemptError::Cancelled);
        }
        let diag = classify_io_error(&err);
        return Err(AttemptError::Retry {
            reason: format!("{}（{err}）", diag.summary),
            delay_override: None,
            use_spare_next: false,
            stall_bump: false,
            connect_failed: false,
            diagnosis: Some(diag),
            http_status: None,
        });
    }
    remove_quietly(&part_etag_path(&item.path), cancel).await;
    if let Some(eng) = engine {
        eng.record_disk_write_ms(started.elapsed().as_secs_f64() * 1000.0, buf.len() as u64);
        if let Some(sha1) = &item.sha1 {
            eng.hydrator()
                .record_file_async(item.path.clone(), sha1.clone(), buf.len() as u64)
                .await;
        }
    }
    Ok(())
}

async fn finish_file(
    part: &Path,
    item: &DownloadItem,
    precomputed_sha1: Option<&str>,
    engine: Option<&DownloadEngine>,
    cancel: &CancellationToken,
) -> Result<(), AttemptError> {
    match verify_with_precomputed(part, item, precomputed_sha1).await {
        Ok(Ok(())) => {}
        Ok(Err(mismatch)) => {
            remove_quietly(part, cancel).await;
            remove_quietly(&part_etag_path(&item.path), cancel).await;
            return Err(AttemptError::retry(mismatch));
        }
        Err(error) => return Err(AttemptError::retry(format!("无法读取下载的文件：{error}"))),
    }

    let mut disk_guard = engine.map(|e| e.metrics().begin_disk_write());
    let _disk_permit = if let Some(eng) = engine {
        Some(
            eng.acquire_disk(cancel)
                .await
                .ok_or(AttemptError::Cancelled)?,
        )
    } else {
        None
    };
    if let Some(g) = disk_guard.as_mut() {
        g.mark_active();
    }

    let started = Instant::now();
    let res = rename_with_win_retry(part, &item.path, cancel).await;
    if let Some(eng) = engine {
        eng.record_disk_write_ms(started.elapsed().as_secs_f64() * 1000.0, 0);
        if res.is_ok()
            && let Some(sha1) = &item.sha1
        {
            let size = fs::metadata(&item.path).await.map_or(0, |m| m.len());
            eng.hydrator()
                .record_file_async(item.path.clone(), sha1.clone(), size)
                .await;
        }
    }
    if res.is_ok() {
        remove_quietly(&part_etag_path(&item.path), cancel).await;
    }
    res.map_err(|e| {
        if e.kind() == io::ErrorKind::Interrupted {
            AttemptError::Cancelled
        } else {
            let diag = classify_io_error(&e);
            AttemptError::Retry {
                reason: format!("{}（{e}）", diag.summary),
                delay_override: None,
                use_spare_next: false,
                stall_bump: false,
                connect_failed: false,
                diagnosis: Some(diag),
                http_status: None,
            }
        }
    })
}

async fn verify_with_precomputed(
    path: &Path,
    item: &DownloadItem,
    precomputed_sha1: Option<&str>,
) -> io::Result<Result<(), String>> {
    let len = fs::metadata(path).await?.len();
    if let Some(size) = item.size
        && len != size
    {
        return Ok(Err(format!(
            "{SIZE_MISMATCH}应为 {size} 字节，实际 {len} 字节"
        )));
    }
    if let Some(expected) = &item.sha1 {
        let actual = match precomputed_sha1 {
            Some(sha1) => sha1.to_owned(),
            None => sha1_file(path).await?,
        };
        if !actual.eq_ignore_ascii_case(expected) {
            return Ok(Err(format!(
                "{SHA1_MISMATCH}应为 {expected}，实际 {actual}"
            )));
        }
    } else if let Err(reason) = verify_structure_file(path, &item.path, len).await? {
        return Ok(Err(reason));
    }
    Ok(Ok(()))
}

async fn verify(path: &Path, item: &DownloadItem) -> io::Result<Result<(), String>> {
    verify_with_precomputed(path, item, None).await
}

async fn is_valid_existing(item: &DownloadItem, quick: bool) -> bool {
    let Ok(metadata) = fs::metadata(&item.path).await else {
        return false;
    };
    if quick && let Some(size) = item.size {
        return metadata.len() == size;
    }
    matches!(verify(&item.path, item).await, Ok(Ok(())))
}

pub(crate) fn is_binary_target(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        ext.as_deref(),
        Some(
            "jar" | "zip" | "mrpack" | "exe" | "dll" | "so" | "gz" | "xz" | "png" | "ogg" | "class"
        )
    )
}

pub(crate) fn looks_like_html_payload(bytes: &[u8]) -> bool {
    let trimmed = bytes.trim_ascii_start();
    if trimmed.is_empty() {
        return false;
    }
    let prefix = trimmed.get(..16).unwrap_or(trimmed).to_ascii_lowercase();
    prefix.starts_with(b"<!doctype html") || prefix.starts_with(b"<html")
}

/// 内存内微秒级结构验尸（仅用于无强哈希文件）。
pub(crate) fn verify_structure_bytes(bytes: &[u8], target: &Path) -> Result<(), String> {
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("jar" | "zip" | "mrpack") => verify_zip_eocd(bytes, bytes.len() as u64),
        Some("json") => serde_json::from_slice::<serde::de::IgnoredAny>(bytes)
            .map(|_| ())
            .map_err(|e| format!("{STRUCTURE_MISMATCH}JSON 语法损坏：{e}")),
        Some("xml") => verify_xml_bytes(bytes),
        _ => Ok(()),
    }
}

/// 磁盘大文件微秒级结构验尸（只读头/尾 64 KiB，绝不二次全盘重读，JSON/XML 上限读 8 MiB）。
async fn verify_structure_file(
    path: &Path,
    target: &Path,
    len: u64,
) -> io::Result<Result<(), String>> {
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("jar" | "zip" | "mrpack") => {
            if len < 22 {
                return Ok(Err(format!("{STRUCTURE_MISMATCH}压缩包过短（{len} 字节）")));
            }
            let mut file = fs::File::open(path).await?;
            let mut head = [0u8; 4];
            file.read_exact(&mut head).await?;
            if &head != b"PK\x03\x04" {
                return Ok(Err(format!("{STRUCTURE_MISMATCH}缺少 ZIP PK 文件头")));
            }
            let tail_len = usize::try_from(len.min(65_557)).unwrap_or(65_557);
            file.seek(io::SeekFrom::End(-(tail_len as i64))).await?;
            let mut tail = vec![0u8; tail_len];
            file.read_exact(&mut tail).await?;
            Ok(find_and_validate_eocd(&tail, len))
        }
        Some("json") if len <= 8 * 1024 * 1024 => {
            let bytes = fs::read(path).await?;
            Ok(serde_json::from_slice::<serde::de::IgnoredAny>(&bytes)
                .map(|_| ())
                .map_err(|e| format!("{STRUCTURE_MISMATCH}JSON 语法损坏：{e}")))
        }
        Some("xml") => {
            let mut file = fs::File::open(path).await?;
            let read_len = usize::try_from(len.min(512)).unwrap_or(512);
            let mut head = vec![0u8; read_len];
            file.read_exact(&mut head).await?;
            Ok(verify_xml_bytes(&head))
        }
        _ => Ok(Ok(())),
    }
}

fn verify_zip_eocd(bytes: &[u8], total_len: u64) -> Result<(), String> {
    if bytes.len() < 22 {
        return Err(format!(
            "{STRUCTURE_MISMATCH}压缩包过短（{} 字节）",
            bytes.len()
        ));
    }
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err(format!("{STRUCTURE_MISMATCH}缺少 ZIP PK 文件头"));
    }
    let tail_start = bytes.len().saturating_sub(65_557);
    find_and_validate_eocd(&bytes[tail_start..], total_len)
}

fn find_and_validate_eocd(tail: &[u8], total_len: u64) -> Result<(), String> {
    let sig = [0x50u8, 0x4b, 0x05, 0x06];
    let Some(pos) = tail
        .windows(4)
        .rposition(|w| w == sig)
        .filter(|&p| p + 22 <= tail.len())
    else {
        return Err(format!("{STRUCTURE_MISMATCH}缺少 ZIP 结束目录标记（EOCD）"));
    };
    let eocd = &tail[pos..];
    let cd_size = u64::from(u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]));
    let cd_offset = u64::from(u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]));
    if cd_offset != u64::from(u32::MAX)
        && cd_size != u64::from(u32::MAX)
        && cd_offset.saturating_add(cd_size) > total_len
    {
        return Err(format!(
            "{STRUCTURE_MISMATCH}ZIP 中央目录越界（文件被截断）"
        ));
    }
    Ok(())
}

fn verify_xml_bytes(bytes: &[u8]) -> Result<(), String> {
    let trimmed = bytes.trim_ascii_start();
    if !trimmed.starts_with(b"<") || looks_like_html_payload(trimmed) {
        return Err(format!("{STRUCTURE_MISMATCH}XML 文件头无效或被劫持为 HTML"));
    }
    Ok(())
}

pub(crate) fn parse_retry_after(headers: &header::HeaderMap) -> Duration {
    let secs = headers
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(3)
        .clamp(1, 60);
    Duration::from_secs(secs)
}

/// 计算文件的 SHA-1（小写十六进制），在后台线程里读，不阻塞异步任务。
pub async fn sha1_file(path: &Path) -> io::Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha1::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(to_hex(&hasher.finalize()))
    })
    .await
    .map_err(io::Error::other)?
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    path.with_file_name(name)
}

fn part_etag_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part.etag");
    path.with_file_name(name)
}

fn validator_matches(
    saved_etag: Option<&str>,
    saved_lm: Option<&str>,
    resp_etag: Option<&str>,
    resp_lm: Option<&str>,
) -> bool {
    if let (Some(a), Some(b)) = (saved_etag, resp_etag) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (saved_lm, resp_lm) {
        return a == b;
    }
    true
}

async fn read_part_validators(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(path).await else {
        return (None, None);
    };
    let mut lines = text.lines();
    let etag = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let lm = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    (etag, lm)
}

async fn save_part_validators(path: &Path, etag: Option<&str>, lm: Option<&str>) {
    if etag.is_none() && lm.is_none() {
        return;
    }
    let content = format!("{}\n{}", etag.unwrap_or_default(), lm.unwrap_or_default());
    let _ = fs::write(path, content).await;
}

fn is_retryable(status: StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY | StatusCode::TOO_MANY_REQUESTS
        )
}

async fn remove_quietly(path: &Path, cancel: &CancellationToken) {
    if let Err(error) = remove_with_win_retry(path, cancel).await
        && error.kind() != io::ErrorKind::Interrupted
    {
        tracing::debug!(path = %path.display(), %error, "删除临时文件失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_header_only_goes_to_listed_hosts() {
        let header = HostHeader {
            hosts: &["edge.forgecdn.net"],
            name: "x-api-key",
            value: Secret::new("k".to_owned()),
        };
        assert!(header.applies_to("https://edge.forgecdn.net/files/1/2/a.jar"));
        assert!(header.applies_to("https://EDGE.forgecdn.net:443/files/a.jar"));
        for other in [
            "http://edge.forgecdn.net/files/a.jar",
            "https://edge.forgecdn.net.evil.com/a.jar",
            "https://evil.com/edge.forgecdn.net/a.jar",
            "https://user@evil.com/a.jar",
            "https://cdn.modrinth.com/a.jar",
        ] {
            assert!(!header.applies_to(other), "{other}");
        }
    }

    #[test]
    fn part_file_sits_next_to_the_target() {
        assert_eq!(
            part_path(Path::new("a/b/client.jar")),
            PathBuf::from("a/b/client.jar.part")
        );
    }

    #[test]
    fn retry_only_transient_statuses() {
        assert!(is_retryable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(!is_retryable(StatusCode::NOT_FOUND));
        assert!(!is_retryable(StatusCode::FORBIDDEN));
    }

    #[test]
    fn structure_checks_catch_corrupt_jar_json_xml_and_accept_valid() {
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
            zip.write_all(b"Manifest-Version: 1.0\n").unwrap();
            zip.finish().unwrap();
        }
        let valid_zip = cursor.into_inner();
        assert!(verify_structure_bytes(&valid_zip, Path::new("mod.jar")).is_ok());

        let mut truncated = valid_zip.clone();
        truncated.truncate(valid_zip.len() - 10);
        assert!(verify_structure_bytes(&truncated, Path::new("mod.jar")).is_err());

        assert!(verify_structure_bytes(br#"{"ok":true}"#, Path::new("m.json")).is_ok());
        assert!(verify_structure_bytes(br#"{"ok":"#, Path::new("m.json")).is_err());
        assert!(
            verify_structure_bytes(b"<metadata></metadata>", Path::new("maven-metadata.xml"))
                .is_ok()
        );
        assert!(
            verify_structure_bytes(b"<!DOCTYPE html><html>", Path::new("maven-metadata.xml"))
                .is_err()
        );
    }

    #[tokio::test]
    async fn engine_reserves_interactive_slots_under_batch_saturation() {
        let doh = Arc::new(Doh::new(true).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());
        let cancel = CancellationToken::new();

        let mut batch_permits = Vec::new();
        for _ in 0..BATCH_NET_SLOTS {
            batch_permits.push(
                engine
                    .acquire_net(DownloadPriority::Batch, &cancel)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(engine.net_batch_sem().available_permits(), 0);
        assert!(engine.net_total_sem().available_permits() >= 16);

        // Interactive 仍能立即拿到连接许可，不被 Batch 饿死
        let interactive = tokio::time::timeout(
            Duration::from_millis(100),
            engine.acquire_net(DownloadPriority::Interactive, &cancel),
        )
        .await
        .unwrap();
        assert!(interactive.is_some());
    }

    #[tokio::test]
    async fn downloads_an_empty_file_without_a_partial_file() {
        use tokio::io::AsyncReadExt;

        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let serve = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        let item = DownloadItem {
            urls: vec![format!("http://{address}/empty")],
            path: path.clone(),
            sha1: Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned()),
            size: Some(0),
        };
        let downloader = Downloader::new(Client::builder().no_proxy().build().unwrap(), 1);
        downloader
            .download_all(&[item], &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        serve.await.unwrap();
        assert!(fs::read(path).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sha1_of_known_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(
            sha1_file(&path).await.unwrap(),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
    }

    #[tokio::test]
    async fn disk_ewma_normalizes_large_merges_and_debounces_backpressure() {
        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine = DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap();

        // 64 MiB 合并耗时 200ms ≈ 3.125 ms/MiB，绝不应把 SSD 误判为 >= 50ms 慢盘背压
        for _ in 0..8 {
            engine.record_disk_write_ms(200.0, 64 * 1024 * 1024);
        }
        assert!(engine.disk_ewma_ms() < 10.0);
        let snap_fast = engine.runtime_metrics();
        assert!(
            !snap_fast
                .recent_events
                .iter()
                .any(|e| e.kind == TransferMetricEventKind::DiskBackpressure)
        );

        // 真正慢盘（20 MiB 耗时 3000ms = 150 ms/MiB）跨过阈值，10s 冷却期内只推一条事件
        for _ in 0..10 {
            engine.record_disk_write_ms(3000.0, 20 * 1024 * 1024);
        }
        assert!(engine.disk_ewma_ms() >= 50.0);
        let snap_slow = engine.runtime_metrics();
        let count = snap_slow
            .recent_events
            .iter()
            .filter(|e| e.kind == TransferMetricEventKind::DiskBackpressure)
            .count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn pause_resume_mixed_batch_has_zero_bounce_and_exact_total() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut small_data = vec![0u8; 700 * 1024];
        let mut big_data = vec![0u8; 4 * 1024 * 1024 + 8192];
        for (i, b) in small_data.iter_mut().enumerate() {
            *b = ((i * 5 + 1) % 256) as u8;
        }
        for (i, b) in big_data.iter_mut().enumerate() {
            *b = ((i * 11 + 7) % 256) as u8;
        }
        let small_sha1 = {
            let mut h = Sha1::new();
            h.update(&small_data);
            super::super::to_hex(&h.finalize())
        };
        let big_sha1 = {
            let mut h = Sha1::new();
            h.update(&big_data);
            super::super::to_hex(&h.finalize())
        };
        let total_expected = (small_data.len() + big_data.len()) as u64;
        let small_srv = Arc::new(small_data.clone());
        let big_srv = Arc::new(big_data.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let s_data = small_srv.clone();
                let b_data = big_srv.clone();
                tokio::spawn(async move {
                    let mut req = [0u8; 4096];
                    let Ok(n) = sock.read(&mut req).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req_text = String::from_utf8_lossy(&req[..n]);
                    let first_line = req_text.lines().next().unwrap_or("");
                    let data = if first_line.contains("/small.jar") {
                        s_data
                    } else {
                        b_data
                    };
                    let total = data.len();
                    let range_line = req_text.lines().find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(str::to_owned)
                    });
                    let (hdr, slice) = if let Some(r) = range_line {
                        let (s, e) = r.trim().split_once('-').unwrap_or(("0", ""));
                        let start = s.parse::<usize>().unwrap_or(0);
                        let end = if e.is_empty() {
                            total - 1
                        } else {
                            e.parse::<usize>().unwrap_or(total - 1).min(total - 1)
                        };
                        let slice = &data[start..=end];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nETag: \"mix-v1\"\r\nConnection: close\r\n\r\n",
                            slice.len()
                        );
                        (hdr, slice)
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nETag: \"mix-v1\"\r\nConnection: close\r\n\r\n"
                        );
                        (hdr, data.as_slice())
                    };
                    if sock.write_all(hdr.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in slice.chunks(32 * 1024) {
                        if sock.write_all(chunk).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(6)).await;
                    }
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let small_path = dir.path().join("small.jar");
        let big_path = dir.path().join("big.jar");
        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());
        let pause = PauseControl::new();
        let downloader = Downloader::from_engine(engine, DownloadPriority::Interactive, 2)
            .with_pause(pause.clone());

        let items = vec![
            DownloadItem {
                urls: vec![format!("http://{addr}/small.jar")],
                path: small_path.clone(),
                sha1: Some(small_sha1),
                size: Some(small_data.len() as u64),
            },
            DownloadItem {
                urls: vec![format!("http://{addr}/big.jar")],
                path: big_path.clone(),
                sha1: Some(big_sha1),
                size: Some(big_data.len() as u64),
            },
        ];

        let max_seen = Arc::new(AtomicU64::new(0));
        let bounced = Arc::new(AtomicBool::new(false));
        let pause_cycles = Arc::new(AtomicU32::new(0));
        let p_ctrl = pause.clone();
        let seen_clone = max_seen.clone();
        let cycles_clone = pause_cycles.clone();
        tokio::spawn(async move {
            for threshold in [300 * 1024, 1500 * 1024] {
                loop {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    if seen_clone.load(Ordering::Acquire) >= threshold {
                        p_ctrl.pause();
                        tokio::time::sleep(Duration::from_millis(120)).await;
                        p_ctrl.resume();
                        cycles_clone.fetch_add(1, Ordering::AcqRel);
                        break;
                    }
                }
            }
        });

        let seen_cb = max_seen.clone();
        let bounced_cb = bounced.clone();
        let report = downloader
            .download_all(&items, &CancellationToken::new(), |p| {
                let prev = seen_cb.load(Ordering::Acquire);
                if p.done_bytes < prev {
                    bounced_cb.store(true, Ordering::Release);
                } else {
                    seen_cb.store(p.done_bytes, Ordering::Release);
                }
            })
            .await
            .unwrap();

        assert_eq!(pause_cycles.load(Ordering::Acquire), 2);
        assert!(
            !bounced.load(Ordering::Acquire),
            "暂停/恢复绝不允许进度回弹"
        );
        assert_eq!(max_seen.load(Ordering::Acquire), total_expected);
        assert_eq!(report.downloaded, 2);
        assert_eq!(std::fs::read(&small_path).unwrap(), small_data);
        assert_eq!(std::fs::read(&big_path).unwrap(), big_data);
    }

    #[tokio::test]
    async fn tampered_source_and_ssrf_redirect_are_blocked_and_fallback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let good_payload = b"PK\x03\x04valid_official_jar_payload".to_vec();
        let good_sha1 = {
            let mut h = Sha1::new();
            h.update(&good_payload);
            to_hex(&h.finalize())
        };

        let bad_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bad_addr = bad_listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = bad_listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/");
                    if path == "/ssrf" {
                        let _ = sock
                            .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await;
                    } else if path == "/html" {
                        let body = b"<!doctype html><html><body>hijack</body></html>";
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    } else {
                        let evil = b"PK\x03\x04corrupted_mirror_bytes_xxx";
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            evil.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(evil).await;
                    }
                });
            }
        });

        let ok_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ok_addr = ok_listener.local_addr().unwrap();
        let ok_bytes = good_payload.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = ok_listener.accept().await {
                let data = ok_bytes.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(&data).await;
                });
            }
        });

        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());
        let downloader = Downloader::from_engine(engine.clone(), DownloadPriority::Interactive, 2);

        let dir = tempfile::tempdir().unwrap();
        let out1 = dir.path().join("recovered.jar");
        let report = downloader
            .download_all(
                &[DownloadItem {
                    urls: vec![
                        format!("http://{bad_addr}/html"),
                        format!("http://{bad_addr}/ssrf"),
                        format!("http://{bad_addr}/tamper"),
                        format!("http://{ok_addr}/ok.jar"),
                    ],
                    path: out1.clone(),
                    sha1: Some(good_sha1),
                    size: Some(good_payload.len() as u64),
                }],
                &CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(report.downloaded, 1);
        assert_eq!(std::fs::read(&out1).unwrap(), good_payload);
        assert!(engine.runtime_metrics().fallbacks >= 1);
    }

    #[tokio::test]
    async fn batch_tail_hedge_rescues_stalled_small_file() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let payloads: Vec<Vec<u8>> = (0..5)
            .map(|idx| vec![b'A' + idx as u8; 16 * 1024])
            .collect();
        let sha1s: Vec<String> = payloads
            .iter()
            .map(|p| {
                let mut h = Sha1::new();
                h.update(p);
                to_hex(&h.finalize())
            })
            .collect();

        let srv_payloads = Arc::new(payloads.clone());
        let f4_requests = Arc::new(AtomicUsize::new(0));
        let f4_req_srv = f4_requests.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let payloads_c = srv_payloads.clone();
                let f4_c = f4_req_srv.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/f0");
                    let idx: usize = path.trim_start_matches("/f").parse().unwrap_or(0);
                    if idx == 4 {
                        let nth = f4_c.fetch_add(1, Ordering::AcqRel);
                        if nth == 0 {
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                    let body = &payloads_c[idx];
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(body).await;
                });
            }
        });

        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());
        engine.set_tail_hedge_delay_override(Some(Duration::from_millis(80)));
        let downloader = Downloader::from_engine(engine.clone(), DownloadPriority::Interactive, 4);

        let dir = tempfile::tempdir().unwrap();
        let items: Vec<DownloadItem> = (0..5)
            .map(|i| DownloadItem {
                urls: vec![format!("http://{addr}/f{i}")],
                path: dir.path().join(format!("f{i}.bin")),
                sha1: Some(sha1s[i].clone()),
                size: Some(payloads[i].len() as u64),
            })
            .collect();
        let total_expected: u64 = payloads.iter().map(|p| p.len() as u64).sum();

        let started = Instant::now();
        let last_done = Arc::new(AtomicU64::new(0));
        let last_done_cb = last_done.clone();
        let report = downloader
            .download_all(&items, &CancellationToken::new(), |p| {
                last_done_cb.store(p.done_bytes, Ordering::Release);
            })
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "批尾对冲应迅速绕过 5s 卡死连接，实际耗时 {elapsed:?}"
        );
        assert_eq!(report.downloaded, 5);
        assert_eq!(last_done.load(Ordering::Acquire), total_expected);
        assert!(engine.tail_hedges_launched() >= 1);
        assert!(engine.tail_hedges_won() >= 1);
        for (i, expected) in payloads.iter().enumerate() {
            assert_eq!(&std::fs::read(&items[i].path).unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn batch_tail_hedge_is_strictly_zero_when_metered_or_throttled() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let payloads: Vec<Vec<u8>> = (0..5).map(|idx| vec![b'K' + idx as u8; 8 * 1024]).collect();
        let sha1s: Vec<String> = payloads
            .iter()
            .map(|p| {
                let mut h = Sha1::new();
                h.update(p);
                to_hex(&h.finalize())
            })
            .collect();

        let srv_payloads = Arc::new(payloads.clone());
        let slow_requests = Arc::new(AtomicUsize::new(0));
        let slow_req_srv = slow_requests.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let payloads_c = srv_payloads.clone();
                let slow_c = slow_req_srv.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/f0");
                    let idx: usize = path.trim_start_matches("/f").parse().unwrap_or(0);
                    if idx == 4 {
                        slow_c.fetch_add(1, Ordering::AcqRel);
                        tokio::time::sleep(Duration::from_millis(220)).await;
                    }
                    let body = &payloads_c[idx];
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(body).await;
                });
            }
        });

        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());
        engine.set_tail_hedge_delay_override(Some(Duration::from_millis(40)));
        let downloader = Downloader::from_engine(engine.clone(), DownloadPriority::Interactive, 4);

        // Case A: 计费网络严格 0 对冲
        engine.set_metered(true);
        let dir_a = tempfile::tempdir().unwrap();
        let items_a: Vec<DownloadItem> = (0..5)
            .map(|i| DownloadItem {
                urls: vec![format!("http://{addr}/f{i}")],
                path: dir_a.path().join(format!("a_{i}.bin")),
                sha1: Some(sha1s[i].clone()),
                size: Some(payloads[i].len() as u64),
            })
            .collect();
        let rep_a = downloader
            .download_all(&items_a, &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(rep_a.downloaded, 5);
        assert_eq!(slow_requests.load(Ordering::Acquire), 1);
        assert_eq!(engine.tail_hedges_launched(), 0);

        // Case B: 限速模式严格 0 对冲
        engine.set_metered(false);
        let cfg = DownloadSettings {
            speed_limit_kib_per_sec: 4096,
            ..DownloadSettings::default()
        };
        engine.apply_settings(&cfg);

        let dir_b = tempfile::tempdir().unwrap();
        let items_b: Vec<DownloadItem> = (0..5)
            .map(|i| DownloadItem {
                urls: vec![format!("http://{addr}/f{i}")],
                path: dir_b.path().join(format!("b_{i}.bin")),
                sha1: Some(sha1s[i].clone()),
                size: Some(payloads[i].len() as u64),
            })
            .collect();
        let rep_b = downloader
            .download_all(&items_b, &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(rep_b.downloaded, 5);
        assert_eq!(slow_requests.load(Ordering::Acquire), 2);
        assert_eq!(engine.tail_hedges_launched(), 0);
    }

    #[tokio::test]
    async fn disk_pool_hot_update_never_deadlocks_acquire_many() {
        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());

        let mut slow_cfg = DownloadSettings {
            disk_write_mode: DiskWriteMode::Manual,
            max_disk_writers: 4,
            ..DownloadSettings::default()
        };
        engine.apply_settings(&slow_cfg);
        engine.record_disk_write_ms(200.0, 1024);

        slow_cfg.max_disk_writers = 32;
        engine.apply_settings(&slow_cfg);

        let cancel = CancellationToken::new();
        let permit = tokio::time::timeout(Duration::from_millis(200), engine.acquire_disk(&cancel))
            .await
            .expect("disk pool hot update must not deadlock acquire_many_owned");
        assert!(permit.is_some());
    }

    #[tokio::test]
    async fn concurrent_downloads_to_same_path_serialize_without_corruption() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut payload = vec![0u8; 1500 * 1024];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(&payload);
            to_hex(&h.finalize())
        };
        let srv_payload = Arc::new(payload.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let data = srv_payload.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(&data).await;
                });
            }
        });

        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(ProxyWatcher::new());
        let engine =
            Arc::new(DownloadEngine::new(doh, watcher, &ProxySettings::default()).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("shared-lib.jar");
        let item = DownloadItem {
            urls: vec![format!("http://{addr}/shared-lib.jar")],
            path: target.clone(),
            sha1: Some(expected_sha1),
            size: Some(payload.len() as u64),
        };
        let dl1 = Downloader::from_engine(engine.clone(), DownloadPriority::Interactive, 2);
        let dl2 = Downloader::from_engine(engine, DownloadPriority::Batch, 2);
        let c = CancellationToken::new();
        let (r1, r2) = tokio::join!(
            dl1.download_all(std::slice::from_ref(&item), &c, |_| {}),
            dl2.download_all(std::slice::from_ref(&item), &c, |_| {}),
        );
        let r1 = r1.unwrap();
        let r2 = r2.unwrap();
        assert_eq!(r1.downloaded + r2.downloaded, 1);
        assert_eq!(r1.skipped + r2.skipped, 1);
        assert_eq!(std::fs::read(&target).unwrap(), payload);
        assert!(!part_path(&target).exists());
    }

    #[tokio::test]
    async fn single_stream_resume_skips_html_sniff_on_middle_bytes_and_validates_etag() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut payload = vec![b'A'; 128];
        payload.extend_from_slice(b"<!DOCTYPE html>embedded-text-in-mod-jar");
        payload.resize(2048, b'B');
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(&payload);
            to_hex(&h.finalize())
        };
        let srv_payload = Arc::new(payload.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let data = srv_payload.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let has_range = req.to_ascii_lowercase().contains("range: bytes=128-");
                    if has_range {
                        let tail = &data[128..];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 128-{}/{}\r\nContent-Length: {}\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                            data.len() - 1,
                            data.len(),
                            tail.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(tail).await;
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                            data.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(&data).await;
                    }
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("mod-with-html-docs.jar");
        std::fs::write(part_path(&target), &payload[..128]).unwrap();
        std::fs::write(part_etag_path(&target), "\"v1\"\n").unwrap();

        let client = crate::net::build_client().unwrap();
        let dl = Downloader::new(client, 1);
        let item = DownloadItem {
            urls: vec![format!("http://{addr}/mod.jar")],
            path: target.clone(),
            sha1: Some(expected_sha1),
            size: Some(payload.len() as u64),
        };
        let rep = dl
            .download_all(&[item], &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(rep.downloaded, 1);
        assert_eq!(std::fs::read(&target).unwrap(), payload);
        assert!(!part_etag_path(&target).exists());
    }
}
