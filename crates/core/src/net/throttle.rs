//! 流量平滑令牌桶、目标磁盘介质识别（SSD / HDD）与 Windows 计费网络检测（全零 `unsafe`）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// 目标磁盘物理介质类型（SSD 可开启高并发与多段并发；HDD 强制 k=1 顺序写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum DiskMediaKind {
    Ssd,
    Hdd,
    #[default]
    Unknown,
}

/// 全局平滑令牌桶（支持 0 = 不限速、<=100ms 热更新限速与立即取消）。
pub struct TokenBucket {
    limit_kib: AtomicU32,
    state: Mutex<(f64, Instant)>,
}

impl TokenBucket {
    pub fn new(limit_kib: u32) -> Self {
        let cap = f64::from(limit_kib) * 1024.0;
        Self {
            limit_kib: AtomicU32::new(limit_kib),
            state: Mutex::new((cap, Instant::now())),
        }
    }

    pub fn set_limit_kib(&self, kib: u32) {
        let prev = self.limit_kib.swap(kib, Ordering::AcqRel);
        if prev != kib {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let new_cap = f64::from(kib) * 1024.0;
            guard.0 = guard.0.min(new_cap);
            guard.1 = Instant::now();
        }
    }

    pub fn limit_kib(&self) -> u32 {
        self.limit_kib.load(Ordering::Acquire)
    }

    /// 消费指定字节数许可；限速下按不超过 100ms 的步长平滑睡眠以保证热改限速与取消瞬时响应。
    pub async fn consume(&self, bytes: usize, cancel: &CancellationToken) -> bool {
        if bytes == 0 {
            return !cancel.is_cancelled();
        }
        let mut remaining = bytes as f64;
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            let kib = self.limit_kib.load(Ordering::Acquire);
            if kib == 0 {
                return true;
            }
            let rate_bps = f64::from(kib) * 1024.0;
            // 最大突发桶 1 秒容量（至少 64 KiB 以避免小包过度碎片化等待）
            let burst_cap = rate_bps.max(64.0 * 1024.0);
            let wait_dur = {
                let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(guard.1).as_secs_f64();
                guard.0 = (guard.0 + elapsed * rate_bps).min(burst_cap);
                guard.1 = now;
                if guard.0 >= remaining {
                    guard.0 -= remaining;
                    return true;
                }
                if guard.0 > 0.0 {
                    remaining -= guard.0;
                    guard.0 = 0.0;
                }
                let need_secs = (remaining / rate_bps).min(0.1);
                Duration::from_secs_f64(need_secs.max(0.002))
            };
            tokio::select! {
                () = tokio::time::sleep(wait_dur) => {}
                () = cancel.cancelled() => return false,
            }
        }
    }
}

/// 根据目标路径所属挂载点识别盘符物理介质（复用 `sysinfo` `"disk"`，全安全零 `unsafe`）。
pub struct DiskMediaDetector {
    mounts: RwLock<Vec<(PathBuf, DiskMediaKind)>>,
}

impl DiskMediaDetector {
    pub fn new() -> Self {
        let mounts = Self::scan_system_disks();
        Self {
            mounts: RwLock::new(mounts),
        }
    }

    pub fn from_entries(entries: Vec<(PathBuf, DiskMediaKind)>) -> Self {
        let mut normalized: Vec<(PathBuf, DiskMediaKind)> = entries
            .into_iter()
            .map(|(p, k)| (normalize_mount_key(&p), k))
            .collect();
        normalized.sort_by_key(|a| std::cmp::Reverse(a.0.as_os_str().len()));
        Self {
            mounts: RwLock::new(normalized),
        }
    }

    pub fn refresh(&self) {
        let mounts = Self::scan_system_disks();
        *self.mounts.write().unwrap_or_else(|e| e.into_inner()) = mounts;
    }

    pub fn detect(&self, path: &Path) -> DiskMediaKind {
        let resolved = resolve_existing_prefix(path);
        let key = normalize_mount_key(&resolved);
        let key_str = key.to_string_lossy();
        let guard = self.mounts.read().unwrap_or_else(|e| e.into_inner());
        for (mount, kind) in guard.iter() {
            let mount_str = mount.to_string_lossy();
            if key_str.starts_with(mount_str.as_ref()) {
                return *kind;
            }
        }
        DiskMediaKind::Unknown
    }

    fn scan_system_disks() -> Vec<(PathBuf, DiskMediaKind)> {
        let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else {
            return Vec::new();
        };
        let mut list: Vec<(PathBuf, DiskMediaKind)> = disks
            .iter()
            .map(|d| {
                let kind = match d.kind() {
                    sysinfo::DiskKind::SSD => DiskMediaKind::Ssd,
                    sysinfo::DiskKind::HDD => DiskMediaKind::Hdd,
                    _ => DiskMediaKind::Unknown,
                };
                (normalize_mount_key(d.mount_point()), kind)
            })
            .collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.0.as_os_str().len()));
        list
    }
}

impl Default for DiskMediaDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.exists() {
            return std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        }
        cur = p.parent();
    }
    path.to_path_buf()
}

fn normalize_mount_key(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let stripped = text.strip_prefix(r"\\?\").unwrap_or(&text);
    #[cfg(windows)]
    {
        PathBuf::from(stripped.to_ascii_lowercase())
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(stripped)
    }
}

/// 读取操作系统当前是否处于计费网络（如手机热点 / 流量卡），零权限、零 `unsafe`。
#[cfg(windows)]
pub fn is_metered_network() -> bool {
    use windows::Networking::Connectivity::{NetworkCostType, NetworkInformation};

    let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
        return false;
    };
    let Ok(cost) = profile.GetConnectionCost() else {
        return false;
    };
    let Ok(cost_type) = cost.NetworkCostType() else {
        return false;
    };
    let roaming = cost.Roaming().unwrap_or(false);
    roaming
        || matches!(
            cost_type,
            NetworkCostType::Fixed | NetworkCostType::Variable
        )
}

#[cfg(not(windows))]
pub fn is_metered_network() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_bucket_limits_and_allows_hot_update() {
        let bucket = TokenBucket::new(0);
        let cancel = CancellationToken::new();
        assert!(bucket.consume(1024 * 1024, &cancel).await);

        // Set 100 KiB/s -> initial burst cap is 100 KiB, extra consume takes measurable time
        bucket.set_limit_kib(100);
        assert_eq!(bucket.limit_kib(), 100);
        assert!(bucket.consume(100 * 1024, &cancel).await);

        let started = Instant::now();
        assert!(bucket.consume(20 * 1024, &cancel).await);
        assert!(started.elapsed() >= Duration::from_millis(80));

        // Cancel interrupts wait promptly
        let bucket = std::sync::Arc::new(TokenBucket::new(10));
        assert!(bucket.consume(64 * 1024, &cancel).await);
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            c2.cancel();
        });
        assert!(!bucket.consume(128 * 1024, &cancel).await);
    }

    #[test]
    fn disk_detector_matches_longest_mount_point() {
        let dir = tempfile::tempdir().unwrap();
        let root_a = dir.path().join("fast_ssd");
        let sub_hdd = root_a.join("Games").join("SlowHdd");
        let root_b = dir.path().join("archive_hdd");
        std::fs::create_dir_all(sub_hdd.join("packs")).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();

        let detector = DiskMediaDetector::from_entries(vec![
            (
                std::fs::canonicalize(&root_a).unwrap_or_else(|_| root_a.clone()),
                DiskMediaKind::Ssd,
            ),
            (
                std::fs::canonicalize(&sub_hdd).unwrap_or_else(|_| sub_hdd.clone()),
                DiskMediaKind::Hdd,
            ),
            (
                std::fs::canonicalize(&root_b).unwrap_or_else(|_| root_b.clone()),
                DiskMediaKind::Hdd,
            ),
        ]);
        assert_eq!(
            detector.detect(&root_a.join("mods\\file.jar")),
            DiskMediaKind::Ssd
        );
        assert_eq!(
            detector.detect(&sub_hdd.join("packs\\a.jar")),
            DiskMediaKind::Hdd
        );
        assert_eq!(
            detector.detect(&root_b.join("Minecraft\\mod.jar")),
            DiskMediaKind::Hdd
        );
    }
}
