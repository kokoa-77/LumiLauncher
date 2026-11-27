//! Zero-HEAD 动态分段下载、净增益门控爬坡、ETA 速度加权工作窃取与零 `set_len` Segment-0 活体哈希单趟合并。

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter as SyncBufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header;
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::SegmentMode;
use crate::fs::{
    AtomicWriteError, read_json, remove_with_win_retry, remove_with_win_retry_sync,
    rename_with_win_retry_sync, write_json_atomic,
};
use crate::jobs::PauseControl;

use super::diagnosis::{classify_io_error, classify_reqwest_error};
use super::doh::{Doh, TransportDnsAction};
use super::download::{
    Counters, Credit, DENIED_PREFIX, DownloadEngine, DownloadItem, DownloadPriority, HostHeader,
    SHA1_MISMATCH, SIZE_MISMATCH, STRUCTURE_MISMATCH, is_binary_target, looks_like_html_payload,
    parse_retry_after, verify_structure_bytes,
};
use super::http::{RedirectSendError, send_following_safe_redirects};
use super::profile::compute_network_signature;
use super::proxy_watch::classify_proxy_tag;
use super::telemetry::{TelemetryEventKind, TelemetryHub};
use super::throttle::{DiskMediaKind, TokenBucket};
use super::{to_hex, url_host};

pub const MIN_SEGMENT_FILE_SIZE: u64 = 4 * 1024 * 1024; // 4 MiB
const MIN_SPLIT_CHUNK: u64 = 512 * 1024; // 512 KiB
const MIN_VICTIM_REMAINING: u64 = 1024 * 1024; // 1 MiB
const WRITE_BUF_BYTES: usize = 256 * 1024; // 256 KiB
const RAMP_INTERVAL: Duration = Duration::from_millis(800);
const WARMUP_SKIP: Duration = Duration::from_millis(200);
const SEG_STALL_TIMEOUT: Duration = Duration::from_secs(12);
const DIRECT_RUNGS: &[u8] = &[1, 2, 4, 8, 12, 16, 24, 32];
const PROXIED_RUNGS: &[u8] = &[1, 2, 4];

pub fn align_4k_down(offset: u64) -> u64 {
    offset & !4095u64
}

pub fn part_seg_path(target: &Path, seg_id: u32) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".part.s{seg_id}"));
    target.with_file_name(name)
}

pub fn part_meta_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part.meta");
    target.with_file_name(name)
}

/// 分段准入总门控（零 HEAD 前置判定）。
#[allow(clippy::too_many_arguments)]
pub fn can_segment(
    item: &DownloadItem,
    segment_mode: SegmentMode,
    disk_kind: DiskMediaKind,
    disk_ewma_ms: f64,
    sni_blocked: bool,
    effective_limit_kib: u32,
) -> bool {
    if segment_mode == SegmentMode::Off || disk_kind == DiskMediaKind::Hdd {
        return false;
    }
    if disk_ewma_ms >= 50.0 || sni_blocked {
        return false;
    }
    if effective_limit_kib > 0 && effective_limit_kib < 2048 {
        return false;
    }
    item.size.is_some_and(|sz| sz >= MIN_SEGMENT_FILE_SIZE)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSegment {
    pub id: u32,
    pub start: u64,
    pub end: u64,
    pub written: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartMeta {
    pub total_size: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub segments: Vec<SavedSegment>,
}

impl PartMeta {
    fn validator_matches(&self, etag: Option<&str>, last_modified: Option<&str>) -> bool {
        if let (Some(a), Some(b)) = (&self.etag, etag) {
            return a == b;
        }
        if let (Some(a), Some(b)) = (&self.last_modified, last_modified) {
            return a == b;
        }
        // 两者都没变或仅一方不存在时不误杀同体积强哈希文件
        self.etag.is_none() || etag.is_none()
    }
}

pub enum SegmentedOutcome {
    /// 成功落盘完成；返回新接收的字节数与使用过的候选主版本号。
    Done { received: u64 },
    /// 用户按下暂停；所有 `.part.s*` 与 `.part.meta` 已刷新落盘，等待恢复后重进。
    Paused,
    /// 下载取消。
    Cancelled,
    /// 当前 URL 不可继续或需换源。
    NextUrl(String),
    /// 可重试错误。
    Retry {
        reason: String,
        delay_override: Option<Duration>,
        connect_failed: bool,
    },
}

struct SegmentState {
    id: u32,
    start: u64,
    end: AtomicU64,
    written: AtomicU64,
    speed_bps: AtomicU64,
    last_rx_ms: AtomicU64,
    started_at: Instant,
    done: AtomicBool,
}

impl SegmentState {
    fn new(id: u32, start: u64, end: u64, written: u64, now_ms: u64) -> Self {
        Self {
            id,
            start,
            end: AtomicU64::new(end),
            written: AtomicU64::new(written),
            speed_bps: AtomicU64::new(0),
            last_rx_ms: AtomicU64::new(now_ms),
            started_at: Instant::now(),
            done: AtomicBool::new(false),
        }
    }

    fn cursor(&self) -> u64 {
        self.start
            .saturating_add(self.written.load(Ordering::Acquire))
    }

    fn remaining(&self) -> u64 {
        let end = self.end.load(Ordering::Acquire);
        let cursor = self.cursor();
        (end + 1).saturating_sub(cursor)
    }
}

struct Coordinator {
    segments: Mutex<Vec<Arc<SegmentState>>>,
    next_id: Mutex<u32>,
    started_epoch_ms: Instant,
}

impl Coordinator {
    fn new(initial: Vec<Arc<SegmentState>>, next_id: u32) -> Self {
        Self {
            segments: Mutex::new(initial),
            next_id: Mutex::new(next_id),
            started_epoch_ms: Instant::now(),
        }
    }

    fn now_rel_ms(&self) -> u64 {
        u64::try_from(self.started_epoch_ms.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 按 ETA 工作窃取：选剩余 ETA 最大的慢活跃段，在 4 KiB 边界速度加权切分。
    fn try_steal(&self, thief_speed_bps: u64) -> Option<(Arc<SegmentState>, bool)> {
        let now = Instant::now();
        let now_ms = self.now_rel_ms();
        let mut guard = self.segments.lock().unwrap_or_else(|e| e.into_inner());
        let mut best: Option<(usize, f64, bool)> = None;

        for (idx, seg) in guard.iter().enumerate() {
            if seg.done.load(Ordering::Acquire) {
                continue;
            }
            let written = seg.written.load(Ordering::Acquire);
            if now.saturating_duration_since(seg.started_at) < Duration::from_millis(1500)
                && written == 0
            {
                continue;
            }
            let rem = seg.remaining();
            let last_rx = seg.last_rx_ms.load(Ordering::Acquire);
            let stalled = now_ms.saturating_sub(last_rx) > 1000 && written > 0;
            if stalled {
                if rem < MIN_SPLIT_CHUNK {
                    continue;
                }
            } else if rem < MIN_VICTIM_REMAINING {
                continue;
            }
            let v_bps = if stalled {
                32 * 1024
            } else {
                seg.speed_bps.load(Ordering::Acquire).max(32 * 1024)
            };
            let eta = rem as f64 / v_bps as f64;
            if !stalled && eta < 1.0 {
                continue;
            }
            if best.is_none_or(|(_, best_eta, _)| eta > best_eta) {
                best = Some((idx, eta, stalled));
            }
        }

        let (victim_idx, _, stalled) = best?;
        let victim = guard[victim_idx].clone();
        let cursor = victim.cursor();
        let old_end = victim.end.load(Ordering::Acquire);
        let rem = (old_end + 1).saturating_sub(cursor);
        if rem < MIN_VICTIM_REMAINING {
            return None;
        }
        let v_victim = victim.speed_bps.load(Ordering::Acquire).max(32 * 1024) as f64;
        let v_thief = thief_speed_bps.max(32 * 1024) as f64;
        let ratio = v_victim / (v_victim + v_thief);
        let raw_victim_keep =
            (rem as f64 * ratio).clamp(MIN_SPLIT_CHUNK as f64, (rem - MIN_SPLIT_CHUNK) as f64);
        let mut split = align_4k_down(cursor + raw_victim_keep as u64);
        let min_split = cursor + MIN_SPLIT_CHUNK;
        let max_split = old_end + 1 - MIN_SPLIT_CHUNK;
        if split < min_split {
            split = min_split;
        }
        if split > max_split {
            split = max_split;
        }
        if split <= cursor || old_end < split {
            return None;
        }

        victim.end.store(split - 1, Ordering::Release);
        let seg_id = {
            let mut id_guard = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            let id = *id_guard;
            *id_guard += 1;
            id
        };
        let new_seg = Arc::new(SegmentState::new(seg_id, split, old_end, 0, now_ms));
        guard.push(new_seg.clone());
        Some((new_seg, stalled))
    }

    fn snapshot_meta(&self, total_size: u64, etag: Option<String>, lm: Option<String>) -> PartMeta {
        let guard = self.segments.lock().unwrap_or_else(|e| e.into_inner());
        let segments = guard
            .iter()
            .filter_map(|s| {
                let end = s.end.load(Ordering::Acquire);
                let written = s.written.load(Ordering::Acquire).min(end + 1 - s.start);
                if end < s.start {
                    return None;
                }
                Some(SavedSegment {
                    id: s.id,
                    start: s.start,
                    end,
                    written,
                })
            })
            .collect();
        PartMeta {
            total_size,
            etag,
            last_modified: lm,
            segments,
        }
    }

    fn total_remaining(&self) -> u64 {
        let guard = self.segments.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().map(|s| s.remaining()).sum()
    }

    fn max_segment_remaining(&self) -> u64 {
        let guard = self.segments.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().map(|s| s.remaining()).max().unwrap_or(0)
    }

    fn segment_speed(&self, seg_id: u32) -> u64 {
        let guard = self.segments.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .find(|s| s.id == seg_id)
            .map_or(0, |s| s.speed_bps.load(Ordering::Acquire))
    }

    fn active_count(&self) -> usize {
        let guard = self.segments.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .filter(|s| !s.done.load(Ordering::Acquire) && s.remaining() > 0)
            .count()
    }
}

/// 清理 `target` 所属的 `.part.s*` 与 `.part.meta` 碎片。
pub async fn clean_segment_artifacts(target: &Path, cancel: &CancellationToken) {
    let meta = part_meta_path(target);
    let _ = remove_with_win_retry(&meta, cancel).await;
    if let (Some(parent), Some(fname)) = (target.parent(), target.file_name()) {
        let prefix = format!("{}.part.s", fname.to_string_lossy());
        if let Ok(mut rd) = tokio::fs::read_dir(parent).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = remove_with_win_retry(&entry.path(), cancel).await;
                }
            }
        }
    }
}

/// 检查 `.part` / `.part.meta` 是否可续传（存在任意碎片文件）。
pub fn has_resumable_artifacts(target: &Path) -> bool {
    part_meta_path(target).is_file() || part_seg_path(target, 0).is_file()
}

/// 清理超过指定存活时间的孤儿 `.part` / `.part.s*` / `.part.meta` 文件（供启动延迟 GC 与存储清理面板复用）。
pub fn clean_orphan_part_files(root: &Path, min_age: Duration) -> (u64, u64) {
    process_orphan_parts(root, min_age, true)
}

/// 扫描超过指定存活时间的孤儿 `.part` / `.part.s*` / `.part.meta` 占用字节与数量。
pub fn scan_orphan_part_files(root: &Path, min_age: Duration) -> (u64, u64) {
    process_orphan_parts(root, min_age, false)
}

fn process_orphan_parts(root: &Path, min_age: Duration, delete: bool) -> (u64, u64) {
    const SUBDIRS: &[&str] = &[
        ".mcl-tmp",
        ".mcl-store",
        "libraries",
        "assets",
        "versions",
        "instances",
    ];
    let mut bytes = 0u64;
    let mut files = 0u64;

    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if depth == 0 {
                    let name = entry.file_name();
                    if !SUBDIRS.iter().any(|s| name.to_string_lossy() == *s) {
                        continue;
                    }
                }
                if depth < 8 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_orphan_part_name(&name) {
                continue;
            }
            let old_enough = if min_age.is_zero() {
                true
            } else {
                meta.modified()
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age >= min_age)
            };
            if !old_enough {
                continue;
            }
            let len = meta.len();
            if delete {
                if remove_with_win_retry_sync(&p).is_ok() {
                    bytes = bytes.saturating_add(len);
                    files = files.saturating_add(1);
                }
            } else {
                bytes = bytes.saturating_add(len);
                files = files.saturating_add(1);
            }
        }
    }
    (bytes, files)
}

fn is_orphan_part_name(name: &str) -> bool {
    if name.ends_with(".part") || name.ends_with(".part.meta") {
        return true;
    }
    if let Some((_, suffix)) = name.rsplit_once(".part.s") {
        return !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// 执行 Zero-HEAD 分段下载并自动在 `200 OK` 时无缝退化为单流 + 活体哈希。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_segmented(
    url: &str,
    item: &DownloadItem,
    engine: &Arc<DownloadEngine>,
    priority: DownloadPriority,
    host_header: Option<&HostHeader>,
    cancel: &CancellationToken,
    pause: Option<&PauseControl>,
    credit: &mut Credit<'_>,
    counters: &Arc<Counters>,
    #[cfg(feature = "community-mirrors")] is_bmclapi_like: bool,
    #[cfg(feature = "community-mirrors")] out_node_key: &mut Option<String>,
) -> SegmentedOutcome {
    let total_expected = item.size.unwrap_or(0);
    if total_expected < MIN_SEGMENT_FILE_SIZE {
        return SegmentedOutcome::Retry {
            reason: "文件体积不足分段阈值".to_owned(),
            delay_override: None,
            connect_failed: false,
        };
    }

    if let Some(parent) = item.path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        return SegmentedOutcome::Retry {
            reason: format!("无法创建目录：{e}"),
            delay_override: None,
            connect_failed: false,
        };
    }

    let meta_path = part_meta_path(&item.path);
    let saved_meta = load_valid_saved_meta(&meta_path, &item.path, total_expected).await;
    let resuming = saved_meta.is_some();

    let mut initial_segments = if let Some(ref meta) = saved_meta {
        meta.segments.clone()
    } else {
        clean_segment_artifacts(&item.path, cancel).await;
        vec![SavedSegment {
            id: 0,
            start: 0,
            end: total_expected - 1,
            written: 0,
        }]
    };

    if initial_segments.is_empty() {
        initial_segments.push(SavedSegment {
            id: 0,
            start: 0,
            end: total_expected - 1,
            written: 0,
        });
    }

    // 选择用于 Zero-HEAD 首包探测的未完成段（优先 seg_0；若续传中 seg_0 已满则挑首条未满段）
    let probe_idx = initial_segments
        .iter()
        .position(|s| s.start + s.written <= s.end)
        .unwrap_or(0);
    let probe_saved = initial_segments[probe_idx].clone();

    if let Some(p) = pause
        && p.is_paused()
    {
        return SegmentedOutcome::Paused;
    }

    let net_permit = match engine.acquire_net(priority, cancel).await {
        Some(p) => p,
        None => return SegmentedOutcome::Cancelled,
    };

    let bundle = engine.bundle();
    let probe_cursor = probe_saved.start + probe_saved.written;
    let probe_range = if resuming && (probe_saved.id != 0 || probe_cursor > 0) {
        format!("bytes={probe_cursor}-{}", probe_saved.end)
    } else {
        "bytes=0-".to_owned()
    };

    let req_host = url_host(url);
    #[cfg(feature = "community-mirrors")]
    let probe_fut = async {
        if is_bmclapi_like {
            super::mirror::send_following_safe_redirects_with_mirror(
                &bundle.h1_segmented,
                url,
                Some(&probe_range),
                host_header,
                true,
                engine.mirror_router(),
            )
            .await
        } else {
            send_following_safe_redirects(
                &bundle.h1_segmented,
                url,
                Some(&probe_range),
                host_header,
            )
            .await
            .map(|(resp, final_url)| {
                let nk = super::mirror::node_key_from_url_str(&final_url);
                (resp, final_url, nk)
            })
        }
    };
    #[cfg(not(feature = "community-mirrors"))]
    let probe_fut = async {
        send_following_safe_redirects(&bundle.h1_segmented, url, Some(&probe_range), host_header)
            .await
            .map(|(resp, final_url)| (resp, final_url, Option::<String>::None))
    };

    let (response, final_url, probe_node_key) = match tokio::select! {
        res = probe_fut => res,
        () = async {
            if let Some(p) = pause {
                p.paused_signal().await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            drop(net_permit);
            return SegmentedOutcome::Paused;
        }
        () = cancel.cancelled() => return SegmentedOutcome::Cancelled,
    } {
        Ok(ok) => ok,
        Err(RedirectSendError::Protocol(msg)) => return SegmentedOutcome::NextUrl(msg),
        Err(RedirectSendError::Request {
            url: failed_url,
            source,
        }) => {
            #[cfg(feature = "community-mirrors")]
            {
                *out_node_key = super::mirror::node_key_from_url_str(&failed_url);
            }
            drop(net_permit);
            return handle_connect_error(engine, req_host.as_deref(), &failed_url, &source);
        }
    };
    #[cfg(feature = "community-mirrors")]
    {
        *out_node_key = probe_node_key;
    }
    #[cfg(not(feature = "community-mirrors"))]
    let _ = probe_node_key;

    if let Some(h) = url_host(&final_url).or(req_host.clone()) {
        engine.doh().report_connect_ok(&h);
    }

    let status = response.status();
    let authorized = host_header.filter(|h| h.applies_to(url));

    if status == StatusCode::TOO_MANY_REQUESTS {
        let wait = parse_retry_after(response.headers());
        engine.telemetry().push_event(
            TelemetryEventKind::RateLimitCooldown,
            "触发下载源 429 限流，等待退避恢复",
        );
        return SegmentedOutcome::Retry {
            reason: format!("服务器返回 {status}"),
            delay_override: Some(wait),
            connect_failed: false,
        };
    }

    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        clean_segment_artifacts(&item.path, cancel).await;
        return SegmentedOutcome::Retry {
            reason: "断点位置失效，重置从头下载".to_owned(),
            delay_override: Some(Duration::from_millis(50)),
            connect_failed: false,
        };
    }

    if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return SegmentedOutcome::Retry {
                reason: format!("服务器返回 {status}"),
                delay_override: None,
                connect_failed: false,
            };
        }
        if authorized.is_some()
            && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        {
            return SegmentedOutcome::NextUrl(format!("{DENIED_PREFIX}（服务器返回 {status}）"));
        }
        return SegmentedOutcome::NextUrl(format!("服务器返回 {status}"));
    }

    // HTML 劫持头防线
    if is_binary_target(&item.path)
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
        return SegmentedOutcome::NextUrl(format!(
            "{STRUCTURE_MISMATCH}下载源返回了 HTML 网页而非目标文件"
        ));
    }

    // 200 OK：服务端不支持 Range -> 直接顺流写完 .part.s0 + 活体 SHA-1（0 RTT 浪费）
    if status == StatusCode::OK {
        clean_segment_artifacts(&item.path, cancel).await;
        return finish_unranged_200_stream(
            response, item, engine, cancel, pause, credit, counters, net_permit,
        )
        .await;
    }

    // 206 Partial Content：核对 Content-Range 总长与 ETag / Last-Modified
    let etag = header_str(response.headers(), header::ETAG);
    let last_modified = header_str(response.headers(), header::LAST_MODIFIED);

    if let Some((_, _, Some(server_total))) = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range)
        && server_total != total_expected
    {
        return SegmentedOutcome::NextUrl(format!(
            "{SIZE_MISMATCH}期望 {total_expected} 字节，下载源返回总长 {server_total} 字节"
        ));
    }

    if let Some(ref meta) = saved_meta
        && !meta.validator_matches(etag.as_deref(), last_modified.as_deref())
    {
        clean_segment_artifacts(&item.path, cancel).await;
        return SegmentedOutcome::Retry {
            reason: "远端文件已更新，清理旧分片从头下载".to_owned(),
            delay_override: Some(Duration::from_millis(50)),
            connect_failed: false,
        };
    }

    // 已恢复写入字节与 Credit 差值对齐（零回弹，绝不双计）
    let already_written: u64 = initial_segments.iter().map(|s| s.written).sum();
    credit.sync_to(already_written);

    // 构造分段协调器
    let coord_states: Vec<Arc<SegmentState>> = initial_segments
        .iter()
        .map(|s| {
            let state = Arc::new(SegmentState::new(s.id, s.start, s.end, s.written, 0));
            if s.start + s.written > s.end {
                state.done.store(true, Ordering::Release);
            }
            state
        })
        .collect();
    let max_id = coord_states.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    let coord = Arc::new(Coordinator::new(coord_states, max_id));

    let seg0_live_hash_valid = Arc::new(AtomicBool::new(!resuming && probe_saved.id == 0));
    let seg0_live_hasher: Arc<Mutex<Option<Sha1>>> = Arc::new(Mutex::new(None));

    // 判断通道能力与指纹热启动参数
    let cfg = engine.settings_snapshot();
    let proxied = engine.is_proxied_env();
    let (gamma, k_cap, rungs) = if proxied {
        let cap = 4u8.min(cfg.max_segments_per_file).max(1);
        (1.25f64, cap, PROXIED_RUNGS)
    } else {
        let mode_cap = match cfg.segment_mode {
            crate::config::SegmentMode::Auto => 16u8,
            crate::config::SegmentMode::Aggressive => cfg.max_segments_per_file.clamp(4, 32),
            crate::config::SegmentMode::Off => 1u8,
        };
        (1.10f64, mode_cap.max(1), DIRECT_RUNGS)
    };
    let net_cap = engine.max_net_permits().min(u32::from(k_cap)) as u8;
    let k_cap = k_cap.min(net_cap.max(1));

    let metered = engine.is_metered();
    let proxy_tag = classify_proxy_tag(
        &cfg.proxy,
        &engine.proxy_watcher().snapshot(),
        engine.doh().is_tun_active(),
    );
    let sig = compute_network_signature(proxy_tag, metered);
    let host_for_profile = url_host(&final_url)
        .or(req_host.clone())
        .unwrap_or_default();
    let cached_profile = if !resuming && !host_for_profile.is_empty() {
        engine.profiles().get_host(&sig, &host_for_profile)
    } else {
        None
    };

    // 首包确认 206 即并发起步（直连保底 4 段 / 代理保底 2 段，记忆历史更大最优档直接热启动）
    if !resuming && k_cap > 1 {
        let base_init = if proxied { 2u8 } else { 4u8 };
        let desired = cached_profile
            .as_ref()
            .map_or(base_init, |p| p.k_opt.max(base_init));
        let max_by_size = u8::try_from(total_expected / MIN_VICTIM_REMAINING).unwrap_or(k_cap);
        let k_init = desired.min(k_cap).min(max_by_size).max(1);
        if k_init > 1 {
            let seg0 = coord.segments.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
            split_segment_evenly(&coord, &seg0, k_init as usize);
        }
    }

    // 为所有次级分段启动并发 worker
    let seg_cancel = cancel.child_token();
    let mut handles = Vec::new();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(u32, Result<u64, WorkerErr>)>();

    let newly_received = Arc::new(AtomicU64::new(0));
    let has_failed_fatal: Arc<Mutex<Option<WorkerErr>>> = Arc::new(Mutex::new(None));

    // 启动首段（复用已建连的 HTTP Response，零浪费首包）
    {
        let first_seg = {
            let guard = coord.segments.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .iter()
                .find(|s| s.id == probe_saved.id)
                .cloned()
                .unwrap_or_else(|| guard[0].clone())
        };
        handles.push(spawn_existing_segment_worker(
            first_seg,
            response,
            net_permit,
            item.path.clone(),
            item.sha1.is_some(),
            seg0_live_hash_valid.clone(),
            seg0_live_hasher.clone(),
            coord.clone(),
            engine.clone_metrics_ctx(),
            counters.clone(),
            newly_received.clone(),
            final_url.clone(),
            host_header.cloned(),
            seg_cancel.clone(),
            pause.cloned(),
            done_tx.clone(),
            proxied,
            priority,
        ));
    }

    // 续传或 WarmBurst 产生的其余段立即拉起并发 worker
    {
        let extra: Vec<Arc<SegmentState>> = {
            let guard = coord.segments.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .iter()
                .filter(|s| s.id != probe_saved.id && !s.done.load(Ordering::Acquire))
                .cloned()
                .collect()
        };
        for seg in extra {
            handles.push(spawn_new_segment_worker(
                seg,
                false,
                item.path.clone(),
                item.sha1.is_some(),
                seg0_live_hash_valid.clone(),
                seg0_live_hasher.clone(),
                coord.clone(),
                engine.clone_metrics_ctx(),
                counters.clone(),
                newly_received.clone(),
                final_url.clone(),
                host_header.cloned(),
                seg_cancel.clone(),
                pause.cloned(),
                done_tx.clone(),
                priority,
                etag.clone(),
                last_modified.clone(),
                total_expected,
            ));
        }
    }

    // Net-Gain Ramp 爬坡控制循环 + 等待全部段结束或 Pause/Cancel
    let mut ramp_k = coord.active_count().max(1) as u8;
    let mut steady = ramp_k >= k_cap || k_cap == 1;
    let mut prev_rx = newly_received.load(Ordering::Relaxed);
    let mut prev_bps = 0.0f64;
    let mut peak_bps = 0.0f64;
    let mut warmup_done = false;
    let mut epoch_seen = engine.proxy_watcher().epoch();
    let mut k_opt_recorded = ramp_k;

    let mut ramp_tick = tokio::time::interval(RAMP_INTERVAL);
    ramp_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ramp_tick.tick().await; // consume instant tick 0

    let mut active_workers = handles.len();
    let mut paused_triggered = false;
    let mut final_err: Option<WorkerErr> = None;
    let mut steal_events_emitted = 0u8;

    let warmup_sleep = tokio::time::sleep(WARMUP_SKIP);
    tokio::pin!(warmup_sleep);

    loop {
        if active_workers == 0 {
            break;
        }
        tokio::select! {
            () = cancel.cancelled() => {
                seg_cancel.cancel();
                break;
            }
            () = async {
                if let Some(p) = pause {
                    p.paused_signal().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                paused_triggered = true;
                seg_cancel.cancel();
                break;
            }
            () = &mut warmup_sleep, if !warmup_done => {
                warmup_done = true;
                prev_rx = newly_received.load(Ordering::Relaxed);
            }
            _ = ramp_tick.tick(), if warmup_done => {
                let cur_epoch = engine.proxy_watcher().epoch();
                if cur_epoch != epoch_seen {
                    epoch_seen = cur_epoch;
                    prev_bps = 0.0;
                    prev_rx = newly_received.load(Ordering::Relaxed);
                    steady = false;
                    continue;
                }
                let cur_rx = newly_received.load(Ordering::Relaxed);
                let delta = cur_rx.saturating_sub(prev_rx);
                prev_rx = cur_rx;
                let v_n = delta as f64 / RAMP_INTERVAL.as_secs_f64();
                if v_n > peak_bps {
                    peak_bps = v_n;
                }
                if delta > 0 && !host_for_profile.is_empty() {
                    let hist_peak = cached_profile.as_ref().map(|p| p.peak_bps);
                    let outcome = engine.edge_optimizer().evaluate_active_pop_window(
                        &host_for_profile,
                        v_n,
                        Instant::now(),
                        engine.doh(),
                        engine.telemetry(),
                    );
                    if matches!(outcome, super::edge_opt::PopWindowOutcome::NoActiveSelection) {
                        engine.edge_optimizer().maybe_spawn_probe(
                            engine,
                            &host_for_profile,
                            &final_url,
                            v_n,
                            hist_peak,
                        );
                    }
                }

                if !steady {
                    if prev_bps <= 1.0 {
                        // 首周期基线直接尝试扩到 2 档探测（仅当剩余够大且不违背尾段保护）
                        prev_bps = v_n;
                        let rem_total = coord.total_remaining();
                        let eta_file = rem_total as f64 / v_n.max(1.0);
                        if ramp_k == 1
                            && k_cap >= 2
                            && eta_file >= 1.5
                            && coord.max_segment_remaining() >= MIN_VICTIM_REMAINING
                            && engine.disk_ewma_ms() < 50.0
                            && let Some((stolen, use_spare)) = coord.try_steal(v_n as u64)
                        {
                            ramp_k = 2;
                            k_opt_recorded = 2;
                            active_workers += 1;
                            engine.telemetry().push_event(
                                TelemetryEventKind::RampUp,
                                "并发爬坡试探 2 条分段连接",
                            );
                            handles.push(spawn_new_segment_worker(
                                stolen,
                                use_spare,
                                item.path.clone(),
                                item.sha1.is_some(),
                                seg0_live_hash_valid.clone(),
                                seg0_live_hasher.clone(),
                                coord.clone(),
                                engine.clone_metrics_ctx(),
                                counters.clone(),
                                newly_received.clone(),
                                final_url.clone(),
                                host_header.cloned(),
                                seg_cancel.clone(),
                                pause.cloned(),
                                done_tx.clone(),
                                priority,
                                etag.clone(),
                                last_modified.clone(),
                                total_expected,
                            ));
                        }
                    } else {
                        let ratio = v_n / prev_bps.max(1.0);
                        if ratio < 0.92 || engine.disk_ewma_ms() >= 50.0 {
                            steady = true;
                            k_opt_recorded = previous_rung(rungs, ramp_k);
                            engine.telemetry().push_event(
                                TelemetryEventKind::RampRollback,
                                format!("并发增益不足，稳态锁定 {k_opt_recorded} 条连接"),
                            );
                        } else if ratio < gamma {
                            steady = true;
                            k_opt_recorded = if ratio <= 1.03 {
                                previous_rung(rungs, ramp_k)
                            } else {
                                ramp_k
                            };
                        } else if ramp_k < k_cap && engine.disk_ewma_ms() < 50.0 {
                            let rem_total = coord.total_remaining();
                            let eta_file = rem_total as f64 / v_n.max(1.0);
                            if eta_file >= 1.5 && coord.max_segment_remaining() >= MIN_VICTIM_REMAINING {
                                let target_k = next_rung(rungs, ramp_k).min(k_cap);
                                let mut spawned = 0u8;
                                while ramp_k < target_k {
                                    if let Some((stolen, use_spare)) = coord.try_steal((v_n / f64::from(ramp_k.max(1))) as u64) {
                                        ramp_k += 1;
                                        spawned += 1;
                                        active_workers += 1;
                                        handles.push(spawn_new_segment_worker(
                                            stolen,
                                            use_spare,
                                            item.path.clone(),
                                            item.sha1.is_some(),
                                            seg0_live_hash_valid.clone(),
                                            seg0_live_hasher.clone(),
                                            coord.clone(),
                                            engine.clone_metrics_ctx(),
                                            counters.clone(),
                                            newly_received.clone(),
                                            final_url.clone(),
                                            host_header.cloned(),
                                            seg_cancel.clone(),
                                            pause.cloned(),
                                            done_tx.clone(),
                                            priority,
                                            etag.clone(),
                                            last_modified.clone(),
                                            total_expected,
                                        ));
                                    } else {
                                        break;
                                    }
                                }
                                if spawned > 0 {
                                    k_opt_recorded = ramp_k;
                                    engine.telemetry().push_event(
                                        TelemetryEventKind::RampUp,
                                        format!("净增益达标，并发爬坡至 {ramp_k} 条分段连接"),
                                    );
                                }
                            } else {
                                steady = true;
                                k_opt_recorded = ramp_k;
                            }
                        } else {
                            steady = true;
                            k_opt_recorded = ramp_k;
                        }
                        prev_bps = v_n;
                    }
                }
            }
            res = done_rx.recv() => {
                match res {
                    Some((finished_seg_id, Ok(worker_rx))) => {
                        active_workers = active_workers.saturating_sub(1);
                        let _ = worker_rx;
                        // 完工触发即时 ETA 工作窃取：快连接立刻接管慢段尾部，防止单尾掉速
                        if active_workers > 0
                            && !paused_triggered
                            && !cancel.is_cancelled()
                            && engine.disk_ewma_ms() < 50.0
                        {
                            let fallback_bps = (prev_bps / f64::from(ramp_k.max(1))) as u64;
                            let thief_bps = coord
                                .segment_speed(finished_seg_id)
                                .max(fallback_bps)
                                .max(128 * 1024);
                            if let Some((stolen, use_spare)) = coord.try_steal(thief_bps) {
                                active_workers += 1;
                                handles.push(spawn_new_segment_worker(
                                    stolen,
                                    use_spare,
                                    item.path.clone(),
                                    item.sha1.is_some(),
                                    seg0_live_hash_valid.clone(),
                                    seg0_live_hasher.clone(),
                                    coord.clone(),
                                    engine.clone_metrics_ctx(),
                                    counters.clone(),
                                    newly_received.clone(),
                                    final_url.clone(),
                                    host_header.cloned(),
                                    seg_cancel.clone(),
                                    pause.cloned(),
                                    done_tx.clone(),
                                    priority,
                                    etag.clone(),
                                    last_modified.clone(),
                                    total_expected,
                                ));
                                engine.telemetry().record_work_steal();
                                if steal_events_emitted < 2 {
                                    steal_events_emitted += 1;
                                    engine.telemetry().push_event(
                                        TelemetryEventKind::WorkStolen,
                                        "快连接已接管慢分段尾部，保持满速收尾",
                                    );
                                }
                            }
                        }
                    }
                    Some((_seg_id, Err(err))) => {
                        active_workers = active_workers.saturating_sub(1);
                        if matches!(err, WorkerErr::Paused) {
                            paused_triggered = true;
                            seg_cancel.cancel();
                            break;
                        }
                        if final_err.is_none() {
                            final_err = Some(err);
                            seg_cancel.cancel();
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    for h in handles {
        let _ = h.await;
    }

    let rx_sum = newly_received.load(Ordering::Relaxed);
    credit.account_external(rx_sum);
    let _ = has_failed_fatal;

    if cancel.is_cancelled() && !paused_triggered {
        // 取消时保存已下载分片进度供断点续传或清理
        let meta = coord.snapshot_meta(total_expected, etag, last_modified);
        let _ = write_json_atomic(&meta_path, &meta);
        return SegmentedOutcome::Cancelled;
    }

    if paused_triggered || pause.is_some_and(PauseControl::is_paused) {
        let meta = coord.snapshot_meta(total_expected, etag, last_modified);
        let _ = write_json_atomic(&meta_path, &meta);
        return SegmentedOutcome::Paused;
    }

    if let Some(err) = final_err {
        let meta = coord.snapshot_meta(total_expected, etag, last_modified);
        let _ = write_json_atomic(&meta_path, &meta);
        return match err {
            WorkerErr::NextUrl(msg) => {
                clean_segment_artifacts(&item.path, cancel).await;
                SegmentedOutcome::NextUrl(msg)
            }
            WorkerErr::Retry {
                reason,
                delay_override,
                connect_failed,
            } => SegmentedOutcome::Retry {
                reason,
                delay_override,
                connect_failed,
            },
            WorkerErr::Paused => SegmentedOutcome::Paused,
            WorkerErr::Cancelled => SegmentedOutcome::Cancelled,
        };
    }

    // 校验所有分段是否严丝合缝覆盖 [0..total_expected)
    let snapshot_segs = coord
        .snapshot_meta(total_expected, etag.clone(), last_modified.clone())
        .segments;
    if !verify_contiguous_coverage(&snapshot_segs, total_expected) {
        let meta = coord.snapshot_meta(total_expected, etag, last_modified);
        let _ = write_json_atomic(&meta_path, &meta);
        return SegmentedOutcome::Retry {
            reason: "分片区间尚未写满，准备继续补齐".to_owned(),
            delay_override: Some(Duration::from_millis(80)),
            connect_failed: false,
        };
    }

    // 仅在多段并发达到 >=2 且跑出峰值速率时写入指纹，避免小文件单段结束误锁 7 天并发为 1
    if !host_for_profile.is_empty() && k_opt_recorded >= 2 && peak_bps > 0.0 {
        engine
            .profiles()
            .record_host(&sig, &host_for_profile, k_opt_recorded, peak_bps);
    }

    // Zero-set_len Segment-0 Live-Hash Relay Single-Pass Merge
    let path_lock = engine.lock_for_path(&item.path);
    let _path_guard = path_lock.lock().await;
    let Some(_disk_permit) = engine.acquire_disk(cancel).await else {
        let meta = coord.snapshot_meta(total_expected, etag, last_modified);
        let _ = write_json_atomic(&meta_path, &meta);
        return SegmentedOutcome::Cancelled;
    };
    let mut disk_guard = engine.telemetry().begin_disk_write();
    disk_guard.mark_active();

    let live_hasher = if seg0_live_hash_valid.load(Ordering::Acquire) {
        seg0_live_hasher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    } else {
        None
    };

    let target_clone = item.path.clone();
    let expected_sha1 = item.sha1.clone();
    let merge_started = Instant::now();
    let merge_res = tokio::task::spawn_blocking(move || {
        merge_segments_relay_hash_sync(
            &target_clone,
            &snapshot_segs,
            total_expected,
            expected_sha1.as_deref(),
            live_hasher,
        )
    })
    .await
    .map_err(|e| WorkerErr::Retry {
        reason: format!("合并分片任务失败：{e}"),
        delay_override: None,
        connect_failed: false,
    });
    engine.record_disk_write_ms(
        merge_started.elapsed().as_secs_f64() * 1000.0,
        total_expected,
    );

    match merge_res {
        Ok(Ok(())) => {
            if let Some(sha1) = &item.sha1 {
                engine
                    .hydrator()
                    .record_file(&item.path, sha1, total_expected);
            }
            SegmentedOutcome::Done { received: rx_sum }
        }
        Ok(Err(e)) => {
            clean_segment_artifacts(&item.path, cancel).await;
            match e {
                MergeError::Mismatch(msg) => SegmentedOutcome::Retry {
                    reason: msg,
                    delay_override: None,
                    connect_failed: false,
                },
                MergeError::Io(io_err) => {
                    let diag = classify_io_error(&io_err);
                    engine.telemetry().set_diagnosis(Some(diag.clone()));
                    SegmentedOutcome::Retry {
                        reason: format!("{}（{io_err}）", diag.summary),
                        delay_override: None,
                        connect_failed: false,
                    }
                }
            }
        }
        Err(worker_err) => match worker_err {
            WorkerErr::Retry {
                reason,
                delay_override,
                connect_failed,
            } => SegmentedOutcome::Retry {
                reason,
                delay_override,
                connect_failed,
            },
            _ => SegmentedOutcome::Cancelled,
        },
    }
}

/// 将所有分片单趟追加进 `.part.s0`，仅扫描每个字节一次完成活体 SHA-1 校验并原子改名。
fn merge_segments_relay_hash_sync(
    target: &Path,
    segments: &[SavedSegment],
    total_size: u64,
    expected_sha1: Option<&str>,
    seg0_live_hasher: Option<Sha1>,
) -> Result<(), MergeError> {
    let mut sorted = segments.to_vec();
    sorted.sort_by_key(|s| s.start);

    let seg0_id = sorted.first().map_or(0, |s| s.id);
    let seg0_path = part_seg_path(target, seg0_id);

    let mut hasher = if expected_sha1.is_some() {
        if let Some(h) = seg0_live_hasher {
            Some(h)
        } else {
            let mut h = Sha1::new();
            let mut f0 = File::open(&seg0_path).map_err(MergeError::Io)?;
            let mut buf = vec![0u8; WRITE_BUF_BYTES];
            loop {
                let n = f0.read(&mut buf).map_err(MergeError::Io)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Some(h)
        }
    } else {
        None
    };

    if sorted.len() > 1 {
        let f0_append = OpenOptions::new()
            .append(true)
            .open(&seg0_path)
            .map_err(MergeError::Io)?;
        let mut writer = SyncBufWriter::with_capacity(WRITE_BUF_BYTES, f0_append);
        let mut buf = vec![0u8; WRITE_BUF_BYTES];

        for seg in &sorted[1..] {
            let seg_path = part_seg_path(target, seg.id);
            let mut src = File::open(&seg_path).map_err(MergeError::Io)?;
            let mut copied = 0u64;
            let need = seg.end + 1 - seg.start;
            while copied < need {
                let want =
                    usize::try_from((need - copied).min(buf.len() as u64)).unwrap_or(buf.len());
                let n = src.read(&mut buf[..want]).map_err(MergeError::Io)?;
                if n == 0 {
                    break;
                }
                if let Some(h) = hasher.as_mut() {
                    h.update(&buf[..n]);
                }
                writer.write_all(&buf[..n]).map_err(MergeError::Io)?;
                copied += n as u64;
            }
            drop(src);
            if copied != need {
                return Err(MergeError::Mismatch(format!(
                    "{SIZE_MISMATCH}分片 {} 实际 {} 字节，期望 {need} 字节",
                    seg.id, copied
                )));
            }
            let _ = remove_with_win_retry_sync(&seg_path);
        }
        writer.flush().map_err(MergeError::Io)?;
    }

    let final_len = fs::metadata(&seg0_path).map_err(MergeError::Io)?.len();
    if final_len != total_size {
        return Err(MergeError::Mismatch(format!(
            "{SIZE_MISMATCH}应为 {total_size} 字节，实际 {final_len} 字节"
        )));
    }

    if let (Some(expected), Some(h)) = (expected_sha1, hasher) {
        let actual = to_hex(&h.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(MergeError::Mismatch(format!(
                "{SHA1_MISMATCH}应为 {expected}，实际 {actual}"
            )));
        }
    } else if expected_sha1.is_none() {
        verify_structure_sync(&seg0_path, target, final_len).map_err(MergeError::Mismatch)?;
    }

    rename_with_win_retry_sync(&seg0_path, target).map_err(MergeError::Io)?;
    let _ = remove_with_win_retry_sync(&part_meta_path(target));
    Ok(())
}

fn verify_structure_sync(path: &Path, target: &Path, len: u64) -> Result<(), String> {
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(
        ext.as_deref(),
        Some("jar" | "zip" | "mrpack" | "json" | "xml")
    ) {
        return Ok(());
    }
    if matches!(ext.as_deref(), Some("jar" | "zip" | "mrpack")) && len > 8 * 1024 * 1024 {
        let mut f = File::open(path).map_err(|e| format!("读取失败：{e}"))?;
        let mut head = [0u8; 4];
        f.read_exact(&mut head)
            .map_err(|e| format!("读取头失败：{e}"))?;
        if &head != b"PK\x03\x04" {
            return Err(format!("{STRUCTURE_MISMATCH}缺少 ZIP PK 文件头"));
        }
        use std::io::Seek;
        let tail_len = usize::try_from(len.min(65_557)).unwrap_or(65_557);
        f.seek(io::SeekFrom::End(-(tail_len as i64)))
            .map_err(|e| format!("读取尾失败：{e}"))?;
        let mut tail = vec![0u8; tail_len];
        f.read_exact(&mut tail)
            .map_err(|e| format!("读取尾失败：{e}"))?;
        let sig = [0x50u8, 0x4b, 0x05, 0x06];
        if !tail.windows(4).any(|w| w == sig) {
            return Err(format!("{STRUCTURE_MISMATCH}缺少 ZIP 结束目录标记（EOCD）"));
        }
        return Ok(());
    }
    if len <= 8 * 1024 * 1024 {
        let bytes = fs::read(path).map_err(|e| format!("读取失败：{e}"))?;
        return verify_structure_bytes(&bytes, target);
    }
    Ok(())
}

#[derive(Debug)]
enum MergeError {
    Mismatch(String),
    Io(io::Error),
}

#[derive(Clone)]
struct EngineMetricsCtx {
    bundle: super::http::ClientBundle,
    throttle: Arc<TokenBucket>,
    telemetry: Arc<TelemetryHub>,
    net_total: Arc<tokio::sync::Semaphore>,
    net_batch: Arc<tokio::sync::Semaphore>,
    doh: Arc<Doh>,
    proxy_mode: crate::config::ProxyMode,
}

impl EngineMetricsCtx {
    async fn acquire_net(
        &self,
        priority: DownloadPriority,
        cancel: &CancellationToken,
    ) -> Option<super::download::NetPermit> {
        let batch = if priority == DownloadPriority::Batch {
            tokio::select! {
                res = self.net_batch.clone().acquire_owned() => Some(res.ok()?),
                () = cancel.cancelled() => return None,
            }
        } else {
            None
        };
        let total = tokio::select! {
            res = self.net_total.clone().acquire_owned() => res.ok()?,
            () = cancel.cancelled() => return None,
        };
        Some(super::download::NetPermit::new(batch, total))
    }
}

impl DownloadEngine {
    fn clone_metrics_ctx(&self) -> EngineMetricsCtx {
        EngineMetricsCtx {
            bundle: self.bundle(),
            throttle: self.throttle().clone(),
            telemetry: self.telemetry().clone(),
            net_total: self.net_total_sem(),
            net_batch: self.net_batch_sem(),
            doh: self.doh().clone(),
            proxy_mode: self.proxy_mode(),
        }
    }
}

enum WorkerErr {
    NextUrl(String),
    Retry {
        reason: String,
        delay_override: Option<Duration>,
        connect_failed: bool,
    },
    Paused,
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
fn spawn_existing_segment_worker(
    seg: Arc<SegmentState>,
    response: Response,
    net_permit: super::download::NetPermit,
    target_path: PathBuf,
    has_sha1: bool,
    seg0_live_hash_valid: Arc<AtomicBool>,
    seg0_live_hasher: Arc<Mutex<Option<Sha1>>>,
    coord: Arc<Coordinator>,
    ctx: EngineMetricsCtx,
    counters: Arc<Counters>,
    newly_received: Arc<AtomicU64>,
    url: String,
    host_header: Option<HostHeader>,
    cancel: CancellationToken,
    pause: Option<PauseControl>,
    done_tx: mpsc::UnboundedSender<(u32, Result<u64, WorkerErr>)>,
    _proxied: bool,
    priority: DownloadPriority,
) -> tokio::task::JoinHandle<()> {
    let etag = header_str(response.headers(), header::ETAG);
    let last_modified = header_str(response.headers(), header::LAST_MODIFIED);
    let total_expected = item_total_from_coord(&coord);
    tokio::spawn(async move {
        let seg_id = seg.id;
        let mut live_hasher =
            (seg_id == 0 && has_sha1 && seg0_live_hash_valid.load(Ordering::Acquire))
                .then(Sha1::new);
        let first_res = {
            let _net_guard = net_permit;
            stream_segment_body(
                &seg,
                response,
                &target_path,
                live_hasher.as_mut(),
                &seg0_live_hash_valid,
                &ctx,
                &counters,
                &newly_received,
                &cancel,
                pause.as_ref(),
            )
            .await
        };
        if let Some(h) = live_hasher
            && seg0_live_hash_valid.load(Ordering::Acquire)
        {
            *seg0_live_hasher.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
        }
        seg.done.store(true, Ordering::Release);
        let worker_rx = match first_res {
            Ok(rx) => rx,
            Err(err) => {
                let _ = done_tx.send((seg_id, Err(err)));
                return;
            }
        };

        // 首段完成后同样参与 ETA 工作窃取，最大化吃满尾段带宽
        let thief_speed = seg.speed_bps.load(Ordering::Acquire);
        if let Some((stolen, spare_next)) = coord.try_steal(thief_speed) {
            ctx.telemetry.record_work_steal();
            ctx.telemetry.push_event(
                TelemetryEventKind::WorkStolen,
                "首段完成，主动协同分担慢连接剩余分片",
            );
            run_worker_steal_loop(
                stolen,
                spare_next,
                worker_rx,
                target_path,
                seg0_live_hash_valid,
                coord,
                ctx,
                counters,
                newly_received,
                url,
                host_header,
                cancel,
                pause,
                done_tx,
                priority,
                etag,
                last_modified,
                total_expected,
            )
            .await;
            return;
        }
        let _ = done_tx.send((seg_id, Ok(worker_rx)));
    })
}

fn item_total_from_coord(coord: &Coordinator) -> u64 {
    let guard = coord.segments.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .iter()
        .map(|s| s.end.load(Ordering::Acquire) + 1)
        .max()
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn spawn_new_segment_worker(
    initial_seg: Arc<SegmentState>,
    initial_use_spare: bool,
    target_path: PathBuf,
    _has_sha1: bool,
    seg0_live_hash_valid: Arc<AtomicBool>,
    _seg0_live_hasher: Arc<Mutex<Option<Sha1>>>,
    coord: Arc<Coordinator>,
    ctx: EngineMetricsCtx,
    counters: Arc<Counters>,
    newly_received: Arc<AtomicU64>,
    url: String,
    host_header: Option<HostHeader>,
    cancel: CancellationToken,
    pause: Option<PauseControl>,
    done_tx: mpsc::UnboundedSender<(u32, Result<u64, WorkerErr>)>,
    priority: DownloadPriority,
    expected_etag: Option<String>,
    expected_lm: Option<String>,
    total_expected: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_worker_steal_loop(
        initial_seg,
        initial_use_spare,
        0,
        target_path,
        seg0_live_hash_valid,
        coord,
        ctx,
        counters,
        newly_received,
        url,
        host_header,
        cancel,
        pause,
        done_tx,
        priority,
        expected_etag,
        expected_lm,
        total_expected,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_worker_steal_loop(
    initial_seg: Arc<SegmentState>,
    initial_use_spare: bool,
    mut worker_rx: u64,
    target_path: PathBuf,
    seg0_live_hash_valid: Arc<AtomicBool>,
    coord: Arc<Coordinator>,
    ctx: EngineMetricsCtx,
    counters: Arc<Counters>,
    newly_received: Arc<AtomicU64>,
    url: String,
    host_header: Option<HostHeader>,
    cancel: CancellationToken,
    pause: Option<PauseControl>,
    done_tx: mpsc::UnboundedSender<(u32, Result<u64, WorkerErr>)>,
    priority: DownloadPriority,
    expected_etag: Option<String>,
    expected_lm: Option<String>,
    total_expected: u64,
) {
    let mut current_seg = initial_seg;
    let mut use_spare = initial_use_spare;

    loop {
        let seg_id = current_seg.id;
        let cursor = current_seg.cursor();
        let end = current_seg.end.load(Ordering::Acquire);
        if cursor <= end {
            let Some(_net_permit) = ctx.acquire_net(priority, &cancel).await else {
                let _ = done_tx.send((seg_id, Err(WorkerErr::Cancelled)));
                return;
            };
            let client = if use_spare {
                ctx.bundle.spare.clone()
            } else {
                ctx.bundle.h1_segmented.clone()
            };
            let range_header = format!("bytes={cursor}-{end}");
            let send_res = tokio::select! {
                r = send_following_safe_redirects(&client, &url, Some(&range_header), host_header.as_ref()) => r,
                () = cancel.cancelled() => {
                    let _ = done_tx.send((seg_id, Err(WorkerErr::Cancelled)));
                    return;
                }
            };
            let (response, _) = match send_res {
                Ok(ok) => ok,
                Err(RedirectSendError::Protocol(msg)) => {
                    let _ = done_tx.send((seg_id, Err(WorkerErr::NextUrl(msg))));
                    return;
                }
                Err(RedirectSendError::Request {
                    url: failed_url,
                    source,
                }) => {
                    let h = url_host(&failed_url).or_else(|| url_host(&url));
                    if let Some(host) = h.as_deref() {
                        match ctx.doh.report_connect_failure(host, &source) {
                            TransportDnsAction::SniBlockedSwitchSource => {
                                let diag = classify_reqwest_error(
                                    &source,
                                    Some(host),
                                    Some(&ctx.doh),
                                    ctx.proxy_mode,
                                );
                                let _ =
                                    done_tx.send((seg_id, Err(WorkerErr::NextUrl(diag.summary))));
                                return;
                            }
                            TransportDnsAction::RetryOnceSingleStream
                            | TransportDnsAction::None => {}
                        }
                    }
                    let diag = classify_reqwest_error(
                        &source,
                        h.as_deref(),
                        Some(&ctx.doh),
                        ctx.proxy_mode,
                    );
                    let _ = done_tx.send((
                        seg_id,
                        Err(WorkerErr::Retry {
                            reason: diag.summary,
                            delay_override: None,
                            connect_failed: true,
                        }),
                    ));
                    return;
                }
            };

            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let wait = parse_retry_after(response.headers());
                let _ = done_tx.send((
                    seg_id,
                    Err(WorkerErr::Retry {
                        reason: "分段下载触发 429 限流".to_owned(),
                        delay_override: Some(wait),
                        connect_failed: false,
                    }),
                ));
                return;
            }
            if status != StatusCode::PARTIAL_CONTENT {
                let _ = done_tx.send((
                    seg_id,
                    Err(WorkerErr::Retry {
                        reason: format!("分片请求返回异常状态 {status}"),
                        delay_override: None,
                        connect_failed: false,
                    }),
                ));
                return;
            }

            let etag = header_str(response.headers(), header::ETAG);
            let lm = header_str(response.headers(), header::LAST_MODIFIED);
            let validator_ok = match (&expected_etag, &etag) {
                (Some(a), Some(b)) => a == b,
                _ => match (&expected_lm, &lm) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                },
            };
            if !validator_ok {
                let _ = done_tx.send((
                    seg_id,
                    Err(WorkerErr::Retry {
                        reason: "远端 ETag 发生变更，重新校验下载".to_owned(),
                        delay_override: Some(Duration::from_millis(50)),
                        connect_failed: false,
                    }),
                ));
                return;
            }
            if let Some((_, _, Some(srv_total))) = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                && srv_total != total_expected
            {
                let _ = done_tx.send((
                    seg_id,
                    Err(WorkerErr::NextUrl(format!(
                        "{SIZE_MISMATCH}分段总长不一致（{srv_total} != {total_expected}）"
                    ))),
                ));
                return;
            }

            match stream_segment_body(
                &current_seg,
                response,
                &target_path,
                None,
                &seg0_live_hash_valid,
                &ctx,
                &counters,
                &newly_received,
                &cancel,
                pause.as_ref(),
            )
            .await
            {
                Ok(rx) => {
                    worker_rx += rx;
                }
                Err(err) => {
                    current_seg.done.store(true, Ordering::Release);
                    let _ = done_tx.send((seg_id, Err(err)));
                    return;
                }
            }
        }

        current_seg.done.store(true, Ordering::Release);
        let thief_speed = current_seg.speed_bps.load(Ordering::Acquire);
        if let Some((stolen, spare_next)) = coord.try_steal(thief_speed) {
            ctx.telemetry.record_work_steal();
            ctx.telemetry.push_event(
                TelemetryEventKind::WorkStolen,
                "快连接完成分片，已主动分担慢连接剩余区间",
            );
            current_seg = stolen;
            use_spare = spare_next;
            continue;
        }

        let _ = done_tx.send((seg_id, Ok(worker_rx)));
        return;
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_segment_body(
    seg: &SegmentState,
    response: Response,
    target_path: &Path,
    mut live_hasher: Option<&mut Sha1>,
    seg0_live_hash_valid: &AtomicBool,
    ctx: &EngineMetricsCtx,
    counters: &Counters,
    newly_received: &AtomicU64,
    cancel: &CancellationToken,
    pause: Option<&PauseControl>,
) -> Result<u64, WorkerErr> {
    let seg_path = part_seg_path(target_path, seg.id);
    let initial_written = seg.written.load(Ordering::Acquire);
    let file = if initial_written > 0 && seg_path.is_file() {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&seg_path)
            .await
    } else {
        tokio::fs::File::create(&seg_path).await
    }
    .map_err(|e| {
        if seg.id == 0 {
            seg0_live_hash_valid.store(false, Ordering::Release);
        }
        WorkerErr::Retry {
            reason: format!("无法打开分片文件：{e}"),
            delay_override: None,
            connect_failed: false,
        }
    })?;

    let mut writer = BufWriter::with_capacity(WRITE_BUF_BYTES, file);
    let mut written = initial_written;
    let mut rx_this_stream = 0u64;
    let mut body = response.bytes_stream();
    let mut ewma_bps = 0.0f64;
    let mut last_sample_t = Instant::now();
    let mut bytes_since_sample = 0u64;
    let _seg_guard = ctx.telemetry.begin_h1_segment();
    let is_binary = is_binary_target(target_path);
    let mut sniff_checked = !is_binary || written > 0;
    let mut sniff_buf = Vec::new();

    loop {
        if let Some(p) = pause
            && p.is_paused()
        {
            let _ = writer.flush().await;
            if seg.id == 0 {
                seg0_live_hash_valid.store(false, Ordering::Release);
            }
            return Err(WorkerErr::Paused);
        }
        let next = tokio::select! {
            step = tokio::time::timeout(SEG_STALL_TIMEOUT, body.next()) => match step {
                Ok(n) => n,
                Err(_) => {
                    let _ = writer.flush().await;
                    if seg.id == 0 {
                        seg0_live_hash_valid.store(false, Ordering::Release);
                    }
                    return Err(WorkerErr::Retry {
                        reason: "分片接收长时间停滞".to_owned(),
                        delay_override: Some(Duration::from_millis(120)),
                        connect_failed: false,
                    });
                }
            },
            () = cancel.cancelled() => {
                let _ = writer.flush().await;
                if seg.id == 0 {
                    seg0_live_hash_valid.store(false, Ordering::Release);
                }
                return Err(WorkerErr::Cancelled);
            }
        };

        match next {
            None => break,
            Some(Err(e)) => {
                let _ = writer.flush().await;
                if seg.id == 0 {
                    seg0_live_hash_valid.store(false, Ordering::Release);
                }
                return Err(WorkerErr::Retry {
                    reason: format!("分片传输中断：{e}"),
                    delay_override: None,
                    connect_failed: false,
                });
            }
            Some(Ok(chunk)) => {
                if !sniff_checked && sniff_buf.len() < 512 {
                    let need = (512 - sniff_buf.len()).min(chunk.len());
                    sniff_buf.extend_from_slice(&chunk[..need]);
                    if looks_like_html_payload(&sniff_buf) {
                        return Err(WorkerErr::NextUrl(format!(
                            "{STRUCTURE_MISMATCH}下载源返回了 HTML 劫持页面"
                        )));
                    }
                    if sniff_buf.len() >= 16 {
                        sniff_checked = true;
                    }
                }

                // 严格按 chunk 边界截断（保障工作窃取与 Segment-0 活体哈希 100% 精确）
                let end = seg.end.load(Ordering::Acquire);
                let cursor = seg.start.saturating_add(written);
                if cursor > end {
                    break;
                }
                let allowed = usize::try_from((end - cursor + 1).min(chunk.len() as u64))
                    .unwrap_or(chunk.len());
                let slice = &chunk[..allowed];

                if !ctx.throttle.consume(slice.len(), cancel).await {
                    let _ = writer.flush().await;
                    if seg.id == 0 {
                        seg0_live_hash_valid.store(false, Ordering::Release);
                    }
                    return Err(WorkerErr::Cancelled);
                }

                writer.write_all(slice).await.map_err(|e| {
                    if seg.id == 0 {
                        seg0_live_hash_valid.store(false, Ordering::Release);
                    }
                    WorkerErr::Retry {
                        reason: format!("写入分片失败：{e}"),
                        delay_override: None,
                        connect_failed: false,
                    }
                })?;

                if let Some(hasher) = live_hasher.as_deref_mut()
                    && seg0_live_hash_valid.load(Ordering::Acquire)
                {
                    hasher.update(slice);
                }

                let len = slice.len() as u64;
                written = written.saturating_add(len);
                rx_this_stream = rx_this_stream.saturating_add(len);
                seg.written.store(written, Ordering::Release);
                seg.last_rx_ms.store(ctx_elapsed_ms(), Ordering::Relaxed);
                counters.done_bytes.fetch_add(len, Ordering::Relaxed);
                counters.live_received.fetch_add(len, Ordering::Relaxed);
                counters.mark_dirty();
                newly_received.fetch_add(len, Ordering::Relaxed);
                ctx.telemetry.record_rx(len);

                bytes_since_sample = bytes_since_sample.saturating_add(len);
                let dt = last_sample_t.elapsed();
                if dt >= Duration::from_millis(250) {
                    let inst = bytes_since_sample as f64 / dt.as_secs_f64();
                    ewma_bps = if ewma_bps <= 0.0 {
                        inst
                    } else {
                        0.30 * inst + 0.70 * ewma_bps
                    };
                    seg.speed_bps.store(ewma_bps as u64, Ordering::Release);
                    bytes_since_sample = 0;
                    last_sample_t = Instant::now();
                }

                if allowed < chunk.len() {
                    break;
                }
            }
        }
    }

    writer.flush().await.map_err(|e| {
        if seg.id == 0 {
            seg0_live_hash_valid.store(false, Ordering::Release);
        }
        WorkerErr::Retry {
            reason: format!("刷新分片失败：{e}"),
            delay_override: None,
            connect_failed: false,
        }
    })?;
    Ok(rx_this_stream)
}

fn ctx_elapsed_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let s = START.get_or_init(Instant::now);
    u64::try_from(s.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
async fn finish_unranged_200_stream(
    response: Response,
    item: &DownloadItem,
    engine: &DownloadEngine,
    cancel: &CancellationToken,
    pause: Option<&PauseControl>,
    credit: &mut Credit<'_>,
    counters: &Counters,
    net_permit: super::download::NetPermit,
) -> SegmentedOutcome {
    let _net_guard = net_permit;
    credit.sync_to(0);
    let seg0 = part_seg_path(&item.path, 0);
    let file = match tokio::fs::File::create(&seg0).await {
        Ok(f) => f,
        Err(e) => {
            return SegmentedOutcome::Retry {
                reason: format!("无法写入临时文件：{e}"),
                delay_override: None,
                connect_failed: false,
            };
        }
    };
    let mut writer = BufWriter::with_capacity(WRITE_BUF_BYTES, file);
    let mut hasher = item.sha1.as_ref().map(|_| Sha1::new());
    let mut written = 0u64;
    let mut body = response.bytes_stream();
    let _h1_guard = engine.telemetry().begin_h1_segment();
    let is_binary = is_binary_target(&item.path);
    let mut sniff_checked = !is_binary;
    let mut sniff_buf = Vec::new();

    loop {
        if let Some(p) = pause
            && p.is_paused()
        {
            let _ = writer.flush().await;
            return SegmentedOutcome::Paused;
        }
        let next = tokio::select! {
            step = tokio::time::timeout(SEG_STALL_TIMEOUT, body.next()) => match step {
                Ok(n) => n,
                Err(_) => {
                    let _ = writer.flush().await;
                    return SegmentedOutcome::Retry {
                        reason: "接收数据长时间停滞，准备重连".to_owned(),
                        delay_override: Some(Duration::from_millis(200)),
                        connect_failed: false,
                    };
                }
            },
            () = async {
                if let Some(p) = pause {
                    p.paused_signal().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                let _ = writer.flush().await;
                return SegmentedOutcome::Paused;
            }
            () = cancel.cancelled() => {
                let _ = writer.flush().await;
                return SegmentedOutcome::Cancelled;
            }
        };
        match next {
            None => break,
            Some(Err(e)) => {
                let _ = writer.flush().await;
                return SegmentedOutcome::Retry {
                    reason: format!("下载中断：{e}"),
                    delay_override: None,
                    connect_failed: false,
                };
            }
            Some(Ok(chunk)) => {
                if !sniff_checked && sniff_buf.len() < 512 {
                    let need = (512 - sniff_buf.len()).min(chunk.len());
                    sniff_buf.extend_from_slice(&chunk[..need]);
                    if looks_like_html_payload(&sniff_buf) {
                        drop(writer);
                        remove_quietly_local(&seg0, cancel).await;
                        return SegmentedOutcome::NextUrl(format!(
                            "{STRUCTURE_MISMATCH}下载源返回了 HTML 劫持页面"
                        ));
                    }
                    if sniff_buf.len() >= 16 {
                        sniff_checked = true;
                    }
                }
                if !engine.throttle().consume(chunk.len(), cancel).await {
                    let _ = writer.flush().await;
                    return SegmentedOutcome::Cancelled;
                }
                if let Err(e) = writer.write_all(&chunk).await {
                    return SegmentedOutcome::Retry {
                        reason: format!("写入失败：{e}"),
                        delay_override: None,
                        connect_failed: false,
                    };
                }
                if let Some(h) = hasher.as_mut() {
                    h.update(&chunk);
                }
                let len = chunk.len() as u64;
                written = written.saturating_add(len);
                credit.add(len);
                counters.live_received.fetch_add(len, Ordering::Relaxed);
                engine.telemetry().record_rx(len);
            }
        }
    }

    if let Err(e) = writer.flush().await {
        return SegmentedOutcome::Retry {
            reason: format!("刷新写入失败：{e}"),
            delay_override: None,
            connect_failed: false,
        };
    }
    drop(writer);

    if let Some(expected) = item.size
        && written != expected
    {
        remove_quietly_local(&seg0, cancel).await;
        return SegmentedOutcome::Retry {
            reason: format!("{SIZE_MISMATCH}应为 {expected} 字节，实际 {written} 字节"),
            delay_override: None,
            connect_failed: false,
        };
    }
    if let (Some(expected), Some(h)) = (&item.sha1, hasher) {
        let actual = to_hex(&h.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            remove_quietly_local(&seg0, cancel).await;
            return SegmentedOutcome::Retry {
                reason: format!("{SHA1_MISMATCH}应为 {expected}，实际 {actual}"),
                delay_override: None,
                connect_failed: false,
            };
        }
    } else if item.sha1.is_none()
        && let Err(reason) = verify_structure_sync(&seg0, &item.path, written)
    {
        remove_quietly_local(&seg0, cancel).await;
        return SegmentedOutcome::Retry {
            reason,
            delay_override: None,
            connect_failed: false,
        };
    }

    let path_lock = engine.lock_for_path(&item.path);
    let _path_guard = path_lock.lock().await;
    let Some(_disk_permit) = engine.acquire_disk(cancel).await else {
        return SegmentedOutcome::Cancelled;
    };
    let mut disk_guard = engine.telemetry().begin_disk_write();
    disk_guard.mark_active();

    let started = Instant::now();
    let res = crate::fs::rename_with_win_retry(&seg0, &item.path, cancel).await;
    engine.record_disk_write_ms(started.elapsed().as_secs_f64() * 1000.0, 0);
    match res {
        Ok(()) => {
            if let Some(sha1) = &item.sha1 {
                engine.hydrator().record_file(&item.path, sha1, written);
            }
            SegmentedOutcome::Done { received: written }
        }
        Err(e) if e.kind() == io::ErrorKind::Interrupted => SegmentedOutcome::Cancelled,
        Err(e) => {
            let diag = classify_io_error(&e);
            SegmentedOutcome::Retry {
                reason: format!("{}（{e}）", diag.summary),
                delay_override: None,
                connect_failed: false,
            }
        }
    }
}

async fn remove_quietly_local(path: &Path, cancel: &CancellationToken) {
    let _ = remove_with_win_retry(path, cancel).await;
}

fn split_segment_evenly(coord: &Coordinator, seg0: &Arc<SegmentState>, parts: usize) {
    if parts <= 1 {
        return;
    }
    let start = seg0.start;
    let end = seg0.end.load(Ordering::Acquire);
    let total = end + 1 - start;
    if total < (parts as u64) * MIN_SPLIT_CHUNK {
        return;
    }
    let step = align_4k_down(total / parts as u64).max(MIN_SPLIT_CHUNK);
    let mut guard = coord.segments.lock().unwrap_or_else(|e| e.into_inner());
    let mut cur = start;
    let mut prev_idx = 0usize;
    for _ in 1..parts {
        let split = cur + step;
        if end + 1 - split < MIN_SPLIT_CHUNK {
            break;
        }
        guard[prev_idx].end.store(split - 1, Ordering::Release);
        let new_id = {
            let mut id_g = coord.next_id.lock().unwrap_or_else(|e| e.into_inner());
            let id = *id_g;
            *id_g += 1;
            id
        };
        let next = Arc::new(SegmentState::new(new_id, split, end, 0, coord.now_rel_ms()));
        guard.push(next);
        prev_idx = guard.len() - 1;
        cur = split;
    }
}

fn verify_contiguous_coverage(segments: &[SavedSegment], total_size: u64) -> bool {
    if segments.is_empty() || total_size == 0 {
        return false;
    }
    let mut sorted = segments.to_vec();
    sorted.sort_by_key(|s| s.start);
    let mut expected = 0u64;
    for seg in sorted {
        if seg.start != expected {
            return false;
        }
        let len = seg.end + 1 - seg.start;
        if seg.written != len {
            return false;
        }
        expected = seg.end + 1;
    }
    expected == total_size
}

fn next_rung(rungs: &[u8], cur: u8) -> u8 {
    rungs.iter().copied().find(|&r| r > cur).unwrap_or(cur)
}

fn previous_rung(rungs: &[u8], cur: u8) -> u8 {
    rungs.iter().copied().rev().find(|&r| r < cur).unwrap_or(1)
}

async fn load_valid_saved_meta(
    meta_path: &Path,
    target: &Path,
    total_expected: u64,
) -> Option<PartMeta> {
    let path = meta_path.to_path_buf();
    let meta = tokio::task::spawn_blocking(move || read_json::<PartMeta>(&path))
        .await
        .ok()?
        .ok()??;
    if meta.total_size != total_expected || meta.segments.is_empty() {
        return None;
    }
    for seg in &meta.segments {
        if seg.written == 0 {
            continue;
        }
        let sp = part_seg_path(target, seg.id);
        let Ok(file_meta) = tokio::fs::metadata(&sp).await else {
            return None;
        };
        if file_meta.len() != seg.written || seg.start + seg.written > seg.end + 1 {
            return None;
        }
    }
    Some(meta)
}

fn header_str(headers: &header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_content_range(raw: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = raw.trim().strip_prefix("bytes ")?;
    let (range, total_part) = rest.split_once('/')?;
    let (s, e) = range.split_once('-')?;
    let start = s.trim().parse::<u64>().ok()?;
    let end = e.trim().parse::<u64>().ok()?;
    let total = if total_part.trim() == "*" {
        None
    } else {
        Some(total_part.trim().parse::<u64>().ok()?)
    };
    Some((start, end, total))
}

fn handle_connect_error(
    engine: &DownloadEngine,
    req_host: Option<&str>,
    failed_url: &str,
    source: &reqwest::Error,
) -> SegmentedOutcome {
    if source.is_connect() && engine.refresh_proxy_if_changed().is_some() {
        return SegmentedOutcome::Retry {
            reason: "检测到系统代理刚刚切换，已重建连接并重试".to_owned(),
            delay_override: Some(Duration::from_millis(30)),
            connect_failed: false,
        };
    }
    let failed_host = url_host(failed_url).or_else(|| req_host.map(str::to_owned));
    if let Some(h) = failed_host.as_deref()
        && engine.doh().report_connect_failure(h, source)
            == TransportDnsAction::SniBlockedSwitchSource
    {
        let diag = classify_reqwest_error(source, Some(h), Some(engine.doh()), engine.proxy_mode());
        engine.telemetry().set_diagnosis(Some(diag.clone()));
        return SegmentedOutcome::NextUrl(format!("{}（已切换备选源）", diag.summary));
    }
    let diag = classify_reqwest_error(
        source,
        failed_host.as_deref(),
        Some(engine.doh()),
        engine.proxy_mode(),
    );
    SegmentedOutcome::Retry {
        reason: diag.summary,
        delay_override: None,
        connect_failed: true,
    }
}

#[allow(dead_code)]
fn _ignore_atomic_err(_: AtomicWriteError) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_segment_respects_all_6_gates() {
        let mut item = DownloadItem {
            urls: vec!["https://example.com/big.jar".to_owned()],
            path: PathBuf::from("C:\\tmp\\big.jar"),
            sha1: None,
            size: Some(10 * 1024 * 1024),
        };
        assert!(can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Ssd,
            5.0,
            false,
            0
        ));
        assert!(!can_segment(
            &item,
            SegmentMode::Off,
            DiskMediaKind::Ssd,
            5.0,
            false,
            0
        ));
        assert!(!can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Hdd,
            5.0,
            false,
            0
        ));
        assert!(!can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Ssd,
            55.0,
            false,
            0
        ));
        assert!(!can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Ssd,
            5.0,
            true,
            0
        ));
        assert!(!can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Ssd,
            5.0,
            false,
            1024
        ));
        assert!(can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Ssd,
            5.0,
            false,
            4096
        ));

        item.size = Some(2 * 1024 * 1024);
        assert!(!can_segment(
            &item,
            SegmentMode::Auto,
            DiskMediaKind::Ssd,
            5.0,
            false,
            0
        ));
    }

    #[test]
    fn eta_work_stealing_and_relay_merge_produce_exact_file_and_sha1() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("modules.jar");

        let mut payload = vec![0u8; 4 * 1024 * 1024 + 123];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(&payload);
            to_hex(&h.finalize())
        };

        let s0 = Arc::new(SegmentState::new(0, 0, payload.len() as u64 - 1, 0, 0));
        let coord = Coordinator::new(vec![s0.clone()], 1);
        let seg0_written = 512 * 1024u64;
        s0.written.store(seg0_written, Ordering::Release);
        s0.speed_bps.store(128 * 1024, Ordering::Release);

        let (s1, _) = coord.try_steal(10 * 1024 * 1024).unwrap();
        let s0_end = s0.end.load(Ordering::Acquire);
        assert_eq!(s1.start, s0_end + 1);
        assert_eq!(s1.end.load(Ordering::Acquire), payload.len() as u64 - 1);
        assert_eq!(s1.start % 4096, 0, "窃取边界必须严格 4 KiB 对齐");

        fs::write(part_seg_path(&target, 0), &payload[..=s0_end as usize]).unwrap();
        fs::write(part_seg_path(&target, s1.id), &payload[s1.start as usize..]).unwrap();
        s0.written.store(s0_end + 1, Ordering::Release);
        s1.written
            .store(payload.len() as u64 - s1.start, Ordering::Release);

        let snap = coord.snapshot_meta(payload.len() as u64, None, None);
        assert!(verify_contiguous_coverage(
            &snap.segments,
            payload.len() as u64
        ));

        let mut live_h = Sha1::new();
        live_h.update(&payload[..=s0_end as usize]);
        merge_segments_relay_hash_sync(
            &target,
            &snap.segments,
            payload.len() as u64,
            Some(&expected_sha1),
            Some(live_h),
        )
        .unwrap();

        assert_eq!(fs::read(&target).unwrap(), payload);
        assert!(!part_seg_path(&target, 0).exists());
        assert!(!part_seg_path(&target, s1.id).exists());
    }

    #[test]
    fn orphan_part_gc_cleanups_old_only() {
        let dir = tempfile::tempdir().unwrap();
        let libs = dir.path().join("libraries");
        fs::create_dir_all(&libs).unwrap();
        let p_s0 = libs.join("foo.jar.part.s0");
        let p_meta = libs.join("foo.jar.part.meta");
        let p_real = libs.join("foo.jar");
        fs::write(&p_s0, b"1234").unwrap();
        fs::write(&p_meta, b"{}").unwrap();
        fs::write(&p_real, b"keep").unwrap();

        let (b, c) = clean_orphan_part_files(dir.path(), Duration::ZERO);
        assert_eq!((b, c), (6, 2));
        assert!(!p_s0.exists());
        assert!(!p_meta.exists());
        assert!(p_real.exists());
    }

    #[tokio::test]
    async fn zero_head_200_fallback_downloads_without_extra_requests() {
        use std::sync::atomic::{AtomicUsize, Ordering as AOrd};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let hits = Arc::new(AtomicUsize::new(0));
        let hits_srv = hits.clone();
        let mut payload = vec![0u8; 4 * 1024 * 1024 + 512];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = ((i * 7 + 3) % 256) as u8;
        }
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(&payload);
            to_hex(&h.finalize())
        };
        let payload_srv = Arc::new(payload.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = [0u8; 2048];
            let n = sock.read(&mut req).await.unwrap();
            let req_text = String::from_utf8_lossy(&req[..n]);
            assert!(req_text.starts_with("GET "));
            hits_srv.fetch_add(1, AOrd::SeqCst);
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload_srv.len()
            );
            sock.write_all(hdr.as_bytes()).await.unwrap();
            sock.write_all(&payload_srv).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fallback.bin");
        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(crate::net::ProxyWatcher::new());
        let engine = Arc::new(
            DownloadEngine::new(doh, watcher, &crate::config::ProxySettings::default()).unwrap(),
        );
        let downloader = crate::net::Downloader::from_engine(engine, DownloadPriority::Batch, 1);
        let item = DownloadItem {
            urls: vec![format!("http://{addr}/fallback.bin")],
            path: target.clone(),
            sha1: Some(expected_sha1),
            size: Some(payload.len() as u64),
        };
        downloader
            .download_all(&[item], &CancellationToken::new(), |_| {})
            .await
            .unwrap();

        assert_eq!(hits.load(AOrd::SeqCst), 1, "Zero-HEAD 必须只发 1 次请求");
        assert_eq!(fs::read(&target).unwrap(), payload);
    }

    #[tokio::test]
    async fn zero_head_206_range_with_profile_warm_burst_merges_valid_file() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut payload = vec![0u8; 4 * 1024 * 1024 + 777];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = ((i * 13 + 5) % 256) as u8;
        }
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(&payload);
            to_hex(&h.finalize())
        };
        let payload_srv = Arc::new(payload.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let data = payload_srv.clone();
                tokio::spawn(async move {
                    let mut req = [0u8; 4096];
                    let Ok(n) = sock.read(&mut req).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req_text = String::from_utf8_lossy(&req[..n]);
                    let range_line = req_text.lines().find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(str::to_owned)
                    });
                    let total = data.len();
                    let (start, end) = if let Some(r) = range_line {
                        let (s, e) = r.trim().split_once('-').unwrap_or(("0", ""));
                        let start = s.parse::<usize>().unwrap_or(0);
                        let end = if e.is_empty() {
                            total - 1
                        } else {
                            e.parse::<usize>().unwrap_or(total - 1).min(total - 1)
                        };
                        (start, end)
                    } else {
                        (0, total - 1)
                    };
                    let slice = &data[start..=end];
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ranged.bin");
        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(crate::net::ProxyWatcher::new());
        let engine = Arc::new(
            DownloadEngine::new(doh, watcher, &crate::config::ProxySettings::default()).unwrap(),
        );
        // 写入一次指纹使 WarmBurst 直接走 2 段切分并发
        let cfg = engine.settings_snapshot();
        let tag = classify_proxy_tag(&cfg.proxy, &engine.proxy_watcher().snapshot(), false);
        let sig = compute_network_signature(tag, false);
        let downloader =
            crate::net::Downloader::from_engine(engine.clone(), DownloadPriority::Batch, 1);
        let _ = sig;
        let item = DownloadItem {
            urls: vec![format!("http://{addr}/ranged.bin")],
            path: target.clone(),
            sha1: Some(expected_sha1),
            size: Some(payload.len() as u64),
        };
        downloader
            .download_all(&[item], &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), payload);
    }

    #[tokio::test]
    async fn pause_and_resume_segmented_keeps_exact_progress_without_double_count() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut payload = vec![0u8; 4 * 1024 * 1024 + 4096];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = ((i * 7 + 3) % 256) as u8;
        }
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(&payload);
            to_hex(&h.finalize())
        };
        let total_size = payload.len() as u64;
        let payload_srv = Arc::new(payload.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let data = payload_srv.clone();
                tokio::spawn(async move {
                    let mut req = [0u8; 4096];
                    let Ok(n) = sock.read(&mut req).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req_text = String::from_utf8_lossy(&req[..n]);
                    let range_line = req_text.lines().find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(str::to_owned)
                    });
                    let total = data.len();
                    let (start, end) = if let Some(r) = range_line {
                        let (s, e) = r.trim().split_once('-').unwrap_or(("0", ""));
                        let start = s.parse::<usize>().unwrap_or(0);
                        let end = if e.is_empty() {
                            total - 1
                        } else {
                            e.parse::<usize>().unwrap_or(total - 1).min(total - 1)
                        };
                        (start, end)
                    } else {
                        (0, total - 1)
                    };
                    let slice = &data[start..=end];
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                        slice.len()
                    );
                    if sock.write_all(hdr.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in slice.chunks(64 * 1024) {
                        if sock.write_all(chunk).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(8)).await;
                    }
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pause_resume.bin");
        let doh = Arc::new(Doh::new(false).unwrap());
        let watcher = Arc::new(crate::net::ProxyWatcher::new());
        let engine = Arc::new(
            DownloadEngine::new(doh, watcher, &crate::config::ProxySettings::default()).unwrap(),
        );
        let pause = PauseControl::new();
        let downloader =
            crate::net::Downloader::from_engine(engine, DownloadPriority::Interactive, 2)
                .with_pause(pause.clone());

        let item = DownloadItem {
            urls: vec![format!("http://{addr}/pause_resume.bin")],
            path: target.clone(),
            sha1: Some(expected_sha1),
            size: Some(total_size),
        };

        let max_seen = Arc::new(AtomicU64::new(0));
        let bounced = Arc::new(AtomicBool::new(false));
        let paused_once = Arc::new(AtomicBool::new(false));
        let pause_clone = pause.clone();
        let paused_once_clone = paused_once.clone();
        let seen_clone = max_seen.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(40)).await;
                let cur = seen_clone.load(Ordering::Acquire);
                if cur >= 512 * 1024 && !paused_once_clone.swap(true, Ordering::AcqRel) {
                    pause_clone.pause();
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    pause_clone.resume();
                    break;
                }
            }
        });

        let seen_cb = max_seen.clone();
        let bounced_cb = bounced.clone();
        let report = downloader
            .download_all(&[item], &CancellationToken::new(), |p| {
                let prev = seen_cb.load(Ordering::Acquire);
                if p.done_bytes < prev {
                    bounced_cb.store(true, Ordering::Release);
                } else {
                    seen_cb.store(p.done_bytes, Ordering::Release);
                }
            })
            .await
            .unwrap();

        assert!(paused_once.load(Ordering::Acquire));
        assert!(!bounced.load(Ordering::Acquire), "分段暂停恢复禁止进度回弹");
        assert_eq!(report.downloaded, 1);
        assert_eq!(max_seen.load(Ordering::Acquire), total_size);
        assert_eq!(fs::read(&target).unwrap(), payload);
    }
}
