//! 实例：游戏目录 `instances/` 下的独立文件夹。
//!
//! ```text
//! <游戏目录>/
//! ├─ instances/<实例名>/              实例文件夹，就是这个实例的游戏目录
//! │  ├─ mods/ config/ saves/ …        实例自己的游戏文件（实例之间天然隔离）
//! │  └─ <启动器代号>/instance.json     本启动器的设置：要启动哪个版本、加载器、内存……
//! └─ versions/ libraries/ assets/     共用的游戏本体：Mojang 的标准布局，版本号是真的（1.21.1、neoforge-21.1.219）
//! ```
//!
//! 实例名和版本号分开：不会和官方启动器、别家启动器的版本撞名，改实例名也不用动版本描述；
//! 整个实例文件夹拷走就是一个完整的实例。早先实例和 PCL、HMCL 一样放在 `versions/<实例名>/` 里，
//! 启动时自动搬过来（[`migrate_legacy`]）。别家启动器的 `.minecraft` 以后用「挂上、原地读」兼容。

mod copy;
mod link;
mod overrides;

pub use copy::{
    COPY_TEMP_PREFIX, CopyError, CopyOptions, CopyProgress, copy, copy_folder, move_to_trash,
};
pub use link::{Link, linked_to, path_key, same_path, unlink, unlink_root};
pub use overrides::{
    InstanceOverrides, InstanceSyncExclusions, LaunchOverride, MemoryOverride, Resolved, resolve,
};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::{UnsafePath, read_json, safe_file_name, write_atomic, write_json_atomic};
use crate::paths::GameDir;

/// 实例里放本启动器文件的文件夹：`instances/<实例名>/Lumi/`（2026-11-15 定名「微光 / Lumi」）
pub const META_DIR: &str = "Lumi";
/// 以前用过的名字（定名前的代号）：启动时改成 [`META_DIR`]（[`migrate_legacy`]）
const LEGACY_META_DIRS: &[&str] = &["UnnamedLauncher"];

/// 是不是 Lumi 放自己文件的文件夹（现在的或以前的名字，不分大小写）。
/// 整合包里的这类文件夹不解压：里面的实例设置能写启动前后运行的命令
pub fn is_meta_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(META_DIR)
        || LEGACY_META_DIRS
            .iter()
            .any(|legacy| name.eq_ignore_ascii_case(legacy))
}

const CONFIG_FILE: &str = "instance.json";
/// 实例级笔记文件：`instances/<ID>/Lumi/notes.md`
pub const NOTES_FILE: &str = "notes.md";
/// 单实例笔记最大字节数（256 KiB）
pub const MAX_NOTES_BYTES: usize = 256 * 1024;
/// 2：实例搬到 `instances/`，记下要启动的版本
const CONFIG_FORMAT: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error("实例设置读取失败")]
    Read(#[from] crate::fs::ReadJsonError),
    #[error("实例设置保存失败")]
    Write(#[from] crate::fs::AtomicWriteError),
    #[error("版本描述格式不对：{0}")]
    VersionJson(String),
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    NeoForge,
    Fabric,
    Forge,
    Quilt,
    LegacyFabric,
    Babric,
}

impl LoaderKind {
    pub fn label(self) -> &'static str {
        match self {
            LoaderKind::NeoForge => "NeoForge",
            LoaderKind::Fabric => "Fabric",
            LoaderKind::Forge => "Forge",
            LoaderKind::Quilt => "Quilt",
            LoaderKind::LegacyFabric => "Legacy Fabric",
            LoaderKind::Babric => "Babric",
        }
    }

    /// Modrinth 上的加载器名（`.mrpack` 依赖里的名字另见导出）
    pub fn slug(self) -> &'static str {
        match self {
            LoaderKind::NeoForge => "neoforge",
            LoaderKind::Fabric => "fabric",
            LoaderKind::Forge => "forge",
            LoaderKind::Quilt => "quilt",
            LoaderKind::LegacyFabric => "legacy-fabric",
            LoaderKind::Babric => "babric",
        }
    }

    /// 各加载器对应的游戏版本兼容范围：
    /// - `NeoForge`: 1.20.1+ / 26.x+ 正式版
    /// - `Forge`: 1.5.2+ / 26.x+ 正式版（1.5.2–1.12.2 走旧版安装器管线，1.13+ 走处理器管线）
    /// - `Fabric` / `Quilt`: 1.14+ / 26.x+ 正式版以及 18w43+ 现代快照 / 愚人节版
    /// - `LegacyFabric`: 1.3.2–1.13.2 正式版
    /// - `Babric`: Beta `b1.7.3`
    pub fn installable_for(self, game_version: &str) -> bool {
        let release = parse_release_version(game_version);
        match self {
            Self::NeoForge => {
                matches!(release, Some((1, 20, p)) if p >= 1)
                    || matches!(release, Some((1, m, _)) if m >= 21)
                    || matches!(release, Some((y, _, _)) if y >= 26)
            }
            Self::Forge => {
                matches!(release, Some((1, 5, p)) if p >= 2)
                    || matches!(release, Some((1, m, _)) if m >= 6)
                    || matches!(release, Some((y, _, _)) if y >= 26)
            }
            Self::Fabric | Self::Quilt => {
                matches!(release, Some((1, m, _)) if m >= 14)
                    || matches!(release, Some((y, _, _)) if y >= 26)
                    || is_modern_snapshot_for_fabric(game_version)
            }
            Self::LegacyFabric => {
                matches!(release, Some((1, 3, p)) if p >= 2)
                    || matches!(release, Some((1, m, _)) if (4..=13).contains(&m))
            }
            Self::Babric => game_version == "b1.7.3",
        }
    }

    pub fn uses_processor(self) -> bool {
        matches!(self, LoaderKind::NeoForge | LoaderKind::Forge)
    }
}

fn parse_release_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = match parts.next() {
        Some(seg) => seg.parse::<u32>().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn is_modern_snapshot_for_fabric(version: &str) -> bool {
    let bytes = version.as_bytes();
    if bytes.len() >= 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'w'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
    {
        let year = (bytes[0] - b'0') * 10 + (bytes[1] - b'0');
        let week = (bytes[3] - b'0') * 10 + (bytes[4] - b'0');
        return year > 18 || (year == 18 && week >= 43);
    }
    if let Some((base, _)) = version.split_once(['-', '_'])
        && let Some((major, minor, _)) = parse_release_version(base)
    {
        return (major == 1 && minor >= 14) || major >= 26;
    }
    false
}

impl LoaderRef {
    /// 装好后 `versions/` 里的版本 ID（和官方安装器一致，例如 `neoforge-21.1.219`、
    /// `fabric-loader-0.16.10-1.21.1`）
    pub fn version_id(&self, game_version: &str) -> String {
        match self.kind {
            LoaderKind::NeoForge => {
                let short = self
                    .version
                    .strip_prefix("1.20.1-")
                    .filter(|v| v.starts_with("47.1."))
                    .unwrap_or(&self.version);
                format!("neoforge-{short}")
            }
            LoaderKind::Fabric => format!("fabric-loader-{}-{game_version}", self.version),
            LoaderKind::Forge => {
                let short = self
                    .version
                    .strip_prefix(&format!("{game_version}-"))
                    .unwrap_or(&self.version);
                format!("{game_version}-forge-{short}")
            }
            LoaderKind::Quilt => format!("quilt-loader-{}-{game_version}", self.version),
            LoaderKind::LegacyFabric => {
                format!("legacy-fabric-loader-{}-{game_version}", self.version)
            }
            LoaderKind::Babric => format!("babric-loader-{}-{game_version}", self.version),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LoaderRef {
    pub kind: LoaderKind,
    pub version: String,
}

/// 实例是从哪个整合包来的（以后做「整合包更新」用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackSource {
    /// `mrpack`、`curseforge`……
    pub kind: String,
    pub name: String,
    pub version: String,
    /// 从 Modrinth 装的才有：项目和版本编号（本地文件导入的没有）
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    /// 整合包官方宣传图或图标地址（无游戏截图时作为默认封面兜底）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

/// `instance.json` 的内容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct InstanceConfig {
    pub format: u32,
    /// 显示名（可以有任何文字；文件夹名是清理过的 ID）
    pub name: String,
    pub game_version: String,
    pub loader: Option<LoaderRef>,
    /// 要启动的版本（`versions/` 里的 ID：原版是 `1.21.1`，NeoForge 是 `neoforge-21.1.219`）。
    /// 老设置没有，用 [`Instance::version_id`] 读（空的时候按游戏版本和加载器推出来）
    #[serde(default)]
    pub version_id: String,
    /// 毫秒时间戳
    pub created_at: f64,
    pub source: Option<PackSource>,
    /// 上次启动的时间（毫秒时间戳）；没玩过是 `None`
    #[serde(default)]
    pub last_played: Option<f64>,
    /// 整合包作者推荐的内存（MB，CurseForge 整合包清单里的 `recommendedRam`）；自动分配内存时优先参考
    #[serde(default)]
    pub recommended_memory_mb: Option<u32>,
    /// 实例自己的内存、Java、启动参数、游戏窗口；为空的那组跟全局
    #[serde(default)]
    pub overrides: InstanceOverrides,
    /// 实例库里的分组（「生存」「整合包」……）；`None` 是没分组
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// 置顶的时间（毫秒时间戳）；`None` 是没置顶。置顶的按这个排，先置顶的在前
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<f64>,
    /// 实例默认矢量封面预设（`<徽章>:<色板>`，如 `lantern:dusk`）；`None` 表示自动分配
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_preset: Option<String>,
    /// 挂上的别家版本：游戏文件都在别家的 `.minecraft` 里原地读；`None` 是本启动器自己的实例
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "specta", specta(skip))]
    pub link: Option<Link>,
}

/// 分组名最长多少个字
pub const MAX_GROUP_CHARS: usize = 24;

/// 整理分组名：去掉首尾空白、截短；空的算没分组。
pub fn clean_group(group: Option<&str>) -> Option<String> {
    let name: String = group?.trim().chars().take(MAX_GROUP_CHARS).collect();
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

impl InstanceConfig {
    pub fn new(name: &str, game_version: &str, loader: Option<LoaderRef>) -> Self {
        Self {
            format: CONFIG_FORMAT,
            name: name.to_owned(),
            game_version: game_version.to_owned(),
            version_id: loader
                .as_ref()
                .map_or_else(|| game_version.to_owned(), |l| l.version_id(game_version)),
            loader,
            created_at: crate::now_millis() as f64,
            source: None,
            last_played: None,
            recommended_memory_mb: None,
            overrides: InstanceOverrides::default(),
            group: None,
            pinned_at: None,
            cover_preset: None,
            link: None,
        }
    }
}

/// 名字最长多少个字（只是显示名；文件夹名不变）
const MAX_NAME_CHARS: usize = 64;

/// 改实例的设置：读 → 改 → 原子写回，返回改完的实例。名字会去掉首尾空白，空名字不改。
pub fn update(
    game: &GameDir,
    id: &str,
    change: impl FnOnce(&mut InstanceConfig),
) -> Result<Option<Instance>, InstanceError> {
    let path = config_path(game, id)?;
    let Some(mut config) = read_json::<InstanceConfig>(&path)? else {
        return Ok(None);
    };
    let before = config.name.clone();
    change(&mut config);
    let name: String = config.name.trim().chars().take(MAX_NAME_CHARS).collect();
    config.name = if name.is_empty() { before } else { name };
    config.overrides = std::mem::take(&mut config.overrides).normalized();
    config.group = clean_group(config.group.as_deref());
    write_json_atomic(&path, &config)?;
    Ok(Some(Instance {
        id: id.to_owned(),
        config,
    }))
}

/// 记下「刚刚启动过」（首页按这个排出最近玩的实例）。
pub fn touch_last_played(game: &GameDir, id: &str) -> Result<(), InstanceError> {
    let path = config_path(game, id)?;
    if let Some(mut config) = read_json::<InstanceConfig>(&path)? {
        config.last_played = Some(crate::now_millis() as f64);
        write_json_atomic(&path, &config)?;
    }
    Ok(())
}

/// 新建实例（实例文件夹不存在时创建）。
pub fn create(
    game: &GameDir,
    id: &str,
    config: &InstanceConfig,
) -> Result<Instance, InstanceError> {
    let dir = game.instance_dir(id)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| InstanceError::VersionJson(format!("无法创建实例文件夹：{e}")))?;
    register(game, id, config)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// 实例文件夹名（`instances/<ID>/`）；和版本号无关
    pub id: String,
    pub config: InstanceConfig,
}

impl Instance {
    /// 游戏工作目录（存档、模组、设置都在这里）：实例文件夹本身；
    /// 挂上的别家版本是那边的版本文件夹（开了版本隔离）或那边的 `.minecraft`
    pub fn game_dir(&self, game: &GameDir) -> Result<PathBuf, UnsafePath> {
        match &self.config.link {
            Some(link) => Ok(link.game_dir.clone()),
            None => game.instance_dir(&self.id),
        }
    }

    /// 版本、依赖库、资源放在哪个游戏目录：自己的实例用共用的那个，挂上的用别家的 `.minecraft`
    pub fn files(&self, game: &GameDir) -> GameDir {
        match &self.config.link {
            Some(link) => GameDir::new(&link.root),
            None => game.clone(),
        }
    }

    pub fn is_linked(&self) -> bool {
        self.config.link.is_some()
    }

    /// 要启动的版本（`versions/` 里的 ID）
    pub fn version_id(&self) -> String {
        if self.config.version_id.is_empty() {
            self.config.loader.as_ref().map_or_else(
                || self.config.game_version.clone(),
                |l| l.version_id(&self.config.game_version),
            )
        } else {
            self.config.version_id.clone()
        }
    }
}

fn config_path(game: &GameDir, id: &str) -> Result<PathBuf, UnsafePath> {
    Ok(game.instance_dir(id)?.join(META_DIR).join(CONFIG_FILE))
}

/// 实例级笔记路径：`instances/<ID>/Lumi/notes.md`（关联外部实例也存放在 Lumi 元数据目录，不污染外部目录）。
pub fn notes_path(game: &GameDir, id: &str) -> Result<PathBuf, UnsafePath> {
    Ok(game.instance_dir(id)?.join(META_DIR).join(NOTES_FILE))
}

/// 读取实例笔记；不存在时返回空字符串，读取上限为 [`MAX_NOTES_BYTES`]。
pub fn read_notes(game: &GameDir, id: &str) -> Result<String, InstanceError> {
    use std::io::Read;
    let path = notes_path(game, id)?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(InstanceError::Io(format!("读取实例笔记失败：{error}"))),
    };
    let mut bytes = Vec::new();
    file.take(MAX_NOTES_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| InstanceError::Io(format!("读取实例笔记失败：{error}")))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// 原子保存实例笔记；纯空白时删除笔记文件保持目录整洁，返回保存后的规范化内容。
pub fn write_notes(game: &GameDir, id: &str, content: &str) -> Result<String, InstanceError> {
    let path = notes_path(game, id)?;
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.len() > MAX_NOTES_BYTES {
        return Err(InstanceError::Io("实例笔记不能超过 256 KB".to_owned()));
    }
    if normalized.trim().is_empty() {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(InstanceError::Io(format!("清空实例笔记失败：{error}")));
        }
        return Ok(String::new());
    }
    write_atomic(&path, normalized.as_bytes())?;
    Ok(normalized)
}

/// 读一个实例；没有这个实例时返回 `None`。
pub fn load(game: &GameDir, id: &str) -> Result<Option<Instance>, InstanceError> {
    let config: Option<InstanceConfig> = read_json(&config_path(game, id)?)?;
    Ok(config.map(|config| Instance {
        id: id.to_owned(),
        config,
    }))
}

/// 列出游戏目录里本启动器的全部实例（按创建时间，新的在前）。读不了的跳过并记日志。
pub fn list(game: &GameDir) -> Vec<Instance> {
    let Ok(entries) = std::fs::read_dir(game.instances()) else {
        return Vec::new();
    };
    let mut instances: Vec<Instance> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|id| match load(game, &id) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(instance = %id, %error, "实例设置读不了，跳过");
                None
            }
        })
        .collect();
    instances.sort_by(|a, b| b.config.created_at.total_cmp(&a.config.created_at));
    instances
}

/// 把分组 `from` 里的实例都挪到 `to`（`None` 或空名字是解散这个分组）。返回改了几个实例；
/// 某个实例写不进去时记日志、接着改别的。
pub fn rename_group(game: &GameDir, from: &str, to: Option<&str>) -> usize {
    let to = clean_group(to);
    let mut changed = 0;
    for instance in list(game) {
        if instance.config.group.as_deref() != Some(from) {
            continue;
        }
        match update(game, &instance.id, |c| c.group.clone_from(&to)) {
            Ok(_) => changed += 1,
            Err(error) => tracing::warn!(instance = %instance.id, %error, "改分组失败，跳过"),
        }
    }
    changed
}

/// 把显示名变成能当文件夹名的 ID：去掉 Windows 不允许的字符、结尾的点和空格，避开设备名。
pub fn sanitize_id(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).to_owned();
    if trimmed.is_empty() {
        return "实例".to_owned();
    }
    if safe_file_name(&trimmed).is_err() {
        // 剩下的只可能是 CON、NUL 这类设备名
        return format!("{trimmed}_");
    }
    trimmed
}

/// 找一个还没被占用的 ID：`名字`、`名字 (2)`、`名字 (3)`……（Windows 不分大小写，比较时也不分）
pub fn unique_id(game: &GameDir, name: &str) -> String {
    let base = sanitize_id(name);
    let taken: Vec<String> = std::fs::read_dir(game.instances())
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let is_free = |id: &str| !taken.contains(&id.to_lowercase());
    if is_free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|id| is_free(id))
        .unwrap_or(base)
}

/// 登记实例：写 `instance.json`。`instances/<id>/` 必须已经存在（导入时由临时目录改名而来）。
/// 版本描述不复制：实例用 `config.version_id` 指向 `versions/` 里共用的那个。
pub fn register(
    game: &GameDir,
    id: &str,
    config: &InstanceConfig,
) -> Result<Instance, InstanceError> {
    let mut config = config.clone();
    config.format = CONFIG_FORMAT;
    let instance = Instance {
        id: id.to_owned(),
        config,
    };
    let mut config = instance.config.clone();
    config.version_id = instance.version_id();
    write_json_atomic(&config_path(game, id)?, &config)?;
    Ok(Instance {
        id: id.to_owned(),
        config,
    })
}

/// 搬了几个、设置文件夹改了几个名、没搬成几个
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Migration {
    pub moved: usize,
    /// 实例里的 `UnnamedLauncher/` 改成 `Lumi/`（2026-11-15 定名）
    pub renamed: usize,
    pub failed: usize,
}

/// 实例里放本启动器文件的文件夹改名：定名前的代号（[`LEGACY_META_DIRS`]）→ [`META_DIR`]。返回改了没有
fn rename_meta_dir(dir: &Path) -> std::io::Result<bool> {
    let current = dir.join(META_DIR);
    if current.exists() {
        return Ok(false);
    }
    for old in LEGACY_META_DIRS {
        let legacy = dir.join(old);
        if legacy.is_dir() {
            std::fs::rename(&legacy, &current)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// 把早先的实例（`versions/<实例名>/<代号>/instance.json`）搬到 `instances/<实例名>/`。
///
/// 一个一个搬：整个文件夹改名（同一个盘，瞬间完成、不复制），再删掉跟着过来的旧版本描述
/// （`<实例名>.json`、`<实例名>.jar`、`natives-*`），设置里记下要启动的版本。
/// NeoForge 实例的加载器版本描述 `versions/` 里没有时，用实例自己那份补一个（不联网也能启动）。
/// 搬不动的（文件被占用，比如正被别的启动器开着）记日志跳过，下次启动再搬。
/// 别家启动器的版本（没有我们的设置文件）不碰。
/// 启动器启动时、换游戏目录后调用，那时没有安装和游戏在跑。
pub fn migrate_legacy(game: &GameDir) -> Migration {
    let mut report = Migration::default();
    // 已经在 instances/ 里的：设置文件夹换成定名后的名字
    if let Ok(entries) = std::fs::read_dir(game.instances()) {
        for entry in entries.flatten() {
            match rename_meta_dir(&entry.path()) {
                Ok(true) => report.renamed += 1,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(dir = %entry.path().display(), %error, "实例的设置文件夹没改成名，下次启动再改");
                    report.failed += 1;
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(game.versions()) else {
        return report;
    };
    for entry in entries.flatten() {
        let Ok(id) = entry.file_name().into_string() else {
            continue;
        };
        let legacy = entry.path();
        if let Err(error) = rename_meta_dir(&legacy) {
            tracing::warn!(instance = %id, %error, "老实例的设置文件夹没改成名，下次启动再搬");
            report.failed += 1;
            continue;
        }
        let legacy_config = legacy.join(META_DIR).join(CONFIG_FILE);
        if !legacy_config.is_file() {
            continue;
        }
        match migrate_one(game, &id, &legacy) {
            Ok(new_id) => {
                tracing::info!(from = %id, to = %new_id, "老实例搬到 instances/");
                report.moved += 1;
            }
            Err(error) => {
                tracing::warn!(instance = %id, %error, "老实例没搬成，下次启动再搬");
                report.failed += 1;
            }
        }
    }
    report
}

fn migrate_one(game: &GameDir, id: &str, legacy: &Path) -> Result<String, String> {
    let config: InstanceConfig = read_json(&legacy.join(META_DIR).join(CONFIG_FILE))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "设置文件不见了".to_owned())?;
    let mut instance = Instance {
        id: id.to_owned(),
        config,
    };
    let version_id = instance.version_id();
    // 加载器的版本描述不在 versions/ 里：用实例自己那份（加载器描述换过 id）补一个
    let legacy_json = legacy.join(format!("{id}.json"));
    if instance.config.loader.is_some() {
        let shared = game.version_json(&version_id).map_err(|e| e.to_string())?;
        if !shared.exists() && legacy_json.is_file() {
            let restored = std::fs::read(&legacy_json)
                .map_err(|e| e.to_string())
                .and_then(|bytes| retag_version_json(&bytes, &version_id))?;
            write_atomic(&shared, &restored).map_err(|e| e.to_string())?;
            tracing::info!(version = %version_id, "补上加载器的版本描述");
        }
    }
    std::fs::create_dir_all(game.instances()).map_err(|e| e.to_string())?;
    let new_id = if game.instance_dir(id).map_err(|e| e.to_string())?.exists() {
        unique_id(game, id)
    } else {
        id.to_owned()
    };
    let target = game.instance_dir(&new_id).map_err(|e| e.to_string())?;
    std::fs::rename(legacy, &target).map_err(|e| format!("改名失败（可能被占用）：{e}"))?;
    // 跟着过来的旧版本描述：以后用 versions/ 里共用的
    for name in [format!("{id}.json"), format!("{id}.jar")] {
        remove_quietly(&target.join(name));
    }
    if let Ok(children) = std::fs::read_dir(&target) {
        for child in children.flatten() {
            let is_natives = child.file_name().to_string_lossy().starts_with("natives-");
            if is_natives && child.path().is_dir() {
                remove_quietly(&child.path());
            }
        }
    }
    instance.id.clone_from(&new_id);
    instance.config.version_id = version_id;
    register(game, &new_id, &instance.config).map_err(|e| e.to_string())?;
    Ok(new_id)
}

/// 把一份版本描述的 `id` 换成 `version_id`
fn retag_version_json(bytes: &[u8], version_id: &str) -> Result<Vec<u8>, String> {
    let mut json: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let object = json
        .as_object_mut()
        .ok_or_else(|| "版本描述不是 JSON 对象".to_owned())?;
    object.insert(
        "id".to_owned(),
        serde_json::Value::String(version_id.to_owned()),
    );
    let mut out = serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?;
    out.push(b'\n');
    Ok(out)
}

fn remove_quietly(path: &Path) {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "删除旧版本描述失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_display_names_into_folder_names() {
        assert_eq!(sanitize_id("Optimization"), "Optimization");
        assert_eq!(sanitize_id("命运齿轮 FOM"), "命运齿轮 FOM");
        assert_eq!(sanitize_id("a/b:c*d?"), "a_b_c_d_");
        assert_eq!(sanitize_id("  pack...  "), "pack");
        assert_eq!(sanitize_id("CON"), "CON_");
        assert_eq!(sanitize_id("..."), "实例");
        assert!(safe_file_name(&sanitize_id("x\u{0}y")).is_ok());
    }

    #[test]
    fn unique_ids_ignore_case_like_windows() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        std::fs::create_dir_all(game.instances().join("optimization")).unwrap();
        assert_eq!(unique_id(&game, "Optimization"), "Optimization (2)");
        std::fs::create_dir_all(game.instances().join("Optimization (2)")).unwrap();
        assert_eq!(unique_id(&game, "Optimization"), "Optimization (3)");
        assert_eq!(unique_id(&game, "Other"), "Other");
    }

    #[test]
    fn registers_loads_and_lists_instances() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let loader = LoaderRef {
            kind: LoaderKind::NeoForge,
            version: "21.1.248".to_owned(),
        };
        let config = InstanceConfig::new("My Pack", "1.21.1", Some(loader));
        assert_eq!(config.version_id, "neoforge-21.1.248");
        let created = create(&game, "My Pack", &config).unwrap();
        assert_eq!(
            created.game_dir(&game).unwrap(),
            dir.path().join("instances").join("My Pack"),
            "实例文件夹就是游戏目录"
        );
        assert!(
            !game.versions().exists(),
            "不往 versions/ 里写实例自己的版本描述"
        );

        let loaded = load(&game, "My Pack").unwrap().unwrap();
        assert_eq!(loaded.config, config);
        assert_eq!(loaded.version_id(), "neoforge-21.1.248");
        assert_eq!(list(&game).len(), 1);
        assert_eq!(load(&game, "1.21.1").unwrap(), None, "普通版本不是实例");
        let vanilla = InstanceConfig::new("原版", "1.21.1", None);
        assert_eq!(vanilla.version_id, "1.21.1");
    }

    #[test]
    fn old_configs_have_no_overrides_and_updates_keep_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let meta = game.instance_dir("Old").unwrap().join(META_DIR);
        std::fs::create_dir_all(&meta).unwrap();
        // 早先写的 instance.json：没有 overrides
        std::fs::write(
            meta.join(CONFIG_FILE),
            r#"{"format":1,"name":"Old","gameVersion":"1.21.1","loader":null,"createdAt":1,"source":null,"recommendedMemoryMb":6016}"#,
        )
        .unwrap();
        let loaded = load(&game, "Old").unwrap().unwrap();
        assert!(loaded.config.overrides.is_empty());

        let updated = update(&game, "Old", |c| {
            c.name = "  新名字  ".to_owned();
            c.overrides.memory = Some(MemoryOverride {
                auto: false,
                max_memory_mb: 100,
            });
        })
        .unwrap()
        .unwrap();
        assert_eq!(updated.config.name, "新名字");
        assert_eq!(updated.config.overrides.memory.unwrap().max_memory_mb, 512);
        assert_eq!(
            updated.config.recommended_memory_mb,
            Some(6016),
            "别的字段不动"
        );
        assert_eq!(load(&game, "Old").unwrap().unwrap(), updated);

        let kept = update(&game, "Old", |c| c.name = "   ".to_owned())
            .unwrap()
            .unwrap();
        assert_eq!(kept.config.name, "新名字", "空名字不改");
        assert!(update(&game, "Nope", |_| {}).unwrap().is_none());
    }

    #[test]
    fn groups_and_pins_survive_and_rename_together() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        for id in ["A", "B", "C"] {
            let config = InstanceConfig::new(id, "1.21.1", None);
            create(&game, id, &config).unwrap();
        }
        // 老的 instance.json 没有这两个字段，写回去时也不多出空字段
        let raw = std::fs::read_to_string(config_path(&game, "A").unwrap()).unwrap();
        assert!(!raw.contains("group") && !raw.contains("pinnedAt"));

        let long = "很长".repeat(20);
        update(&game, "A", |c| c.group = Some(format!("  {long}  "))).unwrap();
        let a = load(&game, "A").unwrap().unwrap();
        assert_eq!(a.config.group.unwrap().chars().count(), MAX_GROUP_CHARS);
        update(&game, "A", |c| c.group = Some("生存".to_owned())).unwrap();
        update(&game, "B", |c| {
            c.group = Some(" 生存 ".to_owned());
            c.pinned_at = Some(5.0);
        })
        .unwrap();
        update(&game, "C", |c| c.group = Some("   ".to_owned())).unwrap();
        assert_eq!(
            load(&game, "C").unwrap().unwrap().config.group,
            None,
            "空名字算没分组"
        );
        let b = load(&game, "B").unwrap().unwrap();
        assert_eq!(b.config.group.as_deref(), Some("生存"));
        assert_eq!(b.config.pinned_at, Some(5.0));

        assert_eq!(rename_group(&game, "生存", Some("长期生存")), 2);
        assert_eq!(
            load(&game, "A").unwrap().unwrap().config.group.as_deref(),
            Some("长期生存")
        );
        assert_eq!(rename_group(&game, "长期生存", None), 2);
        assert!(list(&game).iter().all(|i| i.config.group.is_none()));
        assert_eq!(
            load(&game, "B").unwrap().unwrap().config.pinned_at,
            Some(5.0),
            "解散分组不动置顶"
        );
    }

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// 老布局的实例搬到 instances/，旧版本描述不跟着，加载器描述补到 versions/
    #[test]
    fn legacy_instances_move_to_the_instances_folder() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let versions = game.versions();
        // 老的 NeoForge 实例：版本描述是加载器描述换了 id；versions/ 里没有加载器自己的那份
        write(
            &versions.join("我的整合包").join("我的整合包.json"),
            r#"{"id":"我的整合包","inheritsFrom":"1.21.1","mainClass":"x"}"#,
        );
        write(
            &versions
                .join("我的整合包")
                .join("UnnamedLauncher")
                .join(CONFIG_FILE),
            r#"{"format":1,"name":"我的整合包","gameVersion":"1.21.1","loader":{"kind":"neoforge","version":"21.1.219"},"createdAt":1,"source":null}"#,
        );
        write(&versions.join("我的整合包").join("mods").join("a.jar"), "a");
        write(
            &versions
                .join("我的整合包")
                .join("saves")
                .join("世界")
                .join("level.dat"),
            "w",
        );
        std::fs::create_dir_all(versions.join("我的整合包").join("natives-windows-x86_64"))
            .unwrap();
        // 老的原版实例
        write(
            &versions.join("原版").join("原版.json"),
            r#"{"id":"原版","inheritsFrom":"1.20.1","type":"release"}"#,
        );
        write(
            &versions
                .join("原版")
                .join("UnnamedLauncher")
                .join(CONFIG_FILE),
            r#"{"format":1,"name":"原版","gameVersion":"1.20.1","loader":null,"createdAt":2,"source":null}"#,
        );
        // 别家启动器的版本、真正的版本：不碰
        write(&versions.join("PCL的版本").join("PCL的版本.json"), "{}");
        write(&versions.join("1.21.1").join("1.21.1.json"), "{}");

        let report = migrate_legacy(&game);
        assert_eq!(
            report,
            Migration {
                moved: 2,
                renamed: 0,
                failed: 0
            }
        );

        let pack = load(&game, "我的整合包").unwrap().unwrap();
        assert_eq!(pack.config.version_id, "neoforge-21.1.219");
        assert_eq!(pack.config.format, CONFIG_FORMAT);
        let pack_dir = pack.game_dir(&game).unwrap();
        assert!(pack_dir.join("mods").join("a.jar").exists(), "模组跟着走");
        assert!(
            pack_dir
                .join("saves")
                .join("世界")
                .join("level.dat")
                .exists(),
            "存档跟着走"
        );
        assert!(
            !pack_dir.join("我的整合包.json").exists(),
            "旧版本描述不跟着"
        );
        assert!(!pack_dir.join("natives-windows-x86_64").exists());
        assert!(
            pack_dir.join(META_DIR).join(CONFIG_FILE).exists(),
            "设置文件夹换成定名后的名字"
        );
        assert!(!pack_dir.join("UnnamedLauncher").exists());
        let restored: serde_json::Value = serde_json::from_slice(
            &std::fs::read(game.version_json("neoforge-21.1.219").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            restored["id"], "neoforge-21.1.219",
            "加载器描述补到 versions/"
        );
        assert_eq!(restored["inheritsFrom"], "1.21.1");

        let vanilla = load(&game, "原版").unwrap().unwrap();
        assert_eq!(vanilla.version_id(), "1.20.1");
        assert!(!versions.join("原版").exists());
        assert!(!versions.join("我的整合包").exists());
        assert!(versions.join("PCL的版本").exists(), "别家的版本不碰");
        assert!(versions.join("1.21.1").exists());
        assert_eq!(list(&game).len(), 2);

        assert_eq!(migrate_legacy(&game), Migration::default(), "搬过的不再搬");
    }

    /// 2026-11-15 定名：instances/ 里实例的 `UnnamedLauncher/` 改成 `Lumi/`，里面的东西都在
    #[test]
    fn legacy_meta_folders_are_renamed_after_naming() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let old = game.instances().join("生存").join("UnnamedLauncher");
        write(
            &old.join(CONFIG_FILE),
            r#"{"format":2,"name":"生存","gameVersion":"1.21.1","versionId":"1.21.1","loader":null,"createdAt":1,"source":null}"#,
        );
        write(&old.join("launches").join("1.json"), "{}");
        assert_eq!(migrate_legacy(&game).renamed, 1);
        let instance = load(&game, "生存").unwrap().unwrap();
        let meta = instance.game_dir(&game).unwrap().join(META_DIR);
        assert!(meta.join("launches").join("1.json").exists());
        assert!(!old.exists());
        assert_eq!(migrate_legacy(&game), Migration::default(), "改过的不再改");
    }

    /// instances/ 里已经有同名的：换个名字搬，不覆盖
    #[test]
    fn migration_never_overwrites_an_existing_instance() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        create(&game, "生存", &InstanceConfig::new("生存", "1.21.1", None)).unwrap();
        write(
            &game
                .versions()
                .join("生存")
                .join(META_DIR)
                .join(CONFIG_FILE),
            r#"{"format":1,"name":"老生存","gameVersion":"1.20.1","loader":null,"createdAt":1,"source":null}"#,
        );
        assert_eq!(migrate_legacy(&game).moved, 1);
        assert_eq!(load(&game, "生存").unwrap().unwrap().config.name, "生存");
        assert_eq!(
            load(&game, "生存 (2)").unwrap().unwrap().config.name,
            "老生存"
        );
    }

    #[test]
    fn instance_notes_round_trip_clear_and_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        create(&game, "生存", &InstanceConfig::new("生存", "1.21.1", None)).unwrap();

        // 尚未创建时返回空字符串
        assert_eq!(read_notes(&game, "生存").unwrap(), "");

        // 原子写入并规范化换行符（含 Markdown 与潜在 XSS 文本按原文安全存储，由前端清洗）
        let raw = "# 整合包攻略\r\n- [ ] 调试内存\r\n<script>alert(1)</script>";
        let saved = write_notes(&game, "生存", raw).unwrap();
        assert_eq!(
            saved,
            "# 整合包攻略\n- [ ] 调试内存\n<script>alert(1)</script>"
        );
        assert_eq!(read_notes(&game, "生存").unwrap(), saved);
        assert!(notes_path(&game, "生存").unwrap().is_file());

        // 超出 256 KiB 拒绝写入且不破坏已有笔记
        let oversized = "a".repeat(MAX_NOTES_BYTES + 1);
        assert!(write_notes(&game, "生存", &oversized).is_err());
        assert_eq!(read_notes(&game, "生存").unwrap(), saved);

        // 仅空白内容时自动清理文件
        assert_eq!(write_notes(&game, "生存", "   \r\n\t ").unwrap(), "");
        assert!(!notes_path(&game, "生存").unwrap().exists());
        assert_eq!(read_notes(&game, "生存").unwrap(), "");
    }

    #[test]
    fn loader_compatibility_matrix_covers_legacy_ancient_and_snapshots() {
        assert!(LoaderKind::NeoForge.installable_for("1.21.1"));
        assert!(LoaderKind::NeoForge.installable_for("1.20.1"));
        assert!(LoaderKind::NeoForge.installable_for("26.1"));
        assert!(!LoaderKind::NeoForge.installable_for("1.19.2"));
        assert!(!LoaderKind::NeoForge.installable_for("26.4-pre-1"));

        assert!(LoaderKind::Forge.installable_for("1.21.1"));
        assert!(LoaderKind::Forge.installable_for("1.12.2"));
        assert!(LoaderKind::Forge.installable_for("1.7.10"));
        assert!(LoaderKind::Forge.installable_for("1.5.2"));
        assert!(!LoaderKind::Forge.installable_for("1.5.1"));
        assert!(!LoaderKind::Forge.installable_for("b1.7.3"));

        assert!(LoaderKind::Fabric.installable_for("1.21.1"));
        assert!(LoaderKind::Fabric.installable_for("26.4-pre-1"));
        assert!(LoaderKind::Fabric.installable_for("24w14potato"));
        assert!(LoaderKind::Fabric.installable_for("25w14craftmine"));
        assert!(!LoaderKind::Fabric.installable_for("1.12.2"));
        assert!(!LoaderKind::Fabric.installable_for("b1.7.3"));

        assert!(LoaderKind::LegacyFabric.installable_for("1.13.2"));
        assert!(LoaderKind::LegacyFabric.installable_for("1.8.9"));
        assert!(LoaderKind::LegacyFabric.installable_for("1.3.2"));
        assert!(!LoaderKind::LegacyFabric.installable_for("1.14"));
        assert!(!LoaderKind::LegacyFabric.installable_for("1.2.5"));

        assert!(LoaderKind::Babric.installable_for("b1.7.3"));
        assert!(!LoaderKind::Babric.installable_for("1.7.10"));

        assert_eq!(
            LoaderRef {
                kind: LoaderKind::LegacyFabric,
                version: "0.16.10".to_owned(),
            }
            .version_id("1.8.9"),
            "legacy-fabric-loader-0.16.10-1.8.9"
        );
        assert_eq!(
            LoaderRef {
                kind: LoaderKind::Babric,
                version: "0.16.10".to_owned(),
            }
            .version_id("b1.7.3"),
            "babric-loader-0.16.10-b1.7.3"
        );
    }
}
