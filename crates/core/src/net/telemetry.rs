//! 独立 10 Hz 实时传输遥测与环形自愈事件总线（零锁热路径 + `specta` 全兼容）。
//!
//! - 热路径只做 `AtomicU64::fetch_add(..., Relaxed)`（~2 ns），消除回调锁争抢；
//! - 空闲时由上层推送器自动降频为 0 Hz 零唤醒；
//! - 所有类型严禁出现 `u64` / `usize`（满足 `tauri-specta` `BigIntExportBehavior::Fail`）。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::now_millis;

use super::diagnosis::FailureDiagnosis;
use super::throttle::DiskMediaKind;

const MAX_RECENT_EVENTS: usize = 12;
const SPEED_WINDOW: Duration = Duration::from_millis(1500);
const SPEED_MIN_SPAN: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ActiveRouteKind {
    #[default]
    Direct,
    SystemProxy,
    TunFakeIp,
    CustomProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum TransportProtocol {
    #[default]
    Idle,
    H2,
    H1Segmented,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum TelemetryEventKind {
    RouteSwitched,
    RampUp,
    RampRollback,
    WorkStolen,
    SourceFallback,
    SniBypass,
    DiskBackpressure,
    WatchdogRecovery,
    RateLimitCooldown,
    LocalReused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct TelemetryEvent {
    pub at_ms: f64,
    pub kind: TelemetryEventKind,
    /// 已脱敏简短中文事件，绝不含本机路径与凭据
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct TransferTelemetry {
    pub seq: u32,
    pub timestamp_ms: f64,
    /// 仅计真从网卡收到的字节（B/s）
    pub speed_bytes_per_sec: f64,
    pub route_kind: ActiveRouteKind,
    pub route_label: String,
    pub proxy_epoch: u32,
    pub protocol: TransportProtocol,
    pub active_connections: u32,
    pub max_connections: u32,
    pub active_h2_streams: u32,
    pub active_h1_segments: u32,
    pub mem_buffer_used_bytes: f64,
    pub disk_kind: DiskMediaKind,
    pub disk_writers_active: u32,
    pub disk_writers_limit: u32,
    pub disk_queue_depth: u32,
    pub disk_write_ewma_ms: f64,
    /// 本地零字节水合 / 同盘硬链接节省流量
    pub reused_bytes: f64,
    pub reused_files: u32,
    pub work_steals: u32,
    pub fallbacks: u32,
    pub metered: bool,
    pub metered_throttled: bool,
    pub effective_speed_limit_kib: u32,
    pub recent_events: Vec<TelemetryEvent>,
    pub active_diagnosis: Option<FailureDiagnosis>,
    /// 当前运行态 DoH 简要摘要（如 "阿里 DoH 32ms"），供设置页状态条展示
    pub doh_summary: Option<String>,
}

/// 进程内实时传输遥测中心。
pub struct TelemetryHub {
    seq: AtomicU32,
    active_transfers: AtomicU32,
    active_h2_streams: AtomicU32,
    active_h1_segments: AtomicU32,
    mem_buffer_used_bytes: AtomicU64,
    disk_writers_active: AtomicU32,
    disk_queue_depth: AtomicU32,
    rx_bytes_total: AtomicU64,
    reused_bytes: AtomicU64,
    reused_files: AtomicU32,
    work_steals: AtomicU32,
    fallbacks: AtomicU32,
    speed_window: Mutex<VecDeque<(Instant, u64)>>,
    recent_events: Mutex<VecDeque<TelemetryEvent>>,
    active_diagnosis: Mutex<Option<FailureDiagnosis>>,
}

impl TelemetryHub {
    pub fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
            active_transfers: AtomicU32::new(0),
            active_h2_streams: AtomicU32::new(0),
            active_h1_segments: AtomicU32::new(0),
            mem_buffer_used_bytes: AtomicU64::new(0),
            disk_writers_active: AtomicU32::new(0),
            disk_queue_depth: AtomicU32::new(0),
            rx_bytes_total: AtomicU64::new(0),
            reused_bytes: AtomicU64::new(0),
            reused_files: AtomicU32::new(0),
            work_steals: AtomicU32::new(0),
            fallbacks: AtomicU32::new(0),
            speed_window: Mutex::new(VecDeque::with_capacity(20)),
            recent_events: Mutex::new(VecDeque::with_capacity(MAX_RECENT_EVENTS)),
            active_diagnosis: Mutex::new(None),
        }
    }

    pub fn record_rx(&self, bytes: u64) {
        if bytes > 0 {
            self.rx_bytes_total.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    pub fn record_reuse(&self, bytes: u64, files: u32) {
        if bytes > 0 {
            self.reused_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
        if files > 0 {
            self.reused_files.fetch_add(files, Ordering::Relaxed);
        }
    }

    pub fn reset_batch_reuse(&self) {
        self.reused_bytes.store(0, Ordering::Relaxed);
        self.reused_files.store(0, Ordering::Relaxed);
    }

    pub fn record_work_steal(&self) {
        self.work_steals.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fallback(&self) {
        self.fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn begin_transfer(&self) -> ActiveGuard<'_> {
        self.active_transfers.fetch_add(1, Ordering::AcqRel);
        ActiveGuard(&self.active_transfers)
    }

    pub fn begin_h2_stream(&self) -> ActiveGuard<'_> {
        self.active_h2_streams.fetch_add(1, Ordering::AcqRel);
        ActiveGuard(&self.active_h2_streams)
    }

    pub fn begin_h1_segment(&self) -> ActiveGuard<'_> {
        self.active_h1_segments.fetch_add(1, Ordering::AcqRel);
        ActiveGuard(&self.active_h1_segments)
    }

    pub fn begin_disk_write(&self) -> DiskWriteGuard<'_> {
        self.disk_queue_depth.fetch_add(1, Ordering::AcqRel);
        DiskWriteGuard {
            queue: &self.disk_queue_depth,
            active: &self.disk_writers_active,
            acquired: false,
        }
    }

    pub fn add_mem_bytes(&self, bytes: usize) {
        self.mem_buffer_used_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn sub_mem_bytes(&self, bytes: usize) {
        let len = bytes as u64;
        let _ =
            self.mem_buffer_used_bytes
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                    Some(cur.saturating_sub(len))
                });
    }

    pub fn push_event(&self, kind: TelemetryEventKind, message: impl Into<String>) {
        let mut guard = self.recent_events.lock().unwrap_or_else(|e| e.into_inner());
        let event = TelemetryEvent {
            at_ms: now_millis() as f64,
            kind,
            message: message.into(),
        };
        if guard.len() >= MAX_RECENT_EVENTS {
            guard.pop_front();
        }
        guard.push_back(event);
    }

    pub fn set_diagnosis(&self, diag: Option<FailureDiagnosis>) {
        *self
            .active_diagnosis
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = diag;
    }

    pub fn is_active(&self) -> bool {
        self.active_transfers.load(Ordering::Acquire) > 0
            || self.active_h2_streams.load(Ordering::Acquire) > 0
            || self.active_h1_segments.load(Ordering::Acquire) > 0
            || self.disk_writers_active.load(Ordering::Acquire) > 0
    }

    #[allow(clippy::too_many_arguments)]
    pub fn snapshot(
        &self,
        route_kind: ActiveRouteKind,
        route_label: String,
        proxy_epoch: u32,
        max_connections: u32,
        disk_kind: DiskMediaKind,
        disk_writers_limit: u32,
        disk_write_ewma_ms: f64,
        metered: bool,
        metered_throttled: bool,
        effective_speed_limit_kib: u32,
        doh_summary: Option<String>,
    ) -> TransferTelemetry {
        let h2 = self.active_h2_streams.load(Ordering::Acquire);
        let h1 = self.active_h1_segments.load(Ordering::Acquire);
        let active_connections = h2.saturating_add(h1);
        let protocol = match (h2 > 0, h1 > 0) {
            (false, false) => {
                if self.active_transfers.load(Ordering::Acquire) > 0 {
                    TransportProtocol::H2
                } else {
                    TransportProtocol::Idle
                }
            }
            (true, false) => TransportProtocol::H2,
            (false, true) => TransportProtocol::H1Segmented,
            (true, true) => TransportProtocol::Mixed,
        };

        let rx_total = self.rx_bytes_total.load(Ordering::Relaxed);
        let speed_bytes_per_sec = {
            let now = Instant::now();
            let mut win = self.speed_window.lock().unwrap_or_else(|e| e.into_inner());
            win.push_back((now, rx_total));
            while win.len() > 2
                && win
                    .front()
                    .is_some_and(|&(t, _)| now.duration_since(t) > SPEED_WINDOW)
            {
                win.pop_front();
            }
            if active_connections == 0 && !self.is_active() {
                win.clear();
                0.0
            } else if let Some(&(first_t, first_rx)) = win.front() {
                let span = now.duration_since(first_t);
                if span >= SPEED_MIN_SPAN {
                    (rx_total.saturating_sub(first_rx) as f64 / span.as_secs_f64()).max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            }
        };

        let recent_events: Vec<TelemetryEvent> = self
            .recent_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        let active_diagnosis = self
            .active_diagnosis
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        TransferTelemetry {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            timestamp_ms: now_millis() as f64,
            speed_bytes_per_sec,
            route_kind,
            route_label,
            proxy_epoch,
            protocol,
            active_connections,
            max_connections,
            active_h2_streams: h2,
            active_h1_segments: h1,
            mem_buffer_used_bytes: self.mem_buffer_used_bytes.load(Ordering::Relaxed) as f64,
            disk_kind,
            disk_writers_active: self.disk_writers_active.load(Ordering::Acquire),
            disk_writers_limit,
            disk_queue_depth: self.disk_queue_depth.load(Ordering::Acquire),
            disk_write_ewma_ms,
            reused_bytes: self.reused_bytes.load(Ordering::Relaxed) as f64,
            reused_files: self.reused_files.load(Ordering::Relaxed),
            work_steals: self.work_steals.load(Ordering::Relaxed),
            fallbacks: self.fallbacks.load(Ordering::Relaxed),
            metered,
            metered_throttled,
            effective_speed_limit_kib,
            recent_events,
            active_diagnosis,
            doh_summary,
        }
    }
}

impl Default for TelemetryHub {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ActiveGuard<'a>(&'a AtomicU32);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct DiskWriteGuard<'a> {
    queue: &'a AtomicU32,
    active: &'a AtomicU32,
    acquired: bool,
}

impl DiskWriteGuard<'_> {
    pub fn mark_active(&mut self) {
        if !self.acquired {
            self.acquired = true;
            self.queue.fetch_sub(1, Ordering::AcqRel);
            self.active.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl Drop for DiskWriteGuard<'_> {
    fn drop(&mut self) {
        if self.acquired {
            self.active.fetch_sub(1, Ordering::AcqRel);
        } else {
            self.queue.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
