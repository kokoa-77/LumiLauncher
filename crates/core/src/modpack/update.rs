//! 整合包一键无损更新引擎（Modrinth `.mrpack` / CurseForge `.zip` / MultiMC `.zip`）。
//!
//! 核心机制：
//! 1. **基线清单（`Lumi/pack-origin.json`）**：导入或更新整合包时记录包内全部文件相对路径与 SHA-1；
//! 2. **三方比对（[`preview`]）**：对比「旧基线（Origin）」「当前磁盘（Disk）」「新包（Target）」：
//!    - `saves/` 玩家世界与 `Lumi/` 元数据目录无条件受保护，永不覆盖或删除；
//!    - 玩家自加的模组、资源包、光影包（不在 Origin 也不在 Target）100% 保留；
//!    - 玩家修改过的配置文件（Disk != Origin）默认保留，勾选「覆盖已修改配置」时才更新（且仍可一键回滚）；
//!    - 仅当文件属于旧基线且未被玩家修改时，才随新版自动升级或清理；
//! 3. **安全事务与一键无损回滚（[`execute`] / [`rollback`]）**：
//!    - 先装加载器、补全游戏文件、下载/补件/解压目标文件到暂存区——任意步骤失败或取消时，实例目录零改动；
//!    - 写入实例前先打「改动前快照」，并把所有将被覆盖或移除的旧文件按 SHA-1 存入 [`ContentStore`]，
//!      原子写入 `Lumi/pack-rollback.json`；
//!    - 执行 [`rollback`] 时完整还原全部旧文件、配置、`Lumi/pack-origin.json`、`Lumi/patches.json` 与 `instance.json`。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::ModpackError;
use super::check::open_archive;
use super::plan::{PackFormat, PackPlan, PlannedFile};
use crate::content::InstanceContent;
use crate::content::claim::ManualFile;
use crate::content::detail::BodyFormat;
use crate::content::store::{ContentStore, STORE_DIR};
use crate::fs::{
    read_json, safe_join, write_atomic_from, write_json_atomic, write_json_atomic_async,
};
use crate::install::processors::LoaderContext;
use crate::install::{InstallPhase, LoaderProgress, install_files, load_or_fetch_version, loaders};
use crate::instance::{self, Instance, LoaderRef, META_DIR, PackSource};
use crate::meta::Environment;
use crate::meta::patches::{self, InstancePatches, LOCAL_LIBS_DIR, PATCHES_FILE};
use crate::net::{DownloadItem, DownloadProgress, sha1_file, to_hex};
use crate::paths::GameDir;
use crate::upgrade::ActiveClaim;

/// 整合包基线清单文件名（`instances/<id>/Lumi/pack-origin.json`）。
pub const ORIGIN_FILE: &str = "pack-origin.json";
/// 整合包更新回滚记录文件名（`instances/<id>/Lumi/pack-rollback.json`）。
pub const ROLLBACK_FILE: &str = "pack-rollback.json";
const MANIFEST_FORMAT: u32 = 1;
const CONTENT_DIRS: &[&str] = &["mods", "resourcepacks", "shaderpacks"];

/// 整合包基线清单：记录某个版本整合包放入实例的全部文件与 SHA-1，供下次三方比对。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackOriginManifest {
    pub format_version: u32,
    pub kind: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    /// 规范化相对路径（`/` 分隔）→ 小写 40 位 SHA-1
    pub files: BTreeMap<String, String>,
}

/// 单个文件的回滚记录（对应 [`ContentStore`] 中的对象）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackFile {
    pub path: String,
    pub sha1: String,
}

/// 一次整合包更新的完整回滚记录（`Lumi/pack-rollback.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackRollbackRecord {
    pub format_version: u32,
    pub created_at: f64,
    pub from_pack_name: String,
    pub from_pack_version: String,
    pub to_pack_version: String,
    pub prev_game_version: String,
    pub prev_loader: Option<LoaderRef>,
    pub prev_version_id: String,
    pub prev_source: Option<PackSource>,
    pub prev_recommended_memory_mb: Option<u32>,
    pub prev_origin: Option<PackOriginManifest>,
    pub prev_patches: Option<InstancePatches>,
    /// 更新前已存在、被覆盖或移除的文件（回滚时从 ContentStore 恢复）
    pub restored_files: Vec<RollbackFile>,
    /// 本次更新新写入的文件（回滚时从实例移入 ContentStore 以清除且不丢数据）
    pub added_files: Vec<RollbackFile>,
    /// 事务进行中标记：写盘开始前置为 `true`，全部元数据落盘后置为 `false`。
    /// 若中途失败或进程异常退出，下次启动或启动游戏前将自动按本记录无损回滚。
    #[serde(default)]
    pub in_progress: bool,
}

/// 供界面展示的回滚摘要信息。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackRollbackSummary {
    pub from_version: String,
    pub to_version: String,
    pub created_at: f64,
}

/// 文件在三方比对中的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum PackDiffKind {
    /// 新版整合包新增的文件
    Added,
    /// 旧基线文件未改过且新版已更新
    Updated,
    /// 旧基线文件未改过且新版已移除
    Removed,
    /// 玩家自加的模组/材质/光影（原样保留）
    PlayerAdded,
    /// 玩家修改过的配置或文件（默认保留，可选覆盖）
    PlayerModified,
}

/// 文件所属内容类别，便于前端分组与图标展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum PackFileCategory {
    Mod,
    ResourcePack,
    Shader,
    Config,
    Other,
}

/// 单条三方比对差异项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackDiffItem {
    pub path: String,
    pub kind: PackDiffKind,
    pub category: PackFileCategory,
    /// 该项在新版中存在更新版本（用于 `PlayerModified` 勾选覆盖时判断是否会被覆盖）
    pub has_target: bool,
    /// CurseForge 作者禁止第三方下载、需手动补件
    pub manual: bool,
}

/// 更新预览（展示给玩家确认）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackUpdatePreview {
    pub instance_id: String,
    pub instance_name: String,
    pub pack_name: String,
    pub format: PackFormat,
    pub current_pack_version: Option<String>,
    pub target_pack_version: String,
    pub current_game_version: String,
    pub target_game_version: String,
    pub current_loader: Option<LoaderRef>,
    pub target_loader: Option<LoaderRef>,
    pub changelog: Option<String>,
    pub changelog_format: BodyFormat,
    pub added_count: u32,
    pub updated_count: u32,
    pub removed_count: u32,
    pub unchanged_count: u32,
    pub player_added_count: u32,
    pub player_modified_count: u32,
    pub manual_files: Vec<ManualFile>,
    pub download_bytes: f64,
    pub has_origin_baseline: bool,
    pub mc_version_changed: bool,
    pub loader_changed: bool,
    pub is_downgrade: bool,
    pub has_worlds: bool,
    pub items: Vec<PackDiffItem>,
}

/// 更新执行选项。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackUpdateOptions {
    /// 是否覆盖玩家改过的配置文件（默认 false，保留玩家修改）
    #[serde(default)]
    pub overwrite_modified_configs: bool,
}

/// 更新执行进度。
#[derive(Debug, Clone)]
pub enum PackUpdateProgress {
    Loader(LoaderProgress),
    Game(InstallPhase, DownloadProgress),
    Files(DownloadProgress),
    WaitingManual(u32),
    Overrides,
    Applying,
}

/// 更新执行结果摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackUpdateOutcome {
    pub instance_id: String,
    pub from_version: String,
    pub to_version: String,
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub preserved_player_added: u32,
    pub preserved_player_modified: u32,
    pub overwritten_modified: u32,
    pub skipped_manual: u32,
}

/// 回滚结果摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackRollbackOutcome {
    pub instance_id: String,
    pub restored_version: String,
    pub reverted_from: String,
    pub restored_files: u32,
    pub removed_files: u32,
}

#[derive(Debug, Clone)]
enum TargetSource {
    Download(PlannedFile),
    Manual(ManualFile),
    Override { entry_name: String },
}

#[derive(Debug, Clone)]
struct TargetFile {
    rel_path: String,
    sha1: String,
    source: TargetSource,
    size: Option<u64>,
    keep_disabled: bool,
}

#[derive(Debug, Clone)]
enum PlannedOp {
    Install(TargetFile),
    Update {
        disk_path: String,
        target: TargetFile,
    },
    Remove {
        disk_path: String,
    },
    /// 玩家修改过的条目：仅当用户勾选 `overwrite_modified_configs` 且存在目标文件时才覆盖
    ModifiedConflict {
        disk_path: Option<String>,
        target: Option<TargetFile>,
    },
}

/// 已完成三方比对的整合包更新计划（缓存在后端等待用户确认执行）。
#[derive(Debug, Clone)]
pub struct PackUpdatePlan {
    pub preview: PackUpdatePreview,
    pub target_plan: PackPlan,
    /// 若整合包是从在线平台下载到临时目录的，记录该目录以便完成或放弃时清理
    pub temp_pack_dir: Option<PathBuf>,
    ops: Vec<PlannedOp>,
    target_origin: PackOriginManifest,
}

/// 规范化实例内相对路径（统一 `/`、去除 `.` 与多余分隔符）。
pub(crate) fn normalize_rel(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn walk_rel_files_sync(root: &Path) -> Vec<String> {
    fn visit(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                visit(base, &path, out);
            } else if ft.is_file()
                && let Ok(rel) = path.strip_prefix(base)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    if root.is_dir() {
        visit(root, root, &mut out);
    }
    out.sort();
    out
}

/// `saves/`（玩家存档）与 `Lumi/`（启动器元数据）在整合包更新中无条件受保护。
fn is_protected_path(norm: &str) -> bool {
    let Some(first) = norm.split('/').next() else {
        return true;
    };
    first.eq_ignore_ascii_case("saves") || instance::is_meta_dir(first)
}

fn categorize(norm: &str) -> PackFileCategory {
    let first = norm.split('/').next().unwrap_or("");
    if first.eq_ignore_ascii_case("mods") {
        PackFileCategory::Mod
    } else if first.eq_ignore_ascii_case("resourcepacks") {
        PackFileCategory::ResourcePack
    } else if first.eq_ignore_ascii_case("shaderpacks") {
        PackFileCategory::Shader
    } else if first.eq_ignore_ascii_case("config")
        || first.eq_ignore_ascii_case("defaultconfigs")
        || first.eq_ignore_ascii_case("kubejs")
        || norm.eq_ignore_ascii_case("options.txt")
        || norm.eq_ignore_ascii_case("servers.dat")
    {
        PackFileCategory::Config
    } else {
        PackFileCategory::Other
    }
}

/// 读取实例的 `Lumi/pack-origin.json`；不存在返回 `Ok(None)`。
pub fn load_origin(instance_dir: &Path) -> Result<Option<PackOriginManifest>, ModpackError> {
    let path = instance_dir.join(META_DIR).join(ORIGIN_FILE);
    read_json(&path).map_err(|e| ModpackError::Invalid(format!("读取整合包清单失败：{e}")))
}

/// 原子写入实例的 `Lumi/pack-origin.json`。
pub fn save_origin(instance_dir: &Path, manifest: &PackOriginManifest) -> Result<(), ModpackError> {
    let path = instance_dir.join(META_DIR).join(ORIGIN_FILE);
    write_json_atomic(&path, manifest)?;
    Ok(())
}

/// 读取实例的 `Lumi/pack-rollback.json`；不存在返回 `Ok(None)`。
pub fn load_rollback(instance_dir: &Path) -> Result<Option<PackRollbackRecord>, ModpackError> {
    let path = instance_dir.join(META_DIR).join(ROLLBACK_FILE);
    read_json(&path).map_err(|e| ModpackError::Invalid(format!("读取回滚记录失败：{e}")))
}

/// 是否存在上次中断未完成的整合包更新事务（供启动自愈与启动前检查使用）。
pub fn has_pending_update(instance_dir: &Path) -> bool {
    load_rollback(instance_dir)
        .ok()
        .flatten()
        .is_some_and(|record| record.in_progress)
}

/// 若存在上次中断未完成的整合包更新，自动无损回滚并返回回滚结果；无待恢复事务返回 `Ok(None)`。
pub async fn recover_pending(
    game: &GameDir,
    instance_id: &str,
) -> Result<Option<PackRollbackOutcome>, ModpackError> {
    let Some(instance) = instance::load(game, instance_id)? else {
        return Ok(None);
    };
    if instance.config.link.is_some() {
        return Ok(None);
    }
    let inst_dir = instance.game_dir(game)?;
    let Some(record) = load_rollback(&inst_dir)? else {
        return Ok(None);
    };
    if !record.in_progress {
        return Ok(None);
    }
    let outcome = rollback_with_record(game, instance_id, &inst_dir, &record, true).await?;
    Ok(Some(outcome))
}

/// 实例是否存在可一键撤销的整合包更新记录（处于中断待恢复状态时不作为手动回滚项展示）。
pub fn rollback_summary(instance_dir: &Path) -> Option<PackRollbackSummary> {
    let record = load_rollback(instance_dir).ok()??;
    if record.in_progress {
        return None;
    }
    Some(PackRollbackSummary {
        from_version: record.from_pack_version,
        to_version: record.to_pack_version,
        created_at: record.created_at,
    })
}

/// 整合包首次导入完成后，基于导入计划与目标实例目录生成并写入 `Lumi/pack-origin.json`。
pub(super) async fn write_origin_after_import(
    target_dir: &Path,
    plan: &PackPlan,
) -> Result<(), ModpackError> {
    let mut files = BTreeMap::new();
    for file in &plan.downloads {
        let norm = normalize_rel(&file.path);
        if !is_protected_path(&norm) {
            files.insert(norm, file.sha1.to_ascii_lowercase());
        }
    }
    for file in &plan.preview.manual_files {
        let norm = normalize_rel(&file.path);
        if !is_protected_path(&norm) {
            files.insert(norm, file.sha1.to_ascii_lowercase());
        }
    }
    // 覆盖层会覆盖同名下载文件，按最终磁盘内容计算 SHA-1
    for (_, relative) in &plan.overrides {
        let norm = normalize_rel(relative);
        if is_protected_path(&norm) {
            continue;
        }
        let abs = safe_join(target_dir, &norm)?;
        if abs.is_file()
            && let Ok(sha1) = sha1_file(&abs).await
        {
            files.insert(norm, sha1);
        }
    }
    let manifest = PackOriginManifest {
        format_version: MANIFEST_FORMAT,
        kind: plan.preview.format.source_kind().to_owned(),
        name: plan.preview.name.clone(),
        version: plan.preview.version.clone(),
        project_id: plan.origin.as_ref().map(|o| o.project_id.clone()),
        version_id: plan.origin.as_ref().map(|o| o.version_id.clone()),
        files,
    };
    save_origin(target_dir, &manifest)
}

/// 校验并对比当前实例与目标整合包，生成三方比对预览与可执行计划。
pub async fn preview(
    game: &GameDir,
    instance: &Instance,
    target_plan: PackPlan,
    changelog: Option<String>,
    changelog_format: BodyFormat,
    temp_pack_dir: Option<PathBuf>,
) -> Result<PackUpdatePlan, ModpackError> {
    if instance.config.link.is_some() {
        return Err(ModpackError::Invalid(
            "外部关联实例不能直接更新整合包，请先「复制成 Lumi 的实例」后再更新".to_owned(),
        ));
    }
    let inst_dir = instance.game_dir(game)?;
    let origin = load_origin(&inst_dir)?;
    let has_origin_baseline = origin.is_some();
    let has_worlds = detect_has_worlds(&inst_dir);

    // 1. 流式计算目标整合包中覆盖文件的 SHA-1，避免大文件占满内存
    let pack_path = target_plan.pack_path.clone();
    let overrides_list = target_plan.overrides.clone();
    let override_hashes = tokio::task::spawn_blocking(
        move || -> Result<Vec<(String, String, String, u64)>, ModpackError> {
            if overrides_list.is_empty() {
                return Ok(Vec::new());
            }
            let mut archive = open_archive(&pack_path)?;
            let mut out = Vec::with_capacity(overrides_list.len());
            let mut buf = [0u8; 64 * 1024];
            for (entry_name, rel) in &overrides_list {
                let norm = normalize_rel(rel);
                if is_protected_path(&norm) {
                    continue;
                }
                let mut entry = archive
                    .by_name(entry_name)
                    .map_err(|e| ModpackError::Invalid(format!("读取 {entry_name} 失败：{e}")))?;
                let size = entry.size();
                let mut hasher = Sha1::new();
                loop {
                    let n = entry.read(&mut buf).map_err(|e| {
                        ModpackError::Invalid(format!("读取 {entry_name} 失败：{e}"))
                    })?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
                let sha1 = to_hex(&hasher.finalize());
                out.push((entry_name.clone(), norm, sha1, size));
            }
            Ok(out)
        },
    )
    .await
    .map_err(|e| ModpackError::Invalid(format!("计算整合包差异失败：{e}")))??;

    // 2. 组装目标清单（按 Windows 不区分大小写的规范键索引）
    let mut target_map: BTreeMap<String, TargetFile> = BTreeMap::new();
    for file in &target_plan.downloads {
        let norm = normalize_rel(&file.path);
        if is_protected_path(&norm) {
            continue;
        }
        let key = norm.to_lowercase();
        target_map.insert(
            key,
            TargetFile {
                rel_path: norm,
                sha1: file.sha1.to_ascii_lowercase(),
                source: TargetSource::Download(file.clone()),
                size: file.size,
                keep_disabled: false,
            },
        );
    }
    for file in &target_plan.preview.manual_files {
        let norm = normalize_rel(&file.path);
        if is_protected_path(&norm) {
            continue;
        }
        let key = norm.to_lowercase();
        target_map.insert(
            key,
            TargetFile {
                rel_path: norm,
                sha1: file.sha1.to_ascii_lowercase(),
                source: TargetSource::Manual(file.clone()),
                size: None,
                keep_disabled: false,
            },
        );
    }
    for (entry_name, norm, sha1, size) in override_hashes {
        let key = norm.to_lowercase();
        target_map.insert(
            key,
            TargetFile {
                rel_path: norm,
                sha1,
                source: TargetSource::Override { entry_name },
                size: Some(size),
                keep_disabled: false,
            },
        );
    }

    // 目标基线清单（更新完成后写入 Lumi/pack-origin.json）
    let mut target_origin_files = BTreeMap::new();
    for tf in target_map.values() {
        target_origin_files.insert(tf.rel_path.clone(), tf.sha1.clone());
    }
    let target_origin = PackOriginManifest {
        format_version: MANIFEST_FORMAT,
        kind: target_plan.preview.format.source_kind().to_owned(),
        name: target_plan.preview.name.clone(),
        version: target_plan.preview.version.clone(),
        project_id: target_plan
            .origin
            .as_ref()
            .map(|o| o.project_id.clone())
            .or_else(|| {
                instance
                    .config
                    .source
                    .as_ref()
                    .and_then(|s| s.project_id.clone())
            }),
        version_id: target_plan.origin.as_ref().map(|o| o.version_id.clone()),
        files: target_origin_files,
    };

    // 3. 组装旧基线映射
    let mut origin_map: HashMap<String, (String, String)> = HashMap::new();
    if let Some(manifest) = &origin {
        for (rel, sha1) in &manifest.files {
            let norm = normalize_rel(rel);
            if is_protected_path(&norm) {
                continue;
            }
            origin_map.insert(norm.to_lowercase(), (norm, sha1.to_ascii_lowercase()));
        }
    }

    // 4. 在后台线程里对 Origin ∪ Target 探测磁盘状态，并扫描玩家自加内容
    let all_keys: Vec<(String, String)> = {
        let mut seen = BTreeMap::new();
        for (k, tf) in &target_map {
            seen.insert(k.clone(), tf.rel_path.clone());
        }
        for (k, (rel, _)) in &origin_map {
            seen.entry(k.clone()).or_insert_with(|| rel.clone());
        }
        seen.into_iter().collect()
    };
    let inst_dir_scan = inst_dir.clone();
    let known_keys: HashSet<String> = all_keys.iter().map(|(k, _)| k.clone()).collect();
    let (disk_states, player_added_paths) = tokio::task::spawn_blocking(
        move || -> Result<(HashMap<String, DiskProbe>, Vec<String>), ModpackError> {
            let mut states = HashMap::with_capacity(all_keys.len());
            for (key, rel) in &all_keys {
                if let Some(probe) = probe_disk_file(&inst_dir_scan, rel)? {
                    states.insert(key.clone(), probe);
                }
            }
            let player_added = scan_player_added_content(&inst_dir_scan, &known_keys);
            Ok((states, player_added))
        },
    )
    .await
    .map_err(|e| ModpackError::Invalid(format!("扫描实例文件失败：{e}")))??;

    // 5. 三方比对决策
    let mut ops = Vec::new();
    let mut items = Vec::new();
    let mut manual_needed = Vec::new();
    let mut download_bytes = 0u64;
    let mut added_count = 0u32;
    let mut updated_count = 0u32;
    let mut removed_count = 0u32;
    let mut unchanged_count = 0u32;
    let mut player_modified_count = 0u32;

    let mut union_keys: Vec<String> = target_map
        .keys()
        .chain(origin_map.keys())
        .cloned()
        .collect();
    union_keys.sort();
    union_keys.dedup();

    for key in union_keys {
        let o = origin_map.get(&key);
        let t = target_map.get(&key);
        let d = disk_states.get(&key);

        match (o, t) {
            // 新版新增（不在旧基线）
            (None, Some(tf)) => match d {
                None => {
                    added_count += 1;
                    record_transfer_cost(tf, &mut download_bytes, &mut manual_needed);
                    items.push(PackDiffItem {
                        path: tf.rel_path.clone(),
                        kind: PackDiffKind::Added,
                        category: categorize(&tf.rel_path),
                        has_target: true,
                        manual: matches!(tf.source, TargetSource::Manual(_)),
                    });
                    ops.push(PlannedOp::Install(tf.clone()));
                }
                Some(probe) if !probe.via_disabled && probe.sha1 == tf.sha1 => {
                    unchanged_count += 1;
                }
                Some(probe) => {
                    // 磁盘已有不同内容（玩家手工放的同名文件或配置）：默认保留玩家版本
                    player_modified_count += 1;
                    items.push(PackDiffItem {
                        path: tf.rel_path.clone(),
                        kind: PackDiffKind::PlayerModified,
                        category: categorize(&tf.rel_path),
                        has_target: true,
                        manual: matches!(tf.source, TargetSource::Manual(_)),
                    });
                    ops.push(PlannedOp::ModifiedConflict {
                        disk_path: Some(probe.disk_rel.clone()),
                        target: Some(tf.clone()),
                    });
                }
            },
            // 旧版与新版都有该路径
            (Some((_, o_sha1)), Some(tf)) => {
                if &tf.sha1 == o_sha1 {
                    // 整合包作者没改这个文件
                    match d {
                        Some(probe) if !probe.via_disabled && &probe.sha1 == o_sha1 => {
                            unchanged_count += 1;
                        }
                        Some(probe) => {
                            // 玩家自己改过或停用了这个文件：保留玩家改动
                            player_modified_count += 1;
                            items.push(PackDiffItem {
                                path: tf.rel_path.clone(),
                                kind: PackDiffKind::PlayerModified,
                                category: categorize(&tf.rel_path),
                                has_target: true,
                                manual: matches!(tf.source, TargetSource::Manual(_)),
                            });
                            ops.push(PlannedOp::ModifiedConflict {
                                disk_path: Some(probe.disk_rel.clone()),
                                target: Some(tf.clone()),
                            });
                        }
                        None => {
                            // 玩家删掉了旧版文件：尊重玩家删除，默认不强行恢复
                            player_modified_count += 1;
                            items.push(PackDiffItem {
                                path: tf.rel_path.clone(),
                                kind: PackDiffKind::PlayerModified,
                                category: categorize(&tf.rel_path),
                                has_target: true,
                                manual: matches!(tf.source, TargetSource::Manual(_)),
                            });
                            ops.push(PlannedOp::ModifiedConflict {
                                disk_path: None,
                                target: Some(tf.clone()),
                            });
                        }
                    }
                } else {
                    // 整合包作者更新了这个文件
                    match d {
                        Some(probe) if !probe.via_disabled && &probe.sha1 == o_sha1 => {
                            // 玩家没动过 → 无损更新到新版
                            updated_count += 1;
                            record_transfer_cost(tf, &mut download_bytes, &mut manual_needed);
                            items.push(PackDiffItem {
                                path: tf.rel_path.clone(),
                                kind: PackDiffKind::Updated,
                                category: categorize(&tf.rel_path),
                                has_target: true,
                                manual: matches!(tf.source, TargetSource::Manual(_)),
                            });
                            ops.push(PlannedOp::Update {
                                disk_path: probe.disk_rel.clone(),
                                target: tf.clone(),
                            });
                        }
                        Some(probe) if !probe.via_disabled && probe.sha1 == tf.sha1 => {
                            unchanged_count += 1;
                        }
                        Some(probe) => {
                            // 玩家改过或停用了旧文件：默认保留玩家版本，勾选覆盖配置才更新
                            player_modified_count += 1;
                            items.push(PackDiffItem {
                                path: tf.rel_path.clone(),
                                kind: PackDiffKind::PlayerModified,
                                category: categorize(&tf.rel_path),
                                has_target: true,
                                manual: matches!(tf.source, TargetSource::Manual(_)),
                            });
                            ops.push(PlannedOp::ModifiedConflict {
                                disk_path: Some(probe.disk_rel.clone()),
                                target: Some(tf.clone()),
                            });
                        }
                        None => {
                            // 玩家删掉了旧文件：默认不强行写回
                            player_modified_count += 1;
                            items.push(PackDiffItem {
                                path: tf.rel_path.clone(),
                                kind: PackDiffKind::PlayerModified,
                                category: categorize(&tf.rel_path),
                                has_target: true,
                                manual: matches!(tf.source, TargetSource::Manual(_)),
                            });
                            ops.push(PlannedOp::ModifiedConflict {
                                disk_path: None,
                                target: Some(tf.clone()),
                            });
                        }
                    }
                }
            }
            // 旧基线有、新版已移除
            (Some((o_rel, o_sha1)), None) => match d {
                Some(probe) if &probe.sha1 == o_sha1 => {
                    // 未被玩家改过（包括仅被停用的原版模组）→ 安全移出（进 ContentStore 可回滚）
                    removed_count += 1;
                    items.push(PackDiffItem {
                        path: probe.disk_rel.clone(),
                        kind: PackDiffKind::Removed,
                        category: categorize(o_rel),
                        has_target: false,
                        manual: false,
                    });
                    ops.push(PlannedOp::Remove {
                        disk_path: probe.disk_rel.clone(),
                    });
                }
                Some(_) => {
                    // 玩家自己改过内容：默认保留
                    player_modified_count += 1;
                    items.push(PackDiffItem {
                        path: o_rel.clone(),
                        kind: PackDiffKind::PlayerModified,
                        category: categorize(o_rel),
                        has_target: false,
                        manual: false,
                    });
                    ops.push(PlannedOp::ModifiedConflict {
                        disk_path: None,
                        target: None,
                    });
                }
                None => {}
            },
            (None, None) => {}
        }
    }

    let mut removed_disabled_stems: HashMap<String, usize> = HashMap::new();
    for op in &ops {
        if let PlannedOp::Remove { disk_path } = op
            && disk_path.to_ascii_lowercase().ends_with(".jar.disabled")
            && let Some(stem) = normalize_mod_stem(disk_path)
        {
            *removed_disabled_stems.entry(stem).or_insert(0) += 1;
        }
    }

    // 若无基线（老版导入的实例），按模组名词干识别旧版同名模组并安全替换移除，防新旧 jar 并存崩溃
    let mut kept_player_added = Vec::with_capacity(player_added_paths.len());
    if !has_origin_baseline {
        let target_mod_stems: HashSet<String> = target_map
            .values()
            .filter(|tf| categorize(&tf.rel_path) == PackFileCategory::Mod)
            .filter_map(|tf| normalize_mod_stem(&tf.rel_path))
            .collect();
        for rel in player_added_paths {
            if categorize(&rel) == PackFileCategory::Mod
                && let Some(stem) = normalize_mod_stem(&rel)
                && target_mod_stems.contains(&stem)
            {
                if rel.to_ascii_lowercase().ends_with(".jar.disabled") {
                    *removed_disabled_stems.entry(stem).or_insert(0) += 1;
                }
                removed_count += 1;
                items.push(PackDiffItem {
                    path: rel.clone(),
                    kind: PackDiffKind::Removed,
                    category: PackFileCategory::Mod,
                    has_target: false,
                    manual: false,
                });
                ops.push(PlannedOp::Remove { disk_path: rel });
                continue;
            }
            kept_player_added.push(rel);
        }
    } else {
        kept_player_added = player_added_paths;
    }

    // 把玩家已停用的旧版模组 `.jar.disabled` 停用状态延续到新版改名后的模组（唯一 1-to-1 词干匹配）
    let mut added_mod_stem_counts: HashMap<String, usize> = HashMap::new();
    for op in &ops {
        if let PlannedOp::Install(tf) = op
            && categorize(&tf.rel_path) == PackFileCategory::Mod
            && let Some(stem) = normalize_mod_stem(&tf.rel_path)
        {
            *added_mod_stem_counts.entry(stem).or_insert(0) += 1;
        }
    }
    let unique_disabled_stems: HashSet<String> = removed_disabled_stems
        .into_iter()
        .filter(|(stem, cnt)| *cnt == 1 && added_mod_stem_counts.get(stem).copied() == Some(1))
        .map(|(stem, _)| stem)
        .collect();
    if !unique_disabled_stems.is_empty() {
        for op in &mut ops {
            if let PlannedOp::Install(tf) = op
                && categorize(&tf.rel_path) == PackFileCategory::Mod
                && normalize_mod_stem(&tf.rel_path)
                    .is_some_and(|stem| unique_disabled_stems.contains(&stem))
            {
                tf.keep_disabled = true;
            }
        }
    }

    let player_added_count = u32::try_from(kept_player_added.len()).unwrap_or(u32::MAX);
    for rel in kept_player_added {
        items.push(PackDiffItem {
            category: categorize(&rel),
            path: rel,
            kind: PackDiffKind::PlayerAdded,
            has_target: false,
            manual: false,
        });
    }

    // 按内容重要性（模组优先，其次光影/资源包，最后配置与其他）、变更类型和路径排序
    items.sort_by(|a, b| {
        category_rank(a.category)
            .cmp(&category_rank(b.category))
            .then_with(|| diff_rank(a.kind).cmp(&diff_rank(b.kind)))
            .then_with(|| a.path.cmp(&b.path))
    });

    let current_game_version = instance.config.game_version.clone();
    let target_game_version = target_plan.preview.game_version.clone();
    let mc_version_changed = current_game_version != target_game_version;
    let loader_changed = instance.config.loader != target_plan.preview.loader;
    let is_downgrade = is_mc_downgrade(&current_game_version, &target_game_version);

    let preview = PackUpdatePreview {
        instance_id: instance.id.clone(),
        instance_name: instance.config.name.clone(),
        pack_name: target_plan.preview.name.clone(),
        format: target_plan.preview.format,
        current_pack_version: instance.config.source.as_ref().map(|s| s.version.clone()),
        target_pack_version: target_plan.preview.version.clone(),
        current_game_version,
        target_game_version,
        current_loader: instance.config.loader.clone(),
        target_loader: target_plan.preview.loader.clone(),
        changelog,
        changelog_format,
        added_count,
        updated_count,
        removed_count,
        unchanged_count,
        player_added_count,
        player_modified_count,
        manual_files: manual_needed,
        download_bytes: download_bytes as f64,
        has_origin_baseline,
        mc_version_changed,
        loader_changed,
        is_downgrade,
        has_worlds,
        items,
    };

    Ok(PackUpdatePlan {
        preview,
        target_plan,
        temp_pack_dir,
        ops,
        target_origin,
    })
}

fn normalize_mod_stem(rel_path: &str) -> Option<String> {
    let fname = rel_path.rsplit('/').next()?;
    let without_disabled = fname
        .strip_suffix(".disabled")
        .or_else(|| fname.strip_suffix(".DISABLED"))
        .unwrap_or(fname);
    let stem = without_disabled
        .strip_suffix(".jar")
        .or_else(|| without_disabled.strip_suffix(".JAR"))?;
    let bytes = stem.as_bytes();
    let mut cut = stem.len();
    for i in 1..bytes.len().saturating_sub(1) {
        if matches!(bytes[i], b'-' | b'_' | b'+') {
            let next = bytes[i + 1];
            let next2 = bytes.get(i + 2).copied();
            let looks_like_ver = next.is_ascii_digit()
                || (matches!(next, b'v' | b'V') && next2.is_some_and(|b| b.is_ascii_digit()))
                || stem[i..].to_ascii_lowercase().starts_with("-mc1.")
                || stem[i..].to_ascii_lowercase().starts_with("_mc1.");
            if looks_like_ver {
                cut = i;
                break;
            }
        }
    }
    let normalized = stem[..cut]
        .trim_end_matches(['-', '_', '+'])
        .to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn category_rank(category: PackFileCategory) -> u8 {
    match category {
        PackFileCategory::Mod => 0,
        PackFileCategory::Shader => 1,
        PackFileCategory::ResourcePack => 2,
        PackFileCategory::Config => 3,
        PackFileCategory::Other => 4,
    }
}

fn diff_rank(kind: PackDiffKind) -> u8 {
    match kind {
        PackDiffKind::Updated => 0,
        PackDiffKind::Added => 1,
        PackDiffKind::Removed => 2,
        PackDiffKind::PlayerModified => 3,
        PackDiffKind::PlayerAdded => 4,
    }
}

fn record_transfer_cost(
    tf: &TargetFile,
    download_bytes: &mut u64,
    manual_needed: &mut Vec<ManualFile>,
) {
    match &tf.source {
        TargetSource::Download(_) => {
            *download_bytes = download_bytes.saturating_add(tf.size.unwrap_or(0));
        }
        TargetSource::Manual(m) => {
            manual_needed.push(m.clone());
        }
        TargetSource::Override { .. } => {}
    }
}

struct DiskProbe {
    disk_rel: String,
    sha1: String,
    via_disabled: bool,
}

fn probe_disk_file(inst_dir: &Path, rel: &str) -> Result<Option<DiskProbe>, ModpackError> {
    let abs = safe_join(inst_dir, rel)?;
    if abs.is_file() {
        let sha1 = sha1_file_sync(&abs)?;
        return Ok(Some(DiskProbe {
            disk_rel: rel.to_owned(),
            sha1,
            via_disabled: false,
        }));
    }
    // 若是 mods/ 下的文件，再检查是否被玩家停用成 `.disabled`
    if categorize(rel) == PackFileCategory::Mod && !rel.ends_with(".disabled") {
        let disabled_rel = format!("{rel}.disabled");
        let disabled_abs = safe_join(inst_dir, &disabled_rel)?;
        if disabled_abs.is_file() {
            let sha1 = sha1_file_sync(&disabled_abs)?;
            return Ok(Some(DiskProbe {
                disk_rel: disabled_rel,
                sha1,
                via_disabled: true,
            }));
        }
    }
    Ok(None)
}

fn sha1_file_sync(path: &Path) -> Result<String, ModpackError> {
    let mut file = std::fs::File::open(path).map_err(|source| ModpackError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| ModpackError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

fn scan_player_added_content(inst_dir: &Path, known_keys: &HashSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    for dir_name in CONTENT_DIRS {
        let folder = inst_dir.join(dir_name);
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let rel = format!("{dir_name}/{name}");
            let lower = rel.to_lowercase();
            let base_lower = lower.strip_suffix(".disabled").unwrap_or(&lower);
            if !known_keys.contains(&lower) && !known_keys.contains(base_lower) {
                found.push(rel);
            }
        }
    }
    found.sort();
    found
}

fn detect_has_worlds(inst_dir: &Path) -> bool {
    let saves = inst_dir.join("saves");
    let Ok(entries) = std::fs::read_dir(saves) else {
        return false;
    };
    entries
        .flatten()
        .any(|e| e.path().is_dir() && e.path().join("level.dat").is_file())
}

fn parse_mc_tuple(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch_str = parts.next().unwrap_or("0");
    let patch_digits: String = patch_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = if patch_digits.is_empty() {
        0
    } else {
        patch_digits.parse::<u32>().ok()?
    };
    Some((major, minor, patch))
}

fn is_mc_downgrade(from: &str, to: &str) -> bool {
    match (parse_mc_tuple(from), parse_mc_tuple(to)) {
        (Some(a), Some(b)) => b < a,
        _ => false,
    }
}

struct PreparedWrite {
    disk_path: Option<String>,
    rel_path: String,
    staged_file: PathBuf,
    sha1: String,
    kind: WriteKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    Added,
    Updated,
    OverwrittenModified,
}

/// 执行整合包无损更新：
/// 1. 先安装目标加载器并补全游戏本体（失败/取消时零改动）；
/// 2. 下载、补件并解压所需目标文件到暂存目录（失败/取消时零改动）；
/// 3. 打改动前快照 + 备份旧文件到 `ContentStore` + 写入 `Lumi/pack-rollback.json`；
/// 4. 原子写入目标文件、更新 `Lumi/pack-origin.json`、`Lumi/patches.json` 与 `instance.json`。
pub async fn execute<F, C>(
    ctx: &LoaderContext<'_>,
    temp_root: &Path,
    plan: &PackUpdatePlan,
    options: &PackUpdateOptions,
    on_progress: F,
    mut on_claim: C,
) -> Result<PackUpdateOutcome, ModpackError>
where
    F: Fn(PackUpdateProgress) + Send + Sync,
    C: FnMut(String, Vec<ManualFile>) -> Option<ActiveClaim>,
{
    let game = ctx.game;
    let instance_id = &plan.preview.instance_id;
    let instance = instance::load(game, instance_id)?
        .ok_or_else(|| ModpackError::Invalid("找不到这个实例，可能已经被删除".to_owned()))?;
    if instance.config.link.is_some() {
        return Err(ModpackError::Invalid(
            "外部关联实例不能直接更新整合包，请先「复制成 Lumi 的实例」后再更新".to_owned(),
        ));
    }

    let staging = temp_root.join(format!("pack-update-{}", uuid::Uuid::new_v4().simple()));
    let files_staging = staging.join("files");
    tokio::fs::create_dir_all(&files_staging)
        .await
        .map_err(|source| ModpackError::Io {
            path: files_staging.clone(),
            source,
        })?;

    let result = async {
        // 1. 先安装目标加载器（已安装会自动跳过）
        let version_id = if let Some(loader) = &plan.target_plan.preview.loader
            && !plan.target_plan.skip_loader_install
        {
            let loader_work = staging.join("loader");
            let loader_ctx = LoaderContext {
                work_dir: &loader_work,
                ..*ctx
            };
            loaders::install(
                &loader_ctx,
                &plan.target_plan.preview.game_version,
                loader,
                |p| on_progress(PackUpdateProgress::Loader(p)),
            )
            .await?
        } else {
            plan.target_plan.preview.game_version.clone()
        };

        // 2. 补全目标游戏本体文件与补丁依赖库
        let mut version = load_or_fetch_version(game, &version_id, ctx.client).await?;
        let inst_dir = instance.game_dir(game)?;
        if let Some(bundle) = &plan.target_plan.patches {
            // 若新包带本地补丁库，先解压到暂存目录供补丁校验
            if !plan.target_plan.local_libraries.is_empty() {
                let pack = plan.target_plan.pack_path.clone();
                let libs = plan.target_plan.local_libraries.clone();
                let libs_dest = staging.join(META_DIR).join(LOCAL_LIBS_DIR);
                tokio::task::spawn_blocking(move || {
                    super::plan::extract_overrides(&pack, &libs, &libs_dest)
                })
                .await
                .map_err(|e| ModpackError::Invalid(format!("解压补丁库失败：{e}")))??;
            }
            let patch_root = if plan.target_plan.local_libraries.is_empty() {
                inst_dir.as_path()
            } else {
                staging.as_path()
            };
            version = patches::apply(version, bundle, patch_root)
                .map_err(crate::install::InstallError::from)?;
        }
        install_files(
            version,
            game,
            &Environment::current(),
            ctx.downloader,
            ctx.cancel,
            |phase, p| on_progress(PackUpdateProgress::Game(phase, p)),
        )
        .await?;

        // 3. 确定实际需要下载/补件/解压/删除的操作集合
        let mut download_items = Vec::new();
        let mut manual_needed = Vec::new();
        let mut override_entries: Vec<(String, String)> = Vec::new();
        let mut writes: Vec<PreparedWrite> = Vec::new();
        let mut removals: Vec<String> = Vec::new();
        let mut preserved_player_modified = 0u32;

        let mut enqueue_target = |disk_path: Option<String>,
                                  tf: &TargetFile,
                                  kind: WriteKind|
         -> Result<(), ModpackError> {
            let dest_rel = if tf.keep_disabled
                && categorize(&tf.rel_path) == PackFileCategory::Mod
                && !tf.rel_path.ends_with(".disabled")
            {
                format!("{}.disabled", tf.rel_path)
            } else {
                tf.rel_path.clone()
            };
            let staged_file = safe_join(&files_staging, &dest_rel)?;
            match &tf.source {
                TargetSource::Download(pf) => {
                    download_items.push(DownloadItem {
                        urls: pf.urls.clone(),
                        path: staged_file.clone(),
                        sha1: Some(pf.sha1.clone()),
                        size: pf.size,
                    });
                }
                TargetSource::Manual(m) => {
                    manual_needed.push(m.clone());
                }
                TargetSource::Override { entry_name } => {
                    override_entries.push((entry_name.clone(), dest_rel.clone()));
                }
            }
            writes.push(PreparedWrite {
                disk_path,
                rel_path: dest_rel,
                staged_file,
                sha1: tf.sha1.clone(),
                kind,
            });
            Ok(())
        };

        for op in &plan.ops {
            match op {
                PlannedOp::Install(tf) => {
                    enqueue_target(None, tf, WriteKind::Added)?;
                }
                PlannedOp::Update { disk_path, target } => {
                    enqueue_target(Some(disk_path.clone()), target, WriteKind::Updated)?;
                }
                PlannedOp::Remove { disk_path } => {
                    removals.push(disk_path.clone());
                }
                PlannedOp::ModifiedConflict { disk_path, target } => {
                    if options.overwrite_modified_configs
                        && let Some(tf) = target
                        && categorize(&tf.rel_path) == PackFileCategory::Config
                    {
                        enqueue_target(disk_path.clone(), tf, WriteKind::OverwrittenModified)?;
                    } else {
                        preserved_player_modified += 1;
                    }
                }
            }
        }

        // 4. 下载需要的远程文件
        if !download_items.is_empty() {
            ctx.downloader
                .download_all(&download_items, ctx.cancel, |p| {
                    on_progress(PackUpdateProgress::Files(p))
                })
                .await?;
        }

        // 5. 处理 CurseForge 作者禁止第三方下载的手动补件
        let mut skipped_manual = 0u32;
        let mut skipped_manual_keys: HashSet<String> = HashSet::new();
        if !manual_needed.is_empty() {
            let claim_title = format!(
                "{} {}",
                plan.preview.pack_name, plan.preview.target_pack_version
            );
            if let Some(active) = on_claim(claim_title, manual_needed.clone()) {
                let waiting = active
                    .session
                    .snapshot()
                    .entries
                    .iter()
                    .filter(|e| e.state != crate::content::claim::ClaimState::Claimed)
                    .count();
                on_progress(PackUpdateProgress::WaitingManual(
                    u32::try_from(waiting).unwrap_or(u32::MAX),
                ));
                let claimed = active.session.wait(ctx.cancel).await?;
                let store = ContentStore::new(game.root().join(STORE_DIR));
                let claimed_paths: HashSet<String> = claimed
                    .iter()
                    .map(|f| normalize_rel(&f.path).to_lowercase())
                    .collect();
                for file in claimed {
                    let norm = normalize_rel(&file.path);
                    let dest = safe_join(&files_staging, &norm)?;
                    store
                        .restore(&file.sha1, &dest)
                        .await
                        .map_err(|source| ModpackError::Io { path: dest, source })?;
                }
                // 玩家跳过某些手动补件时，将其从待写入队列移除，并记录以校正 pack-origin.json
                writes.retain(|w| {
                    let canonical = w.rel_path.strip_suffix(".disabled").unwrap_or(&w.rel_path);
                    if w.staged_file.is_file() {
                        true
                    } else if manual_needed
                        .iter()
                        .any(|m| normalize_rel(&m.path).eq_ignore_ascii_case(canonical))
                        && !claimed_paths.contains(&canonical.to_lowercase())
                    {
                        skipped_manual += 1;
                        skipped_manual_keys.insert(canonical.to_lowercase());
                        false
                    } else {
                        true
                    }
                });
            } else {
                // 无宿主补件会话（如纯无头模式）：跳过手动补件并记录数量
                writes.retain(|w| {
                    let canonical = w.rel_path.strip_suffix(".disabled").unwrap_or(&w.rel_path);
                    let is_manual = manual_needed
                        .iter()
                        .any(|m| normalize_rel(&m.path).eq_ignore_ascii_case(canonical));
                    if is_manual {
                        skipped_manual += 1;
                        skipped_manual_keys.insert(canonical.to_lowercase());
                    }
                    !is_manual
                });
            }
        }

        // 6. 解压需要的覆盖文件到暂存目录
        if !override_entries.is_empty() {
            on_progress(PackUpdateProgress::Overrides);
            let pack = plan.target_plan.pack_path.clone();
            let dest = files_staging.clone();
            tokio::task::spawn_blocking(move || {
                super::plan::extract_overrides(&pack, &override_entries, &dest)
            })
            .await
            .map_err(|e| ModpackError::Invalid(format!("解压配置文件失败：{e}")))??;
        }

        // 7. 若新包带本地补丁库，先解压到暂存目录（后续纳入回滚快照与事务写盘）
        let local_libs_staged = if !plan.target_plan.local_libraries.is_empty() {
            let pack = plan.target_plan.pack_path.clone();
            let libs = plan.target_plan.local_libraries.clone();
            let libs_stage = staging.join("local-libs");
            tokio::task::spawn_blocking({
                let libs_stage = libs_stage.clone();
                move || super::plan::extract_overrides(&pack, &libs, &libs_stage)
            })
            .await
            .map_err(|e| ModpackError::Invalid(format!("解压补丁库失败：{e}")))??;
            Some(libs_stage)
        } else {
            None
        };

        // 8. 事务应用阶段：打快照、写回滚记录、落地文件与补丁库、更新元数据
        on_progress(PackUpdateProgress::Applying);
        apply_staged_update_with_extras(
            game,
            &instance,
            plan,
            version_id,
            writes,
            removals,
            preserved_player_modified,
            skipped_manual,
            &skipped_manual_keys,
            local_libs_staged.as_deref(),
        )
        .await
    }
    .await;

    // 无条件清理暂存目录及在线下载的临时整合包目录（计划已被启动消耗，避免报错时漏下整包数百 MB 残留）
    super::discard_staging(&staging).await;
    if let Some(temp_dir) = &plan.temp_pack_dir {
        super::discard_staging(temp_dir).await;
    }
    result
}

#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
async fn apply_staged_update(
    game: &GameDir,
    instance: &Instance,
    plan: &PackUpdatePlan,
    version_id: String,
    writes: Vec<PreparedWrite>,
    removals: Vec<String>,
    preserved_player_modified: u32,
    skipped_manual: u32,
) -> Result<PackUpdateOutcome, ModpackError> {
    apply_staged_update_with_extras(
        game,
        instance,
        plan,
        version_id,
        writes,
        removals,
        preserved_player_modified,
        skipped_manual,
        &HashSet::new(),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_staged_update_with_extras(
    game: &GameDir,
    instance: &Instance,
    plan: &PackUpdatePlan,
    version_id: String,
    writes: Vec<PreparedWrite>,
    removals: Vec<String>,
    preserved_player_modified: u32,
    skipped_manual: u32,
    skipped_manual_keys: &HashSet<String>,
    local_libs_staged: Option<&Path>,
) -> Result<PackUpdateOutcome, ModpackError> {
    let inst_dir = instance.game_dir(game)?;
    let store = ContentStore::new(game.root().join(STORE_DIR));

    // 先对内容目录（mods/resourcepacks/shaderpacks）打一个既有快照，联动「改动历史」与时间线
    let content_mgr = InstanceContent::for_game(game, &inst_dir);
    content_mgr.snapshot_before_change().await;
    let tl_guard = content_mgr.begin_timeline_change().await;

    // 读取更新前的旧基线与旧补丁（用于无损回滚）
    let prev_origin = load_origin(&inst_dir)?;
    let prev_patches =
        patches::load_for_instance(&inst_dir).map_err(crate::install::InstallError::from)?;

    // 第一阶段：把所有将被覆盖或移除的现有文件先复制/备份进 ContentStore，构造完整回滚清单
    let mut restored_files = Vec::new();
    let mut added_files = Vec::new();
    let mut backed_up_paths: HashSet<String> = HashSet::new();

    for disk_rel in &removals {
        let abs = safe_join(&inst_dir, disk_rel)?;
        if abs.is_file() {
            let sha1 = sha1_file(&abs).await.map_err(|source| ModpackError::Io {
                path: abs.clone(),
                source,
            })?;
            store
                .add_copy(&abs, &sha1)
                .await
                .map_err(|source| ModpackError::Io { path: abs, source })?;
            if backed_up_paths.insert(disk_rel.to_lowercase()) {
                restored_files.push(RollbackFile {
                    path: disk_rel.clone(),
                    sha1,
                });
            }
        }
    }

    for write in &writes {
        if let Some(old_rel) = &write.disk_path
            && !old_rel.eq_ignore_ascii_case(&write.rel_path)
        {
            let old_abs = safe_join(&inst_dir, old_rel)?;
            if old_abs.is_file() {
                let sha1 = sha1_file(&old_abs)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: old_abs.clone(),
                        source,
                    })?;
                store
                    .add_copy(&old_abs, &sha1)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: old_abs,
                        source,
                    })?;
                if backed_up_paths.insert(old_rel.to_lowercase()) {
                    restored_files.push(RollbackFile {
                        path: old_rel.clone(),
                        sha1,
                    });
                }
            }
        }

        let target_abs = safe_join(&inst_dir, &write.rel_path)?;
        if target_abs.is_file() {
            let sha1 = sha1_file(&target_abs)
                .await
                .map_err(|source| ModpackError::Io {
                    path: target_abs.clone(),
                    source,
                })?;
            store
                .add_copy(&target_abs, &sha1)
                .await
                .map_err(|source| ModpackError::Io {
                    path: target_abs,
                    source,
                })?;
            if backed_up_paths.insert(write.rel_path.to_lowercase()) {
                restored_files.push(RollbackFile {
                    path: write.rel_path.clone(),
                    sha1,
                });
            }
        } else {
            added_files.push(RollbackFile {
                path: write.rel_path.clone(),
                sha1: write.sha1.clone(),
            });
        }
    }

    // 备份将被新包本地补丁库覆盖的 Lumi/libraries/* 文件，并记录新增补丁库以便回滚删除
    let mut local_lib_copies: Vec<(PathBuf, String)> = Vec::new();
    if let Some(libs_stage) = local_libs_staged {
        for rel_sub in walk_rel_files_sync(libs_stage) {
            let staged = libs_stage.join(&rel_sub);
            let inst_rel = format!("{META_DIR}/{LOCAL_LIBS_DIR}/{}", normalize_rel(&rel_sub));
            let target_abs = safe_join(&inst_dir, &inst_rel)?;
            if target_abs.is_file() {
                let sha1 = sha1_file(&target_abs)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: target_abs.clone(),
                        source,
                    })?;
                store
                    .add_copy(&target_abs, &sha1)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: target_abs,
                        source,
                    })?;
                if backed_up_paths.insert(inst_rel.to_lowercase()) {
                    restored_files.push(RollbackFile {
                        path: inst_rel.clone(),
                        sha1,
                    });
                }
            } else {
                let sha1 = sha1_file(&staged)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: staged.clone(),
                        source,
                    })?;
                added_files.push(RollbackFile {
                    path: inst_rel.clone(),
                    sha1,
                });
            }
            local_lib_copies.push((staged, inst_rel));
        }
    }

    let from_version = instance
        .config
        .source
        .as_ref()
        .map(|s| s.version.clone())
        .unwrap_or_else(|| instance.config.game_version.clone());
    let from_pack_name = instance
        .config
        .source
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| instance.config.name.clone());

    let mut rollback_record = PackRollbackRecord {
        format_version: MANIFEST_FORMAT,
        created_at: crate::now_millis() as f64,
        from_pack_name,
        from_pack_version: from_version.clone(),
        to_pack_version: plan.preview.target_pack_version.clone(),
        prev_game_version: instance.config.game_version.clone(),
        prev_loader: instance.config.loader.clone(),
        prev_version_id: instance.config.version_id.clone(),
        prev_source: instance.config.source.clone(),
        prev_recommended_memory_mb: instance.config.recommended_memory_mb,
        prev_origin,
        prev_patches,
        restored_files,
        added_files,
        in_progress: true,
    };

    // 在改动任何实例文件之前，先原子落盘带 `in_progress: true` 标记的回滚清单
    let rollback_path = inst_dir.join(META_DIR).join(ROLLBACK_FILE);
    write_json_atomic_async(&rollback_path, &rollback_record).await?;

    // 第二、三阶段包裹在事务块内：任何一步出错立即自动回滚，绝不留半新半旧实例
    let apply_result: Result<(u32, u32, u32, u32), ModpackError> = async {
        // 第二阶段：执行移除与原子写入
        let mut removed_done = 0u32;
        for disk_rel in &removals {
            let abs = safe_join(&inst_dir, disk_rel)?;
            if abs.is_file() {
                tokio::fs::remove_file(&abs)
                    .await
                    .map_err(|source| ModpackError::Io { path: abs, source })?;
                removed_done += 1;
            }
        }

        let mut added_done = 0u32;
        let mut updated_done = 0u32;
        let mut overwritten_modified = 0u32;
        for write in writes {
            if let Some(old_rel) = &write.disk_path
                && !old_rel.eq_ignore_ascii_case(&write.rel_path)
            {
                let old_abs = safe_join(&inst_dir, old_rel)?;
                if old_abs.is_file() {
                    tokio::fs::remove_file(&old_abs)
                        .await
                        .map_err(|source| ModpackError::Io {
                            path: old_abs,
                            source,
                        })?;
                }
            }
            let dest = safe_join(&inst_dir, &write.rel_path)?;
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: parent.to_path_buf(),
                        source,
                    })?;
            }
            // 同盘直接 rename，失败则放到阻塞线程池执行原子复制（避免卡住 Tokio 工作线程）
            if tokio::fs::rename(&write.staged_file, &dest).await.is_err() {
                let staged = write.staged_file.clone();
                let dest_copy = dest.clone();
                tokio::task::spawn_blocking(move || -> Result<(), ModpackError> {
                    let mut src =
                        std::fs::File::open(&staged).map_err(|source| ModpackError::Io {
                            path: staged,
                            source,
                        })?;
                    write_atomic_from(&dest_copy, &mut src)?;
                    Ok(())
                })
                .await
                .map_err(|e| ModpackError::Invalid(format!("写入文件任务失败：{e}")))??;
            }
            match write.kind {
                WriteKind::Added => added_done += 1,
                WriteKind::Updated => updated_done += 1,
                WriteKind::OverwrittenModified => overwritten_modified += 1,
            }
        }

        for (staged, inst_rel) in local_lib_copies {
            let dest = safe_join(&inst_dir, &inst_rel)?;
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: parent.to_path_buf(),
                        source,
                    })?;
            }
            if tokio::fs::rename(&staged, &dest).await.is_err() {
                let dest_copy = dest.clone();
                tokio::task::spawn_blocking(move || -> Result<(), ModpackError> {
                    let mut src =
                        std::fs::File::open(&staged).map_err(|source| ModpackError::Io {
                            path: staged,
                            source,
                        })?;
                    write_atomic_from(&dest_copy, &mut src)?;
                    Ok(())
                })
                .await
                .map_err(|e| ModpackError::Invalid(format!("写入补丁库失败：{e}")))??;
            }
        }

        // 第三阶段：更新基线清单（过滤被玩家跳过的手动补件）、补丁与 instance.json
        let mut effective_origin = plan.target_origin.clone();
        if !skipped_manual_keys.is_empty() {
            let prev_map: HashMap<String, (String, String)> = rollback_record
                .prev_origin
                .as_ref()
                .map(|po| {
                    po.files
                        .iter()
                        .map(|(k, v)| (normalize_rel(k).to_lowercase(), (k.clone(), v.clone())))
                        .collect()
                })
                .unwrap_or_default();
            effective_origin
                .files
                .retain(|k, _| !skipped_manual_keys.contains(&normalize_rel(k).to_lowercase()));
            for key_lower in skipped_manual_keys {
                if let Some((old_rel, old_sha1)) = prev_map.get(key_lower) {
                    effective_origin
                        .files
                        .insert(old_rel.clone(), old_sha1.clone());
                }
            }
        }
        let origin_path = inst_dir.join(META_DIR).join(ORIGIN_FILE);
        write_json_atomic_async(&origin_path, &effective_origin).await?;
        if let Some(bundle) = &plan.target_plan.patches {
            patches::save_for_instance(&inst_dir, bundle)?;
        } else {
            let patches_path = inst_dir.join(META_DIR).join(PATCHES_FILE);
            if patches_path.is_file() {
                tokio::fs::remove_file(&patches_path)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: patches_path,
                        source,
                    })?;
            }
        }

        let prev_cover_url = instance
            .config
            .source
            .as_ref()
            .and_then(|s| s.cover_url.clone());
        let next_source = PackSource {
            kind: plan.target_plan.preview.format.source_kind().to_owned(),
            name: plan.target_plan.preview.name.clone(),
            version: plan.target_plan.preview.version.clone(),
            project_id: plan.target_origin.project_id.clone(),
            version_id: plan.target_origin.version_id.clone(),
            cover_url: prev_cover_url,
        };
        let next_game_version = plan.target_plan.preview.game_version.clone();
        let next_loader = plan.target_plan.preview.loader.clone();
        let next_ram = plan
            .target_plan
            .preview
            .recommended_memory_mb
            .or(instance.config.recommended_memory_mb);

        instance::update(game, &instance.id, |config| {
            config.game_version = next_game_version;
            config.loader = next_loader;
            config.version_id = version_id;
            config.source = Some(next_source);
            config.recommended_memory_mb = next_ram;
        })?;

        Ok((removed_done, added_done, updated_done, overwritten_modified))
    }
    .await;

    let (removed_done, added_done, updated_done, overwritten_modified) = match apply_result {
        Ok(counts) => {
            // 全部落盘成功后清除事务进行中标记，保留记录供玩家手动一键回滚
            rollback_record.in_progress = false;
            write_json_atomic_async(&rollback_path, &rollback_record).await?;
            counts
        }
        Err(apply_err) => {
            tracing::warn!(
                instance = %instance.id,
                error = %apply_err,
                "整合包更新写盘阶段失败，正在自动回滚至更新前状态"
            );
            match rollback_with_record(game, &instance.id, &inst_dir, &rollback_record, false).await
            {
                Ok(_) => {
                    return Err(ModpackError::Invalid(format!(
                        "整合包更新中途失败，已自动恢复到更新前状态：{apply_err}"
                    )));
                }
                Err(rb_err) => {
                    tracing::error!(
                        instance = %instance.id,
                        apply_error = %apply_err,
                        rollback_error = %rb_err,
                        "自动回滚也未完成，保留事务标记待下次启动自动重试"
                    );
                    return Err(ModpackError::Invalid(format!(
                        "整合包更新中途失败（{apply_err}），自动恢复时也遇到问题（{rb_err}），下次打开启动器会自动重试恢复"
                    )));
                }
            }
        }
    };

    content_mgr
        .finish_timeline_change(
            tl_guard,
            format!("更新整合包至 {}", plan.preview.target_pack_version),
        )
        .await;

    tracing::info!(
        instance = %instance.id,
        from = %from_version,
        to = %plan.preview.target_pack_version,
        added = added_done,
        updated = updated_done,
        removed = removed_done,
        "整合包无损更新完成"
    );

    Ok(PackUpdateOutcome {
        instance_id: instance.id.clone(),
        from_version,
        to_version: plan.preview.target_pack_version.clone(),
        added: added_done,
        updated: updated_done,
        removed: removed_done,
        preserved_player_added: plan.preview.player_added_count,
        preserved_player_modified,
        overwritten_modified,
        skipped_manual,
    })
}

/// 一键无损回滚最近一次整合包更新：
/// 恢复所有旧文件与配置、移除本次更新新增的文件、还原 `pack-origin.json`、`patches.json` 与 `instance.json`。
pub async fn rollback(
    game: &GameDir,
    instance_id: &str,
) -> Result<PackRollbackOutcome, ModpackError> {
    let instance = instance::load(game, instance_id)?
        .ok_or_else(|| ModpackError::Invalid("找不到这个实例，可能已经被删除".to_owned()))?;
    if instance.config.link.is_some() {
        return Err(ModpackError::Invalid(
            "外部关联实例不支持回滚整合包更新".to_owned(),
        ));
    }
    let inst_dir = instance.game_dir(game)?;
    let record = load_rollback(&inst_dir)?
        .ok_or_else(|| ModpackError::Invalid("这个实例没有可回滚的整合包更新记录".to_owned()))?;
    rollback_with_record(game, instance_id, &inst_dir, &record, true).await
}

async fn rollback_with_record(
    game: &GameDir,
    instance_id: &str,
    inst_dir: &Path,
    record: &PackRollbackRecord,
    record_timeline: bool,
) -> Result<PackRollbackOutcome, ModpackError> {
    let store = ContentStore::new(game.root().join(STORE_DIR));
    let content_mgr = InstanceContent::for_game(game, inst_dir);
    let tl_guard = if record_timeline {
        Some(content_mgr.begin_timeline_change().await)
    } else {
        None
    };

    // 先验证所有待恢复对象都在 ContentStore 中完好无损，避免回滚到一半失败
    for file in &record.restored_files {
        safe_join(inst_dir, &file.path)?;
        if !store.contains(&file.sha1) {
            return Err(ModpackError::Invalid(format!(
                "回滚所需的旧文件缓存已缺失（{}），无法完整回滚",
                file.path
            )));
        }
    }
    for file in &record.added_files {
        safe_join(inst_dir, &file.path)?;
    }

    // 1. 先移除本次更新新增的文件（移进 ContentStore，即便玩家之后改过也不丢字节）
    let mut removed_count = 0u32;
    for file in &record.added_files {
        let abs = safe_join(inst_dir, &file.path)?;
        if abs.is_file() {
            store
                .take(&abs)
                .await
                .map_err(|source| ModpackError::Io { path: abs, source })?;
            removed_count += 1;
        }
    }

    // 2. 从 ContentStore 恢复所有更新前被覆盖或移除的旧文件
    let mut restored_count = 0u32;
    for file in &record.restored_files {
        let dest = safe_join(inst_dir, &file.path)?;
        store
            .restore(&file.sha1, &dest)
            .await
            .map_err(|source| ModpackError::Io { path: dest, source })?;
        restored_count += 1;
    }

    // 3. 恢复 Lumi/pack-origin.json
    let origin_path = inst_dir.join(META_DIR).join(ORIGIN_FILE);
    match &record.prev_origin {
        Some(prev) => save_origin(inst_dir, prev)?,
        None => {
            if origin_path.is_file() {
                tokio::fs::remove_file(&origin_path)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: origin_path,
                        source,
                    })?;
            }
        }
    }

    // 4. 恢复 Lumi/patches.json
    let patches_path = inst_dir.join(META_DIR).join(PATCHES_FILE);
    match &record.prev_patches {
        Some(prev) => patches::save_for_instance(inst_dir, prev)?,
        None => {
            if patches_path.is_file() {
                tokio::fs::remove_file(&patches_path)
                    .await
                    .map_err(|source| ModpackError::Io {
                        path: patches_path,
                        source,
                    })?;
            }
        }
    }

    // 5. 恢复 instance.json
    let prev_game_version = record.prev_game_version.clone();
    let prev_loader = record.prev_loader.clone();
    let prev_version_id = record.prev_version_id.clone();
    let prev_source = record.prev_source.clone();
    let prev_ram = record.prev_recommended_memory_mb;
    instance::update(game, instance_id, |config| {
        config.game_version = prev_game_version;
        config.loader = prev_loader;
        config.version_id = prev_version_id;
        config.source = prev_source;
        config.recommended_memory_mb = prev_ram;
    })?;

    // 6. 删除已消费的回滚记录，并将回滚后状态记入时间线
    let rollback_path = inst_dir.join(META_DIR).join(ROLLBACK_FILE);
    if rollback_path.is_file() {
        tokio::fs::remove_file(&rollback_path)
            .await
            .map_err(|source| ModpackError::Io {
                path: rollback_path,
                source,
            })?;
    }
    if let Some(guard) = tl_guard {
        let label = if record.in_progress {
            format!("自动恢复中断的整合包更新至 {}", record.from_pack_version)
        } else {
            format!("回滚整合包至 {}", record.from_pack_version)
        };
        content_mgr.finish_timeline_change(guard, label).await;
    }

    tracing::info!(
        instance = %instance_id,
        restored_version = %record.from_pack_version,
        reverted_from = %record.to_pack_version,
        restored_files = restored_count,
        removed_files = removed_count,
        auto_recovery = record.in_progress,
        "整合包更新已无损回滚"
    );

    Ok(PackRollbackOutcome {
        instance_id: instance_id.to_owned(),
        restored_version: record.from_pack_version.clone(),
        reverted_from: record.to_pack_version.clone(),
        restored_files: restored_count,
        removed_files: removed_count,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::instance::{InstanceConfig, LoaderKind};
    use crate::modpack::mrpack;

    fn sha1_of(bytes: &[u8]) -> String {
        let mut h = Sha1::new();
        h.update(bytes);
        to_hex(&h.finalize())
    }

    fn make_mrpack(
        dir: &Path,
        filename: &str,
        version_id: &str,
        mc_version: &str,
        loader_version: &str,
        downloads: &[(&str, &[u8])],
        overrides: &[(&str, &[u8])],
    ) -> PathBuf {
        let path = dir.join(filename);
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();

        let files_json: Vec<serde_json::Value> = downloads
            .iter()
            .map(|(rel, content)| {
                serde_json::json!({
                    "path": rel,
                    "hashes": { "sha1": sha1_of(content) },
                    "env": { "client": "required", "server": "required" },
                    "downloads": [format!("https://cdn.modrinth.com/data/test/{rel}")],
                    "fileSize": content.len(),
                })
            })
            .collect();
        let index = serde_json::json!({
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": version_id,
            "name": "Lumi Test Pack",
            "files": files_json,
            "dependencies": {
                "minecraft": mc_version,
                "fabric-loader": loader_version,
            }
        });
        zip.start_file("modrinth.index.json", options).unwrap();
        zip.write_all(serde_json::to_string(&index).unwrap().as_bytes())
            .unwrap();

        for (rel, content) in overrides {
            zip.start_file(format!("overrides/{rel}"), options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[tokio::test]
    async fn three_way_diff_update_and_lossless_rollback() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());
        let packs_dir = root.path().join("packs");
        std::fs::create_dir_all(&packs_dir).unwrap();

        // v1 文件内容
        let mod_keep_v1 = b"mod-keep-bytes-v1";
        let mod_upgrade_v1 = b"mod-upgrade-bytes-v1";
        let mod_removed_v1 = b"mod-removed-bytes-v1";
        let cfg_auto_v1 = b"auto=1\n";
        let cfg_edited_v1 = b"theme=dark\n";

        // v2 文件内容
        let mod_upgrade_v2 = b"mod-upgrade-bytes-v2-newer";
        let mod_added_v2 = b"mod-added-bytes-v2";
        let cfg_auto_v2 = b"auto=2\n";
        let cfg_edited_v2 = b"theme=light\n";

        // 1. 构造 v1 实例 + 写入 pack-origin.json
        let mut config = InstanceConfig::new(
            "Lumi Test Pack",
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::Fabric,
                version: "0.16.9".to_owned(),
            }),
        );
        config.source = Some(PackSource {
            kind: "mrpack".to_owned(),
            name: "Lumi Test Pack".to_owned(),
            version: "1.0.0".to_owned(),
            project_id: Some("proj123".to_owned()),
            version_id: Some("ver1".to_owned()),
            cover_url: Some("https://cdn.modrinth.com/cover.png".to_owned()),
        });
        let inst = instance::register(&game, "test-pack", &config).unwrap();
        let inst_dir = inst.game_dir(&game).unwrap();

        let write_inst = |rel: &str, bytes: &[u8]| {
            let p = inst_dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        write_inst("mods/keep.jar", mod_keep_v1);
        write_inst("mods/upgrade.jar", mod_upgrade_v1);
        write_inst("mods/removed.jar", mod_removed_v1);
        write_inst("config/auto.toml", cfg_auto_v1);
        write_inst("config/custom.toml", cfg_edited_v1);

        let v1_origin = PackOriginManifest {
            format_version: MANIFEST_FORMAT,
            kind: "mrpack".to_owned(),
            name: "Lumi Test Pack".to_owned(),
            version: "1.0.0".to_owned(),
            project_id: Some("proj123".to_owned()),
            version_id: Some("ver1".to_owned()),
            files: BTreeMap::from([
                ("mods/keep.jar".to_owned(), sha1_of(mod_keep_v1)),
                ("mods/upgrade.jar".to_owned(), sha1_of(mod_upgrade_v1)),
                ("mods/removed.jar".to_owned(), sha1_of(mod_removed_v1)),
                ("config/auto.toml".to_owned(), sha1_of(cfg_auto_v1)),
                ("config/custom.toml".to_owned(), sha1_of(cfg_edited_v1)),
            ]),
        };
        save_origin(&inst_dir, &v1_origin).unwrap();

        // 2. 玩家自己改动：自加 2 个模组、改了 config/custom.toml、建了存档 saves/World1
        let player_mod_1 = b"player-added-mod-1";
        let player_mod_2 = b"player-added-mod-2";
        let player_cfg = b"theme=lumi-custom-edited\n";
        let player_world = b"precious-world-level-dat";
        write_inst("mods/player-extra-1.jar", player_mod_1);
        write_inst("mods/player-extra-2.jar", player_mod_2);
        write_inst("config/custom.toml", player_cfg);
        write_inst("saves/World1/level.dat", player_world);

        // 3. 制作 v2 `.mrpack`（升级 1 个模组、新增 1 个模组、移除 1 个模组、更新 2 个配置、还带一个试图写 saves/ 的覆盖）
        let v2_pack = make_mrpack(
            &packs_dir,
            "pack-v2.mrpack",
            "2.0.0",
            "1.21.4",
            "0.16.10",
            &[
                ("mods/keep.jar", mod_keep_v1),
                ("mods/upgrade.jar", mod_upgrade_v2),
                ("mods/added.jar", mod_added_v2),
            ],
            &[
                ("config/auto.toml", cfg_auto_v2),
                ("config/custom.toml", cfg_edited_v2),
                ("saves/World1/level.dat", b"evil-overwrite-attempt"),
            ],
        );
        let mut target_plan = mrpack::inspect(&v2_pack).unwrap();
        target_plan.origin = Some(crate::modpack::PackOrigin {
            project_id: "proj123".to_owned(),
            version_id: "ver2".to_owned(),
        });

        // 4. 运行三方比对预览
        let update_plan = preview(
            &game,
            &inst,
            target_plan,
            Some("## 2.0.0\n- Upgraded!".to_owned()),
            BodyFormat::Markdown,
            None,
        )
        .await
        .unwrap();

        assert!(update_plan.preview.has_origin_baseline);
        assert!(update_plan.preview.has_worlds);
        assert!(update_plan.preview.mc_version_changed);
        assert!(!update_plan.preview.is_downgrade);
        assert_eq!(update_plan.preview.added_count, 1);
        assert_eq!(update_plan.preview.updated_count, 2); // mods/upgrade.jar + config/auto.toml
        assert_eq!(update_plan.preview.removed_count, 1); // mods/removed.jar
        assert_eq!(update_plan.preview.unchanged_count, 1); // mods/keep.jar
        assert_eq!(update_plan.preview.player_added_count, 2);
        assert_eq!(update_plan.preview.player_modified_count, 1); // config/custom.toml

        // 5. 模拟把目标下载/覆盖文件放到暂存目录并执行事务应用（默认不覆盖玩家改过的配置）
        let staging = tempfile::tempdir().unwrap();
        let stage_file = |rel: &str, bytes: &[u8]| {
            let p = staging.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let writes = vec![
            PreparedWrite {
                disk_path: Some("mods/upgrade.jar".to_owned()),
                rel_path: "mods/upgrade.jar".to_owned(),
                staged_file: stage_file("mods/upgrade.jar", mod_upgrade_v2),
                sha1: sha1_of(mod_upgrade_v2),
                kind: WriteKind::Updated,
            },
            PreparedWrite {
                disk_path: None,
                rel_path: "mods/added.jar".to_owned(),
                staged_file: stage_file("mods/added.jar", mod_added_v2),
                sha1: sha1_of(mod_added_v2),
                kind: WriteKind::Added,
            },
            PreparedWrite {
                disk_path: Some("config/auto.toml".to_owned()),
                rel_path: "config/auto.toml".to_owned(),
                staged_file: stage_file("config/auto.toml", cfg_auto_v2),
                sha1: sha1_of(cfg_auto_v2),
                kind: WriteKind::Updated,
            },
        ];
        let outcome = apply_staged_update(
            &game,
            &inst,
            &update_plan,
            "fabric-loader-0.16.10-1.21.4".to_owned(),
            writes,
            vec!["mods/removed.jar".to_owned()],
            1,
            0,
        )
        .await
        .unwrap();

        assert_eq!(outcome.added, 1);
        assert_eq!(outcome.updated, 2);
        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.preserved_player_added, 2);
        assert_eq!(outcome.preserved_player_modified, 1);

        // 验证更新后的磁盘状态
        assert_eq!(
            std::fs::read(inst_dir.join("mods/keep.jar")).unwrap(),
            mod_keep_v1
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/upgrade.jar")).unwrap(),
            mod_upgrade_v2
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/added.jar")).unwrap(),
            mod_added_v2
        );
        assert!(!inst_dir.join("mods/removed.jar").exists());
        assert_eq!(
            std::fs::read(inst_dir.join("config/auto.toml")).unwrap(),
            cfg_auto_v2
        );
        // 玩家改过的配置、自加模组、存档 100% 完整保留！
        assert_eq!(
            std::fs::read(inst_dir.join("config/custom.toml")).unwrap(),
            player_cfg
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/player-extra-1.jar")).unwrap(),
            player_mod_1
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/player-extra-2.jar")).unwrap(),
            player_mod_2
        );
        assert_eq!(
            std::fs::read(inst_dir.join("saves/World1/level.dat")).unwrap(),
            player_world
        );

        // 验证 instance.json 与封面 URL 保留
        let updated_inst = instance::load(&game, "test-pack").unwrap().unwrap();
        assert_eq!(updated_inst.config.game_version, "1.21.4");
        assert_eq!(
            updated_inst.config.source.as_ref().unwrap().version,
            "2.0.0"
        );
        assert_eq!(
            updated_inst
                .config
                .source
                .as_ref()
                .unwrap()
                .cover_url
                .as_deref(),
            Some("https://cdn.modrinth.com/cover.png")
        );
        assert!(rollback_summary(&inst_dir).is_some());

        // 6. 执行一键无损回滚！
        let rb = rollback(&game, "test-pack").await.unwrap();
        assert_eq!(rb.restored_version, "1.0.0");
        assert_eq!(rb.reverted_from, "2.0.0");
        assert_eq!(rb.restored_files, 3); // mods/removed.jar + mods/upgrade.jar + config/auto.toml
        assert_eq!(rb.removed_files, 1); // mods/added.jar
        assert!(rollback_summary(&inst_dir).is_none());

        // 验证回滚后的所有文件与元数据完全回到更新前状态！
        assert_eq!(
            std::fs::read(inst_dir.join("mods/keep.jar")).unwrap(),
            mod_keep_v1
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/upgrade.jar")).unwrap(),
            mod_upgrade_v1
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/removed.jar")).unwrap(),
            mod_removed_v1
        );
        assert!(!inst_dir.join("mods/added.jar").exists());
        assert_eq!(
            std::fs::read(inst_dir.join("config/auto.toml")).unwrap(),
            cfg_auto_v1
        );
        assert_eq!(
            std::fs::read(inst_dir.join("config/custom.toml")).unwrap(),
            player_cfg
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/player-extra-1.jar")).unwrap(),
            player_mod_1
        );
        assert_eq!(
            std::fs::read(inst_dir.join("mods/player-extra-2.jar")).unwrap(),
            player_mod_2
        );
        assert_eq!(
            std::fs::read(inst_dir.join("saves/World1/level.dat")).unwrap(),
            player_world
        );

        let rolled_inst = instance::load(&game, "test-pack").unwrap().unwrap();
        assert_eq!(rolled_inst.config.game_version, "1.21.1");
        assert_eq!(rolled_inst.config.source.as_ref().unwrap().version, "1.0.0");
        assert_eq!(load_origin(&inst_dir).unwrap().unwrap(), v1_origin);
    }

    #[tokio::test]
    async fn missing_origin_baseline_never_deletes_unknown_player_files() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());
        let packs_dir = root.path().join("packs");
        std::fs::create_dir_all(&packs_dir).unwrap();

        let config = InstanceConfig::new("Legacy Pack", "1.21.1", None);
        let inst = instance::register(&game, "legacy-pack", &config).unwrap();
        let inst_dir = inst.game_dir(&game).unwrap();
        std::fs::create_dir_all(inst_dir.join("mods")).unwrap();
        std::fs::write(inst_dir.join("mods/unknown-existing.jar"), b"keep-me").unwrap();

        let pack = make_mrpack(
            &packs_dir,
            "new.mrpack",
            "1.1.0",
            "1.21.1",
            "0.16.10",
            &[("mods/brand-new.jar", b"new-bytes")],
            &[],
        );
        let target_plan = mrpack::inspect(&pack).unwrap();
        let update_plan = preview(&game, &inst, target_plan, None, BodyFormat::Markdown, None)
            .await
            .unwrap();

        assert!(!update_plan.preview.has_origin_baseline);
        assert_eq!(update_plan.preview.removed_count, 0);
        assert_eq!(update_plan.preview.added_count, 1);
        assert_eq!(update_plan.preview.player_added_count, 1);
    }

    #[test]
    fn mc_downgrade_detection_works() {
        assert!(is_mc_downgrade("1.21.4", "1.21.1"));
        assert!(is_mc_downgrade("1.21.1", "1.20.4"));
        assert!(!is_mc_downgrade("1.21.1", "1.21.4"));
        assert!(!is_mc_downgrade("1.21.1", "1.21.1"));
    }

    #[tokio::test]
    async fn interrupted_pack_update_recovers_cleanly_on_startup() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());
        let mut config = InstanceConfig::new("Crash Pack", "1.21.1", None);
        config.source = Some(PackSource {
            kind: "mrpack".to_owned(),
            name: "Crash Pack".to_owned(),
            version: "1.0.0".to_owned(),
            project_id: Some("proj".to_owned()),
            version_id: Some("v1".to_owned()),
            cover_url: None,
        });
        let inst = instance::register(&game, "crash-pack", &config).unwrap();
        let inst_dir = inst.game_dir(&game).unwrap();
        std::fs::create_dir_all(inst_dir.join("mods")).unwrap();
        std::fs::create_dir_all(inst_dir.join(META_DIR)).unwrap();

        let old_mod_bytes = b"old-mod-v1";
        let old_mod_path = inst_dir.join("mods/core-1.0.jar");
        std::fs::write(&old_mod_path, old_mod_bytes).unwrap();
        let old_sha1 = sha1_file(&old_mod_path).await.unwrap();

        let store = ContentStore::new(game.root().join(STORE_DIR));
        store.add_copy(&old_mod_path, &old_sha1).await.unwrap();

        // 模拟写盘到一半崩溃：旧文件已被删除、新文件已写入、pack-rollback.json 仍标记 in_progress = true
        std::fs::remove_file(&old_mod_path).unwrap();
        std::fs::write(inst_dir.join("mods/core-2.0.jar"), b"partial-v2").unwrap();

        let record = PackRollbackRecord {
            format_version: MANIFEST_FORMAT,
            created_at: 1_700_000_000_000.0,
            from_pack_name: "Crash Pack".to_owned(),
            from_pack_version: "1.0.0".to_owned(),
            to_pack_version: "2.0.0".to_owned(),
            prev_game_version: "1.21.1".to_owned(),
            prev_loader: None,
            prev_version_id: "1.21.1".to_owned(),
            prev_source: config.source.clone(),
            prev_recommended_memory_mb: None,
            prev_origin: None,
            prev_patches: None,
            restored_files: vec![RollbackFile {
                path: "mods/core-1.0.jar".to_owned(),
                sha1: old_sha1,
            }],
            added_files: vec![RollbackFile {
                path: "mods/core-2.0.jar".to_owned(),
                sha1: "0000000000000000000000000000000000000000".to_owned(),
            }],
            in_progress: true,
        };
        write_json_atomic(&inst_dir.join(META_DIR).join(ROLLBACK_FILE), &record).unwrap();

        assert!(has_pending_update(&inst_dir));
        assert!(rollback_summary(&inst_dir).is_none());

        let recovered = recover_pending(&game, "crash-pack").await.unwrap().unwrap();
        assert_eq!(recovered.restored_version, "1.0.0");
        assert_eq!(recovered.reverted_from, "2.0.0");
        assert_eq!(recovered.restored_files, 1);
        assert_eq!(recovered.removed_files, 1);

        assert!(!has_pending_update(&inst_dir));
        assert!(!inst_dir.join("mods/core-2.0.jar").exists());
        assert_eq!(std::fs::read(&old_mod_path).unwrap(), old_mod_bytes);
    }

    #[tokio::test]
    async fn preserves_disabled_mod_state_across_renamed_version_update() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());
        let packs_dir = root.path().join("packs");
        std::fs::create_dir_all(&packs_dir).unwrap();

        let config = InstanceConfig::new("Disabled Test", "1.21.1", None);
        let inst = instance::register(&game, "disabled-test", &config).unwrap();
        let inst_dir = inst.game_dir(&game).unwrap();
        std::fs::create_dir_all(inst_dir.join("mods")).unwrap();
        let v1_bytes = b"sodium-v1-bytes";
        std::fs::write(
            inst_dir.join("mods/sodium-fabric-0.5.8.jar.disabled"),
            v1_bytes,
        )
        .unwrap();

        let mut origin_files = BTreeMap::new();
        origin_files.insert("mods/sodium-fabric-0.5.8.jar".to_owned(), sha1_of(v1_bytes));
        save_origin(
            &inst_dir,
            &PackOriginManifest {
                format_version: MANIFEST_FORMAT,
                kind: "mrpack".to_owned(),
                name: "Disabled Test".to_owned(),
                version: "1.0.0".to_owned(),
                project_id: None,
                version_id: None,
                files: origin_files,
            },
        )
        .unwrap();

        let pack = make_mrpack(
            &packs_dir,
            "v2.mrpack",
            "2.0.0",
            "1.21.1",
            "0.16.10",
            &[("mods/sodium-fabric-0.6.0.jar", b"sodium-v2-bytes")],
            &[],
        );
        let target_plan = mrpack::inspect(&pack).unwrap();
        let plan = preview(&game, &inst, target_plan, None, BodyFormat::Markdown, None)
            .await
            .unwrap();
        let install_op = plan.ops.iter().find_map(|op| match op {
            PlannedOp::Install(tf) if tf.rel_path == "mods/sodium-fabric-0.6.0.jar" => Some(tf),
            _ => None,
        });
        assert!(install_op.unwrap().keep_disabled);
    }

    #[tokio::test]
    async fn skipped_manual_file_is_not_written_into_pack_origin() {
        let root = tempfile::tempdir().unwrap();
        let game = GameDir::new(root.path());
        let inst = instance::register(
            &game,
            "manual-skip",
            &InstanceConfig::new("Manual Skip", "1.21.1", None),
        )
        .unwrap();
        let inst_dir = inst.game_dir(&game).unwrap();
        std::fs::create_dir_all(inst_dir.join(META_DIR)).unwrap();

        let packs_dir = root.path().join("packs");
        std::fs::create_dir_all(&packs_dir).unwrap();
        let pack = make_mrpack(
            &packs_dir,
            "v2.mrpack",
            "2.0.0",
            "1.21.1",
            "0.16.10",
            &[("mods/opt.jar", b"ok")],
            &[],
        );
        let mut target_plan = mrpack::inspect(&pack).unwrap();
        target_plan.preview.manual_files.push(ManualFile {
            project_id: 7,
            file_id: 42,
            project_name: "Restricted".to_owned(),
            display_name: "Restricted".to_owned(),
            file_name: "restricted.jar".to_owned(),
            size: 128.0,
            required: false,
            kind: crate::content::ContentKind::Mod,
            page_url: Some("https://www.curseforge.com/projects/7".to_owned()),
            icon_url: None,
            sha1: "1111111111111111111111111111111111111111".to_owned(),
            path: "mods/restricted.jar".to_owned(),
        });
        let update_plan = preview(&game, &inst, target_plan, None, BodyFormat::Markdown, None)
            .await
            .unwrap();

        let skipped = HashSet::from(["mods/restricted.jar".to_owned()]);
        apply_staged_update_with_extras(
            &game,
            &inst,
            &update_plan,
            "1.21.1".to_owned(),
            Vec::new(),
            Vec::new(),
            0,
            1,
            &skipped,
            None,
        )
        .await
        .unwrap();

        let saved_origin = load_origin(&inst_dir).unwrap().unwrap();
        assert!(!saved_origin.files.contains_key("mods/restricted.jar"));
        assert!(saved_origin.files.contains_key("mods/opt.jar"));
    }
}
