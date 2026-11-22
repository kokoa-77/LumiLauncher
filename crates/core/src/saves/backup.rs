//! 世界备份。
//!
//! - 每份备份是一个 zip，放在实例的 `Lumi/backups/<世界文件夹>/<时间>.zip`；
//!   压缩包里带世界文件夹名（`<世界文件夹>/level.dat`），和游戏自己「备份」出来的一样，玩家也能手动解开用。
//! - `Lumi/backups.json` 记每份备份的来由、大小，和备份时世界的「指纹」（文件数、总大小、最新修改时间）：
//!   指纹没变说明世界没变，自动备份就跳过。
//! - 自动备份（玩完、进之前）每个世界只留最近几份；手动备份和「恢复前」那份不自动删。
//! - 恢复：先把现在的世界备一份，解压到 `saves/.mcl-restore-*`，再整个换上去（原来的先改名成 `saves/.mcl-old-*`）。
//!   换之前写 `Lumi/world-restore.json`，换到一半被打断，下次 [`Backups::settle`] 时复原或者收尾。
//!
//! 这里的函数都读写很多文件，在后台线程里调用；游戏开着时不能调（调用方拦）。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::fs::{UnsafePath, read_json, safe_file_name, safe_join, write_json_atomic};
use crate::instance::META_DIR;

const BACKUP_DIR: &str = "backups";
const INDEX_FILE: &str = "backups.json";
const RESTORE_FILE: &str = "world-restore.json";
/// 超过这么大的世界，第一次自动备份前先问用户
pub const LARGE_WORLD_BYTES: u64 = 1 << 30;
/// 启动器在 `saves` 里用的临时文件夹前缀（列世界时跳过）
pub const TEMP_PREFIX: &str = ".mcl-";
/// 玩过的世界：存档在「这次开始」前多久以内被写过都算（时钟误差、游戏先读后写）
const PLAYED_SLACK_MS: u64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("找不到这个世界")]
    NoWorld,
    #[error("找不到这份备份（可能已经被删了）")]
    NoBackup,
    #[error("已取消")]
    Cancelled,
    #[error("备份文件坏了：{0}")]
    BadArchive(String),
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error("保存备份记录失败")]
    Write(#[from] crate::fs::AtomicWriteError),
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> BackupError + '_ {
    move |source| BackupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 这份备份是怎么来的
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum BackupReason {
    /// 玩完自动备的
    AfterPlay,
    /// 从首页进世界之前备的（世界在上次备份之后变过）
    BeforePlay,
    /// 恢复别的备份之前，把当时的世界备了一份
    BeforeRestore,
    /// 用户点的
    Manual,
}

impl BackupReason {
    /// 自动备份：每个世界只留最近几份
    pub fn automatic(self) -> bool {
        matches!(self, BackupReason::AfterPlay | BackupReason::BeforePlay)
    }
}

/// 世界的指纹：文件数、总大小、最新的修改时间（毫秒）。三样都没变就当世界没变
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub files: u64,
    pub bytes: u64,
    pub newest: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    at: f64,
    reason: BackupReason,
    /// 压缩包多大
    size: u64,
    fingerprint: Fingerprint,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorldIndex {
    /// 大世界要不要自动备份（问过用户才有）
    #[serde(default)]
    auto_large: Option<bool>,
    #[serde(default)]
    entries: Vec<Entry>,
}

type Index = BTreeMap<String, WorldIndex>;

/// 给界面看的一份备份
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub id: String,
    /// 备份时间（毫秒）
    pub at: f64,
    pub reason: BackupReason,
    /// 压缩包大小（字节）
    pub size: f64,
}

impl From<&Entry> for BackupInfo {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.clone(),
            at: entry.at,
            reason: entry.reason,
            size: entry.size as f64,
        }
    }
}

/// 自动备份前看一眼该不该备
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCheck {
    /// 和上次备份时一样，不用备
    Unchanged,
    /// 大世界，还没问过用户
    AskLarge { bytes: u64 },
    /// 大世界，用户说过不要自动备份
    SkipLarge,
    /// 要备
    Go,
}

/// 恢复被打断时留的记号
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreMarker {
    folder: String,
    /// 原来的世界改名成的（`saves` 下面）
    old: String,
    /// 解压出来的新世界（`saves` 下面）
    new: String,
}

/// 迁移世界备份目录的结果
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct BackupMigrateResult {
    pub moved_files: u32,
    pub moved_bytes: f64,
}

/// 为自定义备份根目录生成一个稳定、人类可读且跨盘不碰撞的实例子目录名。
pub fn instance_storage_key(instance_dir: &Path) -> String {
    let canonical = instance_dir
        .canonicalize()
        .unwrap_or_else(|_| instance_dir.to_path_buf());
    let norm = canonical.to_string_lossy().to_lowercase();
    let hash = Sha256::digest(norm.as_bytes());
    let short_hash = format!(
        "{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    );
    let base: String = instance_dir
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .take(32)
        .collect::<String>()
        .trim()
        .to_owned();
    if base.is_empty() {
        format!("instance-{short_hash}")
    } else {
        format!("{base}-{short_hash}")
    }
}

/// 一个实例的世界备份
pub struct Backups {
    instance_dir: PathBuf,
    /// 备份压缩包与 `backups.json` 的存放目录：默认是 `<instance>/Lumi`，
    /// 配置自定义备份根目录时是 `<custom_root>/<instance_storage_key>`。
    storage_root: PathBuf,
}

impl Backups {
    pub fn new(instance_dir: impl Into<PathBuf>) -> Self {
        Self::with_root(instance_dir, None)
    }

    pub fn with_root(instance_dir: impl Into<PathBuf>, custom_root: Option<&Path>) -> Self {
        let instance_dir = instance_dir.into();
        let storage_root = match custom_root {
            Some(root) => root.join(instance_storage_key(&instance_dir)),
            None => instance_dir.join(META_DIR),
        };
        Self {
            instance_dir,
            storage_root,
        }
    }

    fn meta(&self, file: &str) -> PathBuf {
        self.instance_dir.join(META_DIR).join(file)
    }

    fn storage(&self, file: &str) -> PathBuf {
        self.storage_root.join(file)
    }

    fn saves(&self) -> PathBuf {
        self.instance_dir.join("saves")
    }

    fn world_dir(&self, folder: &str) -> Result<PathBuf, BackupError> {
        let folder = safe_file_name(folder)?;
        if folder.starts_with(TEMP_PREFIX) {
            return Err(BackupError::NoWorld);
        }
        Ok(self.saves().join(folder))
    }

    fn backup_dir(&self, folder: &str) -> Result<PathBuf, BackupError> {
        Ok(safe_join(
            &self.storage(BACKUP_DIR),
            safe_file_name(folder)?,
        )?)
    }

    fn zip_path(&self, folder: &str, id: &str) -> Result<PathBuf, BackupError> {
        Ok(self
            .backup_dir(folder)?
            .join(format!("{}.zip", safe_file_name(id)?)))
    }

    fn load_index(&self) -> Index {
        match read_json(&self.storage(INDEX_FILE)) {
            Ok(index) => index.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "备份记录读不了，按空的处理（备份文件还在）");
                Index::new()
            }
        }
    }

    fn save_index(&self, index: &Index) -> Result<(), BackupError> {
        Ok(write_json_atomic(&self.storage(INDEX_FILE), index)?)
    }

    /// 这个世界的备份，新的在前（文件被手动删掉的不算）
    pub fn list(&self, folder: &str) -> Vec<BackupInfo> {
        let index = self.load_index();
        let Some(world) = index.get(folder) else {
            return Vec::new();
        };
        let mut list: Vec<BackupInfo> = world
            .entries
            .iter()
            .filter(|e| self.zip_path(folder, &e.id).is_ok_and(|p| p.is_file()))
            .map(BackupInfo::from)
            .collect();
        list.sort_by(|a, b| b.at.total_cmp(&a.at));
        list
    }

    /// 这个实例的备份一共占多少字节、几份
    pub fn usage(&self) -> (u64, u64) {
        let mut bytes = 0;
        let mut count = 0;
        let root = self.storage(BACKUP_DIR);
        let Ok(worlds) = std::fs::read_dir(&root) else {
            return (0, 0);
        };
        for world in worlds.flatten() {
            let Ok(files) = std::fs::read_dir(world.path()) else {
                continue;
            };
            for file in files.flatten() {
                if let Ok(meta) = file.metadata()
                    && meta.is_file()
                {
                    bytes += meta.len();
                    if file.path().extension().is_some_and(|x| x == "zip") {
                        count += 1;
                    }
                }
            }
        }
        (bytes, count)
    }

    /// 删掉这个实例的全部备份（大世界要不要备份的选择留着）。返回删掉了多少字节
    pub fn clear(&self) -> Result<u64, BackupError> {
        let (bytes, _) = self.usage();
        let root = self.storage(BACKUP_DIR);
        match std::fs::remove_dir_all(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(BackupError::Io { path: root, source }),
        }
        let mut index = self.load_index();
        index.retain(|_, world| {
            world.entries.clear();
            world.auto_large.is_some()
        });
        self.save_index(&index)?;
        Ok(bytes)
    }

    /// 自动备份之前看一眼：世界变了没有、是不是要先问用户
    pub fn auto_check(&self, folder: &str) -> Result<AutoCheck, BackupError> {
        let dir = self.world_dir(folder)?;
        if !dir.join("level.dat").is_file() {
            return Err(BackupError::NoWorld);
        }
        let fingerprint = fingerprint(&dir)?;
        let index = self.load_index();
        let world = index.get(folder);
        // 现在的样子和哪一份备份一样（刚备过、刚从备份恢复）：已经有了，不用再备
        let saved = world.is_some_and(|w| {
            w.entries.iter().any(|e| {
                e.fingerprint == fingerprint
                    && self.zip_path(folder, &e.id).is_ok_and(|p| p.is_file())
            })
        });
        if saved {
            return Ok(AutoCheck::Unchanged);
        }
        if fingerprint.bytes > LARGE_WORLD_BYTES {
            return Ok(match world.and_then(|w| w.auto_large) {
                None => AutoCheck::AskLarge {
                    bytes: fingerprint.bytes,
                },
                Some(false) => AutoCheck::SkipLarge,
                Some(true) => AutoCheck::Go,
            });
        }
        Ok(AutoCheck::Go)
    }

    /// 记下用户对大世界的选择
    pub fn set_auto_large(&self, folder: &str, auto: bool) -> Result<(), BackupError> {
        self.world_dir(folder)?;
        let mut index = self.load_index();
        index.entry(folder.to_owned()).or_default().auto_large = Some(auto);
        self.save_index(&index)
    }

    /// 备份一个世界。`keep` 是自动备份每个世界留几份（多出来的删最旧的）。
    /// `on_progress(已打包字节, 总字节)`。返回这份备份。
    pub fn create(
        &self,
        folder: &str,
        reason: BackupReason,
        keep: usize,
        cancel: &CancellationToken,
        on_progress: impl Fn(u64, u64),
    ) -> Result<BackupInfo, BackupError> {
        self.settle()?;
        let dir = self.world_dir(folder)?;
        if !dir.join("level.dat").is_file() {
            return Err(BackupError::NoWorld);
        }
        let files = world_files(&dir)?;
        let fingerprint = fingerprint_of(&files);
        let mut id = crate::now_millis().to_string();
        let out_dir = self.backup_dir(folder)?;
        std::fs::create_dir_all(&out_dir).map_err(io_error(&out_dir))?;
        // 同一毫秒里备了两次（几乎不会）：换个名字
        while out_dir.join(format!("{id}.zip")).exists() {
            id.push('a');
        }
        let target = out_dir.join(format!("{id}.zip"));
        let part = out_dir.join(format!("{id}.zip.part"));
        let written = write_zip(
            &part,
            folder,
            &files,
            fingerprint.bytes,
            cancel,
            &on_progress,
        );
        let size = match written {
            Ok(size) => size,
            Err(error) => {
                if let Err(cleanup) = std::fs::remove_file(&part) {
                    tracing::debug!(%cleanup, "清理没写完的备份失败");
                }
                return Err(error);
            }
        };
        std::fs::rename(&part, &target).map_err(io_error(&target))?;

        let entry = Entry {
            id,
            at: crate::now_millis() as f64,
            reason,
            size,
            fingerprint,
        };
        let info = BackupInfo::from(&entry);
        let mut index = self.load_index();
        let world = index.entry(folder.to_owned()).or_default();
        world.entries.push(entry);
        // 自动备份只留最近 keep 份
        let mut automatic: Vec<(f64, String)> = world
            .entries
            .iter()
            .filter(|e| e.reason.automatic())
            .map(|e| (e.at, e.id.clone()))
            .collect();
        automatic.sort_by(|a, b| b.0.total_cmp(&a.0));
        let dropped: Vec<String> = automatic
            .into_iter()
            .skip(keep.max(1))
            .map(|(_, id)| id)
            .collect();
        world.entries.retain(|e| !dropped.contains(&e.id));
        self.save_index(&index)?;
        for id in dropped {
            let path = self.zip_path(folder, &id)?;
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(%error, file = %path.display(), "删旧备份失败");
            }
        }
        tracing::info!(folder, reason = ?reason, size, "世界备份好了");
        Ok(info)
    }

    /// 删一份备份
    pub fn delete(&self, folder: &str, id: &str) -> Result<(), BackupError> {
        let path = self.zip_path(folder, id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(BackupError::Io { path, source }),
        }
        let mut index = self.load_index();
        if let Some(world) = index.get_mut(folder) {
            world.entries.retain(|e| e.id != id);
        }
        self.save_index(&index)
    }

    /// 备份文件在哪（「在文件夹中显示」用）
    pub fn locate(&self, folder: &str, id: &str) -> Result<PathBuf, BackupError> {
        let path = self.zip_path(folder, id)?;
        if path.is_file() {
            Ok(path)
        } else {
            Err(BackupError::NoBackup)
        }
    }

    /// 恢复：先把现在的世界备一份（和哪份备份都不一样时），解压到临时文件夹，再整个换上去。
    pub fn restore(
        &self,
        folder: &str,
        id: &str,
        keep: usize,
        cancel: &CancellationToken,
        on_progress: impl Fn(u64, u64),
    ) -> Result<(), BackupError> {
        self.settle()?;
        let zip = self.zip_path(folder, id)?;
        if !zip.is_file() {
            return Err(BackupError::NoBackup);
        }
        let world = self.world_dir(folder)?;
        if world.join("level.dat").is_file() {
            let now = fingerprint(&world)?;
            // 和某份备份一样、而且那份的 ZIP 还在，才算已经备过（ZIP 被手动删了、挪走了要重新备）
            let saved = self.load_index().get(folder).is_some_and(|w| {
                w.entries.iter().any(|e| {
                    e.fingerprint == now && self.zip_path(folder, &e.id).is_ok_and(|p| p.is_file())
                })
            });
            if !saved {
                self.create(folder, BackupReason::BeforeRestore, keep, cancel, |_, _| {})?;
            }
        }

        let tag = uuid::Uuid::new_v4().simple().to_string();
        let marker = RestoreMarker {
            folder: folder.to_owned(),
            old: format!("{TEMP_PREFIX}old-{tag}"),
            new: format!("{TEMP_PREFIX}restore-{tag}"),
        };
        write_json_atomic(&self.meta(RESTORE_FILE), &marker)?;
        let fresh = self.saves().join(&marker.new);
        if let Err(error) = extract(&zip, &fresh, cancel, &on_progress) {
            // 解压失败：什么都没换，收拾掉
            if let Err(settle) = self.settle() {
                tracing::warn!(%settle, "收拾解压了一半的备份失败");
            }
            return Err(error);
        }
        if !fresh.join("level.dat").is_file() {
            if let Err(settle) = self.settle() {
                tracing::warn!(%settle, "收拾解压了一半的备份失败");
            }
            return Err(BackupError::BadArchive("里面没有 level.dat".to_owned()));
        }
        // 换上去：原来的先改名挪开，新的改名过来，最后删掉原来的
        if world.exists() {
            let old = self.saves().join(&marker.old);
            std::fs::rename(&world, &old).map_err(io_error(&world))?;
        }
        std::fs::rename(&fresh, &world).map_err(io_error(&world))?;
        self.settle()?;
        // 解压出来的文件修改时间是新的：把这份备份的指纹换成恢复后的样子，
        // 不然下次进世界前会以为世界变了、再备一份一模一样的
        match fingerprint(&world) {
            Ok(now) => {
                let mut index = self.load_index();
                if let Some(entry) = index
                    .get_mut(folder)
                    .and_then(|w| w.entries.iter_mut().find(|e| e.id == id))
                {
                    entry.fingerprint = now;
                }
                self.save_index(&index)?;
            }
            Err(error) => tracing::warn!(%error, "恢复后读世界的指纹失败"),
        }
        tracing::info!(folder, id, "世界恢复好了");
        Ok(())
    }

    /// 是否有中断的世界恢复等着收尾。
    pub fn has_pending_restore(&self) -> bool {
        self.meta(RESTORE_FILE).exists()
    }

    /// 上次恢复做到一半（启动器被关掉）：没换上去的退回原来的世界，换上去了的删掉原来的。
    /// 平时什么都不做。启动时、备份和恢复之前调用。
    pub fn settle(&self) -> Result<(), BackupError> {
        let path = self.meta(RESTORE_FILE);
        let marker: RestoreMarker = match read_json(&path) {
            Ok(Some(marker)) => marker,
            Ok(None) => return Ok(()),
            Err(error) => {
                tracing::error!(%error, "恢复记号读不懂，删掉");
                std::fs::remove_file(&path).map_err(io_error(&path))?;
                return Ok(());
            }
        };
        let world = self.world_dir(&marker.folder)?;
        let old = safe_join(&self.saves(), &marker.old)?;
        let fresh = safe_join(&self.saves(), &marker.new)?;
        if !marker.old.starts_with(TEMP_PREFIX) || !marker.new.starts_with(TEMP_PREFIX) {
            tracing::error!("恢复记号里的文件夹名不对，不动");
            std::fs::remove_file(&path).map_err(io_error(&path))?;
            return Ok(());
        }
        if !world.exists() && old.exists() {
            // 原来的挪开了、新的还没换上：放回去
            std::fs::rename(&old, &world).map_err(io_error(&old))?;
            tracing::warn!(folder = %marker.folder, "上次恢复没做完，放回了原来的世界");
        }
        for leftover in [&old, &fresh] {
            match std::fs::remove_dir_all(leftover) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(BackupError::Io {
                        path: leftover.clone(),
                        source,
                    });
                }
            }
        }
        std::fs::remove_file(&path).map_err(io_error(&path))?;
        Ok(())
    }
}

/// 这次游戏期间玩过的世界（`level.dat` 在 `since` 之后被写过的），按存档文件夹名
pub fn played_since(instance_dir: &Path, since_ms: u64) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(instance_dir.join("saves")) else {
        return Vec::new();
    };
    let from = since_ms.saturating_sub(PLAYED_SLACK_MS);
    entries
        .flatten()
        .filter_map(|e| {
            let folder = e.file_name().into_string().ok()?;
            if folder.starts_with(TEMP_PREFIX) {
                return None;
            }
            let modified = e
                .path()
                .join("level.dat")
                .metadata()
                .ok()?
                .modified()
                .ok()?;
            (millis(modified) >= from).then_some(folder)
        })
        .collect()
}

fn millis(time: std::time::SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// 世界里的一个文件：相对路径（`/` 分隔）、磁盘位置、大小、修改时间
struct WorldFile {
    rel: String,
    path: PathBuf,
    size: u64,
    modified: u64,
}

/// 世界里的所有文件（不要 `session.lock`：游戏开着时它被锁着，恢复后游戏会自己再建；不跟随符号链接）
fn world_files(dir: &Path) -> Result<Vec<WorldFile>, BackupError> {
    let mut files = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, prefix)) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(io_error(&current))?;
        for entry in entries {
            let entry = entry.map_err(io_error(&current))?;
            let Ok(name) = entry.file_name().into_string() else {
                tracing::warn!(path = %entry.path().display(), "文件名不是有效的文字，不备份");
                continue;
            };
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push((entry.path(), rel));
                continue;
            }
            if prefix.is_empty() && name.eq_ignore_ascii_case("session.lock") {
                continue;
            }
            let meta = entry.metadata().map_err(io_error(&entry.path()))?;
            files.push(WorldFile {
                rel,
                path: entry.path(),
                size: meta.len(),
                modified: meta.modified().map(millis).unwrap_or(0),
            });
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

fn fingerprint_of(files: &[WorldFile]) -> Fingerprint {
    Fingerprint {
        files: files.len() as u64,
        bytes: files.iter().map(|f| f.size).sum(),
        newest: files.iter().map(|f| f.modified).max().unwrap_or(0),
    }
}

/// 世界现在的指纹（只读目录信息，不读文件内容，很快）
pub fn fingerprint(dir: &Path) -> Result<Fingerprint, BackupError> {
    Ok(fingerprint_of(&world_files(dir)?))
}

fn zip_error(error: zip::result::ZipError) -> BackupError {
    BackupError::BadArchive(error.to_string())
}

/// 打包成 zip，返回压缩包大小
fn write_zip(
    target: &Path,
    folder: &str,
    files: &[WorldFile],
    total: u64,
    cancel: &CancellationToken,
    on_progress: &impl Fn(u64, u64),
) -> Result<u64, BackupError> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let out = File::create(target).map_err(io_error(target))?;
    let mut zip = zip::ZipWriter::new(BufWriter::with_capacity(1 << 20, out));
    // 区块文件本来就是压缩过的，用最快的压缩档就够了
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(1));
    zip.add_directory(format!("{folder}/"), options)
        .map_err(zip_error)?;
    let mut done = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    for file in files {
        if cancel.is_cancelled() {
            return Err(BackupError::Cancelled);
        }
        let options = options.large_file(file.size >= u64::from(u32::MAX));
        zip.start_file(format!("{folder}/{}", file.rel), options)
            .map_err(zip_error)?;
        let mut input = File::open(&file.path).map_err(io_error(&file.path))?;
        loop {
            let n = input.read(&mut buffer).map_err(io_error(&file.path))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buffer[..n]).map_err(io_error(target))?;
            done += n as u64;
        }
        on_progress(done, total);
    }
    let writer = zip.finish().map_err(zip_error)?;
    let file = writer.into_inner().map_err(|e| BackupError::Io {
        path: target.to_path_buf(),
        source: e.into_error(),
    })?;
    file.sync_all().map_err(io_error(target))?;
    Ok(file.metadata().map(|m| m.len()).unwrap_or(0))
}

/// 解压一份备份到 `dest`：去掉最外面那层世界文件夹名，每个条目都经过 [`safe_join`]，跳过符号链接
fn extract(
    zip_path: &Path,
    dest: &Path,
    cancel: &CancellationToken,
    on_progress: &impl Fn(u64, u64),
) -> Result<(), BackupError> {
    let file = File::open(zip_path).map_err(io_error(zip_path))?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_error)?;
    let total: u64 = (0..archive.len())
        .filter_map(|i| archive.by_index_raw(i).ok().map(|e| e.size()))
        .sum();
    std::fs::create_dir_all(dest).map_err(io_error(dest))?;
    let mut done = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    for i in 0..archive.len() {
        if cancel.is_cancelled() {
            return Err(BackupError::Cancelled);
        }
        let mut entry = archive.by_index(i).map_err(zip_error)?;
        if entry.is_symlink() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| BackupError::BadArchive(e.to_string()))?
            .replace('\\', "/");
        // 去掉最外面的世界文件夹名
        let Some((_, relative)) = name.split_once('/') else {
            continue;
        };
        let relative = relative.trim_end_matches('/');
        if relative.is_empty() {
            continue;
        }
        let target = safe_join(dest, relative)?;
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(io_error(&target))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        let mut out = BufWriter::new(File::create(&target).map_err(io_error(&target))?);
        loop {
            let n = entry
                .read(&mut buffer)
                .map_err(|e| BackupError::BadArchive(e.to_string()))?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n]).map_err(io_error(&target))?;
            done += n as u64;
        }
        out.flush().map_err(io_error(&target))?;
        on_progress(done, total);
    }
    Ok(())
}

/// 把所有实例的世界备份与索引从旧位置安全搬移到新位置。
/// 同盘优先 `rename`，跨盘复制并校验字节数一致后再删除旧文件；绝不删动实例 `Lumi/` 里的其他文件。
pub fn migrate_backups_root(
    instance_dirs: &[PathBuf],
    from_root: Option<&Path>,
    to_root: Option<&Path>,
) -> Result<BackupMigrateResult, BackupError> {
    let mut moved_files = 0u64;
    let mut moved_bytes = 0u64;
    for dir in instance_dirs {
        let src = Backups::with_root(dir, from_root);
        let dst = Backups::with_root(dir, to_root);
        if src.storage_root == dst.storage_root {
            continue;
        }
        let src_backups = src.storage(BACKUP_DIR);
        let src_index_path = src.storage(INDEX_FILE);
        if !src_backups.exists() && !src_index_path.is_file() {
            continue;
        }
        let dst_backups = dst.storage(BACKUP_DIR);
        if let Ok(worlds) = std::fs::read_dir(&src_backups) {
            for world in worlds.flatten() {
                let world_src = world.path();
                if !world_src.is_dir() {
                    continue;
                }
                let world_dst = dst_backups.join(world.file_name());
                std::fs::create_dir_all(&world_dst).map_err(io_error(&world_dst))?;
                if let Ok(files) = std::fs::read_dir(&world_src) {
                    for file in files.flatten() {
                        let file_src = file.path();
                        let Ok(meta) = file.metadata() else {
                            continue;
                        };
                        if !meta.is_file() {
                            continue;
                        }
                        let file_dst = world_dst.join(file.file_name());
                        move_file_verified(&file_src, &file_dst, meta.len())?;
                        moved_files += 1;
                        moved_bytes = moved_bytes.saturating_add(meta.len());
                    }
                }
                let _ = std::fs::remove_dir(&world_src);
            }
            let _ = std::fs::remove_dir(&src_backups);
        }
        if src_index_path.is_file() {
            let src_index = src.load_index();
            let mut dst_index = dst.load_index();
            for (folder, world) in src_index {
                let entry = dst_index.entry(folder).or_default();
                if entry.auto_large.is_none() {
                    entry.auto_large = world.auto_large;
                }
                for item in world.entries {
                    if !entry.entries.iter().any(|existing| existing.id == item.id) {
                        entry.entries.push(item);
                    }
                }
            }
            dst.save_index(&dst_index)?;
            let _ = std::fs::remove_file(&src_index_path);
        }
        // 如果是自定义根目录里的实例空目录，顺手清理掉
        if from_root.is_some() {
            let _ = std::fs::remove_dir(&src.storage_root);
        }
    }
    Ok(BackupMigrateResult {
        moved_files: u32::try_from(moved_files).unwrap_or(u32::MAX),
        moved_bytes: moved_bytes as f64,
    })
}

fn move_file_verified(src: &Path, dst: &Path, expected_len: u64) -> Result<(), BackupError> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let copied = std::fs::copy(src, dst).map_err(io_error(dst))?;
    if copied != expected_len {
        let _ = std::fs::remove_file(dst);
        return Err(BackupError::Io {
            path: dst.to_path_buf(),
            source: io::Error::other("复制校验长度不一致"),
        });
    }
    std::fs::remove_file(src).map_err(io_error(src))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(instance: &Path, folder: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let dir = instance.join("saves").join(folder);
        for (rel, bytes) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        dir
    }

    fn read_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        world_files(dir)
            .unwrap()
            .into_iter()
            .map(|f| (f.rel, std::fs::read(f.path).unwrap()))
            .collect()
    }

    fn create(b: &Backups, folder: &str, reason: BackupReason) -> BackupInfo {
        b.create(folder, reason, 3, &CancellationToken::new(), |_, _| {})
            .unwrap()
    }

    #[test]
    fn backs_up_and_restores_a_world() {
        let dir = tempfile::tempdir().unwrap();
        let w = world(
            dir.path(),
            "新的世界",
            &[
                ("level.dat", b"level v1"),
                ("region/r.0.0.mca", b"chunks v1"),
                ("session.lock", b"lock"),
            ],
        );
        let b = Backups::new(dir.path());
        let first = create(&b, "新的世界", BackupReason::AfterPlay);
        assert_eq!(b.list("新的世界"), std::slice::from_ref(&first));

        // 压缩包里带世界文件夹名，没有 session.lock
        let zip = b.locate("新的世界", &first.id).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&zip).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert!(
            names.contains(&"新的世界/level.dat".to_owned()),
            "{names:?}"
        );
        assert!(names.contains(&"新的世界/region/r.0.0.mca".to_owned()));
        assert!(!names.iter().any(|n| n.ends_with("session.lock")));

        // 世界被弄坏：多了个文件、区块变了
        std::fs::write(w.join("region/r.0.0.mca"), b"broken!!").unwrap();
        std::fs::write(w.join("garbage.dat"), b"x").unwrap();
        b.restore(
            "新的世界",
            &first.id,
            3,
            &CancellationToken::new(),
            |_, _| {},
        )
        .unwrap();
        let tree = read_tree(&w);
        assert_eq!(tree["region/r.0.0.mca"], b"chunks v1");
        assert!(!tree.contains_key("garbage.dat"));
        assert_eq!(
            b.auto_check("新的世界").unwrap(),
            AutoCheck::Unchanged,
            "刚从备份恢复的样子已经有备份了，进世界前不再备"
        );

        // 恢复前把坏掉的那个也备了一份，可以再换回去
        let list = b.list("新的世界");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].reason, BackupReason::BeforeRestore);
        // saves 里没有留下临时文件夹
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("saves"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn restore_backs_up_again_when_the_matching_zip_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let w = world(dir.path(), "w", &[("level.dat", b"v1")]);
        let b = Backups::new(dir.path());
        let old = create(&b, "w", BackupReason::AfterPlay);
        std::fs::write(w.join("level.dat"), b"v2 now").unwrap();
        let latest = create(&b, "w", BackupReason::AfterPlay);
        // 和现在一样的那份 ZIP 被手动删了：记录还在，文件没了
        std::fs::remove_file(b.locate("w", &latest.id).unwrap()).unwrap();

        b.restore("w", &old.id, 3, &CancellationToken::new(), |_, _| {})
            .unwrap();
        assert_eq!(std::fs::read(w.join("level.dat")).unwrap(), b"v1");
        let before = b
            .list("w")
            .into_iter()
            .find(|i| i.reason == BackupReason::BeforeRestore)
            .expect("恢复前要先把现在的世界备一份");
        let zip = b.locate("w", &before.id).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&zip).unwrap()).unwrap();
        let mut level = Vec::new();
        archive
            .by_name("w/level.dat")
            .unwrap()
            .read_to_end(&mut level)
            .unwrap();
        assert_eq!(level, b"v2 now");
    }

    #[test]
    fn unchanged_worlds_are_not_backed_up_again() {
        let dir = tempfile::tempdir().unwrap();
        let w = world(dir.path(), "w", &[("level.dat", b"a")]);
        let b = Backups::new(dir.path());
        assert_eq!(b.auto_check("w").unwrap(), AutoCheck::Go);
        create(&b, "w", BackupReason::AfterPlay);
        assert_eq!(b.auto_check("w").unwrap(), AutoCheck::Unchanged);
        std::fs::write(w.join("level.dat"), b"ab").unwrap();
        assert_eq!(b.auto_check("w").unwrap(), AutoCheck::Go);
        assert!(matches!(b.auto_check("missing"), Err(BackupError::NoWorld)));
    }

    #[test]
    fn keeps_only_the_newest_automatic_backups() {
        let dir = tempfile::tempdir().unwrap();
        let w = world(dir.path(), "w", &[("level.dat", b"0")]);
        let b = Backups::new(dir.path());
        let manual = create(&b, "w", BackupReason::Manual);
        let mut auto = Vec::new();
        for i in 1..=5 {
            std::fs::write(w.join("level.dat"), i.to_string()).unwrap();
            auto.push(create(&b, "w", BackupReason::AfterPlay));
        }
        let list = b.list("w");
        let ids: Vec<&str> = list.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(list.len(), 4, "3 份自动 + 1 份手动：{ids:?}");
        assert!(ids.contains(&manual.id.as_str()), "手动的不自动删");
        assert!(!ids.contains(&auto[0].id.as_str()), "最旧的自动备份删了");
        assert!(b.locate("w", &auto[0].id).is_err(), "文件也删了");
        let (bytes, count) = b.usage();
        assert_eq!(count, 4);
        assert!(bytes > 0);

        assert_eq!(b.clear().unwrap(), bytes);
        assert!(b.list("w").is_empty());
        assert_eq!(b.usage(), (0, 0));
    }

    #[test]
    fn large_worlds_ask_once() {
        let dir = tempfile::tempdir().unwrap();
        world(dir.path(), "big", &[("level.dat", b"a")]);
        let b = Backups::new(dir.path());
        // 不真写 1 GB：直接看判断用的记录
        b.set_auto_large("big", false).unwrap();
        let index = b.load_index();
        assert_eq!(index["big"].auto_large, Some(false));
        b.set_auto_large("big", true).unwrap();
        assert_eq!(b.load_index()["big"].auto_large, Some(true));
        assert!(b.set_auto_large("../x", true).is_err());
    }

    #[test]
    fn an_interrupted_restore_puts_the_old_world_back() {
        let dir = tempfile::tempdir().unwrap();
        let w = world(dir.path(), "w", &[("level.dat", b"current")]);
        let b = Backups::new(dir.path());
        let saves = dir.path().join("saves");

        // 原来的已经挪开、新的解压好了还没换上去时被关掉
        let marker = RestoreMarker {
            folder: "w".to_owned(),
            old: ".mcl-old-1".to_owned(),
            new: ".mcl-restore-1".to_owned(),
        };
        write_json_atomic(&b.meta(RESTORE_FILE), &marker).unwrap();
        std::fs::rename(&w, saves.join(".mcl-old-1")).unwrap();
        world(dir.path(), ".mcl-restore-1", &[("level.dat", b"restored")]);
        b.settle().unwrap();
        assert_eq!(std::fs::read(w.join("level.dat")).unwrap(), b"current");
        assert!(!saves.join(".mcl-old-1").exists());
        assert!(!saves.join(".mcl-restore-1").exists());
        assert!(!b.meta(RESTORE_FILE).exists());

        // 已经换上去了、原来的还没删
        write_json_atomic(&b.meta(RESTORE_FILE), &marker).unwrap();
        world(dir.path(), ".mcl-old-1", &[("level.dat", b"older")]);
        b.settle().unwrap();
        assert_eq!(std::fs::read(w.join("level.dat")).unwrap(), b"current");
        assert!(!saves.join(".mcl-old-1").exists());
        b.settle().unwrap();
    }

    #[test]
    fn finds_the_worlds_played_this_time() {
        let dir = tempfile::tempdir().unwrap();
        world(dir.path(), "played", &[("level.dat", b"a")]);
        world(dir.path(), ".mcl-old-x", &[("level.dat", b"a")]);
        let now = crate::now_millis();
        assert_eq!(played_since(dir.path(), now), ["played"]);
        assert!(played_since(dir.path(), now + 3_600_000).is_empty());
    }

    #[test]
    fn a_cancelled_backup_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        world(dir.path(), "w", &[("level.dat", b"a"), ("b.dat", b"b")]);
        let b = Backups::new(dir.path());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            b.create("w", BackupReason::Manual, 3, &cancel, |_, _| {}),
            Err(BackupError::Cancelled)
        ));
        assert!(b.list("w").is_empty());
        let leftovers = std::fs::read_dir(b.backup_dir("w").unwrap())
            .unwrap()
            .count();
        assert_eq!(leftovers, 0, "没写完的压缩包删掉了");
    }

    #[test]
    fn migrates_backups_to_custom_root_and_back() {
        let inst = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        world(inst.path(), "survival", &[("level.dat", b"hello-world")]);
        let cancel = CancellationToken::new();

        let default_backups = Backups::new(inst.path());
        let created = default_backups
            .create("survival", BackupReason::Manual, 3, &cancel, |_, _| {})
            .unwrap();
        assert_eq!(default_backups.list("survival").len(), 1);

        // 迁移到自定义目录
        let moved =
            migrate_backups_root(&[inst.path().to_path_buf()], None, Some(custom.path())).unwrap();
        assert_eq!(moved.moved_files, 1);
        assert!(moved.moved_bytes > 0.0);
        assert!(default_backups.list("survival").is_empty());

        let custom_backups = Backups::with_root(inst.path(), Some(custom.path()));
        let listed = custom_backups.list("survival");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        assert!(custom_backups.usage().0 > 0);

        // 迁回默认目录
        let moved_back =
            migrate_backups_root(&[inst.path().to_path_buf()], Some(custom.path()), None).unwrap();
        assert_eq!(moved_back.moved_files, 1);
        assert_eq!(default_backups.list("survival").len(), 1);
        assert!(custom_backups.list("survival").is_empty());
    }
}
