//! 零字节跨启动器「本地内容水合器」（`LocalHydrator`）：
//!
//! - **硬约束**：仅对携带强哈希（`item.sha1.is_some()`）的下载项生效；
//! - **只读安全**：对外部启动器目录或系统 `.minecraft` 100% 只读且必须先通过精确字节数 + 全量 SHA-1 复核；
//! - **两级架构**：
//!   - Tier 1（微秒级 `O(1)` 精确路径直查）：自身 `.mcl-store/objects/<2>/<sha1>`、已挂载外部根目录与 `%APPDATA%\.minecraft` 的精确相对路径（绝不扫描整盘）；
//!   - Tier 2（复用 `rusqlite 0.41` 惰性倒排索引 `<data_dir>/hydrator-index.sqlite`）：记录 Lumi 自身实例 `mods/` 与下载落盘的历史 `(sha1, size, path, mtime_ms)`；
//! - **安全物化**：仅对不可变资产（`assets/`、`libraries/`、`.mcl-store/`、`.mcl-java/`）尝试同盘 `std::fs::hard_link`，跨盘或可变 `mods/` 一律走 `write_atomic_from` 复制。

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::UNIX_EPOCH;

use sha1::{Digest, Sha1};
use tokio_util::sync::CancellationToken;

use crate::content::store::STORE_DIR;
use crate::fs::{remove_with_win_retry_sync, rename_with_win_retry_sync};

use super::download::DownloadItem;
use super::to_hex;

/// 本地内容水合器：按需初始化 SQLite 倒排索引并提供只读核验复用。
pub struct LocalHydrator {
    game_root: RwLock<PathBuf>,
    external_roots: RwLock<Vec<PathBuf>>,
    db_path: Option<PathBuf>,
    db: Arc<Mutex<Option<rusqlite::Connection>>>,
}

impl LocalHydrator {
    pub fn new(game_root: PathBuf, data_dir: Option<PathBuf>) -> Self {
        let db_path = data_dir.map(|d| d.join("hydrator-index.sqlite"));
        Self {
            game_root: RwLock::new(game_root),
            external_roots: RwLock::new(Vec::new()),
            db_path,
            db: Arc::new(Mutex::new(None)),
        }
    }

    pub fn in_memory(game_root: PathBuf) -> Self {
        Self {
            game_root: RwLock::new(game_root),
            external_roots: RwLock::new(Vec::new()),
            db_path: None,
            db: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_roots(&self, game_root: PathBuf, external_roots: Vec<PathBuf>) {
        *self.game_root.write().unwrap_or_else(|e| e.into_inner()) = game_root;
        *self
            .external_roots
            .write()
            .unwrap_or_else(|e| e.into_inner()) = external_roots;
    }

    pub fn set_external_roots(&self, external_roots: Vec<PathBuf>) {
        *self
            .external_roots
            .write()
            .unwrap_or_else(|e| e.into_inner()) = external_roots;
    }

    /// 仅当 `item.sha1.is_some()` 时尝试本地零字节水合；成功返回物化字节数。
    pub async fn try_hydrate(
        &self,
        item: &DownloadItem,
        cancel: &CancellationToken,
    ) -> Option<u64> {
        let expected_sha1 = item.sha1.as_deref()?.to_ascii_lowercase();
        if expected_sha1.len() < 4 || cancel.is_cancelled() {
            return None;
        }
        let expected_size = item.size;
        let target = item.path.clone();
        let game_root = self
            .game_root
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let external_roots = self
            .external_roots
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        // 先收集 Tier 1 (O(1)) 候选；Tier 2 SQLite 查询放到 spawn_blocking 内执行，绝不阻塞 Tokio 线程
        let mut candidates = Vec::new();

        // 1) Lumi ContentStore CAS 对象
        candidates.push((
            game_root
                .join(STORE_DIR)
                .join("objects")
                .join(&expected_sha1[..2])
                .join(&expected_sha1),
            true,
        ));

        // 2) 若目标在 game_root 内部且为共享资产/库/版本路径，按精确相对路径直查外部与官方 .minecraft
        if let Ok(rel) = target.strip_prefix(&game_root) {
            let rel_first = rel
                .components()
                .next()
                .and_then(|c| c.as_os_str().to_str())
                .unwrap_or_default();
            if matches!(rel_first, "assets" | "libraries" | "versions") {
                for ext_root in &external_roots {
                    candidates.push((ext_root.join(rel), false));
                }
                if let Some(official) = default_official_minecraft_dir()
                    && !same_dir_path(&official, &game_root)
                {
                    candidates.push((official.join(rel), false));
                }
            }
        }

        let db_path = self.db_path.clone();
        let db = self.db.clone();
        let target_is_immutable = is_immutable_artifact(&target, &game_root);
        let cancel_inner = cancel.clone();

        let matched = tokio::task::spawn_blocking(move || {
            if let Some(ref db_p) = db_path {
                for path in query_index_with(db_p, &db, &expected_sha1, expected_size) {
                    let inside_own = path.starts_with(&game_root);
                    candidates.push((path, inside_own));
                }
            }
            for (src, can_hardlink_src) in candidates {
                if cancel_inner.is_cancelled() {
                    return None;
                }
                if src == target || !src.is_file() {
                    continue;
                }
                let Ok(meta) = fs::metadata(&src) else {
                    continue;
                };
                if let Some(expected_len) = expected_size
                    && meta.len() != expected_len
                {
                    continue;
                }
                let allow_hardlink = target_is_immutable && can_hardlink_src;
                if let Ok(written) = materialize_verified_file(
                    &src,
                    &target,
                    &expected_sha1,
                    expected_size,
                    allow_hardlink,
                ) {
                    return Some(written);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()?;

        Some(matched)
    }

    /// 异步路径入口：将 SQLite 写入放到 `spawn_blocking` 中执行，避免阻塞 Tokio 工作线程。
    pub async fn record_file_async(&self, path: PathBuf, sha1: String, size: u64) {
        let Some(db_path) = self.db_path.clone() else {
            return;
        };
        let db = self.db.clone();
        let _ = tokio::task::spawn_blocking(move || {
            record_file_with(&db_path, &db, &path, &sha1, size);
        })
        .await;
    }

    /// 下载完成后登记文件到 Tier 2 倒排索引。
    pub fn record_file(&self, path: &Path, sha1: &str, size: u64) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        record_file_with(db_path, &self.db, path, sha1, size);
    }

    /// 惰性扫描某个实例 `mods/` 目录，仅对新增或 `(size, mtime)` 变化的文件补算 SHA-1。
    pub fn index_mods_dir(&self, mods_dir: &Path) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        let Ok(entries) = fs::read_dir(mods_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let size = meta.len();
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            let path_str = path.to_string_lossy().into_owned();
            let unchanged = {
                let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
                ensure_db_conn(&mut guard, db_path).is_some_and(|conn| {
                    conn.query_row(
                        "SELECT size, mtime_ms FROM files WHERE path = ?1",
                        rusqlite::params![&path_str],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .ok()
                    .is_some_and(|(s, m)| {
                        s == i64::try_from(size).unwrap_or(i64::MAX) && m == mtime_ms
                    })
                })
            };
            if unchanged {
                continue;
            }
            if let Ok(sha1) = compute_sha1_sync(&path) {
                self.record_file(&path, &sha1, size);
            }
        }
    }

    #[allow(dead_code)]
    fn query_index(&self, sha1: &str, expected_size: Option<u64>) -> Vec<PathBuf> {
        let Some(db_path) = &self.db_path else {
            return Vec::new();
        };
        query_index_with(db_path, &self.db, sha1, expected_size)
    }
}

fn record_file_with(
    db_path: &Path,
    db: &Mutex<Option<rusqlite::Connection>>,
    path: &Path,
    sha1: &str,
    size: u64,
) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let path_str = path.to_string_lossy().into_owned();
    let sha1_lower = sha1.to_ascii_lowercase();
    let mut guard = db.lock().unwrap_or_else(|e| e.into_inner());
    let Some(conn) = ensure_db_conn(&mut guard, db_path) else {
        return;
    };
    let _ = conn.execute(
        "INSERT INTO files (path, sha1, size, mtime_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
           sha1 = excluded.sha1,
           size = excluded.size,
           mtime_ms = excluded.mtime_ms",
        rusqlite::params![
            path_str,
            sha1_lower,
            i64::try_from(size).unwrap_or(i64::MAX),
            mtime_ms
        ],
    );
}

fn query_index_with(
    db_path: &Path,
    db: &Mutex<Option<rusqlite::Connection>>,
    sha1: &str,
    expected_size: Option<u64>,
) -> Vec<PathBuf> {
    let mut guard = db.lock().unwrap_or_else(|e| e.into_inner());
    let Some(conn) = ensure_db_conn(&mut guard, db_path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stale = Vec::new();
    {
        let Ok(mut stmt) = conn.prepare("SELECT path, size FROM files WHERE sha1 = ?1 LIMIT 8")
        else {
            return Vec::new();
        };
        let rows = stmt
            .query_map(rusqlite::params![sha1], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .ok();
        if let Some(iter) = rows {
            for item in iter.flatten() {
                let (p, sz) = item;
                let path = PathBuf::from(&p);
                if !path.is_file() {
                    stale.push(p);
                    continue;
                }
                if let Some(exp) = expected_size
                    && u64::try_from(sz).ok() != Some(exp)
                {
                    continue;
                }
                out.push(path);
            }
        }
    }
    for missing in stale {
        let _ = conn.execute(
            "DELETE FROM files WHERE path = ?1",
            rusqlite::params![missing],
        );
    }
    out
}

fn ensure_db_conn<'a>(
    slot: &'a mut Option<rusqlite::Connection>,
    db_path: &Path,
) -> Option<&'a rusqlite::Connection> {
    if slot.is_none() {
        if let Some(parent) = db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(conn) = rusqlite::Connection::open(db_path) else {
            return None;
        };
        let _ = conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS files (
                 path TEXT PRIMARY KEY,
                 sha1 TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_files_sha1 ON files(sha1);",
        );
        *slot = Some(conn);
    }
    slot.as_ref()
}

fn compute_sha1_sync(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// 单遍边复制（或硬链接）边核验 SHA-1 与长度后再原子改名，彻底消除先查后拷的 TOCTOU 竞态与双倍读盘。
fn materialize_verified_file(
    src: &Path,
    target: &Path,
    expected_sha1: &str,
    expected_size: Option<u64>,
    allow_hardlink: bool,
) -> io::Result<u64> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = target.with_file_name(format!(
        ".{}.{}.hydrate.tmp",
        target
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        uuid::Uuid::new_v4().simple()
    ));

    if allow_hardlink && fs::hard_link(src, &tmp).is_ok() {
        let len_ok = fs::metadata(&tmp)
            .ok()
            .filter(|m| expected_size.is_none_or(|exp| m.len() == exp))
            .map(|m| m.len());
        let verified = len_ok.filter(|_| {
            compute_sha1_sync(&tmp).is_ok_and(|actual| actual.eq_ignore_ascii_case(expected_sha1))
        });
        if let Some(len) = verified
            && rename_with_win_retry_sync(&tmp, target).is_ok()
        {
            return Ok(len);
        }
        let _ = remove_with_win_retry_sync(&tmp);
    }

    let copy_res = (|| -> io::Result<u64> {
        let mut reader = File::open(src)?;
        let out = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, out);
        let mut hasher = Sha1::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            writer.write_all(&buf[..n])?;
            written = written.saturating_add(n as u64);
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);

        if let Some(exp) = expected_size
            && written != exp
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "hydrated length mismatch",
            ));
        }
        let actual_sha1 = to_hex(&hasher.finalize());
        if !actual_sha1.eq_ignore_ascii_case(expected_sha1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "hydrated sha1 mismatch",
            ));
        }
        rename_with_win_retry_sync(&tmp, target)?;
        Ok(written)
    })();

    if copy_res.is_err() {
        let _ = remove_with_win_retry_sync(&tmp);
    }
    copy_res
}

fn is_immutable_artifact(target: &Path, game_root: &Path) -> bool {
    let Ok(rel) = target.strip_prefix(game_root) else {
        return false;
    };
    let top = rel
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or_default();
    matches!(top, "assets" | "libraries" | ".mcl-store" | ".mcl-java")
}

fn default_official_minecraft_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join(".minecraft"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn same_dir_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hydrates_from_external_root_and_rejects_tampered_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let game_root = dir.path().join("lumi");
        let ext_root = dir.path().join("external");
        let data_dir = dir.path().join("data");

        let payload = b"authentic minecraft asset bytes";
        let expected_sha1 = {
            let mut h = Sha1::new();
            h.update(payload);
            to_hex(&h.finalize())
        };

        let rel = Path::new("assets/objects/ab/abcdef");
        let ext_file = ext_root.join(rel);
        fs::create_dir_all(ext_file.parent().unwrap()).unwrap();
        fs::write(&ext_file, payload).unwrap();

        let hydrator = LocalHydrator::new(game_root.clone(), Some(data_dir));
        hydrator.set_external_roots(vec![ext_root.clone()]);

        let target = game_root.join(rel);
        let item = DownloadItem {
            urls: vec!["https://resources.download.minecraft.net/ab/abcdef".to_owned()],
            path: target.clone(),
            sha1: Some(expected_sha1.clone()),
            size: Some(payload.len() as u64),
        };
        let cancel = CancellationToken::new();
        let reused = hydrator.try_hydrate(&item, &cancel).await;
        assert_eq!(reused, Some(payload.len() as u64));
        assert_eq!(fs::read(&target).unwrap(), payload);

        // Tamper external file (same length, wrong bytes) -> must NOT hydrate
        let tampered_target = game_root.join("assets/objects/ab/tampered");
        let ext_tampered = ext_root.join("assets/objects/ab/tampered");
        fs::write(&ext_tampered, b"tampered!!minecraft asset bytes").unwrap();
        let bad_item = DownloadItem {
            urls: vec![],
            path: tampered_target.clone(),
            sha1: Some(expected_sha1),
            size: Some(payload.len() as u64),
        };
        assert_eq!(hydrator.try_hydrate(&bad_item, &cancel).await, None);
        assert!(!tampered_target.exists());
    }

    #[tokio::test]
    async fn tier2_sqlite_hydrates_mod_files_by_copy_not_hardlink() {
        let dir = tempfile::tempdir().unwrap();
        let game_root = dir.path().join("lumi");
        let data_dir = dir.path().join("data");
        let src_mod = game_root.join("instances/inst1/mods/sodium.jar");
        fs::create_dir_all(src_mod.parent().unwrap()).unwrap();
        let bytes = b"PK\x03\x04fake-sodium-jar-data";
        fs::write(&src_mod, bytes).unwrap();

        let hydrator = LocalHydrator::new(game_root.clone(), Some(data_dir));
        hydrator.index_mods_dir(src_mod.parent().unwrap());

        let sha1 = compute_sha1_sync(&src_mod).unwrap();
        let dst_mod = game_root.join("instances/inst2/mods/sodium.jar");
        let item = DownloadItem {
            urls: vec![],
            path: dst_mod.clone(),
            sha1: Some(sha1),
            size: Some(bytes.len() as u64),
        };
        assert_eq!(
            hydrator.try_hydrate(&item, &CancellationToken::new()).await,
            Some(bytes.len() as u64)
        );
        assert_eq!(fs::read(&dst_mod).unwrap(), bytes);
    }
}
