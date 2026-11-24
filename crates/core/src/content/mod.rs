//! 实例内容：模组、光影、材质包。
//!
//! - 列出实例里装了什么（按文件夹扫描，配合本启动器记的来源信息）
//! - 从 Modrinth 解析并安装（自动带前置，装光影自动补 Iris）
//! - 启用 / 停用：模组改名为 `.disabled`；光影写 Iris 设置；材质包写 `options.txt`
//! - 识别（`identify`）：手动放进来的文件也能认出名字、版本、图标（Modrinth 哈希 → CurseForge 指纹 → 文件自带说明）
//! - 检查更新、一键更新、删除；每次改动记一条历史，**被换掉或删掉的文件先进内容仓库**（`store`），
//!   所以都能撤销
//! - 一次改好几个文件的操作都先写改动日志（`journal`）：中途出错或者启动器被关掉，退回改之前

pub mod bundle;
pub mod cache;
pub mod claim;
pub mod curseforge;
pub mod detail;
pub mod dropped;
pub mod favorites;
pub mod fingerprint;
pub mod identify;
pub mod journal;
pub mod maps;
pub mod modrinth;
pub mod names;
pub mod snapshot;
pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::fs::{
    UnsafePath, read_json, safe_file_name, safe_join, write_atomic, write_json_atomic,
};
use crate::instance::META_DIR;
use crate::net::{DownloadItem, DownloadProgress, Downloader, sha1_file};
use crate::paths::GameDir;
use claim::{ClaimSession, ManualFile};
pub use identify::IdentitySource;
use identify::{Identity, IdentityCache, file_key, icon_data_url, read_local_meta, save_icon};
pub use journal::Recovered;
use journal::{Plan, read_text};
pub use snapshot::{ChangeType, OfferChange, RollbackOffer};
use store::{ContentStore, STORE_DIR};

/// 本启动器记录「这个文件是从哪个项目装的」
const RECORDS_FILE: &str = "content.json";
/// 锁定版本的文件键集合（`<文件夹>/<文件名>`），锁定的项跳过检查与批量更新
const LOCKS_FILE: &str = "content-locks.json";
const DISABLED_SUFFIX: &str = ".disabled";

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("网络请求失败")]
    Http(#[from] crate::net::HttpError),
    #[error("Modrinth 返回的数据有问题：{0}")]
    Api(String),
    #[error(
        "这个启动器没有配置 CurseForge 的 API Key，用不了 CurseForge 的内容（Modrinth 不受影响）"
    )]
    NoCurseForgeKey,
    #[error("CurseForge 拒绝了启动器的请求（{what}）：API Key 可能无效、被撤销，或者没有这项权限")]
    CurseForgeDenied { what: String },
    #[error("CurseForge 返回的数据有问题：{0}")]
    CurseForge(String),
    #[error("{project} 没有适用于 Minecraft {game_version} 的版本")]
    NoCompatibleVersion {
        project: String,
        game_version: String,
    },
    #[error("原版实例不能装模组或光影，请新建一个 NeoForge 实例")]
    NeedsLoader,
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error(transparent)]
    Download(#[from] crate::net::DownloadError),
    #[error("保存文件失败")]
    Write(#[from] crate::fs::AtomicWriteError),
    #[error("读取内容记录失败")]
    Read(#[from] crate::fs::ReadJsonError),
    #[error("找不到文件 {0}")]
    NotFound(String),
    #[error("{0}")]
    Message(String),
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// 改到一半出错，已经退回改之前
    #[error("{reason}。「{summary}」没做成，实例保持原样")]
    Reverted { summary: String, reason: String },
    /// 改到一半出错，退回也没成功：改动日志留着，下次打开启动器或者下次改这个实例之前再退
    #[error("{reason}。没改完，退回改之前时也出了问题，下次打开启动器会再试")]
    NotReverted { reason: String },
    /// 上次没做完的改动退不回去
    #[error(
        "这个实例上次「{summary}」没做完，退回时出了问题：{reason}。关掉可能占着文件的程序（游戏、杀毒软件）后再试"
    )]
    Pending { summary: String, reason: String },
}

fn io_error(path: &Path, source: std::io::Error) -> ContentError {
    ContentError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ContentKind {
    Mod,
    Shader,
    ResourcePack,
}

impl ContentKind {
    pub const ALL: [ContentKind; 3] = [
        ContentKind::Mod,
        ContentKind::Shader,
        ContentKind::ResourcePack,
    ];

    /// 实例里的文件夹
    pub fn folder(self) -> &'static str {
        match self {
            ContentKind::Mod => "mods",
            ContentKind::Shader => "shaderpacks",
            ContentKind::ResourcePack => "resourcepacks",
        }
    }

    /// Modrinth 的项目类型
    pub fn project_type(self) -> &'static str {
        match self {
            ContentKind::Mod => "mod",
            ContentKind::Shader => "shader",
            ContentKind::ResourcePack => "resourcepack",
        }
    }

    pub fn from_project_type(value: &str) -> Option<Self> {
        match value {
            "mod" => Some(ContentKind::Mod),
            "shader" => Some(ContentKind::Shader),
            "resourcepack" => Some(ContentKind::ResourcePack),
            _ => None,
        }
    }

    fn accepts(self, file_name: &str) -> bool {
        let lower = file_name.to_ascii_lowercase();
        let lower = lower.strip_suffix(DISABLED_SUFFIX).unwrap_or(&lower);
        match self {
            ContentKind::Mod => lower.ends_with(".jar"),
            ContentKind::Shader | ContentKind::ResourcePack => lower.ends_with(".zip"),
        }
    }
}

/// 内容来自哪个平台。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ContentSource {
    #[default]
    Modrinth,
    CurseForge,
}

/// 资源中心的排序。两个平台各自换成自己的排序参数，结果顺序和平台官网一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum SearchSort {
    /// 有关键词时按相关度；没有关键词时按下载量（和以前一样）
    #[default]
    Relevance,
    /// 总下载量
    Downloads,
    /// 最近更新
    Updated,
    /// 最新发布（项目创建时间）
    Newest,
}

impl SearchSort {
    /// Modrinth 搜索接口的 `index`
    pub fn modrinth_index(self, text: &str) -> &'static str {
        match self {
            SearchSort::Relevance if text.trim().is_empty() => "downloads",
            SearchSort::Relevance => "relevance",
            SearchSort::Downloads => "downloads",
            SearchSort::Updated => "updated",
            SearchSort::Newest => "newest",
        }
    }

    /// CurseForge 搜索接口的 `sortField`：1 推荐、3 最近更新、6 总下载量、11 发布时间。
    /// 2026-11-12 实测：11 按 `dateCreated` 排；3 按 CurseForge 自己索引里的时间排，
    /// 和返回的 `dateModified`、`dateReleased` 都对不齐，所以界面不能按日期重排 CurseForge 的结果。
    pub fn curseforge_field(self, text: &str) -> u32 {
        match self {
            SearchSort::Relevance if text.trim().is_empty() => 6,
            // 「推荐」实测最接近按相关度：搜 create 时 Create 本体排第一，按热度排会被 Xaero's Minimap 挤到第二
            SearchSort::Relevance => 1,
            SearchSort::Downloads => 6,
            SearchSort::Updated => 3,
            SearchSort::Newest => 11,
        }
    }

    /// 搜索缓存的键里用的名字
    pub fn key(self) -> &'static str {
        match self {
            SearchSort::Relevance => "relevance",
            SearchSort::Downloads => "downloads",
            SearchSort::Updated => "updated",
            SearchSort::Newest => "newest",
        }
    }
}

/// Modrinth 的分类名（`technology`、`8x-`、`512x+`）只认小写字母、数字和 `-+_`；不像的不拿去搜
pub fn modrinth_category(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-+_".contains(&b));
    ok.then_some(name)
}

/// 安装时怎么拿到这个文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Delivery {
    /// 从平台直接下载
    #[default]
    Direct,
    /// CurseForge 作者禁止第三方下载，但 Modrinth 上有一模一样的文件，从 Modrinth 下载
    Modrinth,
    /// 只能由用户在浏览器里下载（补件）
    Manual,
}

/// 一条来源记录（本启动器装的）。`project_id` / `version_id` 是 `source` 那个平台的编号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    project_id: String,
    version_id: String,
    title: String,
    version_number: String,
    icon_url: Option<String>,
    #[serde(default)]
    source: ContentSource,
}

/// 要装进实例的一个本地文件（[`InstanceContent::add_local`]）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAdd {
    pub kind: ContentKind,
    pub source: PathBuf,
    /// 顺带换掉的旧版本（实例里的文件名，启用时的名字）
    pub replace: Option<String>,
}

/// 装本地文件的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOutcome {
    Added,
    /// 实例里已经有一模一样的文件
    Same,
}

/// 下载到一半的安装放在内容仓库下面的这个文件夹里，每个任务一个（启动器启动时整个清掉）
pub const STAGING_DIR: &str = ".staging";

/// 下载好、还没放进实例的安装（[`InstanceContent::download_install`] 的结果）。
/// 几个安装可以同时下载（各下各的文件夹），放进实例（[`InstanceContent::finish_install`]）要一个一个来。
/// 丢掉（取消、失败）时下载的文件一起删掉。
pub struct StagedInstall {
    prepared: PreparedInstall,
    dir: PathBuf,
}

impl StagedInstall {
    fn path_of(&self, entry: &Prepared) -> Result<PathBuf, ContentError> {
        Ok(safe_join(
            &self.dir.join(entry.kind.folder()),
            &entry.file_name,
        )?)
    }
}

impl Drop for StagedInstall {
    fn drop(&mut self) {
        remove_staging(&self.dir);
    }
}

fn remove_staging(dir: &Path) {
    if let Err(error) = std::fs::remove_dir_all(dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, dir = %dir.display(), "清理下载临时文件夹失败");
    }
}

fn new_staging_dir(store_root: &Path) -> PathBuf {
    store_root
        .join(STAGING_DIR)
        .join(uuid::Uuid::new_v4().simple().to_string())
}

/// 准备好的安装（[`InstanceContent::prepare_install`] 的结果）。
pub struct PreparedInstall {
    entries: Vec<Prepared>,
    /// 要用户在浏览器里下载的（作者禁止第三方下载、Modrinth 上也没有同样的文件）
    pub manual: Vec<ManualFile>,
}

struct Prepared {
    kind: ContentKind,
    file_name: String,
    sha1: String,
    size: u64,
    /// 下载地址；`None` 表示要用户补（补齐后从内容仓库放进来）
    urls: Option<Vec<String>>,
    record: Record,
}

/// `content.json`：键是 `<文件夹>/<文件名>`（启用时的名字）
type Records = BTreeMap<String, Record>;

fn record_key(kind: ContentKind, file_name: &str) -> String {
    format!("{}/{file_name}", kind.folder())
}

/// 文件在实例里的相对路径（改动日志用，`/` 分隔）：`mods/sodium.jar`、停用的 `mods/sodium.jar.disabled`
fn rel_path(kind: ContentKind, file_name: &str, disabled: bool) -> Result<String, ContentError> {
    safe_file_name(file_name)?;
    let suffix = if disabled { DISABLED_SUFFIX } else { "" };
    Ok(format!("{}/{file_name}{suffix}", kind.folder()))
}

/// 记录文件在实例里的相对路径（`Lumi/content.json` 这些）
fn meta_rel(file: &str) -> String {
    format!("{META_DIR}/{file}")
}

/// 和 `write_json_atomic` 写出来的一样：带缩进、最后换行
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ContentError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| ContentError::Message(format!("整理记录失败：{e}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// 更新后新记录沿用的图标：先看旧的安装记录；手动放进来的没有记录，就用识别缓存里的
fn carried_icon(old: Option<&Record>, cache: &IdentityCache, old_sha1: &str) -> Option<String> {
    old.and_then(|r| r.icon_url.clone())
        .or_else(|| cache.identity(old_sha1).and_then(|i| i.icon_url.clone()))
}

/// 实例里的一项内容。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ContentItem {
    pub kind: ContentKind,
    /// 启用时的文件名（停用的模组磁盘上多一个 `.disabled`）
    pub file_name: String,
    pub title: String,
    pub enabled: bool,
    pub size: f64,
    pub project_id: Option<String>,
    /// 当前安装的版本编号（Modrinth versionId / CurseForge fileId，切换版本高亮用）
    pub version_id: Option<String>,
    pub version_number: Option<String>,
    /// 是否锁定版本（跳过自动/批量检查与更新）
    pub locked: bool,
    /// 图标：网址或 `data:` 地址
    pub icon_url: Option<String>,
    /// 怎么认出来的；`None` 表示还没认出来（只显示文件名）
    pub source: Option<IdentitySource>,
    /// 解开的文件夹（光影、材质包可以这样放）：不能更新、不能删除
    pub is_folder: bool,
    /// 作者（刷新时从平台补，补不到为空）
    pub author: Option<String>,
    /// 作者主页（Modrinth 的才有）
    pub author_url: Option<String>,
    /// 项目在平台官网上的页面（「复制链接」「在浏览器中打开」用）
    pub page_url: Option<String>,
    /// 文件最后修改时间（毫秒；按新旧排序用）
    pub modified: f64,
    /// 中文名（中文名表或模组自带的语言文件）
    pub title_zh: Option<String>,
    /// 同一个项目在另一个平台上的编号（中文名表或同一个文件确认过的；资源中心判断「已安装」用）
    pub twin_id: Option<String>,
}

/// 项目在平台官网上的页面：用平台的通用地址，平台会自己跳到正确的页面
fn project_page(source: ContentSource, project_id: &str) -> Option<String> {
    let safe = !project_id.is_empty()
        && project_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b));
    if !safe {
        return None;
    }
    Some(match source {
        ContentSource::Modrinth => format!("https://modrinth.com/project/{project_id}"),
        ContentSource::CurseForge => format!("https://www.curseforge.com/projects/{project_id}"),
    })
}

/// 这个文件是哪个平台的哪个项目：启动器自己装的看安装记录；认出来的看识别缓存
fn platform_of(
    record: Option<&Record>,
    identity: Option<&Identity>,
    sha1: Option<&str>,
    cache: &IdentityCache,
) -> Option<(ContentSource, String)> {
    if let Some(record) = record {
        return Some((record.source, record.project_id.clone()));
    }
    match identity?.source {
        IdentitySource::Modrinth => identity?
            .project_id
            .clone()
            .map(|id| (ContentSource::Modrinth, id)),
        IdentitySource::CurseForge => sha1
            .and_then(|h| cache.curseforge_project(h))
            .map(|id| (ContentSource::CurseForge, id.to_string())),
        IdentitySource::File => None,
    }
}

/// 一个可以更新（或切换）的内容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub kind: ContentKind,
    pub file_name: String,
    pub title: String,
    pub current_version: Option<String>,
    pub version_id: String,
    pub version_number: String,
    /// 目标版本来自哪个平台（默认 Modrinth，兼容旧历史重试记录）
    #[serde(default)]
    pub source: ContentSource,
    /// 项目在平台上的编号（CurseForge 更新与补件用）
    #[serde(default)]
    pub project_id: Option<String>,
    /// 目标版本下载方式（CurseForge 作者禁止第三方下载时可能为 `Modrinth` 或 `Manual`）
    #[serde(default)]
    pub delivery: Delivery,
    /// 补件时的精确下载页
    #[serde(default)]
    pub page_url: Option<String>,
}

/// 准备好的更新（[`InstanceContent::prepare_update`] 的结果，支持 Modrinth + CurseForge 补件）。
pub struct PreparedUpdate {
    entries: Vec<PreparedUpdateEntry>,
    /// 要用户在浏览器里下载的 CurseForge 受限更新文件
    pub manual: Vec<ManualFile>,
}

struct PreparedUpdateEntry {
    update: UpdateInfo,
    project_id: String,
    new_name: String,
    sha1: String,
    size: u64,
    urls: Option<Vec<String>>,
    source: ContentSource,
}

/// 下载好、待写入实例的更新批次。丢掉时自动清理临时下载目录。
pub struct StagedUpdate {
    prepared: PreparedUpdate,
    dir: PathBuf,
}

impl StagedUpdate {
    fn path_of(&self, index: usize, file_name: &str) -> Result<PathBuf, ContentError> {
        Ok(safe_join(&self.dir.join(index.to_string()), file_name)?)
    }
}

impl Drop for StagedUpdate {
    fn drop(&mut self) {
        remove_staging(&self.dir);
    }
}

/// 一次更新检查的结果。统计单位是实例里的内容项，不是平台返回的哈希数。
#[derive(Debug, Default, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub updates: Vec<UpdateInfo>,
    /// 已找到可验证的兼容版本，包含已有最新版本和可更新的内容。
    pub checked: u32,
    /// 文件夹、读不到的文件，以及平台没给出可验证结果的内容。
    pub unchecked: u32,
    /// 整次平台查询失败，调用方应显示失败说明。
    pub lookup_failed: bool,
}

/// 内容管理针对的实例目录。
pub struct InstanceContent {
    /// 实例游戏目录（`versions/<实例名>/`）
    pub dir: PathBuf,
    store: ContentStore,
    /// 测试用：在改动的某一步模拟出事
    #[cfg(test)]
    fault: std::sync::Mutex<Option<journal::Fault>>,
}

/// 一次改动的草稿：要做的文件操作 + 改完之后的来源记录、光影 / 材质包设置。
/// 排好后交给 [`InstanceContent::commit`]，一起写进改动日志再执行。
struct Draft {
    plan: Plan,
    records: Records,
    records_were: Records,
    active: Active,
}

impl Draft {
    /// 排一个「收进仓库」：来源记录一并拿掉，正在用的光影 / 材质包关掉。返回给改动历史用的记录
    fn take(
        &mut self,
        kind: ContentKind,
        file_name: &str,
        disabled: bool,
        sha1: String,
    ) -> Result<FileRef, ContentError> {
        let enabled = self.active.is_enabled(kind, file_name, disabled);
        self.active.set(kind, file_name, false);
        self.plan
            .take(rel_path(kind, file_name, disabled)?, sha1.clone());
        Ok(FileRef {
            file_name: file_name.to_owned(),
            sha1,
            disabled,
            enabled,
            record: self.records.remove(&record_key(kind, file_name)),
        })
    }
}

/// 扫描到的一个文件
struct Scanned {
    file_name: String,
    disk_name: String,
    metadata: std::fs::Metadata,
    disabled: bool,
}

impl InstanceContent {
    /// `store_root` 是内容仓库的位置（通常是 `<游戏目录>/.mcl-store`）
    pub fn new(dir: impl Into<PathBuf>, store_root: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            store: ContentStore::new(store_root),
            #[cfg(test)]
            fault: std::sync::Mutex::new(None),
        }
    }

    /// 实例在这个游戏目录里，仓库用游戏目录下的 `.mcl-store`
    pub fn for_game(game: &GameDir, instance_dir: impl Into<PathBuf>) -> Self {
        Self::new(instance_dir, game.root().join(STORE_DIR))
    }

    fn meta_path(&self, file: &str) -> PathBuf {
        self.dir.join(META_DIR).join(file)
    }

    fn load_records(&self) -> Records {
        match read_json(&self.meta_path(RECORDS_FILE)) {
            Ok(Some(records)) => records,
            Ok(None) => Records::new(),
            Err(error) => {
                tracing::warn!(%error, "内容记录读不了，按空记录处理");
                Records::new()
            }
        }
    }

    fn load_locks(&self) -> BTreeSet<String> {
        match read_json(&self.meta_path(LOCKS_FILE)) {
            Ok(Some(locks)) => locks,
            Ok(None) => BTreeSet::new(),
            Err(error) => {
                tracing::warn!(%error, "锁定记录读不了，按未锁定处理");
                BTreeSet::new()
            }
        }
    }

    fn save_locks(&self, locks: &BTreeSet<String>) -> Result<(), ContentError> {
        write_json_atomic(&self.meta_path(LOCKS_FILE), locks)?;
        Ok(())
    }

    /// 锁定或取消锁定某项内容的版本。只影响自动/批量检查与更新，不影响启用/停用开关。
    pub fn set_locked(
        &self,
        kind: ContentKind,
        file_name: &str,
        locked: bool,
    ) -> Result<bool, ContentError> {
        safe_file_name(file_name)?;
        let (path, _) = self.existing(kind, file_name)?;
        if path.is_dir() {
            return Err(ContentError::Message(
                "文件夹形式的内容不支持锁定版本".to_owned(),
            ));
        }
        let key = record_key(kind, file_name);
        let mut locks = self.load_locks();
        let changed = if locked {
            locks.insert(key)
        } else {
            locks.remove(&key)
        };
        if changed {
            self.save_locks(&locks)?;
        }
        Ok(locked)
    }

    fn draft(&self) -> Draft {
        let records = self.load_records();
        Draft {
            plan: Plan::default(),
            records_were: records.clone(),
            records,
            active: Active::load(&self.dir),
        }
    }

    /// 草稿收尾：改过的来源记录、光影 / 材质包设置、改动历史（`history` 是写进去的完整历史）排进计划，
    /// 写改动日志、执行（[`Self::apply`]）。`summary` 是这次做什么，出事时告诉用户
    async fn commit(
        &self,
        draft: Draft,
        summary: &str,
        history: Option<Vec<HistoryEntry>>,
    ) -> Result<(), ContentError> {
        let Draft {
            mut plan,
            records,
            records_were,
            active,
        } = draft;
        if records != records_were {
            plan.write(meta_rel(RECORDS_FILE), json_bytes(&records)?);
        }
        active.plan(&mut plan, &self.dir)?;
        if let Some(history) = history {
            plan.write(meta_rel(HISTORY_FILE), json_bytes(&history)?);
        }
        self.apply(plan, summary).await
    }

    fn folder(&self, kind: ContentKind) -> PathBuf {
        self.dir.join(kind.folder())
    }

    fn disk_path(
        &self,
        kind: ContentKind,
        file_name: &str,
        disabled: bool,
    ) -> Result<PathBuf, ContentError> {
        let disk = if disabled {
            format!("{file_name}{DISABLED_SUFFIX}")
        } else {
            file_name.to_owned()
        };
        Ok(safe_join(&self.folder(kind), &disk)?)
    }

    /// 文件现在在磁盘上的位置（启用或停用）
    fn existing(
        &self,
        kind: ContentKind,
        file_name: &str,
    ) -> Result<(PathBuf, bool), ContentError> {
        for disabled in [false, true] {
            let path = self.disk_path(kind, file_name, disabled)?;
            if path.exists() {
                return Ok((path, disabled));
            }
        }
        Err(ContentError::NotFound(file_name.to_owned()))
    }

    fn scan(&self, kind: ContentKind) -> Result<Vec<Scanned>, ContentError> {
        let folder = self.folder(kind);
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(io_error(&folder, source)),
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let Ok(disk_name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            // 光影和材质包也可以是解压开的文件夹
            let is_folder_pack = metadata.is_dir() && kind != ContentKind::Mod;
            if !is_folder_pack && !(metadata.is_file() && kind.accepts(&disk_name)) {
                continue;
            }
            let disabled = kind == ContentKind::Mod && disk_name.ends_with(DISABLED_SUFFIX);
            let file_name = disk_name
                .strip_suffix(DISABLED_SUFFIX)
                .filter(|_| disabled)
                .unwrap_or(&disk_name)
                .to_owned();
            found.push(Scanned {
                file_name,
                disk_name,
                metadata,
                disabled,
            });
        }
        Ok(found)
    }

    /// 列出某类内容（按标题排序）。只用缓存，不联网、不算哈希，很快。
    pub fn list(&self, kind: ContentKind) -> Result<Vec<ContentItem>, ContentError> {
        let records = self.load_records();
        let locks = self.load_locks();
        let cache = IdentityCache::load(self.store.root());
        let learned = names::Learned::from_cache(&cache);
        let active_shader = current_shader(&self.dir);
        let active_packs = enabled_resource_packs(&self.dir);
        let mut items: Vec<ContentItem> = self
            .scan(kind)?
            .into_iter()
            .map(|s| {
                let record = records.get(&record_key(kind, &s.file_name));
                let sha1 = cache.hash_of(&file_key(kind.folder(), &s.disk_name, &s.metadata));
                let identity = sha1.and_then(|sha1| cache.identity(sha1));
                let platform = platform_of(record, identity, sha1.map(String::as_str), &cache);
                let author = platform
                    .as_ref()
                    .and_then(|(src, id)| cache.author(&IdentityCache::author_key(*src, id)))
                    .cloned();
                let author_url = match (&platform, &author) {
                    (Some((ContentSource::Modrinth, _)), Some(name)) => {
                        modrinth::modrinth_user(name)
                    }
                    _ => None,
                };
                let page_url = platform
                    .as_ref()
                    .and_then(|(src, id)| project_page(*src, id));
                let modified = s
                    .metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0.0, |d| d.as_millis() as f64);
                let enabled = match kind {
                    ContentKind::Mod => !s.disabled,
                    ContentKind::Shader => active_shader.as_deref() == Some(s.file_name.as_str()),
                    ContentKind::ResourcePack => {
                        active_packs.contains(&format!("file/{}", s.file_name))
                    }
                };
                let icon_url = record
                    .and_then(|r| r.icon_url.clone())
                    .or_else(|| identity.and_then(|i| i.icon_url.clone()))
                    .or_else(|| {
                        identity
                            .and_then(|i| i.icon_file.as_deref())
                            .and_then(|name| icon_data_url(self.store.root(), name))
                    });
                ContentItem {
                    kind,
                    title: record
                        .map(|r| r.title.clone())
                        .or_else(|| identity.map(|i| i.title.clone()))
                        .unwrap_or_else(|| display_name(&s.file_name)),
                    enabled,
                    size: if s.metadata.is_file() {
                        s.metadata.len() as f64
                    } else {
                        0.0
                    },
                    // 平台上的编号：Modrinth 的或者 CurseForge 的（看 `source`）
                    project_id: platform.as_ref().map(|(_, id)| id.clone()),
                    version_id: record
                        .map(|r| r.version_id.clone())
                        .or_else(|| identity.and_then(|i| i.version_id.clone())),
                    version_number: record
                        .map(|r| r.version_number.clone())
                        .or_else(|| identity.and_then(|i| i.version.clone())),
                    locked: !s.metadata.is_dir() && locks.contains(&record_key(kind, &s.file_name)),
                    icon_url,
                    source: if let Some(record) = record {
                        Some(match record.source {
                            ContentSource::Modrinth => IdentitySource::Modrinth,
                            ContentSource::CurseForge => IdentitySource::CurseForge,
                        })
                    } else {
                        identity.map(|i| i.source)
                    },
                    is_folder: s.metadata.is_dir(),
                    file_name: s.file_name,
                    author,
                    author_url,
                    page_url,
                    modified,
                    title_zh: platform
                        .as_ref()
                        .and_then(|(src, id)| learned.zh(*src, id))
                        .or_else(|| sha1.and_then(|h| cache.zh_name(h)).map(str::to_owned)),
                    twin_id: platform
                        .as_ref()
                        .and_then(|(src, id)| learned.twin(*src, id)),
                }
            })
            .collect();
        items.sort_by_key(|item| item.title.to_lowercase());
        Ok(items)
    }

    /// 算哈希（没改过的文件用缓存），返回 (扫描结果, 哈希)。文件夹形式的包跳过。
    async fn hash_files(
        &self,
        kind: ContentKind,
        cache: &mut IdentityCache,
    ) -> Result<Vec<(Scanned, String)>, ContentError> {
        let mut hashed = Vec::new();
        for scanned in self.scan(kind)? {
            if !scanned.metadata.is_file() {
                continue;
            }
            let sha1 = self.hash_scanned_file(kind, &scanned, cache).await?;
            hashed.push((scanned, sha1));
        }
        Ok(hashed)
    }

    async fn hash_scanned_file(
        &self,
        kind: ContentKind,
        scanned: &Scanned,
        cache: &mut IdentityCache,
    ) -> Result<String, ContentError> {
        let key = file_key(kind.folder(), &scanned.disk_name, &scanned.metadata);
        if let Some(sha1) = cache.hash_of(&key) {
            return Ok(sha1.clone());
        }
        let path = self.folder(kind).join(&scanned.disk_name);
        let sha1 = sha1_file(&path)
            .await
            .map_err(|source| io_error(&path, source))?;
        cache.remember_hash(key, sha1.clone());
        Ok(sha1)
    }

    /// 认出还不认识的文件（先问 Modrinth，再按指纹问 CurseForge，都没有就读文件自带的说明），再列出来。
    /// 断网时联网的步骤跳过，下次再问。
    pub async fn refresh(
        &self,
        client: &Client,
        curseforge: &curseforge::CurseForge,
        kind: ContentKind,
    ) -> Result<Vec<ContentItem>, ContentError> {
        let records = self.load_records();
        let mut cache = IdentityCache::load(self.store.root());
        let hashed = self.hash_files(kind, &mut cache).await?;
        let unknown: Vec<&(Scanned, String)> = hashed
            .iter()
            .filter(|(s, sha1)| {
                !records.contains_key(&record_key(kind, &s.file_name))
                    && !cache.asked_modrinth(sha1)
            })
            .collect();

        if !unknown.is_empty() {
            let hashes: Vec<String> = unknown.iter().map(|(_, sha1)| sha1.clone()).collect();
            match modrinth::identify_detailed(client, &hashes).await {
                Ok(found) => {
                    for sha1 in &hashes {
                        match found.get(sha1) {
                            Some(remote) => cache.remember(
                                sha1.clone(),
                                Identity {
                                    title: remote.title.clone(),
                                    version: Some(remote.version_number.clone()),
                                    icon_url: remote.icon_url.clone(),
                                    icon_file: None,
                                    project_id: Some(remote.project_id.clone()),
                                    version_id: Some(remote.version_id.clone()),
                                    source: IdentitySource::Modrinth,
                                },
                            ),
                            None => cache.mark_not_on_modrinth(sha1),
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "联网识别失败（可能断网），先用文件自带的说明")
                }
            }
        }

        // Modrinth 上没有的，按指纹问 CurseForge（以前只从文件里读到说明的也再问一次）
        if curseforge.usable() {
            let ask: Vec<&(Scanned, String)> = hashed
                .iter()
                .filter(|(s, sha1)| {
                    !records.contains_key(&record_key(kind, &s.file_name))
                        && cache.asked_modrinth(sha1)
                        && cache.should_ask_curseforge(sha1)
                })
                .collect();
            self.identify_on_curseforge(curseforge, kind, &ask, &mut cache)
                .await;
        }

        // 两个平台上都没有（或者没联网）的，读文件自带的说明
        for (scanned, sha1) in &unknown {
            if cache.identity(sha1).is_some() {
                continue;
            }
            let path = self.folder(kind).join(&scanned.disk_name);
            let meta = tokio::task::spawn_blocking(move || read_local_meta(&path))
                .await
                .unwrap_or_default();
            let icon_file = meta
                .icon
                .as_deref()
                .and_then(|bytes| save_icon(self.store.root(), bytes));
            cache.remember(
                sha1.clone(),
                Identity {
                    title: meta
                        .title
                        .unwrap_or_else(|| display_name(&scanned.file_name)),
                    version: meta.version,
                    icon_url: None,
                    icon_file,
                    project_id: None,
                    version_id: None,
                    source: IdentitySource::File,
                },
            );
        }
        if kind == ContentKind::Mod {
            self.fill_zh_names(&hashed, &records, &mut cache).await;
        }
        self.fill_authors(client, curseforge, kind, &records, &mut cache)
            .await;
        cache.save(self.store.root())?;
        self.list(kind)
    }

    /// 读模组自带的简体中文语言文件认中文名（每个文件只认一次，结果按哈希记在识别缓存里）
    async fn fill_zh_names(
        &self,
        hashed: &[(Scanned, String)],
        records: &Records,
        cache: &mut IdentityCache,
    ) {
        let todo: Vec<(PathBuf, String, Vec<String>)> = hashed
            .iter()
            .filter(|(_, sha1)| !cache.knows_zh(sha1))
            .map(|(scanned, sha1)| {
                let known = records
                    .get(&record_key(ContentKind::Mod, &scanned.file_name))
                    .map(|r| r.title.clone())
                    .into_iter()
                    .chain(cache.identity(sha1).map(|i| i.title.clone()))
                    .collect();
                (
                    self.folder(ContentKind::Mod).join(&scanned.disk_name),
                    sha1.clone(),
                    known,
                )
            })
            .collect();
        if todo.is_empty() {
            return;
        }
        let found = tokio::task::spawn_blocking(move || {
            todo.into_iter()
                .map(|(path, sha1, known)| {
                    let known: Vec<&str> = known.iter().map(String::as_str).collect();
                    (sha1, names::zh_name_in_file(&path, &known))
                })
                .collect::<Vec<_>>()
        })
        .await;
        match found {
            Ok(found) => {
                let named = found.iter().filter(|(_, n)| n.is_some()).count();
                tracing::info!(files = found.len(), named, "读模组自带的中文名");
                for (sha1, name) in found {
                    cache.remember_zh(&sha1, name);
                }
            }
            Err(error) => tracing::warn!(%error, "读模组中文名的线程出错，下次刷新再认"),
        }
    }

    /// 补作者：这一类里已经认出来、但还没问过作者的项目，两个平台各一次批量查。
    /// 失败（断网、CurseForge 没有 Key）只记日志，下次刷新再试。
    async fn fill_authors(
        &self,
        client: &Client,
        curseforge: &curseforge::CurseForge,
        kind: ContentKind,
        records: &Records,
        cache: &mut IdentityCache,
    ) {
        let Ok(scanned) = self.scan(kind) else {
            return;
        };
        let mut modrinth_ids: Vec<String> = Vec::new();
        let mut curseforge_ids: Vec<u32> = Vec::new();
        for s in &scanned {
            let record = records.get(&record_key(kind, &s.file_name));
            let sha1 = cache
                .hash_of(&file_key(kind.folder(), &s.disk_name, &s.metadata))
                .cloned();
            let identity = sha1.as_deref().and_then(|h| cache.identity(h));
            let Some((source, id)) = platform_of(record, identity, sha1.as_deref(), cache) else {
                continue;
            };
            if cache.knows_author(&IdentityCache::author_key(source, &id)) {
                continue;
            }
            match source {
                ContentSource::Modrinth => {
                    if !modrinth_ids.contains(&id) {
                        modrinth_ids.push(id);
                    }
                }
                ContentSource::CurseForge => {
                    if let Ok(n) = id.parse::<u32>()
                        && !curseforge_ids.contains(&n)
                    {
                        curseforge_ids.push(n);
                    }
                }
            }
        }
        if !modrinth_ids.is_empty() {
            match modrinth::project_owners(client, &modrinth_ids).await {
                Ok(owners) => {
                    for id in &modrinth_ids {
                        let author = owners.get(id).cloned().unwrap_or_default();
                        cache.remember_author(
                            IdentityCache::author_key(ContentSource::Modrinth, id),
                            author,
                        );
                    }
                }
                Err(error) => tracing::warn!(%error, "从 Modrinth 补作者失败，下次刷新再试"),
            }
        }
        if !curseforge_ids.is_empty() && curseforge.usable() {
            match curseforge.mods(&curseforge_ids).await {
                Ok(mods) => {
                    for id in &curseforge_ids {
                        let author = mods
                            .get(id)
                            .and_then(|m| m.authors.first())
                            .map(|a| a.name.clone())
                            .unwrap_or_default();
                        cache.remember_author(
                            IdentityCache::author_key(ContentSource::CurseForge, &id.to_string()),
                            author,
                        );
                    }
                }
                Err(error) => tracing::warn!(%error, "从 CurseForge 补作者失败，下次刷新再试"),
            }
        }
    }

    /// 按 CurseForge 指纹识别一批文件，结果写进识别缓存。失败（断网、没权限）只记日志。
    async fn identify_on_curseforge(
        &self,
        curseforge: &curseforge::CurseForge,
        kind: ContentKind,
        files: &[&(Scanned, String)],
        cache: &mut IdentityCache,
    ) {
        if files.is_empty() {
            return;
        }
        let mut fingerprints = Vec::with_capacity(files.len());
        for (scanned, sha1) in files {
            let path = self.folder(kind).join(&scanned.disk_name);
            match fingerprint::fingerprint_file_async(&path).await {
                Ok(fp) => fingerprints.push((fp, sha1.clone())),
                Err(error) => tracing::warn!(%error, file = %path.display(), "算指纹失败"),
            }
        }
        let wanted: Vec<u32> = fingerprints.iter().map(|(fp, _)| *fp).collect();
        let found = match curseforge.identify(&wanted).await {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "按指纹问 CurseForge 失败，下次再问");
                return;
            }
        };
        let mod_ids: Vec<u32> = found.values().map(|f| f.mod_id).collect();
        let mods = match curseforge.mods(&mod_ids).await {
            Ok(mods) => mods,
            Err(error) => {
                tracing::warn!(%error, "查 CurseForge 项目名失败，下次再问");
                return;
            }
        };
        for (fp, sha1) in fingerprints {
            match found.get(&fp) {
                Some(file) => {
                    let project = mods.get(&file.mod_id);
                    cache.remember_curseforge_project(&sha1, file.mod_id);
                    cache.remember(
                        sha1,
                        Identity {
                            title: project
                                .map_or_else(|| file.display_name.clone(), |m| m.name.clone()),
                            version: Some(file.display_name.clone()),
                            icon_url: project.and_then(curseforge::Mod::icon_url),
                            icon_file: None,
                            // 项目编号只记 Modrinth 的（检查更新、判断前置都按 Modrinth 算）
                            project_id: None,
                            version_id: None,
                            source: IdentitySource::CurseForge,
                        },
                    );
                }
                None => cache.mark_not_on_curseforge(&sha1),
            }
        }
    }

    /// 启用或停用。模组：改名；光影：设为当前 / 关闭光影；材质包：加入 / 移出 `options.txt` 的列表。
    pub fn set_enabled(
        &self,
        kind: ContentKind,
        file_name: &str,
        enabled: bool,
    ) -> Result<(), ContentError> {
        safe_file_name(file_name)?;
        match kind {
            ContentKind::Mod => {
                let folder = self.folder(kind);
                let on = folder.join(file_name);
                let off = folder.join(format!("{file_name}{DISABLED_SUFFIX}"));
                let (from, to) = if enabled { (&off, &on) } else { (&on, &off) };
                if to.exists() {
                    return Ok(());
                }
                if !from.exists() {
                    return Err(ContentError::NotFound(file_name.to_owned()));
                }
                std::fs::rename(from, to).map_err(|source| io_error(from, source))
            }
            ContentKind::Shader => set_shader(&self.dir, enabled.then_some(file_name)),
            ContentKind::ResourcePack => set_resource_pack(&self.dir, file_name, enabled),
        }
    }

    /// 实例里已经装了哪些 Modrinth 项目（记录 + 识别缓存 + 按哈希联网识别）。
    pub async fn installed_projects(
        &self,
        client: &Client,
    ) -> Result<HashSet<String>, ContentError> {
        let mut installed: HashSet<String> = self
            .load_records()
            .values()
            .filter(|r| r.source == ContentSource::Modrinth)
            .map(|r| r.project_id.clone())
            .collect();
        let mut cache = IdentityCache::load(self.store.root());
        let mut hashes = Vec::new();
        for kind in ContentKind::ALL {
            for (_, sha1) in self.hash_files(kind, &mut cache).await? {
                match cache.identity(&sha1).and_then(|i| i.project_id.clone()) {
                    Some(project) => {
                        installed.insert(project);
                    }
                    None => hashes.push(sha1),
                }
            }
        }
        if let Err(error) = cache.save(self.store.root()) {
            tracing::warn!(%error, "保存识别缓存失败");
        }
        match modrinth::identify(client, &hashes).await {
            Ok(found) => installed.extend(found.into_values()),
            // 识别失败不挡安装，只是可能装重复的前置；记日志
            Err(error) => tracing::warn!(%error, "按哈希识别已装内容失败"),
        }
        Ok(installed)
    }

    /// 安装前把要装的文件都查清楚：下载地址、哈希、放哪个文件夹。
    /// CurseForge 上作者禁止第三方下载的文件，先找 Modrinth 上一模一样的文件（同一个 SHA-1），
    /// 找不到的列进 `manual`：调用方要开始补件，让用户在浏览器里下载。
    pub async fn prepare_install(
        &self,
        client: &Client,
        curseforge: &curseforge::CurseForge,
        plan: &[modrinth::PlanItem],
    ) -> Result<PreparedInstall, ContentError> {
        let mut entries = Vec::new();
        let modrinth_ids: Vec<String> = plan
            .iter()
            .filter(|p| p.source == ContentSource::Modrinth)
            .map(|p| p.version_id.clone())
            .collect();
        for version in modrinth::fetch_versions(client, &modrinth_ids).await? {
            let file = version.primary_file().ok_or_else(|| {
                ContentError::Api(format!("版本 {} 没有文件", version.version_number))
            })?;
            let planned = plan
                .iter()
                .find(|p| p.source == ContentSource::Modrinth && p.version_id == version.id);
            let file_name = safe_file_name(&file.filename)?.to_owned();
            let sha1 =
                file.hashes.get("sha1").cloned().ok_or_else(|| {
                    ContentError::Api(format!("{file_name} 没有 SHA-1，无法校验"))
                })?;
            entries.push(Prepared {
                kind: planned.map_or(ContentKind::Mod, |p| p.kind),
                record: Record {
                    project_id: version.project_id.clone(),
                    version_id: version.id.clone(),
                    title: planned.map_or_else(|| file_name.clone(), |p| p.title.clone()),
                    version_number: version.version_number.clone(),
                    icon_url: planned.and_then(|p| p.icon_url.clone()),
                    source: ContentSource::Modrinth,
                },
                file_name,
                sha1,
                size: file.size,
                urls: Some(vec![file.url.clone()]),
            });
        }

        let mut manual = Vec::new();
        let cf_items: Vec<&modrinth::PlanItem> = plan
            .iter()
            .filter(|p| p.source == ContentSource::CurseForge)
            .collect();
        if !cf_items.is_empty() {
            let ids = cf_items
                .iter()
                .map(|p| {
                    p.version_id.parse::<u32>().map_err(|_| {
                        ContentError::CurseForge(format!("文件编号不对：{}", p.version_id))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let files = curseforge.files(&ids).await?;
            let mut blocked = Vec::new();
            for item in cf_items {
                let file = files
                    .iter()
                    .find(|f| {
                        f.id.to_string() == item.version_id
                            && f.mod_id.to_string() == item.project_id
                    })
                    .ok_or_else(|| {
                        ContentError::CurseForge(format!("找不到 {} 的这个版本", item.title))
                    })?;
                let file_name = safe_file_name(&file.file_name)?.to_owned();
                let sha1 = file.sha1().ok_or_else(|| {
                    ContentError::CurseForge(format!("{file_name} 没有 SHA-1，无法校验"))
                })?;
                let entry = Prepared {
                    kind: item.kind,
                    file_name,
                    sha1,
                    size: file.file_length,
                    urls: file.usable_download_url().map(|url| vec![url.to_owned()]),
                    record: Record {
                        project_id: item.project_id.clone(),
                        version_id: item.version_id.clone(),
                        title: item.title.clone(),
                        version_number: item.version_number.clone(),
                        icon_url: item.icon_url.clone(),
                        source: ContentSource::CurseForge,
                    },
                };
                if entry.urls.is_some() {
                    entries.push(entry);
                } else {
                    blocked.push((entry, item, file.id));
                }
            }
            if !blocked.is_empty() {
                let hashes: Vec<String> = blocked.iter().map(|(e, ..)| e.sha1.clone()).collect();
                let found = modrinth::files_by_sha1(client, &hashes)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::warn!(%error, "问 Modrinth 有没有同样的文件失败，全部走补件");
                        Default::default()
                    });
                for (mut entry, item, file_id) in blocked {
                    match found.get(&entry.sha1) {
                        Some(on_modrinth) => entry.urls = Some(vec![on_modrinth.url.clone()]),
                        None => manual.push(ManualFile {
                            project_id: item.project_id.parse().unwrap_or_default(),
                            file_id,
                            project_name: item.title.clone(),
                            display_name: item.version_number.clone(),
                            file_name: entry.file_name.clone(),
                            size: entry.size as f64,
                            required: true,
                            kind: entry.kind,
                            page_url: item.page_url.clone(),
                            icon_url: item.icon_url.clone(),
                            sha1: entry.sha1.clone(),
                            path: format!("{}/{}", entry.kind.folder(), entry.file_name),
                        }),
                    }
                    entries.push(entry);
                }
            }
        }
        Ok(PreparedInstall { entries, manual })
    }

    /// 安装准备好的文件：[`Self::download_install`] + [`Self::finish_install`]。
    pub async fn install<F>(
        &self,
        downloader: &Downloader,
        prepared: PreparedInstall,
        claim: Option<&ClaimSession>,
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<(), ContentError>
    where
        F: Fn(DownloadProgress) + Send + Sync,
    {
        let staged = self
            .download_install(downloader, prepared, claim, cancel, on_progress)
            .await?;
        self.finish_install(staged).await
    }

    /// 安装的第一步：把文件下到这次安装自己的临时文件夹（不碰实例，几个安装可以同时下）。
    /// 有要用户补的文件（`prepared.manual` 不为空）时必须给 `claim`：先把能下的下完，再等用户补齐。
    pub async fn download_install<F>(
        &self,
        downloader: &Downloader,
        prepared: PreparedInstall,
        claim: Option<&ClaimSession>,
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<StagedInstall, ContentError>
    where
        F: Fn(DownloadProgress) + Send + Sync,
    {
        let staged = StagedInstall {
            prepared,
            dir: self
                .store
                .root()
                .join(STAGING_DIR)
                .join(uuid::Uuid::new_v4().simple().to_string()),
        };
        let mut items = Vec::new();
        for entry in &staged.prepared.entries {
            if let Some(urls) = &entry.urls {
                items.push(DownloadItem {
                    urls: urls.clone(),
                    path: staged.path_of(entry)?,
                    sha1: Some(entry.sha1.clone()),
                    size: (entry.size > 0).then_some(entry.size),
                });
            }
        }
        downloader.download_all(&items, cancel, on_progress).await?;
        if staged.prepared.entries.iter().any(|e| e.urls.is_none()) {
            let session = claim.ok_or_else(|| {
                ContentError::Message("有要手动下载的文件，但没有开始补件".to_owned())
            })?;
            // 补齐的文件进了内容仓库，放进实例时从仓库拿
            session.wait(cancel).await?;
        }
        Ok(staged)
    }

    /// 安装的第二步：把下好的文件放进实例。装完自动启用：光影设为当前，材质包加到列表最上面。
    /// 记一条可撤销的历史。同一个实例同一时间只能有一个在做（调用方排队）。
    /// 实例里已经有一模一样的文件（另一个安装刚装了同一个前置）就不动它，也不记进这次的历史，
    /// 免得撤销这次时把别人要的前置删掉。同名但内容不一样的旧文件先收进仓库，撤销时放回来。
    /// 整批要么都放进去，要么一个都不放（改动日志）。
    pub async fn finish_install(&self, mut staged: StagedInstall) -> Result<(), ContentError> {
        self.settle().await?;
        self.snapshot_before_change().await;
        let mut draft = self.draft();
        let mut changes = Vec::new();
        for entry in std::mem::take(&mut staged.prepared.entries) {
            let rel = rel_path(entry.kind, &entry.file_name, false)?;
            if draft.plan.is_placed(&rel) {
                continue;
            }
            let mut removed = None;
            if let Ok((path, disabled)) = self.existing(entry.kind, &entry.file_name) {
                if path.is_dir() {
                    return Err(ContentError::Message(format!(
                        "实例里已经有一个叫「{}」的文件夹",
                        entry.file_name
                    )));
                }
                let old = sha1_file(&path)
                    .await
                    .map_err(|source| io_error(&path, source))?;
                if old.eq_ignore_ascii_case(&entry.sha1) {
                    tracing::info!(file = %entry.file_name, "实例里已经有一模一样的文件，不重复装");
                    continue;
                }
                // 同名但内容不一样：旧的先收进仓库（以前直接盖掉，撤销不回来）
                removed = Some(draft.take(entry.kind, &entry.file_name, disabled, old)?);
            }
            match &entry.urls {
                Some(_) => {
                    draft
                        .plan
                        .place_staged(rel, entry.sha1.clone(), staged.path_of(&entry)?)
                }
                None => draft.plan.place_stored(rel, entry.sha1.clone()),
            }
            draft.records.insert(
                record_key(entry.kind, &entry.file_name),
                entry.record.clone(),
            );
            draft.active.set(entry.kind, &entry.file_name, true);
            changes.push(Change {
                kind: entry.kind,
                removed,
                added: Some(FileRef {
                    file_name: entry.file_name,
                    sha1: entry.sha1,
                    disabled: false,
                    enabled: true,
                    record: Some(entry.record),
                }),
            });
        }
        if changes.is_empty() {
            return Ok(());
        }
        let summary = summarize("安装", &changes);
        let history = self.pushed_history(HistoryAction::Install, summary.clone(), changes);
        self.commit(draft, &summary, Some(history)).await
    }

    /// 把本地文件（拖进窗口的）装进实例：复制进内容仓库（原文件不动），再放进实例的文件夹，
    /// 整批记成一条改动历史（能撤销），要么都装上、要么都不装。资源包、光影包装好就启用。
    /// - 实例里有一模一样的文件：不动，结果是 [`LocalOutcome::Same`]
    /// - 同名但内容不同、或者 `replace` 指定了要换掉的旧版本：旧文件收进仓库，撤销时放回来
    /// - 同一批里有两个同名的：只装第一个，后面的算 [`LocalOutcome::Same`]
    pub async fn add_local(&self, files: &[LocalAdd]) -> Result<Vec<LocalOutcome>, ContentError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        self.settle().await?;
        self.snapshot_before_change().await;
        let mut draft = self.draft();
        let mut changes = Vec::new();
        let mut outcomes = Vec::new();
        for file in files {
            let raw = file
                .source
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| ContentError::Message("文件名里有认不出的字符".to_owned()))?;
            // 拖进来的是停用的模组（xxx.jar.disabled）也按正常文件名装
            let raw = raw.strip_suffix(DISABLED_SUFFIX).unwrap_or(raw);
            let file_name = safe_file_name(raw)?.to_owned();
            let rel = rel_path(file.kind, &file_name, false)?;
            if draft.plan.is_placed(&rel) {
                tracing::info!(file = %file_name, "同一批里有两个同名的文件，只装第一个");
                outcomes.push(LocalOutcome::Same);
                continue;
            }
            let sha1 = sha1_file(&file.source)
                .await
                .map_err(|source| io_error(&file.source, source))?;

            let mut removed = Vec::new();
            if let Ok((path, disabled)) = self.existing(file.kind, &file_name)
                && !draft
                    .plan
                    .is_taken(&rel_path(file.kind, &file_name, disabled)?)
            {
                if path.is_dir() {
                    return Err(ContentError::Message(format!(
                        "实例里已经有一个叫「{file_name}」的文件夹"
                    )));
                }
                let old = sha1_file(&path)
                    .await
                    .map_err(|source| io_error(&path, source))?;
                if old.eq_ignore_ascii_case(&sha1) {
                    outcomes.push(LocalOutcome::Same);
                    continue;
                }
                removed.push(draft.take(file.kind, &file_name, disabled, old)?);
            }
            if let Some(old_name) = file.replace.as_deref().filter(|n| *n != file_name) {
                safe_file_name(old_name)?;
                if let Ok((path, disabled)) = self.existing(file.kind, old_name)
                    && path.is_file()
                    && !draft
                        .plan
                        .is_taken(&rel_path(file.kind, old_name, disabled)?)
                {
                    let old = sha1_file(&path)
                        .await
                        .map_err(|source| io_error(&path, source))?;
                    removed.push(draft.take(file.kind, old_name, disabled, old)?);
                }
            }

            // 先复制进仓库（改动日志里「放进来」就是从仓库拿）
            self.store
                .add_copy(&file.source, &sha1)
                .await
                .map_err(|source| io_error(&file.source, source))?;
            draft.plan.place_stored(rel, sha1.clone());
            draft.active.set(file.kind, &file_name, true);
            let added = FileRef {
                file_name,
                sha1,
                disabled: false,
                enabled: true,
                record: None,
            };
            let mut removed = removed.into_iter();
            changes.push(Change {
                kind: file.kind,
                removed: removed.next(),
                added: Some(added),
            });
            // 同时换掉了两个旧文件（同名的 + 指定的旧版本）：第二个单独记一条「删除」
            for extra in removed {
                changes.push(Change {
                    kind: file.kind,
                    removed: Some(extra),
                    added: None,
                });
            }
            outcomes.push(LocalOutcome::Added);
        }
        if changes.is_empty() {
            return Ok(outcomes);
        }
        let summary = summarize("安装", &changes);
        let history = self.pushed_history(HistoryAction::Install, summary.clone(), changes);
        self.commit(draft, &summary, Some(history)).await?;
        Ok(outcomes)
    }

    /// 实例里已经装了哪些 CurseForge 项目：本启动器从 CurseForge 装的，加上按指纹认出来的
    /// （从 Modrinth 装的同一个文件也认得出，解析 CurseForge 前置时就不会重复装）。结果进识别缓存。
    pub async fn installed_curseforge(
        &self,
        curseforge: &curseforge::CurseForge,
    ) -> Result<HashSet<u32>, ContentError> {
        let mut installed: HashSet<u32> = self
            .load_records()
            .values()
            .filter(|r| r.source == ContentSource::CurseForge)
            .filter_map(|r| r.project_id.parse().ok())
            .collect();
        if !curseforge.usable() {
            return Ok(installed);
        }
        let mut cache = IdentityCache::load(self.store.root());
        let mut ask = Vec::new();
        for kind in ContentKind::ALL {
            for (scanned, sha1) in self.hash_files(kind, &mut cache).await? {
                if let Some(id) = cache.curseforge_project(&sha1) {
                    installed.insert(id);
                } else if !cache.knows_curseforge(&sha1) {
                    ask.push((self.folder(kind).join(&scanned.disk_name), sha1));
                }
            }
        }
        if !ask.is_empty() {
            let mut fingerprints = Vec::with_capacity(ask.len());
            for (path, sha1) in ask {
                match fingerprint::fingerprint_file_async(&path).await {
                    Ok(fp) => fingerprints.push((fp, sha1)),
                    Err(error) => tracing::warn!(%error, file = %path.display(), "算指纹失败"),
                }
            }
            let wanted: Vec<u32> = fingerprints.iter().map(|(fp, _)| *fp).collect();
            match curseforge.identify(&wanted).await {
                Ok(found) => {
                    for (fp, sha1) in fingerprints {
                        match found.get(&fp) {
                            Some(file) => {
                                installed.insert(file.mod_id);
                                cache.remember_curseforge_project(&sha1, file.mod_id);
                            }
                            None => cache.mark_not_on_curseforge(&sha1),
                        }
                    }
                }
                // 识别失败不挡安装，只是可能装重复的前置；记日志
                Err(error) => tracing::warn!(%error, "按指纹识别已装内容失败"),
            }
        }
        if let Err(error) = cache.save(self.store.root()) {
            tracing::warn!(%error, "保存识别缓存失败");
        }
        Ok(installed)
    }

    /// 按实例的游戏版本和加载器，同时在 Modrinth 与 CurseForge 批量查更新，跳过已锁定项。
    pub async fn check_updates(
        &self,
        client: &Client,
        curseforge: &curseforge::CurseForge,
        kind: ContentKind,
        game_version: &str,
        mod_loader: Option<&str>,
    ) -> Result<UpdateCheckResult, ContentError> {
        let loaders = modrinth::loaders_for(kind, mod_loader);
        let cf_loader = mod_loader.and_then(curseforge::loader_id);
        let locks = self.load_locks();
        let records = self.load_records();
        let mut cache = IdentityCache::load(self.store.root());
        let scanned = self.scan(kind)?;
        let mut result = UpdateCheckResult::default();
        let mut hashed = Vec::new();
        for file in scanned {
            if file.metadata.is_file() && locks.contains(&record_key(kind, &file.file_name)) {
                // 已锁定版本：用户明确跳过，不计入未检查也不报更新
                continue;
            }
            result.unchecked += 1;
            if file.metadata.is_file() {
                match self.hash_scanned_file(kind, &file, &mut cache).await {
                    Ok(sha1) => hashed.push((file, sha1)),
                    Err(error) => tracing::warn!(%error, "读取文件失败，跳过更新检查"),
                }
            }
        }
        if hashed.is_empty() {
            return Ok(result);
        }
        if let Err(error) = cache.save(self.store.root()) {
            tracing::warn!(%error, "保存更新检查的哈希缓存失败");
        }

        let items = self.list(kind)?;
        // 明确来自 CurseForge 的项优先走 CurseForge，其余走 Modrinth（Modrinth 未命中的再回退 CurseForge 指纹识别）
        let mut modrinth_targets: Vec<usize> = Vec::new();
        let mut cf_known: Vec<(usize, u32)> = Vec::new();
        let mut cf_unknown: Vec<usize> = Vec::new();
        for (idx, (scanned, sha1)) in hashed.iter().enumerate() {
            let record = records.get(&record_key(kind, &scanned.file_name));
            let identity = cache.identity(sha1);
            let is_cf_origin = record.is_some_and(|r| r.source == ContentSource::CurseForge)
                || (record.is_none()
                    && identity.is_some_and(|i| i.source == IdentitySource::CurseForge));
            if is_cf_origin && curseforge.usable() {
                if let Some(mod_id) = record
                    .filter(|r| r.source == ContentSource::CurseForge)
                    .and_then(|r| r.project_id.parse::<u32>().ok())
                    .or_else(|| cache.curseforge_project(sha1))
                {
                    cf_known.push((idx, mod_id));
                } else {
                    cf_unknown.push(idx);
                }
            } else {
                modrinth_targets.push(idx);
            }
        }

        let mut modrinth_failed = false;
        let mut cf_failed = false;

        // 1. Modrinth 批量查询
        if !modrinth_targets.is_empty() {
            let hashes: Vec<String> = modrinth_targets
                .iter()
                .map(|&idx| hashed[idx].1.clone())
                .collect();
            match modrinth::latest_versions(client, &hashes, &loaders, game_version).await {
                Ok(latest) => {
                    for &idx in &modrinth_targets {
                        let (scanned, sha1) = &hashed[idx];
                        let Some(version) = latest.get(sha1) else {
                            if curseforge.usable() {
                                if let Some(mod_id) = cache.curseforge_project(sha1) {
                                    cf_known.push((idx, mod_id));
                                } else {
                                    cf_unknown.push(idx);
                                }
                            }
                            continue;
                        };
                        let current = version
                            .files
                            .iter()
                            .filter_map(|file| file.hashes.get("sha1"))
                            .any(|hash| hash.eq_ignore_ascii_case(sha1));
                        let has_primary_hash = version
                            .primary_file()
                            .and_then(|file| file.hashes.get("sha1"))
                            .is_some_and(|hash| {
                                hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                            });
                        if !current && !has_primary_hash {
                            tracing::warn!(version_id = %version.id, "更新结果缺少有效的文件哈希");
                            continue;
                        }
                        result.checked += 1;
                        result.unchecked = result.unchecked.saturating_sub(1);
                        if current {
                            continue;
                        }
                        let item = items.iter().find(|i| i.file_name == scanned.file_name);
                        result.updates.push(UpdateInfo {
                            kind,
                            file_name: scanned.file_name.clone(),
                            title: item.map_or_else(
                                || display_name(&scanned.file_name),
                                |i| i.title.clone(),
                            ),
                            current_version: item.and_then(|i| i.version_number.clone()),
                            version_id: version.id.clone(),
                            version_number: version.version_number.clone(),
                            source: ContentSource::Modrinth,
                            project_id: Some(version.project_id.clone()),
                            delivery: Delivery::Direct,
                            page_url: None,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "查询 Modrinth 更新失败");
                    modrinth_failed = true;
                }
            }
        }

        // 2. CurseForge 批量指纹识别 + 最新兼容文件查询
        if curseforge.usable() && (!cf_known.is_empty() || !cf_unknown.is_empty()) {
            let mut cf_pairs: Vec<(usize, u32)> = cf_known;
            if !cf_unknown.is_empty() {
                let mut fps: Vec<(u32, usize)> = Vec::new();
                for &idx in &cf_unknown {
                    let path = self.folder(kind).join(&hashed[idx].0.disk_name);
                    if let Ok(fp) = fingerprint::fingerprint_file_async(&path).await {
                        fps.push((fp, idx));
                    }
                }
                if !fps.is_empty() {
                    let wanted: Vec<u32> = fps.iter().map(|(fp, _)| *fp).collect();
                    match curseforge.identify(&wanted).await {
                        Ok(found) => {
                            let mut cache_dirty = false;
                            for (fp, idx) in fps {
                                if let Some(file) = found.get(&fp) {
                                    cf_pairs.push((idx, file.mod_id));
                                    cache.remember_curseforge_project(&hashed[idx].1, file.mod_id);
                                    cache_dirty = true;
                                }
                            }
                            if cache_dirty && let Err(error) = cache.save(self.store.root()) {
                                tracing::warn!(%error, "保存 CurseForge 项目缓存失败");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "按指纹查询 CurseForge 失败");
                            cf_failed = true;
                        }
                    }
                }
            }
            if !cf_pairs.is_empty() {
                let mut mod_ids: Vec<u32> = cf_pairs.iter().map(|(_, id)| *id).collect();
                mod_ids.sort_unstable();
                mod_ids.dedup();
                match curseforge
                    .latest_compatible_files(&mod_ids, game_version, cf_loader, kind)
                    .await
                {
                    Ok(latest_cf) => {
                        let cf_candidates: Vec<(&Scanned, &str, u32)> = cf_pairs
                            .iter()
                            .map(|&(idx, mod_id)| (&hashed[idx].0, hashed[idx].1.as_str(), mod_id))
                            .collect();
                        // 作者禁止第三方下载的 CurseForge 更新：先问 Modrinth 是否有同 SHA-1 文件
                        let blocked_hashes: Vec<String> = cf_candidates
                            .iter()
                            .filter_map(|(_, cur_sha1, mod_id)| {
                                let (file, _) = latest_cf.get(mod_id)?;
                                let new_sha1 = file.sha1()?;
                                if new_sha1.eq_ignore_ascii_case(cur_sha1) {
                                    return None;
                                }
                                file.usable_download_url().is_none().then_some(new_sha1)
                            })
                            .collect();
                        let on_modrinth = if blocked_hashes.is_empty() {
                            HashMap::new()
                        } else {
                            modrinth::files_by_sha1(client, &blocked_hashes)
                                .await
                                .unwrap_or_default()
                        };
                        let on_modrinth_set: HashSet<String> = on_modrinth.into_keys().collect();
                        apply_curseforge_updates(
                            kind,
                            &cf_candidates,
                            &records,
                            &items,
                            &latest_cf,
                            &on_modrinth_set,
                            &mut result,
                        );
                    }
                    Err(error) => {
                        tracing::warn!(%error, "查询 CurseForge 更新失败");
                        cf_failed = true;
                    }
                }
            }
        }

        // 两个平台都失败、或唯一需要查询的平台失败时标为查询失败
        let only_modrinth =
            cf_known_or_unknown_empty(modrinth_targets.len(), result.checked, cf_failed);
        if (modrinth_failed && cf_failed)
            || (modrinth_failed && only_modrinth)
            || (cf_failed && modrinth_targets.is_empty())
        {
            result.lookup_failed = true;
        }
        Ok(result)
    }

    #[cfg(test)]
    async fn check_updates_with<Fut>(
        &self,
        kind: ContentKind,
        lookup: impl FnOnce(Vec<String>) -> Fut + Send,
    ) -> Result<UpdateCheckResult, ContentError>
    where
        Fut: Future<Output = Result<BTreeMap<String, modrinth::Version>, ContentError>> + Send,
    {
        let locks = self.load_locks();
        let mut cache = IdentityCache::load(self.store.root());
        let scanned = self.scan(kind)?;
        let mut result = UpdateCheckResult::default();
        let mut hashed = Vec::new();
        for file in scanned {
            if file.metadata.is_file() && locks.contains(&record_key(kind, &file.file_name)) {
                continue;
            }
            result.unchecked += 1;
            if file.metadata.is_file() {
                match self.hash_scanned_file(kind, &file, &mut cache).await {
                    Ok(sha1) => hashed.push((file, sha1)),
                    Err(error) => tracing::warn!(%error, "读取文件失败，跳过更新检查"),
                }
            }
        }
        if hashed.is_empty() {
            return Ok(result);
        }
        if let Err(error) = cache.save(self.store.root()) {
            tracing::warn!(%error, "保存更新检查的哈希缓存失败");
        }
        let hashes: Vec<String> = hashed.iter().map(|(_, sha1)| sha1.clone()).collect();
        let latest = match lookup(hashes).await {
            Ok(latest) => latest,
            Err(error) => {
                tracing::warn!(%error, "查询 Modrinth 更新失败");
                result.lookup_failed = true;
                return Ok(result);
            }
        };
        let items = self.list(kind)?;
        for (scanned, sha1) in &hashed {
            let Some(version) = latest.get(sha1) else {
                continue;
            };
            // 同一版本可以有多个文件，已装的是其中一个时也已经是最新版本。
            let current = version
                .files
                .iter()
                .filter_map(|file| file.hashes.get("sha1"))
                .any(|hash| hash.eq_ignore_ascii_case(sha1));
            let has_primary_hash = version
                .primary_file()
                .and_then(|file| file.hashes.get("sha1"))
                .is_some_and(|hash| {
                    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                });
            if !current && !has_primary_hash {
                tracing::warn!(version_id = %version.id, "更新结果缺少有效的文件哈希");
                continue;
            }
            result.checked += 1;
            result.unchecked = result.unchecked.saturating_sub(1);
            if current {
                continue;
            }
            let item = items.iter().find(|i| i.file_name == scanned.file_name);
            result.updates.push(UpdateInfo {
                kind,
                file_name: scanned.file_name.clone(),
                title: item.map_or_else(|| display_name(&scanned.file_name), |i| i.title.clone()),
                current_version: item.and_then(|i| i.version_number.clone()),
                version_id: version.id.clone(),
                version_number: version.version_number.clone(),
                source: ContentSource::Modrinth,
                project_id: Some(version.project_id.clone()),
                delivery: Delivery::Direct,
                page_url: None,
            });
        }
        Ok(result)
    }

    /// 准备更新（支持 Modrinth 与 CurseForge；CurseForge 受限文件自动尝试 Modrinth 同哈希替补或进入补件）。
    pub async fn prepare_update(
        &self,
        client: &Client,
        curseforge: &curseforge::CurseForge,
        updates: &[UpdateInfo],
    ) -> Result<PreparedUpdate, ContentError> {
        let mut entries = Vec::with_capacity(updates.len());
        let mut manual = Vec::new();
        let modrinth_ids: Vec<String> = updates
            .iter()
            .filter(|u| u.source == ContentSource::Modrinth)
            .map(|u| u.version_id.clone())
            .collect();
        let modrinth_versions = if modrinth_ids.is_empty() {
            Vec::new()
        } else {
            modrinth::fetch_versions(client, &modrinth_ids).await?
        };
        let cf_ids: Vec<u32> = updates
            .iter()
            .filter(|u| u.source == ContentSource::CurseForge)
            .map(|u| {
                u.version_id.parse::<u32>().map_err(|_| {
                    ContentError::CurseForge(format!("文件编号不对：{}", u.version_id))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cf_files = if cf_ids.is_empty() {
            Vec::new()
        } else {
            curseforge.files(&cf_ids).await?
        };

        let mut blocked_cf = Vec::new();
        for update in updates {
            match update.source {
                ContentSource::Modrinth => {
                    let version = modrinth_versions
                        .iter()
                        .find(|v| v.id == update.version_id)
                        .ok_or_else(|| {
                            ContentError::Api(format!("找不到版本 {}", update.version_number))
                        })?;
                    let file = version.primary_file().ok_or_else(|| {
                        ContentError::Api(format!("版本 {} 没有文件", version.version_number))
                    })?;
                    let new_name = safe_file_name(&file.filename)?.to_owned();
                    let sha1 = file.hashes.get("sha1").cloned().ok_or_else(|| {
                        ContentError::Api(format!("{new_name} 没有 SHA-1，无法校验"))
                    })?;
                    entries.push(PreparedUpdateEntry {
                        update: UpdateInfo {
                            version_number: version.version_number.clone(),
                            ..update.clone()
                        },
                        project_id: version.project_id.clone(),
                        new_name,
                        sha1,
                        size: file.size,
                        urls: Some(vec![file.url.clone()]),
                        source: ContentSource::Modrinth,
                    });
                }
                ContentSource::CurseForge => {
                    let fid = update.version_id.parse::<u32>().map_err(|_| {
                        ContentError::CurseForge(format!("文件编号不对：{}", update.version_id))
                    })?;
                    let file = cf_files.iter().find(|f| f.id == fid).ok_or_else(|| {
                        ContentError::CurseForge(format!("找不到版本 {}", update.version_number))
                    })?;
                    let new_name = safe_file_name(&file.file_name)?.to_owned();
                    let sha1 = file.sha1().ok_or_else(|| {
                        ContentError::CurseForge(format!("{new_name} 没有 SHA-1，无法校验"))
                    })?;
                    let entry = PreparedUpdateEntry {
                        update: UpdateInfo {
                            version_number: if file.display_name.trim().is_empty() {
                                update.version_number.clone()
                            } else {
                                file.display_name.clone()
                            },
                            ..update.clone()
                        },
                        project_id: file.mod_id.to_string(),
                        new_name,
                        sha1,
                        size: file.file_length,
                        urls: file.usable_download_url().map(|u| vec![u.to_owned()]),
                        source: ContentSource::CurseForge,
                    };
                    if entry.urls.is_some() {
                        entries.push(entry);
                    } else {
                        blocked_cf.push((entry, file.mod_id, file.id));
                    }
                }
            }
        }
        if !blocked_cf.is_empty() {
            let hashes: Vec<String> = blocked_cf.iter().map(|(e, ..)| e.sha1.clone()).collect();
            let found = modrinth::files_by_sha1(client, &hashes)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "问 Modrinth 有没有同样的更新文件失败，走补件");
                    Default::default()
                });
            let missing_mod_ids: Vec<u32> = blocked_cf
                .iter()
                .filter(|(e, ..)| !found.contains_key(&e.sha1) && e.update.page_url.is_none())
                .map(|(_, mod_id, _)| *mod_id)
                .collect();
            let mods = if missing_mod_ids.is_empty() {
                HashMap::new()
            } else {
                curseforge.mods(&missing_mod_ids).await.unwrap_or_default()
            };
            for (mut entry, mod_id, file_id) in blocked_cf {
                match found.get(&entry.sha1) {
                    Some(on_modrinth) => entry.urls = Some(vec![on_modrinth.url.clone()]),
                    None => {
                        let page_url = entry.update.page_url.clone().or_else(|| {
                            curseforge::download_page(
                                mods.get(&mod_id)
                                    .and_then(|m| m.links.website_url.as_deref()),
                                file_id,
                            )
                        });
                        manual.push(ManualFile {
                            project_id: mod_id,
                            file_id,
                            project_name: entry.update.title.clone(),
                            display_name: entry.update.version_number.clone(),
                            file_name: entry.new_name.clone(),
                            size: entry.size as f64,
                            required: true,
                            kind: entry.update.kind,
                            page_url,
                            icon_url: None,
                            sha1: entry.sha1.clone(),
                            path: format!("{}/{}", entry.update.kind.folder(), entry.new_name),
                        });
                    }
                }
                entries.push(entry);
            }
        }
        Ok(PreparedUpdate { entries, manual })
    }

    /// 更新第一步：把新文件全部下载到临时目录并校验；若有受限补件文件则等待补件完成。
    pub async fn download_update<F>(
        &self,
        downloader: &Downloader,
        prepared: PreparedUpdate,
        claim: Option<&ClaimSession>,
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<StagedUpdate, ContentError>
    where
        F: Fn(DownloadProgress) + Send + Sync,
    {
        let staged = StagedUpdate {
            prepared,
            dir: new_staging_dir(self.store.root()),
        };
        let mut items = Vec::new();
        for (index, entry) in staged.prepared.entries.iter().enumerate() {
            if let Some(urls) = &entry.urls {
                items.push(DownloadItem {
                    urls: urls.clone(),
                    path: staged.path_of(index, &entry.new_name)?,
                    sha1: Some(entry.sha1.clone()),
                    size: (entry.size > 0).then_some(entry.size),
                });
            }
        }
        downloader.download_all(&items, cancel, on_progress).await?;
        if staged.prepared.entries.iter().any(|e| e.urls.is_none()) {
            let session = claim.ok_or_else(|| {
                ContentError::Message("有要手动下载的更新文件，但没有开始补件".to_owned())
            })?;
            session.wait(cancel).await?;
        }
        Ok(staged)
    }

    /// 更新第二步：整批事务替换（要么全换、要么全退），旧文件进仓库，记入可撤销的改动历史；
    /// 若旧文件锁定了版本（如手动切换版本），将锁迁移到新文件名。
    pub async fn finish_update(
        &self,
        mut staged: StagedUpdate,
        verb: &str,
    ) -> Result<(), ContentError> {
        self.settle().await?;
        self.snapshot_before_change().await;
        let mut draft = self.draft();
        let cache = IdentityCache::load(self.store.root());
        let mut locks = self.load_locks();
        let mut locks_changed = false;
        let mut changes = Vec::new();
        let entries = std::mem::take(&mut staged.prepared.entries);
        for (index, entry) in entries.into_iter().enumerate() {
            let update = &entry.update;
            let kind = update.kind;
            let (old_path, disabled) = self.existing(kind, &update.file_name)?;
            let old_sha1 = sha1_file(&old_path)
                .await
                .map_err(|source| io_error(&old_path, source))?;
            let was_enabled = draft.active.is_enabled(kind, &update.file_name, disabled);
            // 正在用的光影 / 材质包：换成新文件名，位置不变
            draft
                .active
                .replace(kind, &update.file_name, &entry.new_name);
            draft.plan.take(
                rel_path(kind, &update.file_name, disabled)?,
                old_sha1.clone(),
            );
            let old_key = record_key(kind, &update.file_name);
            let new_key = record_key(kind, &entry.new_name);
            if old_key != new_key && locks.remove(&old_key) {
                locks.insert(new_key.clone());
                locks_changed = true;
            }
            let old_record = draft.records.remove(&old_key);
            let removed = FileRef {
                file_name: update.file_name.clone(),
                sha1: old_sha1.clone(),
                disabled,
                enabled: was_enabled,
                record: old_record.clone(),
            };

            let new_rel = rel_path(kind, &entry.new_name, disabled)?;
            if self.occupied(&draft.plan, &new_rel) {
                let same = draft.plan.is_placed(&new_rel)
                    || sha1_file(&self.resolve(&new_rel)?)
                        .await
                        .is_ok_and(|h| h.eq_ignore_ascii_case(&entry.sha1));
                if !same {
                    return Err(ContentError::Message(format!(
                        "实例里已经有一个叫「{}」的文件，先删掉它再更新",
                        entry.new_name
                    )));
                }
                changes.push(Change {
                    kind,
                    removed: Some(removed),
                    added: None,
                });
                continue;
            }
            match &entry.urls {
                Some(_) => draft.plan.place_staged(
                    new_rel,
                    entry.sha1.clone(),
                    staged.path_of(index, &entry.new_name)?,
                ),
                None => draft.plan.place_stored(new_rel, entry.sha1.clone()),
            }
            let record = Record {
                project_id: entry.project_id.clone(),
                version_id: update.version_id.clone(),
                title: old_record
                    .as_ref()
                    .map_or_else(|| update.title.clone(), |r| r.title.clone()),
                version_number: update.version_number.clone(),
                icon_url: carried_icon(old_record.as_ref(), &cache, &old_sha1),
                source: entry.source,
            };
            draft.records.insert(new_key, record.clone());
            changes.push(Change {
                kind,
                removed: Some(removed),
                added: Some(FileRef {
                    file_name: entry.new_name,
                    sha1: entry.sha1,
                    disabled,
                    enabled: was_enabled,
                    record: Some(record),
                }),
            });
        }
        let summary = summarize(verb, &changes);
        let history = self.pushed_history(HistoryAction::Update, summary.clone(), changes);
        self.commit(draft, &summary, Some(history)).await?;
        if locks_changed && let Err(error) = self.save_locks(&locks) {
            tracing::warn!(%error, "迁移版本锁定记录失败");
        }
        drop(staged);
        Ok(())
    }

    /// 更新（便捷入口：准备 + 下载 + 替换；若包含 CurseForge 受限补件文件请调用三段式接口）。
    pub async fn update<F>(
        &self,
        curseforge: &curseforge::CurseForge,
        downloader: &Downloader,
        updates: &[UpdateInfo],
        claim: Option<&ClaimSession>,
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<(), ContentError>
    where
        F: Fn(DownloadProgress) + Send + Sync,
    {
        let prepared = self
            .prepare_update(&curseforge.http(), curseforge, updates)
            .await?;
        let staged = self
            .download_update(downloader, prepared, claim, cancel, on_progress)
            .await?;
        self.finish_update(staged, "更新").await
    }

    /// 文件在磁盘上的位置（「在文件夹中显示」用）。
    pub fn locate(&self, kind: ContentKind, file_name: &str) -> Result<PathBuf, ContentError> {
        safe_file_name(file_name)?;
        Ok(self.existing(kind, file_name)?.0)
    }

    /// 一次删好几个（文件进仓库，可以撤销）：只记一条改动历史，一次撤销全部回来。
    /// 文件夹形式的光影、材质包跳过。要么全删、要么一个不删（中途出错就退回）。
    pub async fn remove_many(
        &self,
        kind: ContentKind,
        file_names: &[String],
    ) -> Result<usize, ContentError> {
        for name in file_names {
            safe_file_name(name)?;
        }
        if file_names.is_empty() {
            return Ok(0);
        }
        self.settle().await?;
        self.snapshot_before_change().await;
        let titles: BTreeMap<String, String> = self
            .list(kind)?
            .into_iter()
            .map(|i| (i.file_name, i.title))
            .collect();
        let mut draft = self.draft();
        let mut changes = Vec::new();
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for file_name in file_names {
            if !seen.insert(file_name.to_lowercase()) {
                continue;
            }
            let (path, disabled) = self.existing(kind, file_name)?;
            if path.is_dir() {
                continue;
            }
            let sha1 = sha1_file(&path)
                .await
                .map_err(|source| io_error(&path, source))?;
            let removed = draft.take(kind, file_name, disabled, sha1)?;
            changes.push(Change {
                kind,
                removed: Some(removed),
                added: None,
            });
            names.push(
                titles
                    .get(file_name)
                    .cloned()
                    .unwrap_or_else(|| display_name(file_name)),
            );
        }
        if changes.is_empty() {
            return Ok(0);
        }
        let count = changes.len();
        let summary = match names.as_slice() {
            [one] => format!("删除 {one}"),
            many => {
                let shown: Vec<&str> = many.iter().take(3).map(String::as_str).collect();
                let more = if many.len() > 3 { "……" } else { "" };
                format!("删除 {} 项：{}{more}", many.len(), shown.join("、"))
            }
        };
        let history = self.pushed_history(HistoryAction::Remove, summary.clone(), changes);
        self.commit(draft, &summary, Some(history)).await?;
        let mut locks = self.load_locks();
        let mut locks_changed = false;
        for file_name in file_names {
            if locks.remove(&record_key(kind, file_name)) {
                locks_changed = true;
            }
        }
        if locks_changed && let Err(error) = self.save_locks(&locks) {
            tracing::warn!(%error, "清理已删除文件的版本锁定失败");
        }
        Ok(count)
    }

    /// 删除：文件进仓库（可以撤销），不是真删。
    pub async fn remove(&self, kind: ContentKind, file_name: &str) -> Result<(), ContentError> {
        safe_file_name(file_name)?;
        let (path, _) = self.existing(kind, file_name)?;
        if path.is_dir() {
            return Err(ContentError::Message(
                "文件夹形式的光影或材质包请在文件夹里手动删除".to_owned(),
            ));
        }
        self.remove_many(kind, &[file_name.to_owned()])
            .await
            .map(|_| ())
    }

    /// 一次启用 / 停用好几个：模组改名，光影、材质包改设置。要么全改、要么一个不改。
    /// 返回真的改了的那些（已经是这个样子的不算）。不记改动历史（和单个开关一样），
    /// 界面上的「撤销」就是反过来再调一次。
    pub async fn set_many_enabled(
        &self,
        kind: ContentKind,
        file_names: &[String],
        enabled: bool,
    ) -> Result<Vec<String>, ContentError> {
        for name in file_names {
            safe_file_name(name)?;
        }
        if file_names.is_empty() {
            return Ok(Vec::new());
        }
        self.settle().await?;
        self.snapshot_before_change().await;
        let mut draft = self.draft();
        let mut changed = Vec::new();
        let mut seen = HashSet::new();
        for name in file_names {
            if !seen.insert(name.to_lowercase()) {
                continue;
            }
            let (_, disabled) = self.existing(kind, name)?;
            if draft.active.is_enabled(kind, name, disabled) == enabled {
                continue;
            }
            if kind == ContentKind::Mod {
                draft.plan.rename(
                    rel_path(kind, name, disabled)?,
                    rel_path(kind, name, !disabled)?,
                );
            } else {
                draft.active.set(kind, name, enabled);
            }
            changed.push(name.clone());
        }
        if changed.is_empty() {
            return Ok(changed);
        }
        let verb = if enabled { "启用" } else { "停用" };
        let summary = match changed.as_slice() {
            [one] => format!("{verb} {}", display_name(one)),
            many => format!("{verb} {} 项", many.len()),
        };
        self.commit(draft, &summary, None).await?;
        Ok(changed)
    }

    fn load_history(&self) -> Vec<HistoryEntry> {
        match read_json(&self.meta_path(HISTORY_FILE)) {
            Ok(Some(history)) => history,
            Ok(None) => Vec::new(),
            Err(error) => {
                tracing::warn!(%error, "改动历史读不了，按空处理");
                Vec::new()
            }
        }
    }

    /// 加上一条之后的完整改动历史（超过上限的旧条目丢掉），交给 [`Self::commit`] 写
    fn pushed_history(
        &self,
        action: HistoryAction,
        summary: String,
        changes: Vec<Change>,
    ) -> Vec<HistoryEntry> {
        let mut history = self.load_history();
        history.push(HistoryEntry {
            id: uuid::Uuid::new_v4().simple().to_string(),
            at: crate::now_millis() as f64,
            action,
            summary,
            changes,
            rolled_back: false,
        });
        let overflow = history.len().saturating_sub(MAX_HISTORY);
        history.drain(..overflow);
        history
    }

    /// 改动历史（新的在前）。
    pub fn history(&self) -> Vec<HistoryView> {
        self.load_history()
            .into_iter()
            .rev()
            .map(|e| HistoryView {
                count: u32::try_from(e.changes.len()).unwrap_or(u32::MAX),
                id: e.id,
                at: e.at,
                action: e.action,
                summary: e.summary,
                rolled_back: e.rolled_back,
            })
            .collect()
    }

    /// 撤销一次改动：这次加进来的文件进仓库，这次换掉 / 删掉的文件从仓库放回来。
    /// 要么撤销完整、要么一点不动。
    pub async fn rollback(&self, id: &str) -> Result<(), ContentError> {
        self.settle().await?;
        let mut history = self.load_history();
        let index = history
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| ContentError::Message("找不到这条改动记录".to_owned()))?;
        let entry = &history[index];
        if entry.rolled_back {
            return Err(ContentError::Message("这次改动已经撤销过了".to_owned()));
        }
        // 先确认要放回的文件都还在仓库里，缺一个就整个不做
        // （这次加进来的文件撤销时会先进仓库，内容一样的不用另外有备份：比如只是启用 / 停用变了）
        let incoming: HashSet<&str> = entry
            .changes
            .iter()
            .filter_map(|c| c.added.as_ref())
            .map(|a| a.sha1.as_str())
            .collect();
        if let Some(missing) = entry
            .changes
            .iter()
            .filter_map(|c| c.removed.as_ref())
            .find(|r| !self.store.contains(&r.sha1) && !incoming.contains(r.sha1.as_str()))
        {
            return Err(ContentError::Message(format!(
                "仓库里找不到 {} 的备份，无法撤销",
                missing.file_name
            )));
        }
        self.snapshot_before_change().await;
        let mut draft = self.draft();
        // 这次加进来的先收走
        for change in entry.changes.iter().rev() {
            if let Some(added) = &change.added
                && let Ok((path, disabled)) = self.existing(change.kind, &added.file_name)
                && path.is_file()
            {
                let sha1 = sha1_file(&path)
                    .await
                    .map_err(|source| io_error(&path, source))?;
                draft.take(change.kind, &added.file_name, disabled, sha1)?;
            }
        }
        // 换掉 / 删掉的放回来
        for change in entry.changes.iter().rev() {
            let Some(removed) = &change.removed else {
                continue;
            };
            let rel = rel_path(change.kind, &removed.file_name, removed.disabled)?;
            if self.occupied(&draft.plan, &rel) {
                // 那个位置上已经有文件、而且不是这次要收走的：内容一样就不用放，不一样不敢盖
                let same = !draft.plan.is_placed(&rel)
                    && sha1_file(&self.resolve(&rel)?)
                        .await
                        .is_ok_and(|h| h.eq_ignore_ascii_case(&removed.sha1));
                if !same {
                    return Err(ContentError::Message(format!(
                        "实例里已经有「{}」（可能是后来放进去的），先把它移走再撤销",
                        removed.file_name
                    )));
                }
            } else {
                draft.plan.place_stored(rel, removed.sha1.clone());
            }
            if let Some(record) = &removed.record {
                draft
                    .records
                    .insert(record_key(change.kind, &removed.file_name), record.clone());
            }
            if removed.enabled {
                draft.active.set(change.kind, &removed.file_name, true);
            }
        }
        let summary = format!("撤销「{}」", entry.summary);
        history[index].rolled_back = true;
        self.commit(draft, &summary, Some(history)).await
    }
}

// ---------- 改动历史 ----------

const HISTORY_FILE: &str = "history.json";
/// 最多保留多少条改动历史（仓库里的文件不会因此被删）
const MAX_HISTORY: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum HistoryAction {
    Install,
    Update,
    Remove,
    /// 一键回到快照的状态（崩溃后「回到改动前」）
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    id: String,
    at: f64,
    action: HistoryAction,
    summary: String,
    changes: Vec<Change>,
    rolled_back: bool,
}

/// 一个文件的变化：`removed` 是改动前在的，`added` 是改动后新来的
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Change {
    kind: ContentKind,
    removed: Option<FileRef>,
    added: Option<FileRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileRef {
    file_name: String,
    sha1: String,
    /// 模组是不是停用状态（磁盘上带 `.disabled`）
    disabled: bool,
    /// 当时是否在用（光影是当前光影、材质包在列表里）
    enabled: bool,
    record: Option<Record>,
}

/// 给界面看的历史条目
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct HistoryView {
    pub id: String,
    pub at: f64,
    pub action: HistoryAction,
    pub summary: String,
    /// 涉及几个文件
    pub count: u32,
    pub rolled_back: bool,
}

fn summarize(verb: &str, changes: &[Change]) -> String {
    let first = changes
        .first()
        .and_then(|c| c.added.as_ref().or(c.removed.as_ref()))
        .map(|f| {
            f.record
                .as_ref()
                .map_or_else(|| display_name(&f.file_name), |r| r.title.clone())
        })
        .unwrap_or_default();
    match changes.len() {
        0 | 1 => format!("{verb} {first}"),
        n => format!("{verb} {first} 等 {n} 项"),
    }
}

/// 文件名 → 好看一点的显示名：去掉扩展名
fn display_name(file_name: &str) -> String {
    let stem = file_name
        .strip_suffix(".jar")
        .or_else(|| file_name.strip_suffix(".zip"))
        .unwrap_or(file_name);
    stem.to_owned()
}

// ---------- 光影：Iris 的设置文件 config/iris.properties ----------

fn iris_config(dir: &Path) -> PathBuf {
    dir.join("config").join("iris.properties")
}

fn current_shader(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(iris_config(dir)).ok()?;
    let value = |key: &str| {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim().to_owned())
    };
    if value("enableShaders").as_deref() == Some("false") {
        return None;
    }
    value("shaderPack")
        .map(|v| {
            v.replace("\\:", ":")
                .replace("\\=", "=")
                .replace("\\\\", "\\")
        })
        .filter(|v| !v.is_empty())
}

/// Iris 设置文件在实例里的相对路径（改动日志用）
const IRIS_REL: &str = "config/iris.properties";

fn set_shader(dir: &Path, pack: Option<&str>) -> Result<(), ContentError> {
    let path = iris_config(dir);
    let text = read_text(&path)?.unwrap_or_default();
    write_atomic(&path, shader_text(&text, pack).as_bytes())?;
    Ok(())
}

/// 把「用哪个光影」写进 Iris 设置的文本里，其他行原样保留
fn shader_text(text: &str, pack: Option<&str>) -> String {
    let mut updates = vec![("enableShaders", pack.is_some().to_string())];
    if let Some(pack) = pack {
        updates.push(("shaderPack", pack.to_owned()));
    }
    set_properties(text, &updates)
}

/// 改 `.properties` 文件里的几个键，其他行原样保留
fn set_properties(text: &str, updates: &[(&str, String)]) -> String {
    let mut done = vec![false; updates.len()];
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let key = line.split_once('=').map(|(k, _)| k.trim());
            match updates.iter().position(|(k, _)| Some(*k) == key) {
                Some(i) => {
                    done[i] = true;
                    format!("{}={}", updates[i].0, escape_property(&updates[i].1))
                }
                None => line.to_owned(),
            }
        })
        .collect();
    for (i, (key, value)) in updates.iter().enumerate() {
        if !done[i] {
            lines.push(format!("{key}={}", escape_property(value)));
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `.properties` 里 `\`、`=`、`:` 要转义；Iris 读文件名时会还原
fn escape_property(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('=', "\\=")
        .replace(':', "\\:")
}

// ---------- 材质包：options.txt 的 resourcePacks 列表 ----------

const OPTIONS_KEY: &str = "resourcePacks";

fn enabled_resource_packs(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("options.txt")).unwrap_or_default();
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{OPTIONS_KEY}:")))
        .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

fn set_resource_pack(dir: &Path, file_name: &str, enabled: bool) -> Result<(), ContentError> {
    let mut packs = enabled_resource_packs(dir);
    if packs.is_empty() {
        packs.push("vanilla".to_owned());
    }
    let entry = format!("file/{file_name}");
    packs.retain(|p| p != &entry);
    if enabled {
        // 列表越靠后优先级越高：放到最后，盖过其他材质
        packs.push(entry);
    }
    write_resource_packs(dir, &packs)
}

/// 把 `options.txt` 里的材质包列表整个换掉，其他行原样保留
fn write_resource_packs(dir: &Path, packs: &[String]) -> Result<(), ContentError> {
    let path = dir.join("options.txt");
    let text = read_text(&path)?.unwrap_or_default();
    write_atomic(&path, resource_packs_text(&text, packs)?.as_bytes())?;
    Ok(())
}

fn resource_packs_text(text: &str, packs: &[String]) -> Result<String, ContentError> {
    let value = serde_json::to_string(packs).map_err(|e| ContentError::Api(e.to_string()))?;
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            if line.starts_with(&format!("{OPTIONS_KEY}:")) {
                found = true;
                format!("{OPTIONS_KEY}:{value}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{OPTIONS_KEY}:{value}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

// ---------- 一次改动里的「用哪个光影、哪些材质包」 ----------

/// 光影和材质包现在用哪个：一次改动里先在内存里改，最后各写一次（写文件也是改动日志里的一步）
struct Active {
    shader: Option<String>,
    shader_was: Option<String>,
    packs: Vec<String>,
    packs_were: Vec<String>,
}

impl Active {
    fn load(dir: &Path) -> Self {
        let shader = current_shader(dir);
        let packs = enabled_resource_packs(dir);
        Self {
            shader_was: shader.clone(),
            shader,
            packs_were: packs.clone(),
            packs,
        }
    }

    fn is_enabled(&self, kind: ContentKind, file_name: &str, disabled: bool) -> bool {
        match kind {
            ContentKind::Mod => !disabled,
            ContentKind::Shader => self.shader.as_deref() == Some(file_name),
            ContentKind::ResourcePack => self.packs.contains(&format!("file/{file_name}")),
        }
    }

    /// 打开 / 关掉（模组不在这里管，模组靠改名）
    fn set(&mut self, kind: ContentKind, file_name: &str, enabled: bool) {
        match kind {
            ContentKind::Mod => {}
            ContentKind::Shader if enabled => self.shader = Some(file_name.to_owned()),
            ContentKind::Shader => {
                if self.shader.as_deref() == Some(file_name) {
                    self.shader = None;
                }
            }
            ContentKind::ResourcePack => {
                let entry = format!("file/{file_name}");
                self.packs.retain(|p| p != &entry);
                if enabled {
                    if self.packs.is_empty() {
                        self.packs.push("vanilla".to_owned());
                    }
                    // 列表越靠后优先级越高：放到最后，盖过其他材质
                    self.packs.push(entry);
                }
            }
        }
    }

    /// 换成新版本的文件：还是原来那个位置（以前材质包更新后会跑到列表最后）
    fn replace(&mut self, kind: ContentKind, old: &str, new: &str) {
        match kind {
            ContentKind::Shader if self.shader.as_deref() == Some(old) => {
                self.shader = Some(new.to_owned());
            }
            ContentKind::ResourcePack => {
                let old = format!("file/{old}");
                if let Some(slot) = self.packs.iter_mut().find(|p| **p == old) {
                    *slot = format!("file/{new}");
                }
            }
            _ => {}
        }
    }

    /// 改过的写进计划
    fn plan(&self, plan: &mut Plan, dir: &Path) -> Result<(), ContentError> {
        if self.shader != self.shader_was {
            let text = read_text(&iris_config(dir))?.unwrap_or_default();
            plan.write(
                IRIS_REL.to_owned(),
                shader_text(&text, self.shader.as_deref()).into_bytes(),
            );
        }
        if self.packs != self.packs_were {
            let text = read_text(&dir.join("options.txt"))?.unwrap_or_default();
            plan.write(
                "options.txt".to_owned(),
                resource_packs_text(&text, &self.packs)?.into_bytes(),
            );
        }
        Ok(())
    }
}

fn cf_known_or_unknown_empty(modrinth_count: usize, checked: u32, cf_failed: bool) -> bool {
    modrinth_count > 0 && checked == 0 && !cf_failed
}

/// 将 CurseForge 查到的最新兼容文件合并进更新检查结果（单独提纯，便于不联网的单元测试）。
fn apply_curseforge_updates(
    kind: ContentKind,
    candidates: &[(&Scanned, &str, u32)],
    records: &Records,
    items: &[ContentItem],
    latest_cf: &HashMap<u32, (curseforge::File, Option<String>)>,
    on_modrinth: &HashSet<String>,
    result: &mut UpdateCheckResult,
) {
    for &(scanned, sha1, mod_id) in candidates {
        let Some((file, website)) = latest_cf.get(&mod_id) else {
            continue;
        };
        let Some(new_sha1) = file.sha1() else {
            tracing::warn!(file_id = file.id, "CurseForge 更新结果缺少有效 SHA-1");
            continue;
        };
        let record = records.get(&record_key(kind, &scanned.file_name));
        let same_version_id = record
            .filter(|r| r.source == ContentSource::CurseForge)
            .is_some_and(|r| r.version_id == file.id.to_string());
        let current = same_version_id || new_sha1.eq_ignore_ascii_case(sha1);
        result.checked += 1;
        result.unchecked = result.unchecked.saturating_sub(1);
        if current {
            continue;
        }
        let delivery = if file.usable_download_url().is_some() {
            Delivery::Direct
        } else if on_modrinth.contains(&new_sha1) {
            Delivery::Modrinth
        } else {
            Delivery::Manual
        };
        let page_url = (delivery == Delivery::Manual)
            .then(|| curseforge::download_page(website.as_deref(), file.id))
            .flatten();
        let item = items.iter().find(|i| i.file_name == scanned.file_name);
        let version_number = if file.display_name.trim().is_empty() {
            file.file_name.clone()
        } else {
            file.display_name.clone()
        };
        result.updates.push(UpdateInfo {
            kind,
            file_name: scanned.file_name.clone(),
            title: item.map_or_else(|| display_name(&scanned.file_name), |i| i.title.clone()),
            current_version: item.and_then(|i| i.version_number.clone()),
            version_id: file.id.to_string(),
            version_number,
            source: ContentSource::CurseForge,
            project_id: Some(mod_id.to_string()),
            delivery,
            page_url,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance() -> (tempfile::TempDir, InstanceContent) {
        let dir = tempfile::tempdir().unwrap();
        let content = InstanceContent::new(dir.path(), dir.path().join(STORE_DIR));
        (dir, content)
    }

    /// 下好的一个模组（测试用）：文件放进下载临时文件夹，返回对应的安装项
    fn staged_mod(staged: &StagedInstall, file_name: &str, bytes: &[u8]) -> Prepared {
        let entry = Prepared {
            kind: ContentKind::Mod,
            file_name: file_name.to_owned(),
            sha1: crate::net::to_hex(&<sha1::Sha1 as sha1::Digest>::digest(bytes)),
            size: bytes.len() as u64,
            urls: Some(vec![format!("https://cdn.modrinth.com/{file_name}")]),
            record: Record {
                project_id: file_name.to_owned(),
                version_id: "v1".to_owned(),
                title: file_name.to_owned(),
                version_number: "1.0".to_owned(),
                icon_url: None,
                source: ContentSource::Modrinth,
            },
        };
        let path = staged.path_of(&entry).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        entry
    }

    fn staged(content: &InstanceContent, files: &[(&str, &[u8])]) -> StagedInstall {
        let mut staged = StagedInstall {
            prepared: PreparedInstall {
                entries: Vec::new(),
                manual: Vec::new(),
            },
            dir: content
                .store
                .root()
                .join(STAGING_DIR)
                .join(uuid::Uuid::new_v4().simple().to_string()),
        };
        for (name, bytes) in files {
            let entry = staged_mod(&staged, name, bytes);
            staged.prepared.entries.push(entry);
        }
        staged
    }

    #[tokio::test]
    async fn update_check_counts_files_instead_of_response_hashes() {
        let (_dir, content) = instance();
        let kind = ContentKind::Mod;
        let mut curseforge = staged(&content, &[("cf-only.jar", b"C")]);
        curseforge.prepared.entries[0].record.source = ContentSource::CurseForge;
        content.finish_install(curseforge).await.unwrap();
        for (name, bytes) in [
            ("latest.jar", b"A"),
            ("latest-copy.jar.disabled", b"A"),
            ("update.jar", b"B"),
            ("unknown.jar", b"D"),
            ("missing-checksum.jar", b"E"),
            ("secondary.jar", b"F"),
        ] {
            std::fs::write(content.folder(kind).join(name), bytes).unwrap();
        }

        let hash = |bytes: &[u8]| crate::net::to_hex(&<sha1::Sha1 as sha1::Digest>::digest(bytes));
        let file = |sha1: Option<String>, primary| {
            serde_json::json!({
                "url": "https://cdn.modrinth.com/data/project/versions/latest/new.jar",
                "filename": "new.jar",
                "primary": primary,
                "hashes": sha1.map_or_else(|| serde_json::json!({}), |sha1| serde_json::json!({ "sha1": sha1 })),
            })
        };
        let version = |files| {
            serde_json::from_value(serde_json::json!({
                "id": "latest",
                "project_id": "project",
                "version_number": "2.0",
                "files": files,
            }))
            .unwrap()
        };
        let latest = BTreeMap::from([
            (hash(b"A"), version(vec![file(Some(hash(b"A")), true)])),
            (hash(b"B"), version(vec![file(Some(hash(b"new")), true)])),
            (hash(b"E"), version(vec![file(None, true)])),
            (
                hash(b"F"),
                version(vec![
                    file(Some(hash(b"primary")), true),
                    file(Some(hash(b"F")), false),
                ]),
            ),
        ]);
        let result = content
            .check_updates_with(kind, |hashes| async move {
                assert_eq!(hashes.len(), 7);
                assert!(hashes.contains(&hash(b"C")));
                assert!(hashes.contains(&hash(b"D")));
                Ok(latest)
            })
            .await
            .unwrap();

        assert_eq!(result.checked, 4);
        assert_eq!(result.unchecked, 3);
        assert!(!result.lookup_failed);
        assert_eq!(result.updates.len(), 1);
        assert_eq!(result.updates[0].file_name, "update.jar");
        assert_eq!(result.updates[0].version_id, "latest");
        assert_eq!(
            std::fs::read(content.folder(kind).join("update.jar")).unwrap(),
            b"B"
        );
        assert_eq!(
            content.history().len(),
            1,
            "只有准备 CurseForge 文件的安装记录"
        );
    }

    #[tokio::test]
    async fn update_check_reports_unmatched_and_unpacked_content() {
        let (_dir, content) = instance();
        let kind = ContentKind::ResourcePack;
        let folder = content.folder(kind);
        std::fs::create_dir_all(folder.join("unpacked")).unwrap();
        std::fs::write(folder.join("unknown.zip"), b"unknown").unwrap();

        let result = content
            .check_updates_with(kind, |hashes| async move {
                assert_eq!(hashes.len(), 1, "文件夹不参加联网查询");
                Ok(BTreeMap::new())
            })
            .await
            .unwrap();

        assert_eq!(result.checked, 0);
        assert_eq!(result.unchecked, 2);
        assert!(result.updates.is_empty());
        assert!(!result.lookup_failed);
    }

    #[tokio::test]
    async fn update_check_marks_a_failed_platform_lookup() {
        let (_dir, content) = instance();
        let kind = ContentKind::Mod;
        let folder = content.folder(kind);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mod.jar"), b"mod").unwrap();

        let result = content
            .check_updates_with(kind, |_| async {
                Err(ContentError::Api("查询失败".to_owned()))
            })
            .await
            .unwrap();

        assert_eq!(result.checked, 0);
        assert_eq!(result.unchecked, 1);
        assert!(result.updates.is_empty());
        assert!(result.lookup_failed);
    }

    /// 连续安装两个共用前置的模组：两个一起下完，先后放进实例。
    /// 前置只放一份，第二次的改动历史里没有它（撤销第二个时不会把第一个要的前置删掉）
    #[tokio::test]
    async fn two_installs_sharing_a_dependency_place_it_once() {
        let (dir, content) = instance();
        let first = staged(&content, &[("anvils.jar", b"A"), ("puzzleslib.jar", b"P")]);
        let second = staged(
            &content,
            &[("workbench.jar", b"W"), ("puzzleslib.jar", b"P")],
        );
        let first_dir = first.dir.clone();

        content.finish_install(first).await.unwrap();
        content.finish_install(second).await.unwrap();

        let mut names: Vec<String> = content
            .list(ContentKind::Mod)
            .unwrap()
            .into_iter()
            .map(|i| i.file_name)
            .collect();
        names.sort();
        assert_eq!(names, ["anvils.jar", "puzzleslib.jar", "workbench.jar"]);
        let history = content.history();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[0].count, 1,
            "第二次只记它自己：{}",
            history[0].summary
        );
        assert_eq!(history[1].count, 2);
        assert!(!first_dir.exists(), "放完下载临时文件夹就删掉");

        content.rollback(&history[0].id).await.unwrap();
        assert!(
            dir.path().join("mods/puzzleslib.jar").exists(),
            "撤销第二个，前置还在"
        );
        assert!(!dir.path().join("mods/workbench.jar").exists());
    }

    /// 下到一半取消、失败：下载的文件一起删掉，实例不动
    #[test]
    fn dropped_staging_is_cleaned_up() {
        let (dir, content) = instance();
        let staged = staged(&content, &[("anvils.jar", b"A")]);
        let staging = staged.dir.clone();
        assert!(staging.join("mods/anvils.jar").exists());
        drop(staged);
        assert!(!staging.exists());
        assert!(!dir.path().join("mods").exists());
    }

    #[test]
    fn update_keeps_icon_of_hand_added_mods() {
        // 手动放进来的模组没有安装记录，更新后仍保留原有图标
        let dir = tempfile::tempdir().unwrap();
        let mut cache = IdentityCache::load(dir.path());
        let old_sha1 = "a".repeat(40);
        cache.remember(
            old_sha1.clone(),
            Identity {
                title: "Jade".to_owned(),
                version: Some("15.10.3".to_owned()),
                icon_url: Some("https://cdn.modrinth.com/cache.webp".to_owned()),
                icon_file: None,
                project_id: Some("nvQzSEkH".to_owned()),
                version_id: Some("old".to_owned()),
                source: IdentitySource::Modrinth,
            },
        );
        assert_eq!(
            carried_icon(None, &cache, &old_sha1).as_deref(),
            Some("https://cdn.modrinth.com/cache.webp")
        );

        let record = Record {
            project_id: "nvQzSEkH".to_owned(),
            version_id: "old".to_owned(),
            title: "Jade".to_owned(),
            version_number: "15.10.3".to_owned(),
            icon_url: Some("https://cdn.modrinth.com/record.webp".to_owned()),
            source: ContentSource::Modrinth,
        };
        assert_eq!(
            carried_icon(Some(&record), &cache, &old_sha1).as_deref(),
            Some("https://cdn.modrinth.com/record.webp"),
            "有安装记录时优先用记录里的"
        );
        assert_eq!(carried_icon(None, &cache, &"b".repeat(40)), None);
    }

    #[test]
    fn lists_and_toggles_mods() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("sodium.jar"), b"x").unwrap();
        std::fs::write(mods.join("lithium.jar.disabled"), b"xy").unwrap();
        std::fs::write(mods.join("readme.txt"), b"not a mod").unwrap();

        let items = content.list(ContentKind::Mod).unwrap();
        assert_eq!(items.len(), 2);
        let lithium = items.iter().find(|i| i.file_name == "lithium.jar").unwrap();
        assert!(!lithium.enabled);
        assert_eq!(lithium.title, "lithium");

        content
            .set_enabled(ContentKind::Mod, "lithium.jar", true)
            .unwrap();
        content
            .set_enabled(ContentKind::Mod, "sodium.jar", false)
            .unwrap();
        assert!(mods.join("lithium.jar").exists());
        assert!(mods.join("sodium.jar.disabled").exists());
        assert!(
            content
                .set_enabled(ContentKind::Mod, "missing.jar", true)
                .is_err()
        );
        assert!(
            content
                .set_enabled(ContentKind::Mod, "../evil.jar", true)
                .is_err()
        );
    }

    #[test]
    fn shader_selection_is_written_to_iris_settings() {
        let (dir, content) = instance();
        std::fs::create_dir_all(dir.path().join("shaderpacks")).unwrap();
        std::fs::write(dir.path().join("shaderpacks/BSL_v8.4.zip"), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            iris_config(dir.path()),
            "maxShadowRenderDistance=32\nenableShaders=false\n",
        )
        .unwrap();

        content
            .set_enabled(ContentKind::Shader, "BSL_v8.4.zip", true)
            .unwrap();
        let text = std::fs::read_to_string(iris_config(dir.path())).unwrap();
        assert!(text.contains("maxShadowRenderDistance=32"), "其他设置保留");
        assert!(text.contains("enableShaders=true"));
        assert!(text.contains("shaderPack=BSL_v8.4.zip"));
        assert!(content.list(ContentKind::Shader).unwrap()[0].enabled);

        content
            .set_enabled(ContentKind::Shader, "BSL_v8.4.zip", false)
            .unwrap();
        assert!(!content.list(ContentKind::Shader).unwrap()[0].enabled);
    }

    #[test]
    fn resource_packs_are_added_to_options_txt() {
        let (dir, content) = instance();
        std::fs::create_dir_all(dir.path().join("resourcepacks")).unwrap();
        std::fs::write(dir.path().join("resourcepacks/Faithful.zip"), b"x").unwrap();
        std::fs::write(
            dir.path().join("options.txt"),
            "lang:zh_cn\nresourcePacks:[\"vanilla\"]\n",
        )
        .unwrap();

        content
            .set_enabled(ContentKind::ResourcePack, "Faithful.zip", true)
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("options.txt")).unwrap();
        assert!(text.contains("lang:zh_cn"));
        assert!(text.contains(r#"resourcePacks:["vanilla","file/Faithful.zip"]"#));
        assert!(content.list(ContentKind::ResourcePack).unwrap()[0].enabled);

        content
            .set_enabled(ContentKind::ResourcePack, "Faithful.zip", false)
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("options.txt")).unwrap();
        assert!(text.contains(r#"resourcePacks:["vanilla"]"#));
    }

    #[test]
    fn options_txt_is_created_when_missing() {
        let (dir, content) = instance();
        content
            .set_enabled(ContentKind::ResourcePack, "Pack.zip", true)
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("options.txt")).unwrap();
        assert_eq!(text, "resourcePacks:[\"vanilla\",\"file/Pack.zip\"]\n");
    }

    #[tokio::test]
    async fn removing_goes_to_the_store_and_can_be_undone() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("a.jar"), b"mod a").unwrap();
        std::fs::write(mods.join("b.jar.disabled"), b"mod b").unwrap();

        content.remove(ContentKind::Mod, "a.jar").await.unwrap();
        content.remove(ContentKind::Mod, "b.jar").await.unwrap();
        assert!(content.list(ContentKind::Mod).unwrap().is_empty());

        let history = content.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].action, HistoryAction::Remove, "新的在前");
        assert_eq!(history[0].summary, "删除 b");

        content.rollback(&history[0].id).await.unwrap();
        assert!(
            mods.join("b.jar.disabled").exists(),
            "停用的模组放回来还是停用"
        );
        assert!(
            content.rollback(&history[0].id).await.is_err(),
            "不能撤销两次"
        );

        content.rollback(&history[1].id).await.unwrap();
        assert_eq!(std::fs::read(mods.join("a.jar")).unwrap(), b"mod a");
        assert!(content.history()[1].rolled_back);
    }

    #[tokio::test]
    async fn dropped_files_are_copied_in_and_can_be_undone() {
        let (dir, content) = instance();
        let downloads = tempfile::tempdir().unwrap();
        let new = downloads.path().join("sodium-0.6.jar");
        std::fs::write(&new, b"sodium 0.6").unwrap();
        let pack = downloads.path().join("faithful.zip");
        std::fs::write(&pack, b"pack").unwrap();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("sodium-0.5.jar"), b"sodium 0.5").unwrap();

        let outcomes = content
            .add_local(&[
                LocalAdd {
                    kind: ContentKind::Mod,
                    source: new.clone(),
                    replace: Some("sodium-0.5.jar".to_owned()),
                },
                LocalAdd {
                    kind: ContentKind::ResourcePack,
                    source: pack.clone(),
                    replace: None,
                },
            ])
            .await
            .unwrap();
        assert_eq!(outcomes, [LocalOutcome::Added, LocalOutcome::Added]);
        assert!(new.exists(), "原文件不动");
        assert_eq!(
            std::fs::read(mods.join("sodium-0.6.jar")).unwrap(),
            b"sodium 0.6"
        );
        assert!(!mods.join("sodium-0.5.jar").exists(), "旧版本换掉了");
        assert!(
            enabled_resource_packs(dir.path()).contains(&"file/faithful.zip".to_owned()),
            "资源包装好就启用"
        );
        let history = content.history();
        assert_eq!(history.len(), 1, "一批算一条");
        assert_eq!(history[0].summary, "安装 sodium-0.6 等 2 项");

        // 再拖一次一模一样的：不动
        let again = content
            .add_local(&[LocalAdd {
                kind: ContentKind::Mod,
                source: new,
                replace: None,
            }])
            .await
            .unwrap();
        assert_eq!(again, [LocalOutcome::Same]);
        assert_eq!(content.history().len(), 1);

        content.rollback(&history[0].id).await.unwrap();
        assert_eq!(
            std::fs::read(mods.join("sodium-0.5.jar")).unwrap(),
            b"sodium 0.5"
        );
        assert!(!mods.join("sodium-0.6.jar").exists());
        assert!(!dir.path().join("resourcepacks/faithful.zip").exists());
    }

    #[tokio::test]
    async fn removing_many_is_one_history_entry_and_one_undo() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        for name in ["a.jar", "b.jar", "c.jar", "d.jar"] {
            std::fs::write(mods.join(name), name.as_bytes()).unwrap();
        }
        let names: Vec<String> = ["a.jar", "b.jar", "c.jar", "d.jar"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            content.remove_many(ContentKind::Mod, &names).await.unwrap(),
            4
        );
        assert!(content.list(ContentKind::Mod).unwrap().is_empty());

        let history = content.history();
        assert_eq!(history.len(), 1, "四个只记一条");
        assert_eq!(history[0].summary, "删除 4 项：a、b、c……");

        content.rollback(&history[0].id).await.unwrap();
        assert_eq!(
            content.list(ContentKind::Mod).unwrap().len(),
            4,
            "一次撤销全部回来"
        );

        // 找文件位置：停用的也找得到
        content
            .set_enabled(ContentKind::Mod, "a.jar", false)
            .unwrap();
        assert_eq!(
            content.locate(ContentKind::Mod, "a.jar").unwrap(),
            mods.join("a.jar.disabled")
        );
        assert!(content.locate(ContentKind::Mod, "../x.jar").is_err());
    }

    #[tokio::test]
    async fn removing_the_active_shader_turns_shaders_off() {
        let (dir, content) = instance();
        std::fs::create_dir_all(dir.path().join("shaderpacks")).unwrap();
        std::fs::write(dir.path().join("shaderpacks/BSL.zip"), b"x").unwrap();
        content
            .set_enabled(ContentKind::Shader, "BSL.zip", true)
            .unwrap();

        content
            .remove(ContentKind::Shader, "BSL.zip")
            .await
            .unwrap();
        assert_eq!(current_shader(dir.path()), None);

        let id = content.history()[0].id.clone();
        content.rollback(&id).await.unwrap();
        assert_eq!(
            current_shader(dir.path()).as_deref(),
            Some("BSL.zip"),
            "撤销后恢复成当前光影"
        );
    }

    #[test]
    fn property_values_are_escaped() {
        let text = set_properties("", &[("shaderPack", "a=b:c.zip".to_owned())]);
        assert_eq!(text, "shaderPack=a\\=b\\:c.zip\n");
    }

    // ---------- 改到一半能恢复 ----------

    use journal::Fault;

    /// 实例里所有文件的内容（不算内容仓库、快照，也不算文件夹）：比较「改之前」和「退回之后」
    fn files_of(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == STORE_DIR || rel == format!("{META_DIR}/snapshot.json") {
                    continue;
                }
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(rel, std::fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// 一个有点东西的实例：两个装的模组（有来源记录、有一条历史）、一个手动放的停用模组、
    /// 一个正在用的材质包、一个正在用的光影
    async fn stocked() -> (tempfile::TempDir, InstanceContent) {
        let (dir, content) = instance();
        content
            .finish_install(staged(&content, &[("a.jar", b"a1"), ("c.jar", b"c1")]))
            .await
            .unwrap();
        let root = dir.path();
        std::fs::write(root.join("mods/b.jar.disabled"), b"b1").unwrap();
        std::fs::create_dir_all(root.join("resourcepacks")).unwrap();
        std::fs::write(root.join("resourcepacks/R.zip"), b"r1").unwrap();
        std::fs::create_dir_all(root.join("shaderpacks")).unwrap();
        std::fs::write(root.join("shaderpacks/S.zip"), b"s1").unwrap();
        std::fs::write(
            root.join("options.txt"),
            "lang:zh_cn\nresourcePacks:[\"vanilla\",\"file/R.zip\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(iris_config(root), "enableShaders=true\nshaderPack=S.zip\n").unwrap();
        std::fs::create_dir_all(root.join("downloads")).unwrap();
        std::fs::write(root.join("downloads/e.jar"), b"e1").unwrap();
        std::fs::write(root.join("downloads/T.zip"), b"t1").unwrap();
        (dir, content)
    }

    #[derive(Debug, Clone, Copy)]
    enum Op {
        RemoveMods,
        RemoveShader,
        RemovePack,
        Install,
        DropIn,
        DisableAll,
        Undo,
        Restore,
    }

    const OPS: [Op; 8] = [
        Op::RemoveMods,
        Op::RemoveShader,
        Op::RemovePack,
        Op::Install,
        Op::DropIn,
        Op::DisableAll,
        Op::Undo,
        Op::Restore,
    ];

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    /// 做这个操作之前要先做好的（不会被打断）
    async fn before_op(content: &InstanceContent, op: Op) {
        let root = content.dir.clone();
        match op {
            Op::Undo => {
                content
                    .remove_many(ContentKind::Mod, &names(&["a.jar", "c.jar"]))
                    .await
                    .unwrap();
            }
            Op::Restore => {
                content.mark_good().await.unwrap();
                content.remove(ContentKind::Mod, "a.jar").await.unwrap();
                content
                    .add_local(&[LocalAdd {
                        kind: ContentKind::Mod,
                        source: root.join("downloads/e.jar"),
                        replace: Some("c.jar".to_owned()),
                    }])
                    .await
                    .unwrap();
                content
                    .set_enabled(ContentKind::Mod, "b.jar", true)
                    .unwrap();
                content
                    .remove(ContentKind::ResourcePack, "R.zip")
                    .await
                    .unwrap();
            }
            _ => {}
        }
    }

    async fn run_op(content: &InstanceContent, op: Op) -> Result<(), ContentError> {
        let root = content.dir.clone();
        match op {
            Op::RemoveMods => content
                .remove_many(ContentKind::Mod, &names(&["a.jar", "b.jar", "c.jar"]))
                .await
                .map(|_| ()),
            Op::RemoveShader => content.remove(ContentKind::Shader, "S.zip").await,
            Op::RemovePack => content.remove(ContentKind::ResourcePack, "R.zip").await,
            Op::Install => {
                // a.jar 换成内容不一样的新文件（旧的要收进仓库），再加一个 d.jar
                let staged = staged(content, &[("a.jar", b"a2"), ("d.jar", b"d1")]);
                content.finish_install(staged).await
            }
            Op::DropIn => content
                .add_local(&[
                    LocalAdd {
                        kind: ContentKind::Mod,
                        source: root.join("downloads/e.jar"),
                        replace: Some("c.jar".to_owned()),
                    },
                    LocalAdd {
                        kind: ContentKind::ResourcePack,
                        source: root.join("downloads/T.zip"),
                        replace: None,
                    },
                ])
                .await
                .map(|_| ()),
            Op::DisableAll => content
                .set_many_enabled(
                    ContentKind::Mod,
                    &names(&["a.jar", "b.jar", "c.jar"]),
                    false,
                )
                .await
                .map(|_| ()),
            Op::Undo => {
                let id = content.history()[0].id.clone();
                content.rollback(&id).await
            }
            Op::Restore => content.restore_snapshot().await.map(|_| ()),
        }
    }

    fn set_fault(content: &InstanceContent, fault: Option<Fault>) {
        *content.fault.lock().unwrap() = fault;
    }

    /// 崩溃恢复不变量：每种操作在每一步之前被打断（启动器被关掉），下次打开都退回成一模一样的「改之前」，
    /// 退回本身再做一遍也不出错
    #[tokio::test]
    async fn every_interruption_goes_back_to_exactly_before() {
        for op in OPS {
            let mut step = 0;
            loop {
                let (dir, content) = stocked().await;
                before_op(&content, op).await;
                let before = files_of(dir.path());
                set_fault(&content, Some(Fault::CrashAt(step)));
                let result = run_op(&content, op).await;
                set_fault(&content, None);
                if result.is_ok() {
                    // 这一步已经超过总步数：没被打断、正常做完了
                    assert!(!content.has_pending(), "{op:?} 做完了日志要删掉");
                    assert_ne!(files_of(dir.path()), before, "{op:?} 做完了要有变化");
                    assert!(step >= 2, "{op:?} 只有 {step} 步，不像");
                    break;
                }
                assert!(
                    content.has_pending(),
                    "{op:?} 第 {step} 步被打断，日志要留着"
                );
                let journal = std::fs::read(content.journal_path()).unwrap();
                // 被打断时复制留下的半个临时文件，退回时在这次动过的文件夹里一起清掉
                let folder = match op {
                    Op::RemoveShader => "shaderpacks",
                    Op::RemovePack => "resourcepacks",
                    _ => "mods",
                };
                std::fs::write(
                    dir.path()
                        .join(folder)
                        .join("x.part-0123456789abcdef0123456789abcdef"),
                    b"half",
                )
                .unwrap();

                let recovered = content.recover().await.unwrap().unwrap();
                assert!(!recovered.summary.is_empty());
                assert!(!content.has_pending());
                assert_eq!(
                    files_of(dir.path()),
                    before,
                    "{op:?} 第 {step} 步被打断后没退回原样"
                );

                // 退回到一半又被关掉：再退一次也一样
                std::fs::write(content.journal_path(), journal).unwrap();
                content.recover().await.unwrap().unwrap();
                assert_eq!(
                    files_of(dir.path()),
                    before,
                    "{op:?} 第 {step} 步：退回做两遍不一样"
                );
                step += 1;
            }
        }
    }

    /// 中途出错（这里模拟磁盘满）：当场退回，什么都没变，告诉用户为什么
    #[tokio::test]
    async fn a_failure_midway_reverts_on_the_spot() {
        for op in OPS {
            let (dir, content) = stocked().await;
            before_op(&content, op).await;
            let before = files_of(dir.path());
            set_fault(
                &content,
                Some(Fault::FailAt(1, std::io::ErrorKind::StorageFull)),
            );
            let error = run_op(&content, op).await.unwrap_err();
            let message = error.to_string();
            assert!(
                matches!(error, ContentError::Reverted { .. }),
                "{op:?}：{message}"
            );
            assert!(message.contains("磁盘空间不够"), "{op:?}：{message}");
            assert!(message.contains("实例保持原样"), "{op:?}：{message}");
            assert!(!content.has_pending());
            assert_eq!(files_of(dir.path()), before, "{op:?}");
        }
    }

    /// 游戏开着时 Java 就是这样打开模组的：别人能读，不能改名、删除
    #[cfg(windows)]
    #[tokio::test]
    async fn a_locked_file_reverts_the_whole_batch() {
        use std::os::windows::fs::OpenOptionsExt;
        let (dir, content) = stocked().await;
        let before = files_of(dir.path());
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ
            .open(dir.path().join("mods/c.jar"))
            .unwrap();
        let error = content
            .remove_many(ContentKind::Mod, &names(&["a.jar", "b.jar", "c.jar"]))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("「c.jar」被别的程序占用"), "{message}");
        assert!(message.contains("实例保持原样"), "{message}");
        assert!(!content.has_pending());
        drop(lock);
        assert_eq!(files_of(dir.path()), before);
        assert_eq!(content.history().len(), 1, "没多出历史");
    }

    /// 来源记录写不进去（只读）：文件都退回去
    #[tokio::test]
    async fn records_that_cannot_be_written_revert_the_files() {
        let (dir, content) = stocked().await;
        let before = files_of(dir.path());
        let records = dir.path().join("Lumi/content.json");
        let mut readonly = std::fs::metadata(&records).unwrap().permissions();
        readonly.set_readonly(true);
        std::fs::set_permissions(&records, readonly.clone()).unwrap();

        let error = content
            .remove_many(ContentKind::Mod, &names(&["a.jar", "c.jar"]))
            .await
            .unwrap_err();
        let message = error.to_string();

        #[allow(clippy::permissions_set_readonly_false)]
        readonly.set_readonly(false);
        std::fs::set_permissions(&records, readonly).unwrap();
        assert!(message.contains("content.json"), "{message}");
        assert!(message.contains("实例保持原样"), "{message}");
        assert!(!content.has_pending());
        assert_eq!(files_of(dir.path()), before);
    }

    /// 改动日志都写不进去：一个文件都没动
    #[tokio::test]
    async fn nothing_is_touched_when_the_journal_cannot_be_written() {
        let (dir, content) = stocked().await;
        let before = files_of(dir.path());
        set_fault(&content, Some(Fault::NoJournal));
        let error = content
            .remove_many(ContentKind::Mod, &names(&["a.jar", "c.jar"]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("记不下要做的事"), "{error}");
        assert_eq!(files_of(dir.path()), before);
    }

    /// 上次没做完的，下次改之前先退回，再做这次的
    #[tokio::test]
    async fn the_next_change_settles_the_last_one_first() {
        let (dir, content) = stocked().await;
        set_fault(&content, Some(Fault::CrashAt(1)));
        run_op(&content, Op::RemoveMods).await.unwrap_err();
        set_fault(&content, None);
        assert!(content.has_pending());

        content.remove(ContentKind::Shader, "S.zip").await.unwrap();
        assert!(!content.has_pending());
        let mods = dir.path().join("mods");
        assert!(mods.join("a.jar").exists() && mods.join("c.jar").exists());
        assert!(mods.join("b.jar.disabled").exists());
        assert!(!dir.path().join("shaderpacks/S.zip").exists());
        assert_eq!(content.history()[0].summary, "删除 S");
    }

    #[tokio::test]
    async fn nothing_to_recover_is_fine() {
        let (_dir, content) = instance();
        assert!(!content.has_pending());
        assert_eq!(content.recover().await.unwrap(), None);
    }

    /// 同名但内容不一样的安装：以前直接盖掉，现在旧的进仓库，撤销能回来
    #[tokio::test]
    async fn installing_over_a_different_file_keeps_the_old_one() {
        let (dir, content) = stocked().await;
        content
            .finish_install(staged(&content, &[("a.jar", b"a2")]))
            .await
            .unwrap();
        let a = dir.path().join("mods/a.jar");
        assert_eq!(std::fs::read(&a).unwrap(), b"a2");
        let id = content.history()[0].id.clone();
        content.rollback(&id).await.unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"a1");
    }

    /// 批量停用一次做完，再反过来调一次就是撤销
    #[tokio::test]
    async fn disabling_many_and_back() {
        let (dir, content) = stocked().await;
        let changed = content
            .set_many_enabled(
                ContentKind::Mod,
                &names(&["a.jar", "b.jar", "c.jar"]),
                false,
            )
            .await
            .unwrap();
        assert_eq!(changed, ["a.jar", "c.jar"], "已经停用的 b 不算");
        let mods = dir.path().join("mods");
        assert!(mods.join("a.jar.disabled").exists() && mods.join("c.jar.disabled").exists());
        content
            .set_many_enabled(ContentKind::Mod, &changed, true)
            .await
            .unwrap();
        assert!(mods.join("a.jar").exists() && mods.join("c.jar").exists());
        assert!(mods.join("b.jar.disabled").exists());

        let packs = content
            .set_many_enabled(ContentKind::ResourcePack, &names(&["R.zip"]), false)
            .await
            .unwrap();
        assert_eq!(packs, ["R.zip"]);
        assert!(!enabled_resource_packs(dir.path()).contains(&"file/R.zip".to_owned()));
    }

    /// 材质包更新后还在列表里原来的位置
    #[test]
    fn an_updated_pack_keeps_its_place() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("options.txt"),
            "resourcePacks:[\"vanilla\",\"file/A.zip\",\"file/B.zip\"]\n",
        )
        .unwrap();
        let mut active = Active::load(dir.path());
        active.replace(ContentKind::ResourcePack, "A.zip", "A2.zip");
        assert_eq!(active.packs, ["vanilla", "file/A2.zip", "file/B.zip"]);
        active.set(ContentKind::ResourcePack, "C.zip", true);
        assert_eq!(active.packs.last().map(String::as_str), Some("file/C.zip"));
        active.set(ContentKind::Shader, "X.zip", false);
        assert_eq!(active.shader, None, "关一个没在用的光影不影响");
    }

    #[tokio::test]
    async fn locking_content_persists_and_skips_update_checks() {
        let (_dir, content) = instance();
        let kind = ContentKind::Mod;
        std::fs::create_dir_all(content.folder(kind)).unwrap();
        std::fs::write(content.folder(kind).join("locked.jar"), b"old-locked").unwrap();
        std::fs::write(content.folder(kind).join("free.jar"), b"old-free").unwrap();

        assert!(content.set_locked(kind, "locked.jar", true).unwrap());
        let listed = content.list(kind).unwrap();
        assert!(
            listed
                .iter()
                .find(|i| i.file_name == "locked.jar")
                .unwrap()
                .locked
        );
        assert!(
            !listed
                .iter()
                .find(|i| i.file_name == "free.jar")
                .unwrap()
                .locked
        );

        let hash = |bytes: &[u8]| crate::net::to_hex(&<sha1::Sha1 as sha1::Digest>::digest(bytes));
        let new_ver = |id: &str| {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "project_id": "proj",
                "version_number": "2.0",
                "files": [{
                    "url": "https://cdn.modrinth.com/data/proj/versions/v2/new.jar",
                    "filename": "new.jar",
                    "primary": true,
                    "hashes": { "sha1": hash(b"new-bytes") }
                }]
            }))
            .unwrap()
        };
        let latest = BTreeMap::from([
            (hash(b"old-locked"), new_ver("v2-locked")),
            (hash(b"old-free"), new_ver("v2-free")),
        ]);
        let res = content
            .check_updates_with(kind, |hashes| async move {
                assert_eq!(hashes, vec![hash(b"old-free")], "锁定项不参与联网查询");
                Ok(latest)
            })
            .await
            .unwrap();
        assert_eq!(res.checked, 1);
        assert_eq!(res.unchecked, 0);
        assert_eq!(res.updates.len(), 1);
        assert_eq!(res.updates[0].file_name, "free.jar");

        assert!(!content.set_locked(kind, "locked.jar", false).unwrap());
        assert!(
            !content
                .list(kind)
                .unwrap()
                .iter()
                .find(|i| i.file_name == "locked.jar")
                .unwrap()
                .locked
        );
    }

    #[test]
    fn curseforge_update_check_detects_direct_modrinth_and_manual_updates() {
        let (_dir, content) = instance();
        let kind = ContentKind::Mod;
        std::fs::create_dir_all(content.folder(kind)).unwrap();
        std::fs::write(content.folder(kind).join("direct.jar"), b"old1").unwrap();
        std::fs::write(content.folder(kind).join("manual.jar"), b"old2").unwrap();
        std::fs::write(content.folder(kind).join("current.jar"), b"cur3").unwrap();
        let scanned = content.scan(kind).unwrap();
        let items = content.list(kind).unwrap();
        let hash = |bytes: &[u8]| crate::net::to_hex(&<sha1::Sha1 as sha1::Digest>::digest(bytes));
        let mk_file = |id: u32, mod_id: u32, name: &str, sha1: &str, url: Option<&str>| {
            serde_json::from_value::<curseforge::File>(serde_json::json!({
                "id": id,
                "modId": mod_id,
                "displayName": format!("{name} v2"),
                "fileName": format!("{name}-2.jar"),
                "releaseType": 1,
                "hashes": [{ "value": sha1, "algo": 1 }],
                "fileLength": 123,
                "downloadUrl": url,
                "gameVersions": ["1.21.1"],
                "dependencies": [],
                "fileFingerprint": 1
            }))
            .unwrap()
        };
        let direct_s = scanned
            .iter()
            .find(|s| s.file_name == "direct.jar")
            .unwrap();
        let manual_s = scanned
            .iter()
            .find(|s| s.file_name == "manual.jar")
            .unwrap();
        let cur_s = scanned
            .iter()
            .find(|s| s.file_name == "current.jar")
            .unwrap();
        let h_old1 = hash(b"old1");
        let h_old2 = hash(b"old2");
        let h_cur3 = hash(b"cur3");
        let h_new1 = hash(b"new1");
        let h_new2 = hash(b"new2");
        let latest_cf = HashMap::from([
            (
                101,
                (
                    mk_file(
                        501,
                        101,
                        "Direct",
                        &h_new1,
                        Some("https://edge.forgecdn.net/files/501/Direct-2.jar"),
                    ),
                    Some("https://www.curseforge.com/minecraft/mc-mods/direct".to_owned()),
                ),
            ),
            (
                102,
                (
                    mk_file(502, 102, "Manual", &h_new2, None),
                    Some("https://www.curseforge.com/minecraft/mc-mods/manual".to_owned()),
                ),
            ),
            (
                103,
                (
                    mk_file(
                        503,
                        103,
                        "Current",
                        &h_cur3,
                        Some("https://edge.forgecdn.net/files/503/Current.jar"),
                    ),
                    None,
                ),
            ),
        ]);
        let mut res = UpdateCheckResult {
            unchecked: 3,
            ..UpdateCheckResult::default()
        };
        apply_curseforge_updates(
            kind,
            &[
                (direct_s, &h_old1, 101),
                (manual_s, &h_old2, 102),
                (cur_s, &h_cur3, 103),
            ],
            &Records::new(),
            &items,
            &latest_cf,
            &HashSet::new(),
            &mut res,
        );
        assert_eq!(res.checked, 3);
        assert_eq!(res.unchecked, 0);
        assert_eq!(res.updates.len(), 2);
        let d = res
            .updates
            .iter()
            .find(|u| u.file_name == "direct.jar")
            .unwrap();
        assert_eq!(d.source, ContentSource::CurseForge);
        assert_eq!(d.delivery, Delivery::Direct);
        let m = res
            .updates
            .iter()
            .find(|u| u.file_name == "manual.jar")
            .unwrap();
        assert_eq!(m.source, ContentSource::CurseForge);
        assert_eq!(m.delivery, Delivery::Manual);
        assert_eq!(
            m.page_url.as_deref(),
            Some("https://www.curseforge.com/minecraft/mc-mods/manual/download/502")
        );
    }

    #[tokio::test]
    async fn finish_update_transfers_lock_and_supports_rollback() {
        let (_dir, content) = instance();
        let kind = ContentKind::Mod;
        let old_staged = staged(&content, &[("mod-1.0.jar", b"v1")]);
        content.finish_install(old_staged).await.unwrap();
        content.set_locked(kind, "mod-1.0.jar", true).unwrap();

        let new_bytes = b"v2";
        let new_sha1 = crate::net::to_hex(&<sha1::Sha1 as sha1::Digest>::digest(new_bytes));
        let staging_dir = new_staging_dir(content.store.root());
        let new_temp = safe_join(&staging_dir.join("0"), "mod-2.0.jar").unwrap();
        std::fs::create_dir_all(new_temp.parent().unwrap()).unwrap();
        std::fs::write(&new_temp, new_bytes).unwrap();
        let staged_upd = StagedUpdate {
            prepared: PreparedUpdate {
                entries: vec![PreparedUpdateEntry {
                    update: UpdateInfo {
                        kind,
                        file_name: "mod-1.0.jar".to_owned(),
                        title: "Mod".to_owned(),
                        current_version: Some("1.0".to_owned()),
                        version_id: "2002".to_owned(),
                        version_number: "2.0".to_owned(),
                        source: ContentSource::CurseForge,
                        project_id: Some("1001".to_owned()),
                        delivery: Delivery::Direct,
                        page_url: None,
                    },
                    project_id: "1001".to_owned(),
                    new_name: "mod-2.0.jar".to_owned(),
                    sha1: new_sha1,
                    size: new_bytes.len() as u64,
                    urls: Some(vec!["https://edge.forgecdn.net/mod-2.0.jar".to_owned()]),
                    source: ContentSource::CurseForge,
                }],
                manual: Vec::new(),
            },
            dir: staging_dir,
        };
        content.finish_update(staged_upd, "切换版本").await.unwrap();
        let items = content.list(kind).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file_name, "mod-2.0.jar");
        assert!(items[0].locked, "版本锁应自动迁移到新文件名");
        assert_eq!(items[0].source, Some(IdentitySource::CurseForge));

        let hist_id = content.history()[0].id.clone();
        content.rollback(&hist_id).await.unwrap();
        let restored = content.list(kind).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].file_name, "mod-1.0.jar");
    }
}
