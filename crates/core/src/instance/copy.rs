//! 复制实例、把实例移到回收站。
//!
//! 复制：先复制到游戏目录的临时文件夹（`.mcl-tmp/copy-<编号>`），全部复制完再改名成
//! `instances/<新 ID>`，中途取消或失败不会在实例列表里留下半个实例。
//! 版本描述不用复制：副本和原来的指向 `versions/` 里同一个版本。
//! 不复制：日志、崩溃报告、截图、游戏的下载缓存、natives（启动时重新解出）、启动记录、改动历史（撤销记录指向原实例）；
//! 不带世界时也不带存档、游戏自己做的存档备份和世界封面。存档里的 `session.lock` 永远不复制（游戏开着时它被锁着）。
//!
//! 挂上的别家版本复制出来是本启动器自己的实例：只复制游戏文件（别家的版本描述、依赖库、
//! PCL 自己的设置这些不要），游戏本体和加载器由本启动器自己装。从别家导入实例也走 [`copy_folder`]。

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use super::{CONFIG_FILE, Instance, InstanceConfig, InstanceError, META_DIR, load, register};
use crate::fs::UnsafePath;
use crate::paths::GameDir;

/// 放复制到一半的实例的临时文件夹前缀（启动器启动时清理）
pub const COPY_TEMP_PREFIX: &str = "copy-";

/// 实例文件夹第一层里不复制的
const SKIP_TOP: &[&str] = &[
    "logs",
    "downloads",
    "crash-reports",
    "screenshots",
    "debug",
    ".mixin.out",
    ".fabric",
    ".cache",
];
/// 不带世界时第一层里也不复制的
const WORLD_TOP: &[&str] = &["saves", "backups"];
/// 启动器自己的文件夹里不复制的（instance.json 另外写新的；世界备份很大，留在原实例）
const SKIP_META: &[&str] = &[
    "launches",
    "history.json",
    CONFIG_FILE,
    "backups",
    "backups.json",
    "world-restore.json",
    "journal.json",
];
/// 跟着世界走的：世界封面、世界笔记（不带世界时不复制）
const META_WORLD: &[&str] = &["covers", "world-notes.json"];

#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error("找不到这个实例")]
    NotFound,
    #[error("已经有一个叫「{0}」的实例了")]
    Taken(String),
    #[error("已取消")]
    Cancelled,
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("移到回收站失败：{0}")]
    Trash(String),
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> CopyError + '_ {
    move |source| CopyError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct CopyOptions {
    /// 新实例的显示名
    pub name: String,
    /// 新实例的文件夹名（用 [`super::unique_id`] 取一个没被占用的）
    pub id: String,
    pub include_worlds: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: usize,
    pub total_files: usize,
}

/// 要复制的一个文件：相对实例文件夹的路径（按段）
struct Planned {
    parts: Vec<String>,
    size: u64,
}

/// 这个相对路径要不要复制。`extra_skip` 是第一层里另外不要的（小写比较）
fn wanted(parts: &[String], include_worlds: bool, extra_skip: &[String]) -> bool {
    let Some(top) = parts.first().map(String::as_str) else {
        return false;
    };
    let top_lower = top.to_lowercase();
    if SKIP_TOP.contains(&top_lower.as_str())
        || top_lower.starts_with("natives")
        || extra_skip
            .iter()
            .any(|skip| skip.to_lowercase() == top_lower)
    {
        return false;
    }
    if !include_worlds && WORLD_TOP.contains(&top_lower.as_str()) {
        return false;
    }
    if top == META_DIR {
        let second = parts.get(1).map(String::as_str).unwrap_or_default();
        if SKIP_META.contains(&second) || (!include_worlds && META_WORLD.contains(&second)) {
            return false;
        }
    }
    let is_lock = parts
        .last()
        .is_some_and(|name| name.eq_ignore_ascii_case("session.lock"));
    !(top_lower == "saves" && is_lock)
}

/// 列出要复制的文件（不跟随符号链接：链接指向哪里我们管不着）
fn plan(
    src: &Path,
    include_worlds: bool,
    extra_skip: &[String],
) -> Result<Vec<Planned>, CopyError> {
    let mut found = Vec::new();
    let mut stack: Vec<(PathBuf, Vec<String>)> = vec![(src.to_path_buf(), Vec::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(io_error(&dir))?;
        for entry in entries {
            let entry = entry.map_err(io_error(&dir))?;
            let Ok(name) = entry.file_name().into_string() else {
                tracing::warn!(path = %entry.path().display(), "文件名不是有效的文字，跳过");
                continue;
            };
            let mut parts = prefix.clone();
            parts.push(name);
            if !wanted(&parts, include_worlds, extra_skip) {
                continue;
            }
            let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
            if file_type.is_symlink() {
                tracing::warn!(path = %entry.path().display(), "符号链接不复制");
            } else if file_type.is_dir() {
                stack.push((entry.path(), parts));
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                found.push(Planned { parts, size });
            }
        }
    }
    Ok(found)
}

fn join(root: &Path, parts: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(parts);
    path
}

/// 把列好的文件复制到临时文件夹，再整个改名成 `dest`
#[allow(clippy::too_many_arguments)]
fn copy_files(
    src: &Path,
    temp: &Path,
    dest: &Path,
    files: &[Planned],
    options: &CopyOptions,
    cancel: &CancellationToken,
    progress: &mut CopyProgress,
    on_progress: &impl Fn(CopyProgress),
) -> Result<(), CopyError> {
    for file in files {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let target = join(temp, &file.parts);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        let source = join(src, &file.parts);
        std::fs::copy(&source, &target).map_err(io_error(&source))?;
        progress.done_bytes += file.size;
        progress.done_files += 1;
        on_progress(*progress);
    }
    std::fs::create_dir_all(temp).map_err(io_error(temp))?;
    if dest.exists() {
        return Err(CopyError::Taken(options.id.clone()));
    }
    std::fs::rename(temp, dest).map_err(io_error(dest))?;
    Ok(())
}

/// 把一个文件夹里的游戏文件复制成新实例的文件夹 `instances/<options.id>/`（很慢，在后台线程里调用）：
/// 先复制到游戏目录的临时文件夹，全部复制完再改名，中途取消或失败不留半个。返回新实例的文件夹
/// （还没登记，调用方写 `instance.json`）。`extra_skip` 是第一层里另外不要的文件和文件夹
pub fn copy_folder(
    game: &GameDir,
    src: &Path,
    options: &CopyOptions,
    extra_skip: &[String],
    cancel: &CancellationToken,
    on_progress: impl Fn(CopyProgress),
) -> Result<PathBuf, CopyError> {
    let dest = game.instance_dir(&options.id)?;
    if dest.exists() {
        return Err(CopyError::Taken(options.id.clone()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }

    let files = plan(src, options.include_worlds, extra_skip)?;
    let total_bytes = files.iter().map(|f| f.size).sum();
    let mut progress = CopyProgress {
        done_bytes: 0,
        total_bytes,
        done_files: 0,
        total_files: files.len(),
    };
    on_progress(progress);

    let temp = game.root().join(".mcl-tmp").join(format!(
        "{COPY_TEMP_PREFIX}{}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = copy_files(
        src,
        &temp,
        &dest,
        &files,
        options,
        cancel,
        &mut progress,
        &on_progress,
    );
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_dir_all(&temp)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %cleanup, dir = %temp.display(), "清理复制到一半的文件失败");
        }
        return Err(error);
    }
    tracing::info!(
        to = %options.id,
        files = files.len(),
        bytes = total_bytes,
        worlds = options.include_worlds,
        "文件夹已复制成实例"
    );
    Ok(dest)
}

/// 挂上的别家版本复制出来时第一层不要的：别家的游戏本体、版本描述、启动器自己的设置
fn linked_skip(link: &super::Link) -> Vec<String> {
    let version = &link.version;
    let mut skip: Vec<String> = [
        "versions",
        "libraries",
        "assets",
        "runtime",
        "instances",
        "PCL",
        "BHL",
        "hmclversion.cfg",
        "launcher_profiles.json",
        "launcher_profiles_microsoft_store.json",
        "launcher_accounts.json",
        "launcher_accounts_microsoft_store.json",
        "launcher_settings.json",
        "launcher_log.txt",
        "launcher_msa_credentials.bin",
        "axolotl_config.json",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    skip.extend([
        format!("{version}.json"),
        format!("{version}.jar"),
        format!("{version}-natives"),
    ]);
    skip
}

/// 复制一个实例（很慢，在后台线程里调用）。返回新实例。
pub fn copy(
    game: &GameDir,
    from_id: &str,
    options: &CopyOptions,
    cancel: &CancellationToken,
    on_progress: impl Fn(CopyProgress),
) -> Result<Instance, CopyError> {
    let from = load(game, from_id)?.ok_or(CopyError::NotFound)?;
    let src = from.game_dir(game)?;
    let extra_skip = from
        .config
        .link
        .as_ref()
        .map(linked_skip)
        .unwrap_or_default();
    let dest = copy_folder(game, &src, options, &extra_skip, cancel, on_progress)?;

    let mut config = InstanceConfig {
        name: options.name.trim().to_owned(),
        created_at: crate::now_millis() as f64,
        last_played: None,
        // 副本和原来的在同一个分组，但不跟着置顶（置顶的是常玩的那个）
        pinned_at: None,
        ..from.config
    };
    if config.link.take().is_some() {
        // 别家的版本复制成自己的实例：版本按游戏版本和加载器由本启动器自己装
        config.version_id = String::new();
    }
    match register(game, &options.id, &config) {
        Ok(instance) => {
            tracing::info!(from = %from_id, to = %options.id, "实例已复制");
            Ok(instance)
        }
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_dir_all(&dest) {
                tracing::warn!(error = %cleanup, dir = %dest.display(), "清理复制失败的实例失败");
            }
            Err(error.into())
        }
    }
}

/// 把实例整个文件夹移到系统回收站（在后台线程里调用）。放不进回收站时（太大、盘上关了回收站）
/// Windows 会自己弹窗问要不要永久删除，不会悄悄删掉。
pub fn move_to_trash(game: &GameDir, id: &str) -> Result<(), CopyError> {
    if load(game, id)?.is_none() {
        return Err(CopyError::NotFound);
    }
    let dir = game.instance_dir(id)?;
    trash_dir(&dir)?;
    tracing::info!(instance = %id, "实例已移到回收站");
    Ok(())
}

#[cfg(windows)]
fn trash_dir(dir: &Path) -> Result<(), CopyError> {
    trash::delete(dir).map_err(|e| CopyError::Trash(e.to_string()))
}

#[cfg(not(windows))]
fn trash_dir(_dir: &Path) -> Result<(), CopyError> {
    Err(CopyError::Trash("这个系统还不支持".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{LoaderKind, LoaderRef, MemoryOverride};

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn sample(game: &GameDir) -> PathBuf {
        let dir = game.instance_dir("Pack").unwrap();
        let mut config = InstanceConfig::new(
            "Pack",
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.265".to_owned(),
            }),
        );
        config.last_played = Some(5.0);
        config.group = Some("生存".to_owned());
        config.pinned_at = Some(6.0);
        config.overrides.memory = Some(MemoryOverride {
            auto: false,
            max_memory_mb: 8192,
        });
        super::super::create(game, "Pack", &config).unwrap();
        write(&dir.join("mods/a.jar"), "jar");
        write(&dir.join("config/a.toml"), "x=1");
        write(&dir.join("options.txt"), "lang:zh_cn");
        write(&dir.join("logs/latest.log"), "log");
        write(&dir.join("natives-windows-x86_64/lwjgl.dll"), "dll");
        write(&dir.join("screenshots/a.png"), "png");
        write(&dir.join("saves/World/level.dat"), "nbt");
        write(&dir.join("saves/World/session.lock"), "lock");
        write(&dir.join("backups/World.zip"), "zip");
        write(&dir.join("Lumi/content.json"), "{}");
        write(&dir.join("Lumi/history.json"), "{}");
        write(&dir.join("Lumi/launches/1.json"), "{}");
        write(&dir.join("Lumi/covers/auto/x/1.jpg"), "jpg");
        dir
    }

    fn options(include_worlds: bool) -> CopyOptions {
        CopyOptions {
            name: "Pack 副本".to_owned(),
            id: "Pack 副本".to_owned(),
            include_worlds,
        }
    }

    #[test]
    fn copies_with_worlds_and_skips_logs_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        sample(&game);
        let copied = copy(
            &game,
            "Pack",
            &options(true),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();
        let new_dir = game.instance_dir("Pack 副本").unwrap();
        for kept in [
            "mods/a.jar",
            "config/a.toml",
            "options.txt",
            "saves/World/level.dat",
            "backups/World.zip",
            "Lumi/content.json",
            "Lumi/covers/auto/x/1.jpg",
        ] {
            assert!(new_dir.join(kept).is_file(), "{kept} 应该复制");
        }
        for skipped in [
            "logs",
            "natives-windows-x86_64",
            "screenshots",
            "saves/World/session.lock",
            "Lumi/history.json",
            "Lumi/launches",
        ] {
            assert!(!new_dir.join(skipped).exists(), "{skipped} 不该复制");
        }
        assert_eq!(
            copied.version_id(),
            "neoforge-21.1.265",
            "副本和原来的用同一个版本"
        );
        assert_eq!(copied.config.name, "Pack 副本");
        assert_eq!(copied.config.last_played, None);
        assert_eq!(
            copied.config.group.as_deref(),
            Some("生存"),
            "副本在同一个分组"
        );
        assert_eq!(copied.config.pinned_at, None, "副本不跟着置顶");
        assert_eq!(
            copied.config.overrides.memory.unwrap().max_memory_mb,
            8192,
            "单独设置跟着复制"
        );
        assert!(
            !game
                .root()
                .join(".mcl-tmp")
                .read_dir()
                .unwrap()
                .any(|_| true),
            "临时文件夹清掉了"
        );
    }

    #[test]
    fn copies_without_worlds() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        sample(&game);
        copy(
            &game,
            "Pack",
            &options(false),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();
        let new_dir = game.instance_dir("Pack 副本").unwrap();
        assert!(new_dir.join("mods/a.jar").is_file());
        assert!(!new_dir.join("saves").exists());
        assert!(!new_dir.join("backups").exists());
        assert!(!new_dir.join("Lumi/covers").exists());
    }

    #[test]
    fn cancelled_copy_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        sample(&game);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = copy(&game, "Pack", &options(true), &cancel, |_| {}).unwrap_err();
        assert!(matches!(error, CopyError::Cancelled));
        assert!(!game.instance_dir("Pack 副本").unwrap().exists());
        // 一开始就取消：临时文件夹可能根本没建；建了的话也要是空的
        let leftovers = game
            .root()
            .join(".mcl-tmp")
            .read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        assert!(!leftovers, "临时文件夹清掉了");
    }

    #[test]
    fn refuses_taken_names_and_missing_instances() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        sample(&game);
        std::fs::create_dir_all(game.instance_dir("Pack 副本").unwrap()).unwrap();
        let token = CancellationToken::new();
        assert!(matches!(
            copy(&game, "Pack", &options(true), &token, |_| {}),
            Err(CopyError::Taken(_))
        ));
        assert!(matches!(
            copy(&game, "Nope", &options(true), &token, |_| {}),
            Err(CopyError::NotFound)
        ));
    }
}
