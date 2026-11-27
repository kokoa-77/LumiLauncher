//! 存储空间分析、安全缓存清理与引用感知的闲置版本清理。
//!
//! 绝不触碰玩家的世界存档、模组、配置或正在进行的任务暂存目录。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::content::STAGING_DIR;
use crate::content::store::STORE_DIR;
use crate::external::Launcher;
use crate::external::root::detect_launcher;
use crate::fs::{read_json, safe_file_name};
use crate::install::processors::RECEIPT_FILE;
use crate::instance::{META_DIR, list as list_instances};
use crate::paths::GameDir;

#[derive(Deserialize)]
struct VersionParent {
    #[serde(rename = "inheritsFrom")]
    inherits_from: Option<String>,
}

const JAVA_DIR: &str = ".mcl-java";
const THUMBS_DIR: &str = "thumbs";
const BACKUPS_SUBDIR: &str = "backups";
const TEMP_PREFIXES: &[&str] = &[".mcl-temp", ".mcl-drops", ".mcl-pack-"];
/// 清理旧日志时至少保留最近几份，方便排查最近的问题
const KEEP_RECENT_LOGS: usize = 2;

/// 磁盘占用分项报告（前端统一用 f64 字节数，避免 JS 精度丢失）
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct StorageReport {
    /// 实例文件夹总计（不含其中的世界备份，避免重复统计）
    pub instances_bytes: f64,
    /// 共用游戏本体：versions + libraries + assets
    pub shared_game_bytes: f64,
    /// Lumi 自动下载的 Java 运行时
    pub java_bytes: f64,
    /// 世界备份总占用
    pub backups_bytes: f64,
    /// 可安全清理的缓存、缩略图、残留临时文件与旧日志
    pub cache_bytes: f64,
    /// 以上各项合计
    pub total_bytes: f64,
    /// 可清理的缓存文件数
    pub cache_files: u32,
    /// 游戏目录路径中是否包含 `!`（Java 解析含 `!` 路径时易异常）
    pub path_has_bang: bool,
}

/// 缓存清理结果
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct CacheCleanResult {
    pub freed_bytes: f64,
    pub removed_files: u32,
}

/// 未被任何实例引用的闲置游戏版本目录
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UnusedVersion {
    pub id: String,
    pub bytes: f64,
}

/// 清理闲置版本的结果
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UnusedVersionsCleanResult {
    pub freed_bytes: f64,
    pub removed_count: u32,
}

/// 路径里是否包含 `!`（Java JAR URL 语法用 `!` 分隔归档与内部条目，路径带 `!` 常导致启动失败）。
pub fn path_contains_bang(path: &Path) -> bool {
    path.to_string_lossy().contains('!')
}

/// 扫描当前游戏目录与数据目录的磁盘占用分布。
pub fn scan_storage(
    game: &GameDir,
    data_dir: &Path,
    keep_staging: &HashSet<PathBuf>,
) -> StorageReport {
    scan_storage_with_backups(game, data_dir, keep_staging, None)
}

/// 扫描当前游戏目录、数据目录以及可选自定义世界备份目录的磁盘占用分布。
pub fn scan_storage_with_backups(
    game: &GameDir,
    data_dir: &Path,
    keep_staging: &HashSet<PathBuf>,
    custom_backup_dir: Option<&Path>,
) -> StorageReport {
    let mut instances_bytes = 0u64;
    let mut backups_bytes = 0u64;
    let instances_root = game.instances();
    if let Ok(entries) = std::fs::read_dir(&instances_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let b = dir_size(&path.join(META_DIR).join(BACKUPS_SUBDIR)).0;
            let total = dir_size(&path).0;
            backups_bytes = backups_bytes.saturating_add(b);
            instances_bytes = instances_bytes.saturating_add(total.saturating_sub(b));
        }
    }
    if let Some(custom) = custom_backup_dir {
        let inside_instances = custom
            .canonicalize()
            .ok()
            .zip(instances_root.canonicalize().ok())
            .is_some_and(|(c, r)| c.starts_with(&r));
        if !inside_instances {
            backups_bytes = backups_bytes.saturating_add(dir_size(custom).0);
        }
    }

    let shared_game_bytes = dir_size(&game.versions())
        .0
        .saturating_add(dir_size(&game.libraries()).0)
        .saturating_add(dir_size(&game.assets()).0);

    let java_bytes = dir_size(&game.root().join(JAVA_DIR)).0;

    let (cache_bytes, cache_files) = scan_cache_targets(game, data_dir, keep_staging);
    let total_bytes = instances_bytes
        .saturating_add(shared_game_bytes)
        .saturating_add(java_bytes)
        .saturating_add(backups_bytes)
        .saturating_add(cache_bytes);

    StorageReport {
        instances_bytes: instances_bytes as f64,
        shared_game_bytes: shared_game_bytes as f64,
        java_bytes: java_bytes as f64,
        backups_bytes: backups_bytes as f64,
        cache_bytes: cache_bytes as f64,
        total_bytes: total_bytes as f64,
        cache_files: u32::try_from(cache_files).unwrap_or(u32::MAX),
        path_has_bang: path_contains_bang(game.root()),
    }
}

/// 安全清理可再生缓存（缩略图缓存、闲置临时暂存目录、过期启动器日志），绝不碰存档、模组和备份。
pub fn clean_cache(
    game: &GameDir,
    data_dir: &Path,
    keep_staging: &HashSet<PathBuf>,
) -> CacheCleanResult {
    let mut freed_bytes = 0u64;
    let mut removed_files = 0u64;

    // 1. 缩略图缓存（可随时按需重新生成）
    let thumbs = game.root().join(STORE_DIR).join(THUMBS_DIR);
    let (b, c) = remove_dir_contents(&thumbs);
    freed_bytes = freed_bytes.saturating_add(b);
    removed_files = removed_files.saturating_add(c);

    // 2. 内容暂存目录中不再被任务持有的目录
    let staging = game.root().join(STORE_DIR).join(STAGING_DIR);
    if let Ok(entries) = std::fs::read_dir(&staging) {
        for entry in entries.flatten() {
            let p = entry.path();
            if keep_staging.contains(&p) {
                continue;
            }
            let (b, c) = remove_path_counting(&p);
            freed_bytes = freed_bytes.saturating_add(b);
            removed_files = removed_files.saturating_add(c);
        }
    }

    // 3. 游戏根目录下的临时目录（排除正在进行的暂存）
    if let Ok(entries) = std::fs::read_dir(game.root()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !TEMP_PREFIXES
                .iter()
                .any(|prefix| name_str.starts_with(prefix))
            {
                continue;
            }
            let p = entry.path();
            if keep_staging.contains(&p) {
                continue;
            }
            let (b, c) = remove_path_counting(&p);
            freed_bytes = freed_bytes.saturating_add(b);
            removed_files = removed_files.saturating_add(c);
        }
    }

    // 4. 超过 24 小时的孤儿分段碎片（`.part` / `.part.s*` / `.part.meta`）
    let (orphan_bytes, orphan_files) =
        crate::net::clean_orphan_part_files(game.root(), std::time::Duration::from_secs(24 * 3600));
    freed_bytes = freed_bytes.saturating_add(orphan_bytes);
    removed_files = removed_files.saturating_add(orphan_files);

    // 5. 旧启动器日志文件（保留最近 2 份）
    for old_log in removable_old_logs(&data_dir.join("logs")) {
        let (b, c) = remove_path_counting(&old_log);
        freed_bytes = freed_bytes.saturating_add(b);
        removed_files = removed_files.saturating_add(c);
    }

    CacheCleanResult {
        freed_bytes: freed_bytes as f64,
        removed_files: u32::try_from(removed_files).unwrap_or(u32::MAX),
    }
}

/// 找出 `versions/` 下已没有任何 Lumi 实例直接或间接（`inheritsFrom`）引用的闲置版本目录。
/// 只列能放心删的：游戏目录不是别家启动器的，版本文件夹里也只有能重新下载的文件（[`only_game_files`]）。
pub fn find_unused_versions(game: &GameDir) -> Vec<UnusedVersion> {
    let mut unused = Vec::new();
    if shared_with_other_launcher(game) {
        return unused;
    }
    let referenced = collect_referenced_versions(game);
    let Ok(entries) = std::fs::read_dir(game.versions()) else {
        return unused;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') || referenced.contains(&id) || !only_game_files(&path, &id) {
            continue;
        }
        let bytes = dir_size(&path).0;
        unused.push(UnusedVersion {
            id,
            bytes: bytes as f64,
        });
    }
    unused.sort_by(|a, b| b.bytes.total_cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    unused
}

/// 仅删除明确传入、且**复核后仍是闲置**的版本目录（没被任何实例引用、不是别家启动器的目录、里面只有能重新下载的文件）。
pub fn clean_unused_versions(game: &GameDir, ids: &[String]) -> UnusedVersionsCleanResult {
    let mut freed_bytes = 0u64;
    let mut removed_count = 0u32;
    if shared_with_other_launcher(game) {
        return UnusedVersionsCleanResult {
            freed_bytes: 0.0,
            removed_count,
        };
    }
    let referenced = collect_referenced_versions(game);
    let mut seen = HashSet::new();

    for id in ids {
        if !seen.insert(id.as_str()) || referenced.contains(id) {
            continue;
        }
        let Ok(safe_name) = safe_file_name(id) else {
            continue;
        };
        let dir = game.versions().join(safe_name);
        // 不跟着链接走：只认真正的文件夹
        let plain_dir = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir());
        if !plain_dir || !only_game_files(&dir, id) {
            continue;
        }
        let size = dir_size(&dir).0;
        if std::fs::remove_dir_all(&dir).is_ok() {
            freed_bytes = freed_bytes.saturating_add(size);
            removed_count = removed_count.saturating_add(1);
        }
    }

    UnusedVersionsCleanResult {
        freed_bytes: freed_bytes as f64,
        removed_count,
    }
}

/// 游戏目录是别家启动器的（官方启动器、PCL、HMCL）：里面的版本可能是别家装的、别家在用，不清理
fn shared_with_other_launcher(game: &GameDir) -> bool {
    !matches!(detect_launcher(game.root()), Launcher::Other)
}

/// 版本文件夹里是不是只有 Lumi 装游戏时放的、能重新下载的文件：版本描述、游戏本体、安装回执、natives 文件夹。
/// 别家启动器开了版本隔离时，存档、模组、配置都在版本文件夹里（PCL 还有 `PCL/`、HMCL 有 `hmclversion.cfg`）；
/// 多出任何一样都不算闲置，一律不碰。
fn only_game_files(dir: &Path, id: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let json = format!("{id}.json");
    let jar = format!("{id}.jar");
    entries.into_iter().all(|entry| {
        let Ok(entry) = entry else {
            return false;
        };
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_file() {
            name == json || name == jar || name == RECEIPT_FILE
        } else if kind.is_dir() && !kind.is_symlink() {
            name.starts_with("natives-")
        } else {
            false
        }
    })
}

fn collect_referenced_versions(game: &GameDir) -> HashSet<String> {
    let mut referenced = HashSet::new();
    let mut queue = Vec::new();
    for inst in list_instances(game) {
        let vid = inst.version_id();
        if !vid.is_empty() && referenced.insert(vid.clone()) {
            queue.push(vid);
        }
        if !inst.config.game_version.is_empty()
            && referenced.insert(inst.config.game_version.clone())
        {
            queue.push(inst.config.game_version);
        }
    }
    // 顺着 version json 的 inheritsFrom 递归把父版本（如 NeoForge 继承的原版 1.21.1）全部保护起来
    let mut depth = 0usize;
    while let Some(version_id) = queue.pop() {
        depth += 1;
        if depth > 256 {
            break;
        }
        let Ok(json_path) = game.version_json(&version_id) else {
            continue;
        };
        if let Ok(Some(meta)) = read_json::<VersionParent>(&json_path)
            && let Some(parent) = meta.inherits_from
            && referenced.insert(parent.clone())
        {
            queue.push(parent);
        }
    }
    referenced
}

fn scan_cache_targets(
    game: &GameDir,
    data_dir: &Path,
    keep_staging: &HashSet<PathBuf>,
) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;

    let (b, c) = dir_size(&game.root().join(STORE_DIR).join(THUMBS_DIR));
    bytes = bytes.saturating_add(b);
    files = files.saturating_add(c);

    let staging = game.root().join(STORE_DIR).join(STAGING_DIR);
    if let Ok(entries) = std::fs::read_dir(&staging) {
        for entry in entries.flatten() {
            let p = entry.path();
            if keep_staging.contains(&p) {
                continue;
            }
            let (b, c) = dir_size(&p);
            bytes = bytes.saturating_add(b);
            files = files.saturating_add(c);
        }
    }

    if let Ok(entries) = std::fs::read_dir(game.root()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !TEMP_PREFIXES
                .iter()
                .any(|prefix| name_str.starts_with(prefix))
            {
                continue;
            }
            let p = entry.path();
            if keep_staging.contains(&p) {
                continue;
            }
            let (b, c) = dir_size(&p);
            bytes = bytes.saturating_add(b);
            files = files.saturating_add(c);
        }
    }

    let (orphan_bytes, orphan_files) =
        crate::net::scan_orphan_part_files(game.root(), std::time::Duration::from_secs(24 * 3600));
    bytes = bytes.saturating_add(orphan_bytes);
    files = files.saturating_add(orphan_files);

    for old_log in removable_old_logs(&data_dir.join("logs")) {
        if let Ok(meta) = std::fs::symlink_metadata(&old_log)
            && meta.is_file()
        {
            bytes = bytes.saturating_add(meta.len());
            files = files.saturating_add(1);
        }
    }

    let (orphan_bytes, orphan_files) =
        crate::net::scan_orphan_part_files(game.root(), std::time::Duration::from_secs(24 * 3600));
    bytes = bytes.saturating_add(orphan_bytes);
    files = files.saturating_add(orphan_files);

    (bytes, files)
}

fn removable_old_logs(log_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            Some((modified, path))
        })
        .collect();
    if logs.len() <= KEEP_RECENT_LOGS {
        return Vec::new();
    }
    // 最新排前，只返回超过保留份数的旧日志
    logs.sort_by_key(|a| std::cmp::Reverse(a.0));
    logs.into_iter()
        .skip(KEEP_RECENT_LOGS)
        .map(|(_, p)| p)
        .collect()
}

/// 递归统计文件大小与文件数，不跟随符号链接避免死循环或跨目录误算。
fn dir_size(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return (0, 0);
    };
    if meta.file_type().is_symlink() {
        return (0, 0);
    }
    if meta.is_file() {
        return (meta.len(), 1);
    }
    if !meta.is_dir() {
        return (0, 0);
    }
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(m) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if m.file_type().is_symlink() {
                continue;
            }
            if m.is_file() {
                bytes = bytes.saturating_add(m.len());
                files = files.saturating_add(1);
            } else if m.is_dir() {
                stack.push(p);
            }
        }
    }
    (bytes, files)
}

fn remove_dir_contents(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in entries.flatten() {
        let (b, c) = remove_path_counting(&entry.path());
        bytes = bytes.saturating_add(b);
        files = files.saturating_add(c);
    }
    (bytes, files)
}

fn remove_path_counting(path: &Path) -> (u64, u64) {
    let (b, c) = dir_size(path);
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return (0, 0);
    };
    let ok = if meta.is_dir() && !meta.file_type().is_symlink() {
        std::fs::remove_dir_all(path).is_ok()
    } else {
        std::fs::remove_file(path).is_ok()
    };
    if ok { (b, c) } else { (0, 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_storage_without_double_counting_backups_and_cleans_only_cache() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());

        // 实例文件（100 字节）+ 实例内世界备份（200 字节）
        let inst = game.instances().join("survival");
        std::fs::create_dir_all(inst.join("saves/world")).unwrap();
        std::fs::write(inst.join("saves/world/level.dat"), vec![1u8; 100]).unwrap();
        let backup_dir = inst.join(META_DIR).join(BACKUPS_SUBDIR).join("world");
        std::fs::create_dir_all(&backup_dir).unwrap();
        std::fs::write(backup_dir.join("1.zip"), vec![2u8; 200]).unwrap();

        // 共享资产（300 字节）+ Java（400 字节）
        std::fs::create_dir_all(game.versions().join("1.21.1")).unwrap();
        std::fs::write(game.versions().join("1.21.1/1.21.1.jar"), vec![3u8; 300]).unwrap();
        std::fs::create_dir_all(game.root().join(JAVA_DIR)).unwrap();
        std::fs::write(game.root().join(JAVA_DIR).join("java.bin"), vec![4u8; 400]).unwrap();

        // 缓存缩略图（50 字节）+ 3 份日志（只清最老那 1 份，60 字节）
        let thumbs = game.root().join(STORE_DIR).join(THUMBS_DIR);
        std::fs::create_dir_all(&thumbs).unwrap();
        std::fs::write(thumbs.join("a.jpg"), vec![5u8; 50]).unwrap();
        let logs = data.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("launcher.2026-01-01.log"), vec![6u8; 60]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(logs.join("launcher.2026-01-02.log"), vec![6u8; 70]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(logs.join("launcher.2026-01-03.log"), vec![6u8; 80]).unwrap();

        let keep = HashSet::new();
        let report = scan_storage(&game, data.path(), &keep);
        assert_eq!(report.instances_bytes, 100.0);
        assert_eq!(report.backups_bytes, 200.0);
        assert_eq!(report.shared_game_bytes, 300.0);
        assert_eq!(report.java_bytes, 400.0);
        assert_eq!(report.cache_bytes, 110.0);
        assert_eq!(report.total_bytes, 1110.0);
        assert_eq!(report.cache_files, 2);

        let cleaned = clean_cache(&game, data.path(), &keep);
        assert_eq!(cleaned.freed_bytes, 110.0);
        assert_eq!(cleaned.removed_files, 2);
        // 存档、备份、最近两份日志完好无损
        assert!(inst.join("saves/world/level.dat").exists());
        assert!(backup_dir.join("1.zip").exists());
        assert!(logs.join("launcher.2026-01-02.log").exists());
        assert!(logs.join("launcher.2026-01-03.log").exists());
        assert!(!logs.join("launcher.2026-01-01.log").exists());
    }

    #[test]
    fn finds_and_cleans_only_unreferenced_versions_preserving_parents() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());

        // 实例引用 neoforge-1.21.1，后者 inheritsFrom 1.21.1；1.19.4 无人引用
        let inst = game.instances().join("my-pack");
        std::fs::create_dir_all(inst.join(META_DIR)).unwrap();
        std::fs::write(
            inst.join(META_DIR).join("instance.json"),
            r#"{"format":2,"name":"我的整合包","versionId":"neoforge-1.21.1","gameVersion":"1.21.1","loader":null,"source":null,"createdAt":1}"#,
        )
        .unwrap();

        for (id, json) in [
            (
                "neoforge-1.21.1",
                r#"{"id":"neoforge-1.21.1","inheritsFrom":"1.21.1","type":"release","mainClass":"m"}"#,
            ),
            (
                "1.21.1",
                r#"{"id":"1.21.1","type":"release","mainClass":"m"}"#,
            ),
            (
                "1.19.4",
                r#"{"id":"1.19.4","type":"release","mainClass":"m"}"#,
            ),
        ] {
            let vdir = game.versions().join(id);
            std::fs::create_dir_all(&vdir).unwrap();
            std::fs::write(vdir.join(format!("{id}.json")), json).unwrap();
            std::fs::write(vdir.join(format!("{id}.jar")), vec![9u8; 500]).unwrap();
        }

        let unused = find_unused_versions(&game);
        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].id, "1.19.4");

        // 即使强传仍在使用的 1.21.1，也绝不会删
        let res = clean_unused_versions(&game, &["1.21.1".to_owned(), "1.19.4".to_owned()]);
        assert_eq!(res.removed_count, 1);
        assert!(game.versions().join("1.21.1").exists());
        assert!(game.versions().join("neoforge-1.21.1").exists());
        assert!(!game.versions().join("1.19.4").exists());
    }

    #[test]
    fn never_lists_or_cleans_versions_holding_other_files() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());
        let version = |id: &str, extra: &[&str]| {
            let vdir = game.versions().join(id);
            std::fs::create_dir_all(vdir.join("natives-windows-x86_64")).unwrap();
            std::fs::write(vdir.join(format!("{id}.json")), "{}").unwrap();
            std::fs::write(vdir.join(format!("{id}.jar")), vec![9u8; 100]).unwrap();
            for name in extra {
                if let Some(dir) = name.strip_suffix('/') {
                    std::fs::create_dir_all(vdir.join(dir).join("New World")).unwrap();
                } else {
                    std::fs::write(vdir.join(name), "x").unwrap();
                }
            }
        };
        // Lumi 装的：版本描述、本体、回执、natives
        version("1.19.4", &[RECEIPT_FILE]);
        // 版本隔离：存档、模组、配置在版本文件夹里
        version("1.20.1-隔离", &["saves/", "mods/", "options.txt"]);

        let unused: Vec<String> = find_unused_versions(&game)
            .into_iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(unused, ["1.19.4"]);

        // 界面强传带存档的版本也不删
        let all = ["1.19.4", "1.20.1-隔离"].map(str::to_owned);
        let res = clean_unused_versions(&game, &all);
        assert_eq!(res.removed_count, 1);
        assert!(!game.versions().join("1.19.4").exists());
        assert!(
            game.versions()
                .join("1.20.1-隔离")
                .join("saves")
                .join("New World")
                .is_dir()
        );

        // 认出是 PCL / HMCL 的游戏目录：整个目录都不清理，连只有游戏文件的版本也不列
        for marker in ["PCL/", "hmclversion.cfg"] {
            let root = tempfile::tempdir().unwrap();
            let game = GameDir::new(root.path());
            for (id, extra) in [("1.19.4", &[][..]), ("1.20.4-别家", &[marker][..])] {
                let vdir = game.versions().join(id);
                std::fs::create_dir_all(&vdir).unwrap();
                std::fs::write(vdir.join(format!("{id}.json")), "{}").unwrap();
                for name in extra {
                    match name.strip_suffix('/') {
                        Some(dir) => std::fs::create_dir_all(vdir.join(dir)).unwrap(),
                        None => std::fs::write(vdir.join(name), "x").unwrap(),
                    }
                }
            }
            assert!(find_unused_versions(&game).is_empty(), "{marker}");
            let res = clean_unused_versions(&game, &["1.19.4".to_owned()]);
            assert_eq!(res.removed_count, 0, "{marker}");
            assert!(game.versions().join("1.19.4").is_dir(), "{marker}");
        }
    }

    #[test]
    fn detects_bang_character_in_game_dir_path() {
        assert!(path_contains_bang(Path::new(r"D:\Games\MC!\.minecraft")));
        assert!(!path_contains_bang(Path::new(
            r"D:\Games\Minecraft\.minecraft"
        )));
    }
}
