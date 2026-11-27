//! 导出整合包（四格式全支持 + 语义隐私过滤 + 完整文件树 + 原子写盘）。
//!
//! 支持 4 种导出格式（[`ExportFormat`]）：
//! 1. **Modrinth（`.mrpack`）**：SHA-1 / SHA-512 精确外链 + `env` 客户端/服务端标注 + `overrides/`；
//! 2. **CurseForge（`.zip`）**：MurmurHash2 指纹外链写 `manifest.json` + 自动生成 `modlist.html` + `recommendedRam` + `overrides/`；
//! 3. **MultiMC / Prism（`.zip`）**：与 `V1.1-A1` 双向闭环，还原 `mmc-pack.json`、`instance.cfg`、`patches/*.json`、`libraries/` 与 `.minecraft/`；
//! 4. **完整离线包（`.zip`）**：无外链全量内嵌（`modrinth.index.json` + `overrides/`），离线直接导入。
//!
//! 流程拆为两段：
//! 1. [`scan_plan`]：纯本地快速扫描实例文件（跳过全部符号链接与硬黑名单，应用语义隐私分类），界面毫秒级打开；
//! 2. [`match_platforms`]：后台异步匹配 Modrinth SHA-1 与 CurseForge MurmurHash2 外链，不阻塞用户先勾选；
//! 3. [`ExportPlan::write`]：按选定格式与勾选节点流式打包到 `<目标>.part` 后原子改名，取消或失败零残留。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio_util::sync::CancellationToken;

use super::ModpackError;
use crate::content::curseforge::CurseForge;
use crate::content::fingerprint::fingerprint_file;
use crate::content::identify::{IdentityCache, file_key};
use crate::content::modrinth;
use crate::content::store::STORE_DIR;
use crate::instance::{Instance, InstanceOverrides, LoaderKind, LoaderRef, META_DIR};
use crate::meta::patches::{self, InstancePatches, LOCAL_LIBS_DIR};
use crate::paths::GameDir;

const INDEX_FILE: &str = super::mrpack::INDEX_FILE;
const MANIFEST_FILE: &str = super::curseforge::MANIFEST_FILE;
const MODLIST_FILE: &str = "modlist.html";
const MMC_PACK_FILE: &str = super::multimc::PACK_FILE;
const OVERRIDES: &str = "overrides";
const DISABLED_SUFFIX: &str = ".disabled";

/// 顶层强制黑名单（永不扫描、永不导出）：日志、崩溃、启动器元数据、凭据与游戏本体目录
const HARD_EXCLUDE_TOP: &[&str] = &[
    META_DIR,
    "UnnamedLauncher",
    "logs",
    "crash-reports",
    "debug",
    "downloads",
    "backups",
    ".mixin.out",
    ".fabric",
    ".quilt",
    ".cache",
    ".git",
    "__macosx",
    "essential",
    "usercache.json",
    "usernamecache.json",
    "realms_persistence.json",
    "versions",
    "libraries",
    "assets",
    "runtime",
    "instances",
    "PCL",
    "BHL",
    "hmclversion.cfg",
    "launcher_profiles.json",
    "launcher_accounts.json",
    "launcher_settings.json",
    "axolotl_config.json",
];

/// 顶层折叠为单个隐私/缓存节点（默认不勾选，防止上万张小地图瓦片或截图撑爆文件树）
const COLLAPSED_PRIVACY_DIRS: &[&str] = &[
    "screenshots",
    "cache",
    "dynamic-data-pack-cache",
    "dynamic-resource-pack-cache",
    "journeymap",
    "xaero",
    "xaerominimap",
    "xaeroworldmap",
    "xaerowaypoints",
    "voxelmap",
    ".bobby",
    "distant_horizons_server_data",
];

/// 顶层单文件隐私/个人记录（默认不勾选）
const PRIVACY_TOP_FILES: &[&str] = &[
    "command_history.txt",
    "hotbar.nbt",
    "servers.dat",
    "servers.dat_old",
    "patchouli_data.json",
    "emi.json",
];

/// 游戏本体设置文件（归入 GameOptions 语义组）
const GAME_OPTION_FILES: &[&str] = &["options.txt", "optionsof.txt", "optionsshaders.txt"];

/// 模组配置顶层目录（归入 ModConfigs 语义组）
const CONFIG_TOP_DIRS: &[&str] = &[
    "config",
    "defaultconfigs",
    "kubejs",
    "scripts",
    "openloader",
];

/// 单个文件的上限：超过 4 GB 要用 ZIP64，部分启动器不兼容
const MAX_FILE_BYTES: u64 = u32::MAX as u64;

/// 导出整合包的目标格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    /// Modrinth `.mrpack`（SHA-1/SHA-512 外链 + `overrides/`）
    Modrinth,
    /// CurseForge `.zip`（`manifest.json` + `modlist.html` + `overrides/`）
    CurseForge,
    /// MultiMC / Prism `.zip`（`mmc-pack.json` + `instance.cfg` + `patches/` + `.minecraft/`）
    MultiMc,
    /// 完整离线包 `.zip`（无外链全量内嵌）
    FullOffline,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Modrinth => "mrpack",
            ExportFormat::CurseForge | ExportFormat::MultiMc | ExportFormat::FullOffline => "zip",
        }
    }

    pub fn filter_label(self) -> &'static str {
        match self {
            ExportFormat::Modrinth => "Modrinth 整合包（.mrpack）",
            ExportFormat::CurseForge => "CurseForge 整合包（.zip）",
            ExportFormat::MultiMc => "MultiMC / Prism 实例包（.zip）",
            ExportFormat::FullOffline => "完整离线整合包（.zip）",
        }
    }
}

/// 语义分类组（驱动默认层开关与隐私隔离）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ExportGroup {
    Mods,
    DisabledMods,
    ResourcePacks,
    ShaderPacks,
    ModConfigs,
    GameOptions,
    Worlds,
    Privacy,
    Other,
}

/// 一个实际要写入压缩包的物理文件。
#[derive(Debug, Clone)]
struct PackedFile {
    /// 实例内相对路径（`/` 分隔）
    rel: String,
    path: PathBuf,
    size: u64,
}

#[derive(Debug, Clone)]
struct ModrinthLink {
    url: String,
    sha512: String,
}

#[derive(Debug, Clone)]
struct CurseForgeLink {
    project_id: u32,
    file_id: u32,
    project_name: String,
    author: String,
    website_url: Option<String>,
}

/// 一个可被用户勾选/取消勾选的逻辑导出节点（单文件或折叠目录如世界/小地图缓存）。
#[derive(Debug, Clone)]
struct ExportNode {
    key: String,
    label: String,
    group: ExportGroup,
    folder: bool,
    default_on: bool,
    disabled: bool,
    /// 是否为 `mods/resourcepacks/shaderpacks` 内的单个内容包（可参与双平台外链匹配）
    is_content_file: bool,
    sha1: Option<String>,
    modrinth: Option<ModrinthLink>,
    curseforge: Option<CurseForgeLink>,
    files: Vec<PackedFile>,
}

impl ExportNode {
    fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// 完整的导出计划（缓存在后端，避免导出时重复扫描与重复联网匹配）。
#[derive(Debug, Clone)]
pub struct ExportPlan {
    name: String,
    game_version: String,
    loader: Option<LoaderRef>,
    recommended_memory_mb: Option<u32>,
    overrides_config: InstanceOverrides,
    patches: Option<InstancePatches>,
    local_libraries: Vec<PackedFile>,
    notes: Option<String>,
    nodes: Vec<ExportNode>,
}

/// 供界面渲染的单条导出项（同时支撑「语义勾选」与「文件树微调」两层视图）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportItemView {
    pub key: String,
    pub label: String,
    pub group: ExportGroup,
    pub folder: bool,
    pub size: f64,
    pub default_on: bool,
    pub disabled: bool,
    pub is_content_file: bool,
    pub modrinth_linked: bool,
    pub curseforge_linked: bool,
}

/// 本地快速扫描后立即返回给界面的预览数据。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    pub name: String,
    pub game_version: String,
    /// 例如「NeoForge 21.1.265」；原版为空
    pub loader: Option<String>,
    pub recommended_memory_mb: Option<u32>,
    pub patch_count: u32,
    pub has_notes: bool,
    pub curseforge_available: bool,
    pub items: Vec<ExportItemView>,
}

/// 后台异步双平台外链匹配结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportMatchReport {
    pub modrinth_linked: Vec<String>,
    pub curseforge_linked: Vec<String>,
    pub modrinth_error: Option<String>,
    pub curseforge_error: Option<String>,
}

/// 用户确认导出的完整参数。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub recommended_memory_mb: Option<u32>,
    /// 勾选要包含的节点键列表（对应 [`ExportItemView::key`]）
    pub selected_keys: Vec<String>,
}

/// 导出结果摘要。
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub linked: usize,
    pub bundled: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
}

// ---------- Modrinth 清单 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MrIndex<'a> {
    format_version: u32,
    game: &'static str,
    version_id: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    files: Vec<MrIndexFile<'a>>,
    dependencies: BTreeMap<&'static str, &'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MrIndexFile<'a> {
    path: &'a str,
    hashes: BTreeMap<&'static str, &'a str>,
    env: MrFileEnv,
    downloads: [&'a str; 1],
    file_size: u64,
}

#[derive(Serialize)]
struct MrFileEnv {
    client: &'static str,
    server: &'static str,
}

// ---------- CurseForge 清单 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CfManifest<'a> {
    minecraft: CfMinecraft<'a>,
    manifest_type: &'static str,
    manifest_version: u32,
    name: &'a str,
    version: &'a str,
    author: &'a str,
    files: Vec<CfManifestFile>,
    overrides: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CfMinecraft<'a> {
    version: &'a str,
    mod_loaders: Vec<CfModLoader>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended_ram: Option<u32>,
}

#[derive(Serialize)]
struct CfModLoader {
    id: String,
    primary: bool,
}

#[derive(Serialize)]
struct CfManifestFile {
    #[serde(rename = "projectID")]
    project_id: u32,
    #[serde(rename = "fileID")]
    file_id: u32,
    required: bool,
}

// ---------- MultiMC / Prism 清单 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MmcPackOut {
    format_version: u32,
    components: Vec<MmcComponentOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MmcComponentOut {
    uid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_name: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    important: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> ModpackError + '_ {
    move |source| ModpackError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 递归收集文件夹下所有普通文件，严格跳过全部符号链接。
fn walk(
    root: &Path,
    prefix: &str,
    skip: &dyn Fn(&str) -> bool,
) -> Result<Vec<PackedFile>, ModpackError> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), prefix.to_owned())];
    while let Some((dir, rel)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(io_error(&dir))?;
        for entry in entries {
            let entry = entry.map_err(io_error(&dir))?;
            let Ok(name) = entry.file_name().into_string() else {
                tracing::warn!(path = %entry.path().display(), "文件名不是有效 UTF-8，跳过导出");
                continue;
            };
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if skip(&child) {
                continue;
            }
            let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
            if file_type.is_symlink() {
                tracing::warn!(path = %entry.path().display(), "符号链接不导出");
            } else if file_type.is_dir() {
                stack.push((entry.path(), child));
            } else {
                let size = entry.metadata().map_err(io_error(&entry.path()))?.len();
                files.push(PackedFile {
                    rel: child,
                    path: entry.path(),
                    size,
                });
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

fn no_skip(_: &str) -> bool {
    false
}

fn is_session_lock(rel: &str) -> bool {
    rel.rsplit('/')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("session.lock"))
}

/// 判断 `config/`、`local/` 等子路径是否属于玩家个人隐私/收藏/地图缓存（默认不导出）。
fn is_nested_privacy_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    // JEI / REI / EMI 个人收藏与世界历史
    if lower == "config/jei/bookmarks.ini"
        || lower.starts_with("config/jei/world/")
        || lower == "config/jei/world"
        || lower == "config/rei/favorites.json5"
        || lower == "config/emi.json"
        || lower.starts_with("config/journeymap/data/")
        || lower == "config/journeymap/data"
        || lower.starts_with("local/ftbchunks/")
        || lower == "local/ftbchunks"
        || lower.starts_with("local/ftbquests/")
        || lower == "local/ftbquests"
    {
        return true;
    }
    let file_name = lower.rsplit('/').next().unwrap_or(&lower);
    matches!(
        file_name,
        "patchouli_data.json" | "bookmarks.ini" | "favorites.json5"
    )
}

fn sha1_of(path: &Path) -> Result<String, ModpackError> {
    let mut file = std::fs::File::open(path).map_err(io_error(path))?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(io_error(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(crate::net::to_hex(&hasher.finalize()))
}

/// 内容文件夹里的一项算不算内容包：模组认 `.jar`（含 `.jar.disabled`），资源包/光影包认 `.zip`。
fn content_name(folder: &str, name: &str) -> Option<bool> {
    let lower = name.to_ascii_lowercase();
    let (base, disabled) = match lower.strip_suffix(DISABLED_SUFFIX) {
        Some(base) if folder == "mods" => (base.to_owned(), true),
        _ => (lower, false),
    };
    let wanted = if folder == "mods" { ".jar" } else { ".zip" };
    base.ends_with(wanted).then_some(disabled)
}

fn display_name(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.strip_suffix(DISABLED_SUFFIX)
        .unwrap_or(name)
        .to_owned()
}

/// 纯本地扫描实例，毫秒级产出导出计划（尚未联网匹配外链）。
fn scan_sync(game: &GameDir, instance: &Instance) -> Result<ExportPlan, ModpackError> {
    let dir = instance.game_dir(game)?;
    let cache = IdentityCache::load(&game.root().join(STORE_DIR));
    let version_name = instance
        .config
        .link
        .as_ref()
        .map_or(instance.id.as_str(), |l| l.version.as_str());
    let own_json = format!("{version_name}.json").to_lowercase();
    let own_jar = format!("{version_name}.jar").to_lowercase();
    let own_natives = format!("{version_name}-natives").to_lowercase();

    let mut nodes: Vec<ExportNode> = Vec::new();

    // 1. 扫描 mods / resourcepacks / shaderpacks
    for folder in ["mods", "resourcepacks", "shaderpacks"] {
        let root = dir.join(folder);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(ModpackError::Io { path: root, source }),
        };
        for entry in entries {
            let entry = entry.map_err(io_error(&root))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
            if file_type.is_symlink() {
                tracing::warn!(path = %entry.path().display(), "符号链接不导出");
                continue;
            }
            let rel = format!("{folder}/{name}");
            if file_type.is_dir() {
                // 模组目录里的子目录通常是运行缓存（如 .connector），不导出；解压态资源包/光影包作为一个节点
                if folder != "mods" {
                    let files = walk(&entry.path(), &rel, &no_skip)?;
                    if !files.is_empty() {
                        let group = if folder == "resourcepacks" {
                            ExportGroup::ResourcePacks
                        } else {
                            ExportGroup::ShaderPacks
                        };
                        nodes.push(ExportNode {
                            key: rel.clone(),
                            label: name,
                            group,
                            folder: true,
                            default_on: true,
                            disabled: false,
                            is_content_file: false,
                            sha1: None,
                            modrinth: None,
                            curseforge: None,
                            files,
                        });
                    }
                }
                continue;
            }
            let metadata = entry.metadata().map_err(io_error(&entry.path()))?;
            let file = PackedFile {
                rel: rel.clone(),
                path: entry.path(),
                size: metadata.len(),
            };
            match content_name(folder, &name) {
                Some(disabled) => {
                    let sha1 = match cache.hash_of(&file_key(folder, &name, &metadata)) {
                        Some(hash) => hash.clone(),
                        None => sha1_of(&file.path)?,
                    };
                    let (group, default_on) = match folder {
                        "mods" if disabled => (ExportGroup::DisabledMods, false),
                        "mods" => (ExportGroup::Mods, true),
                        "resourcepacks" => (ExportGroup::ResourcePacks, true),
                        _ => (ExportGroup::ShaderPacks, true),
                    };
                    nodes.push(ExportNode {
                        key: rel.clone(),
                        label: display_name(&rel),
                        group,
                        folder: false,
                        default_on,
                        disabled,
                        is_content_file: true,
                        sha1: Some(sha1),
                        modrinth: None,
                        curseforge: None,
                        files: vec![file],
                    });
                }
                // 光影设置（如 `Complementary.zip.txt`）归入光影包组，随光影导出
                None if folder == "shaderpacks" && name.to_ascii_lowercase().ends_with(".txt") => {
                    nodes.push(ExportNode {
                        key: rel,
                        label: name,
                        group: ExportGroup::ShaderPacks,
                        folder: false,
                        default_on: true,
                        disabled: false,
                        is_content_file: false,
                        sha1: None,
                        modrinth: None,
                        curseforge: None,
                        files: vec![file],
                    });
                }
                None if folder == "resourcepacks" => {
                    nodes.push(ExportNode {
                        key: rel,
                        label: name,
                        group: ExportGroup::ResourcePacks,
                        folder: false,
                        default_on: true,
                        disabled: false,
                        is_content_file: false,
                        sha1: None,
                        modrinth: None,
                        curseforge: None,
                        files: vec![file],
                    });
                }
                None => {}
            }
        }
    }

    // 2. 扫描顶层文件与文件夹（严格排除硬黑名单与符号链接）
    let top = std::fs::read_dir(&dir).map_err(io_error(&dir))?;
    for entry in top {
        let entry = entry.map_err(io_error(&dir))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let lower = name.to_lowercase();
        let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
        let skipped = HARD_EXCLUDE_TOP
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&name))
            || lower.starts_with("natives")
            || lower == own_json
            || lower == own_jar
            || lower == own_natives
            || ["mods", "resourcepacks", "shaderpacks"].contains(&lower.as_str())
            || file_type.is_symlink();
        if skipped {
            continue;
        }

        // 单人世界存档：每个世界作为一个独立节点（默认不勾选，自动剔除 session.lock）
        if lower == "saves" && file_type.is_dir() {
            scan_worlds(&entry.path(), &mut nodes)?;
            continue;
        }

        // 顶层折叠型隐私/缓存文件夹（小地图/截图/缓存）→ 单个折叠节点，默认关闭
        if file_type.is_dir()
            && COLLAPSED_PRIVACY_DIRS
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name))
        {
            let files = walk(&entry.path(), &name, &no_skip)?;
            if !files.is_empty() {
                nodes.push(ExportNode {
                    key: name.clone(),
                    label: format!("{name}/"),
                    group: ExportGroup::Privacy,
                    folder: true,
                    default_on: false,
                    disabled: false,
                    is_content_file: false,
                    sha1: None,
                    modrinth: None,
                    curseforge: None,
                    files,
                });
            }
            continue;
        }

        if file_type.is_dir() {
            let is_config_dir = CONFIG_TOP_DIRS
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name));
            let files = walk(&entry.path(), &name, &no_skip)?;
            for f in files {
                let is_privacy = is_nested_privacy_path(&f.rel);
                let (group, default_on) = if is_privacy {
                    (ExportGroup::Privacy, false)
                } else if is_config_dir {
                    (ExportGroup::ModConfigs, true)
                } else {
                    (ExportGroup::Other, true)
                };
                nodes.push(ExportNode {
                    key: f.rel.clone(),
                    label: f.rel.clone(),
                    group,
                    folder: false,
                    default_on,
                    disabled: false,
                    is_content_file: false,
                    sha1: None,
                    modrinth: None,
                    curseforge: None,
                    files: vec![f],
                });
            }
        } else {
            let size = entry.metadata().map_err(io_error(&entry.path()))?.len();
            let file = PackedFile {
                rel: name.clone(),
                path: entry.path(),
                size,
            };
            let (group, default_on) = if PRIVACY_TOP_FILES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name))
            {
                (ExportGroup::Privacy, false)
            } else if GAME_OPTION_FILES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name))
            {
                (ExportGroup::GameOptions, true)
            } else {
                (ExportGroup::Other, true)
            };
            nodes.push(ExportNode {
                key: name.clone(),
                label: name,
                group,
                folder: false,
                default_on,
                disabled: false,
                is_content_file: false,
                sha1: None,
                modrinth: None,
                curseforge: None,
                files: vec![file],
            });
        }
    }

    // 3. 读取 MultiMC/Prism 双向闭环所需的实例补丁、本地补丁库与实例笔记
    let meta_root = game
        .instance_dir(&instance.id)
        .unwrap_or_else(|_| dir.clone());
    let patches = patches::load_for_instance(&meta_root).ok().flatten();
    let local_libs_dir = meta_root.join(META_DIR).join(LOCAL_LIBS_DIR);
    let local_libraries = if local_libs_dir.is_dir() {
        walk(&local_libs_dir, "", &no_skip).unwrap_or_default()
    } else {
        Vec::new()
    };
    let notes = std::fs::read_to_string(meta_root.join(META_DIR).join("notes.md"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    nodes.sort_by_key(|a| a.key.to_lowercase());

    Ok(ExportPlan {
        name: instance.config.name.clone(),
        game_version: instance.config.game_version.clone(),
        loader: instance.config.loader.clone(),
        recommended_memory_mb: instance.config.recommended_memory_mb,
        overrides_config: instance.config.overrides.clone(),
        patches,
        local_libraries,
        notes,
        nodes,
    })
}

fn scan_worlds(saves: &Path, nodes: &mut Vec<ExportNode>) -> Result<(), ModpackError> {
    let instance_dir = saves.parent().unwrap_or(saves);
    let names: BTreeMap<String, String> = crate::saves::list(instance_dir, "")
        .into_iter()
        .map(|w| (w.folder, w.name))
        .collect();
    for entry in std::fs::read_dir(saves).map_err(io_error(saves))? {
        let entry = entry.map_err(io_error(saves))?;
        let Ok(folder) = entry.file_name().into_string() else {
            continue;
        };
        let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let key = format!("saves/{folder}");
        let files = walk(&entry.path(), &key, &is_session_lock)?;
        if files.is_empty() {
            continue;
        }
        let world_name = names
            .get(&folder)
            .cloned()
            .unwrap_or_else(|| folder.clone());
        let label = if world_name == folder {
            world_name
        } else {
            format!("{world_name}（{folder}）")
        };
        nodes.push(ExportNode {
            key,
            label,
            group: ExportGroup::Worlds,
            folder: true,
            default_on: false,
            disabled: false,
            is_content_file: false,
            sha1: None,
            modrinth: None,
            curseforge: None,
            files,
        });
    }
    Ok(())
}

/// 第一步：本地快速扫描，立即产出可展示的 [`ExportPlan`]。
pub async fn scan_plan(game: &GameDir, instance: &Instance) -> Result<ExportPlan, ModpackError> {
    let scan_game = game.clone();
    let scan_instance = instance.clone();
    tokio::task::spawn_blocking(move || scan_sync(&scan_game, &scan_instance))
        .await
        .map_err(|e| ModpackError::Invalid(format!("读取实例文件的任务异常结束：{e}")))?
}

/// 第二步：后台异步并行匹配 Modrinth（SHA-1）与 CurseForge（MurmurHash2）外链。
/// 任一平台不可达或未配置 Key 时不阻塞另一平台或离线/MultiMC 格式导出。
pub async fn match_platforms(
    client: &Client,
    curseforge: &CurseForge,
    plan: &mut ExportPlan,
) -> ExportMatchReport {
    let content_indices: Vec<usize> = plan
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.is_content_file && !n.disabled && n.files.len() == 1)
        .map(|(i, _)| i)
        .collect();

    if content_indices.is_empty() {
        return ExportMatchReport {
            modrinth_linked: Vec::new(),
            curseforge_linked: Vec::new(),
            modrinth_error: None,
            curseforge_error: None,
        };
    }

    // 1. Modrinth SHA-1 批量匹配
    let hashes: Vec<String> = content_indices
        .iter()
        .filter_map(|&i| plan.nodes[i].sha1.clone())
        .collect();
    let mr_fut = modrinth::files_by_sha1(client, &hashes);

    // 2. CurseForge MurmurHash2 指纹计算与批量匹配（仅当配置了 API Key 时执行）
    let cf_paths: Vec<(usize, PathBuf)> = if curseforge.usable() {
        content_indices
            .iter()
            .filter_map(|&i| plan.nodes[i].files.first().map(|f| (i, f.path.clone())))
            .collect()
    } else {
        Vec::new()
    };
    let cf_fut = async {
        if cf_paths.is_empty() {
            return Ok::<_, crate::content::ContentError>(HashMap::new());
        }
        let computed = tokio::task::spawn_blocking(move || {
            let mut map = HashMap::with_capacity(cf_paths.len());
            for (idx, path) in cf_paths {
                if let Ok(fp) = fingerprint_file(&path) {
                    map.insert(idx, fp);
                }
            }
            map
        })
        .await
        .unwrap_or_default();
        let fps: Vec<u32> = computed.values().copied().collect();
        if fps.is_empty() {
            return Ok(HashMap::new());
        }
        let matched_files = curseforge.identify(&fps).await?;
        let mut mod_ids: Vec<u32> = matched_files.values().map(|f| f.mod_id).collect();
        mod_ids.sort_unstable();
        mod_ids.dedup();
        let mods_meta = if mod_ids.is_empty() {
            HashMap::new()
        } else {
            curseforge.mods(&mod_ids).await.unwrap_or_default()
        };
        let mut by_index = HashMap::new();
        for (idx, fp) in computed {
            if let Some(file) = matched_files.get(&fp) {
                let meta = mods_meta.get(&file.mod_id);
                let project_name = meta
                    .map(|m| m.name.clone())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| file.display_name.clone());
                let author = meta
                    .and_then(|m| m.authors.first())
                    .map(|a| a.name.clone())
                    .unwrap_or_default();
                let website_url = meta.and_then(|m| m.links.website_url.clone());
                by_index.insert(
                    idx,
                    CurseForgeLink {
                        project_id: file.mod_id,
                        file_id: file.id,
                        project_name,
                        author,
                        website_url,
                    },
                );
            }
        }
        Ok(by_index)
    };

    let (mr_res, cf_res) = tokio::join!(mr_fut, cf_fut);

    let mut modrinth_linked = Vec::new();
    let mut modrinth_error = None;
    match mr_res {
        Ok(found) => {
            for &idx in &content_indices {
                let node = &mut plan.nodes[idx];
                let Some(sha1) = node.sha1.as_deref() else {
                    continue;
                };
                let Some(file) = found.get(sha1) else {
                    continue;
                };
                let sha512 = file.hashes.get("sha512").map(|h| h.to_ascii_lowercase());
                let expected_size = node.files.first().map_or(0, |f| f.size);
                if let Some(sha512) = sha512
                    && file.size == expected_size
                {
                    node.modrinth = Some(ModrinthLink {
                        url: file.url.clone(),
                        sha512,
                    });
                    modrinth_linked.push(node.key.clone());
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, "导出整合包时查询 Modrinth 外链失败");
            modrinth_error = Some(error.to_string());
        }
    }

    let mut curseforge_linked = Vec::new();
    let mut curseforge_error = None;
    match cf_res {
        Ok(found) => {
            for (idx, link) in found {
                if let Some(node) = plan.nodes.get_mut(idx) {
                    node.curseforge = Some(link);
                    curseforge_linked.push(node.key.clone());
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, "导出整合包时查询 CurseForge 外链失败");
            curseforge_error = Some(error.to_string());
        }
    }

    ExportMatchReport {
        modrinth_linked,
        curseforge_linked,
        modrinth_error,
        curseforge_error,
    }
}

impl ExportPlan {
    pub fn preview(&self, curseforge_available: bool) -> ExportPreview {
        let patch_count = self
            .patches
            .as_ref()
            .map_or(0, |b| u32::try_from(b.patches.len()).unwrap_or(u32::MAX));
        ExportPreview {
            name: self.name.clone(),
            game_version: self.game_version.clone(),
            loader: self
                .loader
                .as_ref()
                .map(|l| format!("{} {}", l.kind.label(), l.version)),
            recommended_memory_mb: self.recommended_memory_mb,
            patch_count,
            has_notes: self.notes.is_some(),
            curseforge_available,
            items: self
                .nodes
                .iter()
                .map(|n| ExportItemView {
                    key: n.key.clone(),
                    label: n.label.clone(),
                    group: n.group,
                    folder: n.folder,
                    size: n.size() as f64,
                    default_on: n.default_on,
                    disabled: n.disabled,
                    is_content_file: n.is_content_file,
                    modrinth_linked: n.modrinth.is_some(),
                    curseforge_linked: n.curseforge.is_some(),
                })
                .collect(),
        }
    }

    /// 按选定格式写出整合包（在后台阻塞线程调用），中途取消或失败原子清理 `.part` 文件。
    pub fn write(
        &self,
        options: &ExportOptions,
        dest: &Path,
        cancel: &CancellationToken,
        on_progress: impl Fn(ExportProgress),
    ) -> Result<ExportSummary, ModpackError> {
        let name = options.name.trim();
        let name = if name.is_empty() {
            self.name.as_str()
        } else {
            name
        };
        let version = options.version.trim();
        let version = if version.is_empty() { "1.0.0" } else { version };
        let author = options
            .author
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Lumi Player");
        let summary = options
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let selected: HashSet<&str> = options.selected_keys.iter().map(String::as_str).collect();

        let selected_nodes: Vec<&ExportNode> = self
            .nodes
            .iter()
            .filter(|n| selected.contains(n.key.as_str()))
            .collect();

        let mut extra_entries: Vec<(String, Vec<u8>)> = Vec::new();
        let mut packed: Vec<(String, &PackedFile)> = Vec::new();
        let mut linked_count = 0usize;

        match options.format {
            ExportFormat::Modrinth => {
                let mut index_files = Vec::new();
                for node in &selected_nodes {
                    if !node.disabled
                        && let (Some(link), Some(sha1), Some(file)) =
                            (&node.modrinth, node.sha1.as_deref(), node.files.first())
                    {
                        linked_count += 1;
                        let server_env = if matches!(
                            node.group,
                            ExportGroup::ResourcePacks | ExportGroup::ShaderPacks
                        ) {
                            "unsupported"
                        } else {
                            "required"
                        };
                        index_files.push(MrIndexFile {
                            path: &file.rel,
                            hashes: BTreeMap::from([
                                ("sha1", sha1),
                                ("sha512", link.sha512.as_str()),
                            ]),
                            env: MrFileEnv {
                                client: "required",
                                server: server_env,
                            },
                            downloads: [link.url.as_str()],
                            file_size: file.size,
                        });
                    } else {
                        for f in &node.files {
                            packed.push((format!("{OVERRIDES}/{}", f.rel), f));
                        }
                    }
                }
                let index_json = build_mr_index(
                    name,
                    version,
                    summary,
                    &self.game_version,
                    self.loader.as_ref(),
                    index_files,
                )?;
                extra_entries.push((INDEX_FILE.to_owned(), index_json));
            }

            ExportFormat::CurseForge => {
                let mut cf_files = Vec::new();
                let mut modlist_items: Vec<&CurseForgeLink> = Vec::new();
                for node in &selected_nodes {
                    if !node.disabled
                        && let Some(link) = &node.curseforge
                    {
                        linked_count += 1;
                        cf_files.push(CfManifestFile {
                            project_id: link.project_id,
                            file_id: link.file_id,
                            required: true,
                        });
                        modlist_items.push(link);
                    } else {
                        for f in &node.files {
                            packed.push((format!("{OVERRIDES}/{}", f.rel), f));
                        }
                    }
                }
                let mod_loaders = self
                    .loader
                    .as_ref()
                    .map(|l| {
                        let clean_ver = canonical_loader_version(&self.game_version, l);
                        vec![CfModLoader {
                            id: format!("{}-{clean_ver}", l.kind.slug()),
                            primary: true,
                        }]
                    })
                    .unwrap_or_default();
                let manifest = CfManifest {
                    minecraft: CfMinecraft {
                        version: &self.game_version,
                        mod_loaders,
                        recommended_ram: options
                            .recommended_memory_mb
                            .or(self.recommended_memory_mb),
                    },
                    manifest_type: "minecraftModpack",
                    manifest_version: 1,
                    name,
                    version,
                    author,
                    files: cf_files,
                    overrides: OVERRIDES,
                };
                let mut manifest_json = serde_json::to_vec_pretty(&manifest)
                    .map_err(|e| ModpackError::Invalid(format!("生成 manifest.json 失败：{e}")))?;
                manifest_json.push(b'\n');
                extra_entries.push((MANIFEST_FILE.to_owned(), manifest_json));
                extra_entries.push((
                    MODLIST_FILE.to_owned(),
                    build_modlist_html(&modlist_items).into_bytes(),
                ));
            }

            ExportFormat::MultiMc => {
                let (mmc_pack_json, patch_files) = build_mmc_pack(
                    &self.game_version,
                    self.loader.as_ref(),
                    self.patches.as_ref(),
                )?;
                extra_entries.push((MMC_PACK_FILE.to_owned(), mmc_pack_json));
                let cfg_text = build_instance_cfg(
                    name,
                    &self.overrides_config,
                    options.recommended_memory_mb.or(self.recommended_memory_mb),
                    self.notes.as_deref().or(summary),
                );
                extra_entries.push(("instance.cfg".to_owned(), cfg_text.into_bytes()));
                extra_entries.extend(patch_files);

                for lib in &self.local_libraries {
                    packed.push((format!("libraries/{}", lib.rel), lib));
                }
                for node in &selected_nodes {
                    for f in &node.files {
                        packed.push((format!(".minecraft/{}", f.rel), f));
                    }
                }
            }

            ExportFormat::FullOffline => {
                for node in &selected_nodes {
                    for f in &node.files {
                        packed.push((format!("{OVERRIDES}/{}", f.rel), f));
                    }
                }
                let index_json = build_mr_index(
                    name,
                    version,
                    summary,
                    &self.game_version,
                    self.loader.as_ref(),
                    Vec::new(),
                )?;
                extra_entries.push((INDEX_FILE.to_owned(), index_json));
            }
        }

        if let Some((_, big)) = packed.iter().find(|(_, f)| f.size > MAX_FILE_BYTES) {
            return Err(ModpackError::Invalid(format!(
                "{} 超过 4 GB，整合包里放不下",
                big.rel
            )));
        }

        let total_bytes = packed.iter().map(|(_, f)| f.size).sum::<u64>()
            + extra_entries
                .iter()
                .map(|(_, b)| b.len() as u64)
                .sum::<u64>();
        let mut progress = ExportProgress {
            done_bytes: 0,
            total_bytes,
        };
        on_progress(progress);

        let part = part_path(dest);
        let result = write_archive(&part, &extra_entries, &packed, cancel, |n| {
            progress.done_bytes += n;
            on_progress(progress);
        })
        .and_then(|()| std::fs::rename(&part, dest).map_err(io_error(dest)));
        if let Err(error) = result {
            if let Err(cleanup) = std::fs::remove_file(&part)
                && cleanup.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %cleanup, file = %part.display(), "删除导出到一半的文件失败");
            }
            return Err(error);
        }
        let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        tracing::info!(
            format = ?options.format,
            file = %dest.display(),
            bytes,
            linked = linked_count,
            packed = packed.len(),
            "整合包已导出"
        );
        Ok(ExportSummary {
            path: dest.to_path_buf(),
            bytes,
            linked: linked_count,
            bundled: packed.len(),
        })
    }
}

/// 去掉可能冗余的 `<游戏版本>-` 前缀（例如 Forge `1.20.1-47.3.0` → `47.3.0`），
/// 保证 Modrinth / CurseForge / MultiMC 清单都能被各类启动器正确解析。
fn canonical_loader_version<'a>(game_version: &str, loader: &'a LoaderRef) -> &'a str {
    match loader.kind {
        LoaderKind::Forge => loader
            .version
            .strip_prefix(game_version)
            .and_then(|rest| rest.strip_prefix('-'))
            .filter(|v| !v.is_empty())
            .unwrap_or(&loader.version),
        LoaderKind::NeoForge => loader
            .version
            .strip_prefix("1.20.1-")
            .filter(|v| v.starts_with("47.1."))
            .unwrap_or(&loader.version),
        LoaderKind::Fabric | LoaderKind::Quilt | LoaderKind::LegacyFabric | LoaderKind::Babric => {
            &loader.version
        }
    }
}

fn build_mr_index<'a>(
    name: &'a str,
    version: &'a str,
    summary: Option<&'a str>,
    game_version: &'a str,
    loader: Option<&'a LoaderRef>,
    files: Vec<MrIndexFile<'a>>,
) -> Result<Vec<u8>, ModpackError> {
    let mut dependencies = BTreeMap::from([("minecraft", game_version)]);
    if let Some(loader) = loader {
        let key = match loader.kind {
            LoaderKind::NeoForge => "neoforge",
            LoaderKind::Fabric => "fabric-loader",
            LoaderKind::Forge => "forge",
            LoaderKind::Quilt => "quilt-loader",
            LoaderKind::LegacyFabric => "legacy-fabric-loader",
            LoaderKind::Babric => "babric-loader",
        };
        dependencies.insert(key, canonical_loader_version(game_version, loader));
    }
    let index = MrIndex {
        format_version: 1,
        game: "minecraft",
        version_id: version,
        name,
        summary,
        files,
        dependencies,
    };
    let mut out = serde_json::to_vec_pretty(&index)
        .map_err(|e| ModpackError::Invalid(format!("生成 modrinth.index.json 失败：{e}")))?;
    out.push(b'\n');
    Ok(out)
}

fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn build_modlist_html(links: &[&CurseForgeLink]) -> String {
    let mut sorted: Vec<&CurseForgeLink> = links.to_vec();
    sorted.sort_by(|a, b| {
        a.project_name
            .to_lowercase()
            .cmp(&b.project_name.to_lowercase())
    });
    let mut html = String::from("<ul>\n");
    for item in sorted {
        let name = escape_html(&item.project_name);
        let author = escape_html(&item.author);
        let label = if author.is_empty() {
            name
        } else {
            format!("{name} (by {author})")
        };
        if let Some(url) = item
            .website_url
            .as_deref()
            .filter(|u| u.starts_with("https://"))
        {
            html.push_str(&format!(
                "<li><a href=\"{}\">{label}</a></li>\n",
                escape_html(url)
            ));
        } else {
            html.push_str(&format!("<li>{label}</li>\n"));
        }
    }
    html.push_str("</ul>\n");
    html
}

type MmcPackOutput = (Vec<u8>, Vec<(String, Vec<u8>)>);

fn build_mmc_pack(
    game_version: &str,
    loader: Option<&LoaderRef>,
    patches_bundle: Option<&InstancePatches>,
) -> Result<MmcPackOutput, ModpackError> {
    let mut components = vec![MmcComponentOut {
        uid: "net.minecraft".to_owned(),
        version: Some(game_version.to_owned()),
        cached_name: Some("Minecraft".to_owned()),
        important: true,
    }];
    if let Some(loader) = loader {
        let (uid, cached_name) = match loader.kind {
            LoaderKind::NeoForge => ("net.neoforge", "NeoForge"),
            LoaderKind::Fabric => ("net.fabricmc.fabric-loader", "Fabric Loader"),
            LoaderKind::Forge => ("net.minecraftforge", "Forge"),
            LoaderKind::Quilt => ("org.quiltmc.quilt-loader", "Quilt Loader"),
            LoaderKind::LegacyFabric => ("net.fabricmc.fabric-loader", "Legacy Fabric"),
            LoaderKind::Babric => ("net.fabricmc.fabric-loader", "Babric Loader"),
        };
        components.push(MmcComponentOut {
            uid: uid.to_owned(),
            version: Some(canonical_loader_version(game_version, loader).to_owned()),
            cached_name: Some(cached_name.to_owned()),
            important: false,
        });
    }

    let mut patch_files = Vec::new();
    if let Some(bundle) = patches_bundle {
        if bundle.replace_lwjgl2
            && !bundle
                .patches
                .iter()
                .any(|p| p.uid.eq_ignore_ascii_case("org.lwjgl3"))
        {
            components.push(MmcComponentOut {
                uid: "org.lwjgl3".to_owned(),
                version: None,
                cached_name: Some("LWJGL 3".to_owned()),
                important: false,
            });
        }
        for patch in &bundle.patches {
            if !components
                .iter()
                .any(|c| c.uid.eq_ignore_ascii_case(&patch.uid))
            {
                components.push(MmcComponentOut {
                    uid: patch.uid.clone(),
                    version: patch.version.clone(),
                    cached_name: patch.name.clone(),
                    important: false,
                });
            }
            let mut bytes = serde_json::to_vec_pretty(patch).map_err(|e| {
                ModpackError::Invalid(format!("生成补丁 patches/{}.json 失败：{e}", patch.uid))
            })?;
            bytes.push(b'\n');
            patch_files.push((format!("patches/{}.json", patch.uid), bytes));
        }
    }

    let pack = MmcPackOut {
        format_version: 1,
        components,
    };
    let mut pack_json = serde_json::to_vec_pretty(&pack)
        .map_err(|e| ModpackError::Invalid(format!("生成 mmc-pack.json 失败：{e}")))?;
    pack_json.push(b'\n');
    Ok((pack_json, patch_files))
}

fn build_instance_cfg(
    name: &str,
    overrides: &InstanceOverrides,
    recommended_ram_mb: Option<u32>,
    notes: Option<&str>,
) -> String {
    let mut lines = vec![
        "InstanceType=OneSix".to_owned(),
        format!("name={}", name.replace(['\r', '\n'], " ")),
    ];
    let max_mem = match &overrides.memory {
        Some(mem) if !mem.auto => Some(mem.max_memory_mb),
        _ => recommended_ram_mb,
    };
    if let Some(max_mb) = max_mem {
        lines.push("OverrideMemory=true".to_owned());
        lines.push(format!("MinMemAlloc={}", (max_mb / 2).max(512)));
        lines.push(format!("MaxMemAlloc={max_mb}"));
    }
    if let Some(launch) = &overrides.launch
        && !launch.jvm_args.trim().is_empty()
    {
        lines.push("OverrideJavaArgs=true".to_owned());
        lines.push(format!("JvmArgs={}", launch.jvm_args.trim()));
    }
    if let Some(window) = &overrides.window {
        lines.push("OverrideWindow=true".to_owned());
        lines.push(format!("MinecraftWinWidth={}", window.width));
        lines.push(format!("MinecraftWinHeight={}", window.height));
    }
    if let Some(notes) = notes {
        let escaped = notes.replace('\\', "\\\\").replace('\n', "\\n");
        lines.push(format!("notes={escaped}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// 已经压过的文件（模组、压缩包、图片）只存不压，省时间
fn compressed_already(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let lower = lower.strip_suffix(DISABLED_SUFFIX).unwrap_or(&lower);
    [
        ".jar", ".zip", ".png", ".jpg", ".jpeg", ".ogg", ".mca", ".dat", ".gz",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn write_archive(
    part: &Path,
    extra_entries: &[(String, Vec<u8>)],
    packed: &[(String, &PackedFile)],
    cancel: &CancellationToken,
    mut advance: impl FnMut(u64),
) -> Result<(), ModpackError> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let zip_error = |e: zip::result::ZipError| ModpackError::Io {
        path: part.to_path_buf(),
        source: std::io::Error::other(e),
    };
    let file = std::fs::File::create(part).map_err(io_error(part))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let store = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for (entry_name, bytes) in extra_entries {
        if cancel.is_cancelled() {
            return Err(ModpackError::Cancelled);
        }
        zip.start_file(entry_name, deflate).map_err(zip_error)?;
        zip.write_all(bytes).map_err(io_error(part))?;
        advance(bytes.len() as u64);
    }

    let mut buffer = vec![0u8; 256 * 1024];
    for (entry_name, file) in packed {
        if cancel.is_cancelled() {
            return Err(ModpackError::Cancelled);
        }
        let options = if compressed_already(&file.rel) {
            store
        } else {
            deflate
        };
        zip.start_file(entry_name, options).map_err(zip_error)?;
        let mut source = std::fs::File::open(&file.path).map_err(io_error(&file.path))?;
        loop {
            let n = source.read(&mut buffer).map_err(io_error(&file.path))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buffer[..n]).map_err(io_error(part))?;
            advance(n as u64);
            if cancel.is_cancelled() {
                return Err(ModpackError::Cancelled);
            }
        }
    }
    let writer = zip.finish().map_err(zip_error)?;
    let file = writer.into_inner().map_err(|e| ModpackError::Io {
        path: part.to_path_buf(),
        source: e.into_error(),
    })?;
    file.sync_all().map_err(io_error(part))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{InstanceConfig, create};
    use crate::meta::patches::MetaPatch;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn sample(game: &GameDir) -> Instance {
        let dir = game.instance_dir("Pack").unwrap();
        let mut config = InstanceConfig::new(
            "我的整合包",
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.265".to_owned(),
            }),
        );
        config.recommended_memory_mb = Some(4096);
        let instance = create(game, "Pack", &config).unwrap();
        write(&dir.join("mods/on-both.jar"), b"both-mod");
        write(&dir.join("mods/only-here.jar.disabled"), b"local-disabled");
        write(&dir.join("mods/.connector/cache.bin"), b"cache");
        write(&dir.join("mods/readme.txt"), b"x");
        write(&dir.join("resourcepacks/Folder Pack/pack.mcmeta"), b"{}");
        write(&dir.join("shaderpacks/Complementary.zip"), b"shader-zip");
        write(&dir.join("shaderpacks/Complementary.zip.txt"), b"setting=1");
        write(&dir.join("config/a.toml"), b"x=1");
        write(&dir.join("config/jei/bookmarks.ini"), b"secret-bookmark");
        write(&dir.join("patchouli_data.json"), b"{\"viewed\":true}");
        write(&dir.join("options.txt"), b"lang:zh_cn");
        write(&dir.join("journeymap/data/tile.png"), b"tile");
        write(&dir.join("screenshots/shot.png"), b"png");
        write(&dir.join("command_history.txt"), b"/gamemode creative");
        write(&dir.join("servers.dat"), b"servers");
        write(&dir.join("logs/latest.log"), b"log");
        write(&dir.join("essential/account.json"), b"secret");
        write(&dir.join("saves/World/level.dat"), b"nbt");
        write(&dir.join("saves/World/session.lock"), b"lock");
        write(&dir.join("Lumi/content.json"), b"{}");
        write(&dir.join("Lumi/notes.md"), b"Pack notes line 1\nLine 2");
        write(&dir.join("Lumi/libraries/custom-patch.jar"), b"patch-lib");
        let bundle = InstancePatches::new(
            vec![MetaPatch {
                format_version: Some(1),
                uid: "org.example.custom".to_owned(),
                name: Some("Custom Patch".to_owned()),
                version: Some("1.0".to_owned()),
                main_class: None,
                minecraft_arguments: None,
                tweakers: vec!["org.example.Tweaker".to_owned()],
                jvm_args: vec!["-Dexample=true".to_owned()],
                traits: Vec::new(),
                compatible_java_majors: vec![21],
                main_jar: None,
                libraries: Vec::new(),
                maven_files: Vec::new(),
            }],
            false,
        );
        patches::save_for_instance(&dir, &bundle).unwrap();
        instance
    }

    fn planned(game: &GameDir, instance: &Instance) -> ExportPlan {
        let mut plan = scan_sync(game, instance).unwrap();
        for node in &mut plan.nodes {
            if node.key == "mods/on-both.jar" {
                node.modrinth = Some(ModrinthLink {
                    url: "https://cdn.modrinth.com/data/AAAA/versions/BBBB/on-both.jar".to_owned(),
                    sha512: "ab".repeat(64),
                });
                node.curseforge = Some(CurseForgeLink {
                    project_id: 12345,
                    file_id: 67890,
                    project_name: "On Both <Mod>".to_owned(),
                    author: "Kokoa & Co".to_owned(),
                    website_url: Some(
                        "https://www.curseforge.com/minecraft/mc-mods/on-both".to_owned(),
                    ),
                });
            }
        }
        plan
    }

    #[test]
    fn preview_applies_semantic_privacy_filter_and_hard_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let preview = planned(&game, &instance).preview(true);

        assert_eq!(preview.name, "我的整合包");
        assert_eq!(preview.patch_count, 1);
        assert!(preview.has_notes);
        assert_eq!(preview.recommended_memory_mb, Some(4096));

        let by_key = |k: &str| preview.items.iter().find(|i| i.key == k);
        // 正常核心内容默认勾选
        assert!(by_key("mods/on-both.jar").unwrap().default_on);
        assert!(by_key("mods/on-both.jar").unwrap().modrinth_linked);
        assert!(by_key("mods/on-both.jar").unwrap().curseforge_linked);
        assert!(by_key("config/a.toml").unwrap().default_on);
        assert!(by_key("options.txt").unwrap().default_on);
        assert!(
            by_key("shaderpacks/Complementary.zip.txt")
                .unwrap()
                .default_on
        );

        // 停用模组、世界默认不勾选
        assert!(!by_key("mods/only-here.jar.disabled").unwrap().default_on);
        assert!(!by_key("saves/World").unwrap().default_on);

        // 隐私与个人缓存归入 Privacy 且默认不勾选
        for privacy_key in [
            "config/jei/bookmarks.ini",
            "patchouli_data.json",
            "journeymap",
            "screenshots",
            "command_history.txt",
            "servers.dat",
        ] {
            let item = by_key(privacy_key).expect(privacy_key);
            assert_eq!(item.group, ExportGroup::Privacy, "{privacy_key}");
            assert!(!item.default_on, "{privacy_key} 默认必须关闭");
        }

        // 硬黑名单永不出现
        for banned in [
            "Lumi/content.json",
            "logs/latest.log",
            "essential/account.json",
        ] {
            assert!(by_key(banned).is_none(), "{banned} 绝不能出现");
        }
    }

    #[test]
    fn writes_all_four_formats_and_round_trips_through_importers() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let plan = planned(&game, &instance);
        let default_keys: Vec<String> = plan
            .nodes
            .iter()
            .filter(|n| n.default_on)
            .map(|n| n.key.clone())
            .collect();

        // 1. Modrinth (.mrpack)
        let mr_dest = dir.path().join("out.mrpack");
        let mr_sum = plan
            .write(
                &ExportOptions {
                    format: ExportFormat::Modrinth,
                    name: " 导出的包 ".to_owned(),
                    version: "1.2.0".to_owned(),
                    author: Some("Tester".to_owned()),
                    summary: Some("A great pack".to_owned()),
                    recommended_memory_mb: Some(6144),
                    selected_keys: default_keys.clone(),
                },
                &mr_dest,
                &CancellationToken::new(),
                |_| {},
            )
            .unwrap();
        assert_eq!(mr_sum.linked, 1);
        let mr_inspected = crate::modpack::mrpack::inspect(&mr_dest).unwrap();
        assert_eq!(mr_inspected.preview.name, "导出的包");
        assert_eq!(mr_inspected.preview.version, "1.2.0");
        assert_eq!(mr_inspected.preview.file_count, 1);
        assert!(
            !mr_inspected
                .overrides
                .iter()
                .any(|(_, rel)| rel.contains("bookmarks.ini") || rel.contains("patchouli_data"))
        );

        // 2. CurseForge (.zip + modlist.html)
        let cf_dest = dir.path().join("out-cf.zip");
        let cf_sum = plan
            .write(
                &ExportOptions {
                    format: ExportFormat::CurseForge,
                    name: "CF Pack".to_owned(),
                    version: "2.0.0".to_owned(),
                    author: Some("Kokoa".to_owned()),
                    summary: None,
                    recommended_memory_mb: Some(6144),
                    selected_keys: default_keys.clone(),
                },
                &cf_dest,
                &CancellationToken::new(),
                |_| {},
            )
            .unwrap();
        assert_eq!(cf_sum.linked, 1);
        let cf_inspected = crate::modpack::curseforge::inspect(&cf_dest).unwrap();
        assert_eq!(cf_inspected.name(), "CF Pack");
        let mut cf_zip = zip::ZipArchive::new(std::fs::File::open(&cf_dest).unwrap()).unwrap();
        let mut modlist = String::new();
        cf_zip
            .by_name(MODLIST_FILE)
            .unwrap()
            .read_to_string(&mut modlist)
            .unwrap();
        assert!(modlist.contains("On Both &lt;Mod&gt; (by Kokoa &amp; Co)"));

        // 3. MultiMC / Prism (.zip，含 patches/*.json、libraries/ 与 notes)
        let mmc_dest = dir.path().join("out-mmc.zip");
        plan.write(
            &ExportOptions {
                format: ExportFormat::MultiMc,
                name: "Prism Pack".to_owned(),
                version: "1.0.0".to_owned(),
                author: None,
                summary: None,
                recommended_memory_mb: Some(4096),
                selected_keys: default_keys.clone(),
            },
            &mmc_dest,
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();
        let mmc_inspected = crate::modpack::multimc::inspect(&mmc_dest).unwrap();
        assert_eq!(mmc_inspected.preview.name, "Prism Pack");
        assert_eq!(mmc_inspected.preview.patch_count, 1);
        assert_eq!(mmc_inspected.preview.recommended_memory_mb, Some(4096));

        // 4. 完整离线包 (.zip，无外链全量内嵌，且可被本启动器重新识别导入)
        let offline_dest = dir.path().join("out-offline.zip");
        let off_sum = plan
            .write(
                &ExportOptions {
                    format: ExportFormat::FullOffline,
                    name: "Offline Pack".to_owned(),
                    version: "1.0.0".to_owned(),
                    author: None,
                    summary: None,
                    recommended_memory_mb: None,
                    selected_keys: default_keys,
                },
                &offline_dest,
                &CancellationToken::new(),
                |_| {},
            )
            .unwrap();
        assert_eq!(off_sum.linked, 0);
        assert_eq!(
            crate::modpack::detect(&offline_dest).unwrap(),
            crate::modpack::PackFormat::Modrinth
        );
        let off_inspected = crate::modpack::mrpack::inspect(&offline_dest).unwrap();
        assert_eq!(off_inspected.preview.file_count, 0);
        assert!(
            off_inspected
                .overrides
                .iter()
                .any(|(_, rel)| rel == "mods/on-both.jar")
        );
    }

    #[test]
    fn cancelled_export_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let plan = planned(&game, &instance);
        let dest = dir.path().join("out.mrpack");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let options = ExportOptions {
            format: ExportFormat::Modrinth,
            name: String::new(),
            version: "2.0".to_owned(),
            author: None,
            summary: None,
            recommended_memory_mb: None,
            selected_keys: Vec::new(),
        };
        assert!(matches!(
            plan.write(&options, &dest, &cancel, |_| {}),
            Err(ModpackError::Cancelled)
        ));
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[test]
    fn recognizes_content_files() {
        assert_eq!(content_name("mods", "a.jar"), Some(false));
        assert_eq!(content_name("mods", "a.JAR.disabled"), Some(true));
        assert_eq!(content_name("mods", "a.txt"), None);
        assert_eq!(content_name("shaderpacks", "s.zip"), Some(false));
        assert_eq!(content_name("shaderpacks", "s.zip.txt"), None);
        assert_eq!(content_name("resourcepacks", "r.zip.disabled"), None);
    }

    #[test]
    fn normalizes_loader_versions_and_marks_client_packs_server_unsupported() {
        let forge = LoaderRef {
            kind: LoaderKind::Forge,
            version: "1.20.1-47.3.0".to_owned(),
        };
        assert_eq!(canonical_loader_version("1.20.1", &forge), "47.3.0");
        let nf = LoaderRef {
            kind: LoaderKind::NeoForge,
            version: "1.20.1-47.1.106".to_owned(),
        };
        assert_eq!(canonical_loader_version("1.20.1", &nf), "47.1.106");

        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let mut plan = planned(&game, &instance);
        // 给光影包和停用模组都挂上假外链：光影包应标 server=unsupported，停用模组应留在 overrides/
        for node in &mut plan.nodes {
            if node.key == "shaderpacks/Complementary.zip"
                || node.key == "mods/only-here.jar.disabled"
            {
                node.modrinth = Some(ModrinthLink {
                    url: "https://cdn.modrinth.com/data/x/versions/y/file.zip".to_owned(),
                    sha512: "b".repeat(128),
                });
                node.curseforge = Some(CurseForgeLink {
                    project_id: 999,
                    file_id: 888,
                    project_name: "Dummy".to_owned(),
                    author: "Dev".to_owned(),
                    website_url: None,
                });
            }
        }
        let keys = vec![
            "mods/on-both.jar".to_owned(),
            "mods/only-here.jar.disabled".to_owned(),
            "shaderpacks/Complementary.zip".to_owned(),
            "options.txt".to_owned(),
        ];

        let mr_dest = dir.path().join("env.mrpack");
        let mr_sum = plan
            .write(
                &ExportOptions {
                    format: ExportFormat::Modrinth,
                    name: "EnvTest".to_owned(),
                    version: "1.0.0".to_owned(),
                    author: None,
                    summary: None,
                    recommended_memory_mb: None,
                    selected_keys: keys.clone(),
                },
                &mr_dest,
                &CancellationToken::new(),
                |_| {},
            )
            .unwrap();
        // 2 个外链（on-both.jar + shaderpacks/Complementary.zip），2 个内嵌（only-here.jar.disabled + options.txt）
        assert_eq!(mr_sum.linked, 2);
        assert_eq!(mr_sum.bundled, 2);

        let mut mr_zip = zip::ZipArchive::new(std::fs::File::open(&mr_dest).unwrap()).unwrap();
        let mut index_text = String::new();
        mr_zip
            .by_name(INDEX_FILE)
            .unwrap()
            .read_to_string(&mut index_text)
            .unwrap();
        let index_val: serde_json::Value = serde_json::from_str(&index_text).unwrap();
        let files = index_val["files"].as_array().unwrap();
        let shader = files
            .iter()
            .find(|f| f["path"] == "shaderpacks/Complementary.zip")
            .unwrap();
        assert_eq!(shader["env"]["client"], "required");
        assert_eq!(shader["env"]["server"], "unsupported");
        assert!(
            !files
                .iter()
                .any(|f| f["path"] == "mods/only-here.jar.disabled"),
            "停用模组应留在 overrides/ 中保留 .disabled 后缀"
        );
        assert!(
            mr_zip
                .by_name("overrides/mods/only-here.jar.disabled")
                .is_ok()
        );
    }
}
