//! 本机跨实例同步（6 项）：纯本机、零云端、不破坏单实例独立性。
//!
//! 共享库位于 `<game_dir>/Lumi/shared/`：
//! - `options.txt`：以共享库中已启用键为单一真源跨版本安全合并（自动跳过版本号与资源包清单等危险键）；
//! - `servers.dat`：复用 NBT 引擎按地址无损去重合并，完整保留图标与原生属性；
//! - `resourcepacks/` & `shaderpacks/`：同盘优先硬链接（零额外占用）、跨盘自动回退复制，按清单追踪同步条目，绝不删玩家私有包；
//! - `command_history.txt`：按行去重合并（保留最近 200 条）；
//! - `hotbar.nbt`：安全镜像同步创造模式保存物品栏（首次覆盖前自动备份）；
//! - 首次修改任意实例的 `options.txt` / `servers.dat` / `hotbar.nbt` 前，先自动备份原文件至 `<instance>/Lumi/sync-backup/`。

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{OptionsSyncCategories, SyncSettings};
use crate::fs::{read_json, safe_file_name, write_atomic, write_json_atomic};
use crate::instance::{self, Instance, InstanceSyncExclusions, META_DIR};
use crate::paths::GameDir;
use crate::saves::servers::{self, ServerEntry, ServersError};

const SHARED_SUBDIR: &str = "shared";
const OPTIONS_FILE: &str = "options.txt";
const SERVERS_FILE: &str = "servers.dat";
const COMMAND_HISTORY_FILE: &str = "command_history.txt";
const HOTBAR_FILE: &str = "hotbar.nbt";
const RESOURCE_PACKS_DIR: &str = "resourcepacks";
const SHADER_PACKS_DIR: &str = "shaderpacks";
const FIRST_BACKUP_DIR: &str = "sync-backup";
const PACK_MANIFEST_FILE: &str = "sync-packs.json";
const MAX_COMMAND_HISTORY_LINES: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("找不到指定的源实例")]
    SourceNotFound,
    #[error("合并服务器列表失败：{0}")]
    Servers(#[from] ServersError),
    #[error("写入同步文件失败")]
    Write(#[from] crate::fs::AtomicWriteError),
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> SyncError + '_ {
    move |source| SyncError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 共享资源文件夹类型（供「打开共享文件夹」命令使用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum SharedFolderKind {
    Root,
    ResourcePacks,
    ShaderPacks,
}

/// 手动执行同步时的冲突处理策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum SyncStrategy {
    /// 智能合并（推荐）：先汇总各实例独有设置/服务器/命令记录到共享库，再推送到所有参与实例
    Merge,
    /// 以共享库为准覆盖所有参与实例的已启用项（首次覆盖前自动备份）
    OverwriteFromShared,
    /// 从指定实例采集设置与服务器更新共享库，再同步给其余参与实例
    PullFromInstance,
}

/// 实例同步总览（用于设置页顶部状态条与 6 行资产实时计数）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SyncOverview {
    pub participating_instances: u32,
    pub detached_instances: u32,
    pub linked_instances: u32,
    pub shared_options_keys: u32,
    pub shared_servers: u32,
    pub shared_resource_packs: u32,
    pub shared_shader_packs: u32,
    pub shared_commands: u32,
    pub shared_hotbar_bytes: Option<f64>,
}

/// 单个实例相对共享库的差异摘要
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct InstanceSyncDiff {
    pub instance_id: String,
    pub instance_name: String,
    /// 该实例与共享库在已启用设置下值不同的设置键数量
    pub options_conflicts: u32,
    /// 共享库有而该实例没有（或反之）的服务器数量
    pub servers_diff: u32,
    /// 待同步的通用资源包与光影包数量
    pub packs_pending: u32,
    /// 命令历史差异条数
    pub commands_diff: u32,
    /// 创造模式物品栏是否与共享库不一致
    pub hotbar_diff: bool,
    /// 是否尚未创建过首次原文件备份（首次写入时会自动创建）
    pub will_backup_first: bool,
}

/// 同步冲突与差异预览
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SyncPreview {
    pub instances: Vec<InstanceSyncDiff>,
    pub shared_has_data: bool,
}

/// 执行一次批量同步的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SyncApplyResult {
    pub updated_instances: u32,
    pub backups_created: u32,
    pub options_written: u32,
    pub servers_merged: u32,
    pub packs_synced: u32,
    pub commands_merged: u32,
    pub hotbars_synced: u32,
}

/// 绝不能跨版本强行同步的 `options.txt` 危险键（覆盖会导致旧版/新版实例崩或包顺序错乱）。
fn is_forbidden_option_key(key: &str) -> bool {
    matches!(
        key,
        "version"
            | "resourcePacks"
            | "incompatibleResourcePacks"
            | "lastServer"
            | "overrideWidth"
            | "overrideHeight"
            | "tutorialStep"
            | "onboardAccessibility"
            | "fullscreenResolution"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionCategory {
    Keybinds,
    Audio,
    Video,
    Accessibility,
}

/// 用于空库首次自动种子采集的默认分类判断。
fn classify_option_key(key: &str) -> Option<OptionCategory> {
    if is_forbidden_option_key(key) {
        return None;
    }
    if key.starts_with("key_") {
        return Some(OptionCategory::Keybinds);
    }
    if key.starts_with("soundCategory_")
        || matches!(key, "soundDevice" | "directionalAudio" | "showSubtitles")
    {
        return Some(OptionCategory::Audio);
    }
    if matches!(
        key,
        "fov"
            | "renderDistance"
            | "simulationDistance"
            | "entityDistanceScaling"
            | "maxFps"
            | "fullscreen"
            | "graphicsMode"
            | "guiScale"
            | "gamma"
            | "enableVsync"
            | "ao"
            | "biomeBlendRadius"
            | "mipmapLevels"
            | "particles"
            | "renderClouds"
            | "bobView"
            | "entityShadows"
            | "attackIndicator"
            | "fovEffectScale"
            | "screenEffectScale"
            | "darknessEffectScale"
            | "glintSpeed"
            | "glintStrength"
            | "prioritizeChunkUpdates"
    ) {
        return Some(OptionCategory::Video);
    }
    if key.starts_with("modelPart_")
        || matches!(
            key,
            "mainHand"
                | "lang"
                | "narrator"
                | "autoJump"
                | "toggleCrouch"
                | "toggleSprint"
                | "chatVisibility"
                | "chatColors"
                | "chatLinks"
                | "chatLinksPrompt"
                | "chatOpacity"
                | "chatLineSpacing"
                | "chatScale"
                | "chatWidth"
                | "chatHeightFocused"
                | "chatHeightUnfocused"
                | "textBackgroundOpacity"
                | "backgroundForChatOnly"
                | "highContrast"
                | "hideSplashTexts"
                | "damageTiltStrength"
                | "darkMojangStudiosBackground"
                | "forceUnicodeFont"
                | "mouseSensitivity"
                | "mouseWheelSensitivity"
                | "invertYMouse"
                | "rawMouseInput"
                | "operatorItemsTab"
        )
    {
        return Some(OptionCategory::Accessibility);
    }
    None
}

fn category_enabled(cat: OptionCategory, flags: OptionsSyncCategories) -> bool {
    match cat {
        OptionCategory::Keybinds => flags.keybinds,
        OptionCategory::Audio => flags.audio,
        OptionCategory::Video => flags.video,
        OptionCategory::Accessibility => flags.accessibility,
    }
}

fn key_in_seed_categories(key: &str, flags: OptionsSyncCategories) -> bool {
    classify_option_key(key).is_some_and(|cat| category_enabled(cat, flags))
}

/// 单个实例的有效同步配置（全局开关 ∧ ¬单实例排除开关）
#[derive(Debug, Clone, Copy)]
struct EffectiveSync {
    options: bool,
    servers: bool,
    resource_packs: bool,
    shader_packs: bool,
    command_history: bool,
    hotbar: bool,
    categories: OptionsSyncCategories,
}

fn effective_sync(settings: &SyncSettings, exclusions: &InstanceSyncExclusions) -> EffectiveSync {
    EffectiveSync {
        options: settings.options && !exclusions.options,
        servers: settings.servers && !exclusions.servers,
        resource_packs: settings.resource_packs && !exclusions.resource_packs,
        shader_packs: settings.shader_packs && !exclusions.shader_packs,
        command_history: settings.command_history && !exclusions.command_history,
        hotbar: settings.hotbar && !exclusions.hotbar,
        categories: settings.options_categories,
    }
}

/// 共享库根目录：`<game_dir>/Lumi/shared`
pub fn shared_root(game: &GameDir) -> PathBuf {
    game.root().join(META_DIR).join(SHARED_SUBDIR)
}

pub fn shared_folder_path(game: &GameDir, kind: SharedFolderKind) -> PathBuf {
    let root = shared_root(game);
    match kind {
        SharedFolderKind::Root => root,
        SharedFolderKind::ResourcePacks => root.join(RESOURCE_PACKS_DIR),
        SharedFolderKind::ShaderPacks => root.join(SHADER_PACKS_DIR),
    }
}

/// 实例是否参与跨实例同步：仅 Lumi 管理的实例且未单独关闭同步。
pub fn is_participating(instance: &Instance) -> bool {
    !instance.is_linked() && !instance.config.overrides.sync_disabled
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncPackManifest {
    #[serde(default)]
    resource_packs: Vec<String>,
    #[serde(default)]
    shader_packs: Vec<String>,
}

/// 读取 `options.txt` 中的所有 `key:value` 对（忽略空行和非法行）。
fn read_options_map(path: &Path) -> BTreeMap<String, String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut map = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            if !key.is_empty() {
                map.insert(key.to_owned(), v.to_owned());
            }
        }
    }
    map
}

/// 将 `updates` 中满足 `predicate` 的键写入 `target` 的 `options.txt`（保留原有顺序与所有未同步键）。
fn write_options_filtered<F>(
    target: &Path,
    updates: &BTreeMap<String, String>,
    predicate: F,
    overwrite_existing: bool,
) -> Result<usize, SyncError>
where
    F: Fn(&str) -> bool,
{
    let filtered: BTreeMap<&str, &str> = updates
        .iter()
        .filter(|(k, _)| !is_forbidden_option_key(k) && predicate(k))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if filtered.is_empty() {
        return Ok(0);
    }

    let existing_text = match std::fs::read_to_string(target) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(SyncError::Io {
                path: target.to_path_buf(),
                source,
            });
        }
    };

    let mut lines: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut changed = 0usize;

    for raw_line in existing_text.lines() {
        if let Some((k, old_val)) = raw_line.split_once(':') {
            let key = k.trim();
            if let Some(&new_val) = filtered.get(key) {
                seen.insert(key.to_owned());
                if overwrite_existing && old_val != new_val {
                    lines.push(format!("{key}:{new_val}"));
                    changed += 1;
                    continue;
                }
            }
        }
        lines.push(raw_line.to_owned());
    }

    for (key, new_val) in filtered {
        if !seen.contains(key) {
            lines.push(format!("{key}:{new_val}"));
            changed += 1;
        }
    }

    if changed == 0 {
        return Ok(0);
    }
    let mut output = lines.join("\n");
    output.push('\n');
    write_atomic(target, output.as_bytes())?;
    Ok(changed)
}

/// 读取 `command_history.txt`（保留非空行并保序去重，越新越靠后）。
fn read_command_lines(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    dedupe_commands(content.lines().map(str::to_owned))
}

fn dedupe_commands(lines: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in lines {
        let cleaned = raw.trim().replace(['\r', '\n'], "");
        if cleaned.is_empty() {
            continue;
        }
        out.retain(|existing| existing != &cleaned);
        out.push(cleaned);
    }
    if out.len() > MAX_COMMAND_HISTORY_LINES {
        let excess = out.len() - MAX_COMMAND_HISTORY_LINES;
        out.drain(0..excess);
    }
    out
}

fn write_command_lines(path: &Path, lines: &[String]) -> Result<(), SyncError> {
    let cleaned = dedupe_commands(lines.iter().cloned());
    if cleaned.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    let mut out = cleaned.join("\n");
    out.push('\n');
    write_atomic(path, out.as_bytes())?;
    Ok(())
}

/// 合并 `src` 的命令历史到 `dst`（保留最新顺序并去重）；返回新增条数。
fn merge_command_files(src: &Path, dst: &Path) -> Result<usize, SyncError> {
    let src_lines = read_command_lines(src);
    if src_lines.is_empty() {
        return Ok(0);
    }
    let dst_lines = read_command_lines(dst);
    let dst_set: BTreeSet<&str> = dst_lines.iter().map(String::as_str).collect();
    let added = src_lines
        .iter()
        .filter(|line| !dst_set.contains(line.as_str()))
        .count();
    if added == 0 {
        return Ok(0);
    }
    let merged = dedupe_commands(dst_lines.into_iter().chain(src_lines));
    write_command_lines(dst, &merged)?;
    Ok(added)
}

fn files_identical(a: &Path, b: &Path) -> bool {
    let (Ok(bytes_a), Ok(bytes_b)) = (std::fs::read(a), std::fs::read(b)) else {
        return false;
    };
    bytes_a == bytes_b
}

/// 首次修改实例的 `options.txt` / `servers.dat` / `hotbar.nbt` 前自动备份原始文件。
fn ensure_first_backup(instance_dir: &Path, file_name: &str) -> Result<bool, SyncError> {
    let source = instance_dir.join(file_name);
    if !source.is_file() {
        return Ok(false);
    }
    let backup_path = instance_dir
        .join(META_DIR)
        .join(FIRST_BACKUP_DIR)
        .join(file_name);
    if backup_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    std::fs::copy(&source, &backup_path).map_err(io_err(&backup_path))?;
    Ok(true)
}

fn list_pack_entries(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if meta.is_file() || meta.is_dir() {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

fn sync_pack_dir(
    shared_dir: &Path,
    instance_dir: &Path,
    subdir: &str,
    tracked: &mut Vec<String>,
) -> Result<usize, SyncError> {
    let shared_entries = list_pack_entries(shared_dir);
    let shared_set: BTreeSet<&str> = shared_entries.iter().map(String::as_str).collect();
    let dst_dir = instance_dir.join(subdir);
    let mut synced = 0usize;

    // 1. 仅移除之前由 Lumi 同步过、现在已从共享文件夹删掉的条目（绝不删玩家私有包）
    for old in tracked.iter() {
        if !shared_set.contains(old.as_str()) {
            let target = dst_dir.join(old);
            if target.is_dir() {
                let _ = std::fs::remove_dir_all(&target);
            } else if target.is_file() {
                let _ = std::fs::remove_file(&target);
            }
        }
    }

    let mut next_tracked = Vec::new();
    if !shared_entries.is_empty() {
        std::fs::create_dir_all(&dst_dir).map_err(io_err(&dst_dir))?;
    }
    for name in shared_entries {
        let src = shared_dir.join(&name);
        let dst = dst_dir.join(&name);
        let already_tracked = tracked.iter().any(|item| item == &name);
        // 如果实例里已经存在同名文件/目录但并非由同步放进去的，不强制覆写，保护玩家自定义版本
        if dst.exists() && !already_tracked {
            continue;
        }
        if needs_pack_refresh(&src, &dst) {
            if dst.is_dir() {
                let _ = std::fs::remove_dir_all(&dst);
            } else if dst.is_file() {
                let _ = std::fs::remove_file(&dst);
            }
            link_or_copy_entry(&src, &dst)?;
            synced += 1;
        }
        next_tracked.push(name);
    }

    *tracked = next_tracked;
    Ok(synced)
}

fn needs_pack_refresh(src: &Path, dst: &Path) -> bool {
    let Ok(src_meta) = std::fs::metadata(src) else {
        return false;
    };
    let Ok(dst_meta) = std::fs::metadata(dst) else {
        return true;
    };
    if src_meta.is_dir() != dst_meta.is_dir() {
        return true;
    }
    if src_meta.is_file() {
        if src_meta.len() != dst_meta.len() {
            return true;
        }
        if let (Ok(sm), Ok(dm)) = (src_meta.modified(), dst_meta.modified()) {
            return sm != dm;
        }
    }
    false
}

fn link_or_copy_entry(src: &Path, dst: &Path) -> Result<(), SyncError> {
    let meta = std::fs::metadata(src).map_err(io_err(src))?;
    if meta.is_file() {
        if std::fs::hard_link(src, dst).is_ok() {
            return Ok(());
        }
        std::fs::copy(src, dst).map_err(io_err(dst))?;
        return Ok(());
    }
    if meta.is_dir() {
        copy_dir_recursive(src, dst)?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), SyncError> {
    std::fs::create_dir_all(dst).map_err(io_err(dst))?;
    for entry in std::fs::read_dir(src).map_err(io_err(src))?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if meta.is_file() && std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to).map_err(io_err(&to))?;
        }
    }
    Ok(())
}

/// 统计当前同步总览（参与实例数、独立实例数、共享库 6 类资产数量）。
pub fn sync_overview(game: &GameDir, _settings: &SyncSettings) -> SyncOverview {
    let instances = instance::list(game);
    let mut participating = 0u32;
    let mut detached = 0u32;
    let mut linked = 0u32;
    for inst in &instances {
        if inst.is_linked() {
            linked += 1;
        } else if inst.config.overrides.sync_disabled {
            detached += 1;
        } else {
            participating += 1;
        }
    }
    let root = shared_root(game);
    let shared_opts = read_options_map(&root.join(OPTIONS_FILE));
    let shared_options_keys = shared_opts
        .keys()
        .filter(|k| !is_forbidden_option_key(k))
        .count();
    let shared_servers = servers::count_file(&root.join(SERVERS_FILE));
    let shared_resource_packs = list_pack_entries(&root.join(RESOURCE_PACKS_DIR)).len();
    let shared_shader_packs = list_pack_entries(&root.join(SHADER_PACKS_DIR)).len();
    let shared_commands = read_command_lines(&root.join(COMMAND_HISTORY_FILE)).len();
    let shared_hotbar_bytes = std::fs::metadata(root.join(HOTBAR_FILE))
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len() as f64);

    SyncOverview {
        participating_instances: participating,
        detached_instances: detached,
        linked_instances: linked,
        shared_options_keys: u32::try_from(shared_options_keys).unwrap_or(u32::MAX),
        shared_servers: u32::try_from(shared_servers).unwrap_or(u32::MAX),
        shared_resource_packs: u32::try_from(shared_resource_packs).unwrap_or(u32::MAX),
        shared_shader_packs: u32::try_from(shared_shader_packs).unwrap_or(u32::MAX),
        shared_commands: u32::try_from(shared_commands).unwrap_or(u32::MAX),
        shared_hotbar_bytes,
    }
}

/// 预览各参与实例与共享库之间的差异与潜在冲突（只读，不改动任何文件）。
pub fn sync_preview(game: &GameDir, settings: &SyncSettings) -> SyncPreview {
    let root = shared_root(game);
    let shared_opts = read_options_map(&root.join(OPTIONS_FILE));
    let shared_servers_path = root.join(SERVERS_FILE);
    let shared_rp = list_pack_entries(&root.join(RESOURCE_PACKS_DIR));
    let shared_sp = list_pack_entries(&root.join(SHADER_PACKS_DIR));
    let shared_cmds = read_command_lines(&root.join(COMMAND_HISTORY_FILE));
    let shared_hotbar_path = root.join(HOTBAR_FILE);

    let shared_has_data = shared_opts.keys().any(|k| !is_forbidden_option_key(k))
        || servers::count_file(&shared_servers_path) > 0
        || !shared_rp.is_empty()
        || !shared_sp.is_empty()
        || !shared_cmds.is_empty()
        || shared_hotbar_path.is_file();

    let mut diffs = Vec::new();
    for inst in instance::list(game) {
        if !is_participating(&inst) {
            continue;
        }
        let Ok(dir) = inst.game_dir(game) else {
            continue;
        };
        let eff = effective_sync(settings, &inst.config.overrides.sync_exclusions);

        let mut options_conflicts = 0usize;
        if eff.options {
            let inst_opts = read_options_map(&dir.join(OPTIONS_FILE));
            for (k, shared_val) in &shared_opts {
                if is_forbidden_option_key(k) {
                    continue;
                }
                match inst_opts.get(k) {
                    Some(v) if v != shared_val => options_conflicts += 1,
                    None => options_conflicts += 1,
                    _ => {}
                }
            }
        }

        let servers_diff = if eff.servers {
            let inst_servers = dir.join(SERVERS_FILE);
            servers::diff_missing_count(&shared_servers_path, &inst_servers)
                + servers::diff_missing_count(&inst_servers, &shared_servers_path)
        } else {
            0
        };

        let mut packs_pending = 0usize;
        if eff.resource_packs {
            let dst = dir.join(RESOURCE_PACKS_DIR);
            packs_pending += shared_rp
                .iter()
                .filter(|name| {
                    needs_pack_refresh(&root.join(RESOURCE_PACKS_DIR).join(name), &dst.join(name))
                })
                .count();
        }
        if eff.shader_packs {
            let dst = dir.join(SHADER_PACKS_DIR);
            packs_pending += shared_sp
                .iter()
                .filter(|name| {
                    needs_pack_refresh(&root.join(SHADER_PACKS_DIR).join(name), &dst.join(name))
                })
                .count();
        }

        let commands_diff = if eff.command_history {
            let inst_cmds = read_command_lines(&dir.join(COMMAND_HISTORY_FILE));
            let inst_set: BTreeSet<&str> = inst_cmds.iter().map(String::as_str).collect();
            let shared_set: BTreeSet<&str> = shared_cmds.iter().map(String::as_str).collect();
            shared_cmds
                .iter()
                .filter(|c| !inst_set.contains(c.as_str()))
                .count()
                + inst_cmds
                    .iter()
                    .filter(|c| !shared_set.contains(c.as_str()))
                    .count()
        } else {
            0
        };

        let hotbar_diff = if eff.hotbar {
            let inst_hotbar = dir.join(HOTBAR_FILE);
            match (shared_hotbar_path.is_file(), inst_hotbar.is_file()) {
                (true, true) => !files_identical(&shared_hotbar_path, &inst_hotbar),
                (true, false) | (false, true) => true,
                (false, false) => false,
            }
        } else {
            false
        };

        let backup_root = dir.join(META_DIR).join(FIRST_BACKUP_DIR);
        let will_backup_first = (eff.options
            && dir.join(OPTIONS_FILE).is_file()
            && !backup_root.join(OPTIONS_FILE).exists())
            || (eff.servers
                && dir.join(SERVERS_FILE).is_file()
                && !backup_root.join(SERVERS_FILE).exists())
            || (eff.hotbar
                && dir.join(HOTBAR_FILE).is_file()
                && !backup_root.join(HOTBAR_FILE).exists());

        diffs.push(InstanceSyncDiff {
            instance_id: inst.id,
            instance_name: inst.config.name,
            options_conflicts: u32::try_from(options_conflicts).unwrap_or(u32::MAX),
            servers_diff: u32::try_from(servers_diff).unwrap_or(u32::MAX),
            packs_pending: u32::try_from(packs_pending).unwrap_or(u32::MAX),
            commands_diff: u32::try_from(commands_diff).unwrap_or(u32::MAX),
            hotbar_diff,
            will_backup_first,
        });
    }

    SyncPreview {
        instances: diffs,
        shared_has_data,
    }
}

/// 按所选策略立即执行一次跨实例同步。
pub fn sync_apply(
    game: &GameDir,
    settings: &SyncSettings,
    strategy: SyncStrategy,
    source_instance_id: Option<&str>,
) -> Result<SyncApplyResult, SyncError> {
    let root = shared_root(game);
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    let shared_opts_path = root.join(OPTIONS_FILE);
    let shared_servers_path = root.join(SERVERS_FILE);
    let shared_cmds_path = root.join(COMMAND_HISTORY_FILE);
    let shared_hotbar_path = root.join(HOTBAR_FILE);

    let instances: Vec<Instance> = instance::list(game)
        .into_iter()
        .filter(is_participating)
        .collect();

    // 阶段 1：更新共享库
    match strategy {
        SyncStrategy::Merge => {
            let shared_started_empty = read_options_map(&shared_opts_path).is_empty();
            for inst in &instances {
                let Ok(dir) = inst.game_dir(game) else {
                    continue;
                };
                let eff = effective_sync(settings, &inst.config.overrides.sync_exclusions);
                if eff.options && shared_started_empty {
                    let inst_opts = read_options_map(&dir.join(OPTIONS_FILE));
                    // 仅当共享库原本为空时从实例采集种子键；共享库已有明确配置时尊重用户的单项链接选择
                    write_options_filtered(
                        &shared_opts_path,
                        &inst_opts,
                        |k| key_in_seed_categories(k, eff.categories),
                        false,
                    )?;
                }
                if eff.servers {
                    servers::merge_files(&dir.join(SERVERS_FILE), &shared_servers_path)?;
                }
                if eff.command_history {
                    merge_command_files(&dir.join(COMMAND_HISTORY_FILE), &shared_cmds_path)?;
                }
                if eff.hotbar && !shared_hotbar_path.is_file() {
                    let src_hotbar = dir.join(HOTBAR_FILE);
                    if src_hotbar.is_file() {
                        std::fs::copy(&src_hotbar, &shared_hotbar_path)
                            .map_err(io_err(&shared_hotbar_path))?;
                    }
                }
            }
        }
        SyncStrategy::PullFromInstance => {
            let src_id = source_instance_id.ok_or(SyncError::SourceNotFound)?;
            let src_inst = instances
                .iter()
                .find(|inst| inst.id == src_id)
                .ok_or(SyncError::SourceNotFound)?;
            let src_dir = src_inst
                .game_dir(game)
                .map_err(|_| SyncError::SourceNotFound)?;
            if settings.options {
                let src_opts = read_options_map(&src_dir.join(OPTIONS_FILE));
                write_options_filtered(
                    &shared_opts_path,
                    &src_opts,
                    |k| classify_option_key(k).is_some(),
                    true,
                )?;
            }
            if settings.servers {
                let src_servers = src_dir.join(SERVERS_FILE);
                if src_servers.is_file() {
                    servers::overwrite_file(&src_servers, &shared_servers_path)?;
                }
            }
            if settings.command_history {
                let src_cmds = read_command_lines(&src_dir.join(COMMAND_HISTORY_FILE));
                write_command_lines(&shared_cmds_path, &src_cmds)?;
            }
            if settings.hotbar {
                let src_hotbar = src_dir.join(HOTBAR_FILE);
                if src_hotbar.is_file() {
                    std::fs::copy(&src_hotbar, &shared_hotbar_path)
                        .map_err(io_err(&shared_hotbar_path))?;
                }
            }
        }
        SyncStrategy::OverwriteFromShared => {}
    }

    // 阶段 2：将共享库下发到所有参与同步的实例
    let overwrite_servers = matches!(
        strategy,
        SyncStrategy::OverwriteFromShared | SyncStrategy::PullFromInstance
    );
    let mut updated_instances = 0u32;
    let mut backups_created = 0u32;
    let mut options_written = 0u32;
    let mut servers_merged = 0u32;
    let mut packs_synced = 0u32;
    let mut commands_merged = 0u32;
    let mut hotbars_synced = 0u32;

    for inst in &instances {
        let Ok(dir) = inst.game_dir(game) else {
            continue;
        };
        let eff = effective_sync(settings, &inst.config.overrides.sync_exclusions);
        let stats = push_shared_to_instance(&root, &dir, eff, overwrite_servers)?;
        if stats.changed() {
            updated_instances += 1;
        }
        backups_created += stats.backups_created;
        options_written += stats.options_written;
        servers_merged += stats.servers_merged;
        packs_synced += stats.packs_synced;
        commands_merged += stats.commands_merged;
        hotbars_synced += stats.hotbars_synced;
    }

    Ok(SyncApplyResult {
        updated_instances,
        backups_created,
        options_written,
        servers_merged,
        packs_synced,
        commands_merged,
        hotbars_synced,
    })
}

#[derive(Default)]
struct PushStats {
    backups_created: u32,
    options_written: u32,
    servers_merged: u32,
    packs_synced: u32,
    commands_merged: u32,
    hotbars_synced: u32,
}

impl PushStats {
    fn changed(&self) -> bool {
        self.options_written > 0
            || self.servers_merged > 0
            || self.packs_synced > 0
            || self.commands_merged > 0
            || self.hotbars_synced > 0
    }
}

fn push_shared_to_instance(
    shared_root: &Path,
    instance_dir: &Path,
    eff: EffectiveSync,
    overwrite_servers: bool,
) -> Result<PushStats, SyncError> {
    let mut stats = PushStats::default();

    if eff.options {
        let shared_opts = read_options_map(&shared_root.join(OPTIONS_FILE));
        let target = instance_dir.join(OPTIONS_FILE);
        if !shared_opts.is_empty() {
            let inst_opts = read_options_map(&target);
            let will_change = shared_opts.iter().any(|(k, v)| {
                !is_forbidden_option_key(k) && inst_opts.get(k).is_none_or(|existing| existing != v)
            });
            if will_change {
                if ensure_first_backup(instance_dir, OPTIONS_FILE)? {
                    stats.backups_created += 1;
                }
                let written = write_options_filtered(&target, &shared_opts, |_| true, true)?;
                stats.options_written += u32::try_from(written).unwrap_or(u32::MAX);
            }
        }
    }

    if eff.servers {
        let shared_servers = shared_root.join(SERVERS_FILE);
        let target = instance_dir.join(SERVERS_FILE);
        if shared_servers.is_file() {
            let need_write = if overwrite_servers {
                true
            } else {
                servers::diff_missing_count(&shared_servers, &target) > 0
            };
            if need_write {
                if ensure_first_backup(instance_dir, SERVERS_FILE)? {
                    stats.backups_created += 1;
                }
                let count = if overwrite_servers {
                    servers::overwrite_file(&shared_servers, &target)?
                } else {
                    servers::merge_files(&shared_servers, &target)?
                };
                stats.servers_merged += u32::try_from(count).unwrap_or(u32::MAX);
            }
        }
    }

    if eff.resource_packs || eff.shader_packs {
        let manifest_path = instance_dir.join(META_DIR).join(PACK_MANIFEST_FILE);
        let mut manifest: SyncPackManifest =
            read_json(&manifest_path).ok().flatten().unwrap_or_default();
        let before = manifest.clone();
        if eff.resource_packs {
            let synced = sync_pack_dir(
                &shared_root.join(RESOURCE_PACKS_DIR),
                instance_dir,
                RESOURCE_PACKS_DIR,
                &mut manifest.resource_packs,
            )?;
            stats.packs_synced += u32::try_from(synced).unwrap_or(u32::MAX);
        }
        if eff.shader_packs {
            let synced = sync_pack_dir(
                &shared_root.join(SHADER_PACKS_DIR),
                instance_dir,
                SHADER_PACKS_DIR,
                &mut manifest.shader_packs,
            )?;
            stats.packs_synced += u32::try_from(synced).unwrap_or(u32::MAX);
        }
        if manifest.resource_packs != before.resource_packs
            || manifest.shader_packs != before.shader_packs
        {
            write_json_atomic(&manifest_path, &manifest)?;
        }
    }

    if eff.command_history {
        let shared_cmds = shared_root.join(COMMAND_HISTORY_FILE);
        if shared_cmds.is_file() {
            let added =
                merge_command_files(&shared_cmds, &instance_dir.join(COMMAND_HISTORY_FILE))?;
            stats.commands_merged += u32::try_from(added).unwrap_or(u32::MAX);
        }
    }

    if eff.hotbar {
        let shared_hotbar = shared_root.join(HOTBAR_FILE);
        let target = instance_dir.join(HOTBAR_FILE);
        if shared_hotbar.is_file() && !files_identical(&shared_hotbar, &target) {
            if ensure_first_backup(instance_dir, HOTBAR_FILE)? {
                stats.backups_created += 1;
            }
            let bytes = std::fs::read(&shared_hotbar).map_err(io_err(&shared_hotbar))?;
            write_atomic(&target, &bytes)?;
            stats.hotbars_synced += 1;
        }
    }

    Ok(stats)
}

/// 共享资源包 / 光影包条目（用于可视化管理弹窗）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SharedPackEntry {
    pub name: String,
    pub is_dir: bool,
    pub bytes: f64,
}

/// 共享服务器条目（用于可视化管理弹窗）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SharedServerEntry {
    pub name: String,
    pub address: String,
}

impl From<ServerEntry> for SharedServerEntry {
    fn from(entry: ServerEntry) -> Self {
        Self {
            name: entry.name,
            address: entry.address,
        }
    }
}

/// 共享库读取：当前已同步的 `options.txt` 全部键值
pub fn read_shared_options(game: &GameDir) -> BTreeMap<String, String> {
    read_options_map(&shared_root(game).join(OPTIONS_FILE))
}

/// 共享库保存：整体原子写回 `options.txt`（支持在可视化编辑器中增改或取消单条同步）
pub fn write_shared_options(
    game: &GameDir,
    entries: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, SyncError> {
    let root = shared_root(game);
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    let mut cleaned = BTreeMap::new();
    for (k, v) in entries {
        let key = k.trim();
        if key.is_empty() || key.contains([':', '\r', '\n']) || is_forbidden_option_key(key) {
            continue;
        }
        let val: String = v.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
        cleaned.insert(key.to_owned(), val);
    }
    let path = root.join(OPTIONS_FILE);
    if cleaned.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        let mut lines: Vec<String> = cleaned.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        lines.sort();
        let mut out = lines.join("\n");
        out.push('\n');
        write_atomic(&path, out.as_bytes())?;
    }
    Ok(cleaned)
}

/// 读取共享库中的服务器列表
pub fn list_shared_servers(game: &GameDir) -> Vec<SharedServerEntry> {
    servers::list_file(&shared_root(game).join(SERVERS_FILE))
        .into_iter()
        .map(SharedServerEntry::from)
        .collect()
}

/// 向共享库添加一条服务器
pub fn add_shared_server(
    game: &GameDir,
    name: &str,
    address: &str,
) -> Result<Vec<SharedServerEntry>, SyncError> {
    let root = shared_root(game);
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    servers::add_to_file(&root.join(SERVERS_FILE), name, address)?;
    Ok(list_shared_servers(game))
}

/// 从共享库删除一条服务器
pub fn remove_shared_server(
    game: &GameDir,
    address: &str,
) -> Result<Vec<SharedServerEntry>, SyncError> {
    let path = shared_root(game).join(SERVERS_FILE);
    servers::remove_from_file(&path, address)?;
    Ok(list_shared_servers(game))
}

/// 读取共享库中的命令历史记录
pub fn read_shared_commands(game: &GameDir) -> Vec<String> {
    read_command_lines(&shared_root(game).join(COMMAND_HISTORY_FILE))
}

/// 保存共享库中的命令历史记录
pub fn write_shared_commands(game: &GameDir, lines: &[String]) -> Result<Vec<String>, SyncError> {
    let root = shared_root(game);
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    let cleaned = dedupe_commands(lines.iter().cloned());
    write_command_lines(&root.join(COMMAND_HISTORY_FILE), &cleaned)?;
    Ok(cleaned)
}

/// 清空共享库中的创造模式物品栏快照
pub fn clear_shared_hotbar(game: &GameDir) -> Result<(), SyncError> {
    let path = shared_root(game).join(HOTBAR_FILE);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(io_err(&path))?;
    }
    Ok(())
}

/// 列出共享资源包或共享光影包文件夹中的条目与大小
pub fn list_shared_packs(game: &GameDir, kind: SharedFolderKind) -> Vec<SharedPackEntry> {
    if kind == SharedFolderKind::Root {
        return Vec::new();
    }
    let dir = shared_folder_path(game, kind);
    list_pack_entries(&dir)
        .into_iter()
        .map(|name| {
            let path = dir.join(&name);
            let is_dir = path.is_dir();
            let bytes = entry_size_bytes(&path) as f64;
            SharedPackEntry {
                name,
                is_dir,
                bytes,
            }
        })
        .collect()
}

/// 从共享资源包或共享光影包文件夹安全删除单个包
pub fn remove_shared_pack(
    game: &GameDir,
    kind: SharedFolderKind,
    name: &str,
) -> Result<Vec<SharedPackEntry>, SyncError> {
    if kind == SharedFolderKind::Root {
        return Ok(Vec::new());
    }
    let clean_name = safe_file_name(name).map_err(|_| SyncError::SourceNotFound)?;
    let target = shared_folder_path(game, kind).join(clean_name);
    if target.is_dir() {
        std::fs::remove_dir_all(&target).map_err(io_err(&target))?;
    } else if target.is_file() {
        std::fs::remove_file(&target).map_err(io_err(&target))?;
    }
    Ok(list_shared_packs(game, kind))
}

/// 从指定实例导入设置、服务器、命令记录和/或创造模式物品栏到共享库
pub fn import_instance_to_shared(
    game: &GameDir,
    instance_id: &str,
    import_options: bool,
    import_servers: bool,
    import_commands: bool,
    import_hotbar: bool,
) -> Result<(), SyncError> {
    let inst = instance::load(game, instance_id)
        .ok()
        .flatten()
        .ok_or(SyncError::SourceNotFound)?;
    let dir = inst.game_dir(game).map_err(|_| SyncError::SourceNotFound)?;
    let root = shared_root(game);
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    if import_options {
        let inst_opts = read_options_map(&dir.join(OPTIONS_FILE));
        write_options_filtered(
            &root.join(OPTIONS_FILE),
            &inst_opts,
            |k| classify_option_key(k).is_some(),
            true,
        )?;
    }
    if import_servers {
        let src_servers = dir.join(SERVERS_FILE);
        if src_servers.is_file() {
            servers::merge_files(&src_servers, &root.join(SERVERS_FILE))?;
        }
    }
    if import_commands {
        let src_cmds = dir.join(COMMAND_HISTORY_FILE);
        if src_cmds.is_file() {
            merge_command_files(&src_cmds, &root.join(COMMAND_HISTORY_FILE))?;
        }
    }
    if import_hotbar {
        let src_hotbar = dir.join(HOTBAR_FILE);
        if src_hotbar.is_file() {
            let bytes = std::fs::read(&src_hotbar).map_err(io_err(&src_hotbar))?;
            write_atomic(&root.join(HOTBAR_FILE), &bytes)?;
        }
    }
    Ok(())
}

fn entry_size_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return 0;
    }
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            total = total.saturating_add(entry_size_bytes(&entry.path()));
        }
    }
    total
}

/// 游戏启动前自动将共享库增量同步进该实例（开启总开关且该实例参与同步时生效）。
pub fn sync_before_launch(
    game: &GameDir,
    settings: &SyncSettings,
    instance: &Instance,
) -> Result<(), SyncError> {
    if !settings.enabled || !is_participating(instance) {
        return Ok(());
    }
    let root = shared_root(game);
    if !root.is_dir() {
        return Ok(());
    }
    let Ok(dir) = instance.game_dir(game) else {
        return Ok(());
    };
    let eff = effective_sync(settings, &instance.config.overrides.sync_exclusions);
    let _ = push_shared_to_instance(&root, &dir, eff, false)?;
    Ok(())
}

/// 游戏正常退出后自动将该实例改动的设置、新增服务器、命令历史与创造快捷栏增量回写共享库。
pub fn sync_after_exit(
    game: &GameDir,
    settings: &SyncSettings,
    instance: &Instance,
) -> Result<(), SyncError> {
    if !settings.enabled || !is_participating(instance) {
        return Ok(());
    }
    let Ok(dir) = instance.game_dir(game) else {
        return Ok(());
    };
    let eff = effective_sync(settings, &instance.config.overrides.sync_exclusions);
    let root = shared_root(game);
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;

    if eff.options {
        let shared_path = root.join(OPTIONS_FILE);
        let shared_opts = read_options_map(&shared_path);
        let inst_opts = read_options_map(&dir.join(OPTIONS_FILE));
        if shared_opts.is_empty() {
            // 共享库初次为空时按默认种子分类采集，避免全新用户开着同步却零数据
            write_options_filtered(
                &shared_path,
                &inst_opts,
                |k| key_in_seed_categories(k, eff.categories),
                true,
            )?;
        } else {
            // 共享库已有明确配置时：严格只回写共享库中已启用同步的键（绝不重新塞入用户已取消同步的键）
            write_options_filtered(
                &shared_path,
                &inst_opts,
                |k| shared_opts.contains_key(k),
                true,
            )?;
        }
    }
    if eff.servers {
        let inst_servers = dir.join(SERVERS_FILE);
        if inst_servers.is_file() {
            servers::merge_files(&inst_servers, &root.join(SERVERS_FILE))?;
        }
    }
    if eff.command_history {
        let inst_cmds = dir.join(COMMAND_HISTORY_FILE);
        if inst_cmds.is_file() {
            merge_command_files(&inst_cmds, &root.join(COMMAND_HISTORY_FILE))?;
        }
    }
    if eff.hotbar {
        let inst_hotbar = dir.join(HOTBAR_FILE);
        let shared_hotbar = root.join(HOTBAR_FILE);
        if inst_hotbar.is_file() && !files_identical(&inst_hotbar, &shared_hotbar) {
            let bytes = std::fs::read(&inst_hotbar).map_err(io_err(&inst_hotbar))?;
            write_atomic(&shared_hotbar, &bytes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{InstanceConfig, InstanceOverrides};

    fn make_instance(
        game: &GameDir,
        id: &str,
        sync_disabled: bool,
        sync_exclusions: InstanceSyncExclusions,
    ) -> Instance {
        let config = InstanceConfig {
            format: 2,
            name: id.to_owned(),
            game_version: "1.21.1".to_owned(),
            loader: None,
            version_id: "1.21.1".to_owned(),
            created_at: 1.0,
            source: None,
            last_played: None,
            recommended_memory_mb: None,
            overrides: InstanceOverrides {
                sync_disabled,
                sync_exclusions,
                ..InstanceOverrides::default()
            },
            group: None,
            pinned_at: None,
            cover_preset: None,
            link: None,
        };
        let inst_dir = game.instances().join(id);
        std::fs::create_dir_all(inst_dir.join(META_DIR)).unwrap();
        write_json_atomic(&inst_dir.join(META_DIR).join("instance.json"), &config).unwrap();
        instance::load(game, id).unwrap().unwrap()
    }

    #[test]
    fn options_sync_only_touches_enabled_keys_and_creates_first_backup() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path());
        let inst_a = make_instance(&game, "A", false, InstanceSyncExclusions::default());
        let inst_b = make_instance(&game, "B", false, InstanceSyncExclusions::default());

        let dir_a = inst_a.game_dir(&game).unwrap();
        let dir_b = inst_b.game_dir(&game).unwrap();

        std::fs::write(
            dir_a.join("options.txt"),
            "key_key.jump:key.keyboard.r\nsoundCategory_master:0.4\nrenderDistance:20\nmod.customKey:fromA\n",
        )
        .unwrap();
        std::fs::write(
            dir_b.join("options.txt"),
            "key_key.jump:key.keyboard.space\nrenderDistance:8\nmod.customKey:fromB\n",
        )
        .unwrap();

        let settings = SyncSettings {
            enabled: true,
            options: true,
            options_categories: OptionsSyncCategories {
                keybinds: true,
                audio: true,
                video: false,
                accessibility: true,
            },
            servers: false,
            resource_packs: false,
            shader_packs: false,
            command_history: false,
            hotbar: false,
        };

        sync_after_exit(&game, &settings, &inst_a).unwrap();
        sync_before_launch(&game, &settings, &inst_b).unwrap();

        let b_opts = read_options_map(&dir_b.join("options.txt"));
        assert_eq!(
            b_opts.get("key_key.jump").map(String::as_str),
            Some("key.keyboard.r")
        );
        assert_eq!(
            b_opts.get("soundCategory_master").map(String::as_str),
            Some("0.4")
        );
        assert_eq!(
            b_opts.get("renderDistance").map(String::as_str),
            Some("8"),
            "未启用的视频键保持实例 B 自己的值"
        );
        assert_eq!(
            b_opts.get("mod.customKey").map(String::as_str),
            Some("fromB"),
            "模组特有键不受影响"
        );

        let backup = std::fs::read_to_string(
            dir_b
                .join(META_DIR)
                .join(FIRST_BACKUP_DIR)
                .join("options.txt"),
        )
        .unwrap();
        assert!(backup.contains("key_key.jump:key.keyboard.space"));
    }

    #[test]
    fn servers_sync_merges_without_duplicates_or_data_loss() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path());
        let inst_a = make_instance(&game, "A", false, InstanceSyncExclusions::default());
        let inst_b = make_instance(&game, "B", false, InstanceSyncExclusions::default());

        let dir_a = inst_a.game_dir(&game).unwrap();
        let dir_b = inst_b.game_dir(&game).unwrap();

        servers::add(&dir_a, "生存服", "play.a.example").unwrap();
        servers::add(&dir_b, "建筑服", "play.b.example").unwrap();

        let settings = SyncSettings {
            enabled: true,
            options: false,
            servers: true,
            ..SyncSettings::default()
        };

        let result = sync_apply(&game, &settings, SyncStrategy::Merge, None).unwrap();
        assert_eq!(result.updated_instances, 2);

        let a_list = servers::list(&dir_a);
        let b_list = servers::list(&dir_b);
        assert_eq!(a_list.len(), 2);
        assert_eq!(b_list.len(), 2);
        assert!(a_list.iter().any(|s| s.address == "play.b.example"));
        assert!(b_list.iter().any(|s| s.address == "play.a.example"));
    }

    #[test]
    fn pack_sync_links_shared_packs_and_never_deletes_private_packs() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path());
        let inst = make_instance(&game, "Survival", false, InstanceSyncExclusions::default());
        let dir = inst.game_dir(&game).unwrap();

        std::fs::create_dir_all(dir.join(RESOURCE_PACKS_DIR)).unwrap();
        std::fs::write(dir.join(RESOURCE_PACKS_DIR).join("private.zip"), b"private").unwrap();

        let shared_rp = shared_folder_path(&game, SharedFolderKind::ResourcePacks);
        std::fs::create_dir_all(&shared_rp).unwrap();
        std::fs::write(shared_rp.join("shared.zip"), b"shared-pack").unwrap();

        let settings = SyncSettings {
            enabled: true,
            options: false,
            servers: false,
            resource_packs: true,
            shader_packs: false,
            ..SyncSettings::default()
        };

        sync_before_launch(&game, &settings, &inst).unwrap();
        assert!(dir.join(RESOURCE_PACKS_DIR).join("shared.zip").is_file());
        assert!(dir.join(RESOURCE_PACKS_DIR).join("private.zip").is_file());

        std::fs::remove_file(shared_rp.join("shared.zip")).unwrap();
        sync_before_launch(&game, &settings, &inst).unwrap();
        assert!(!dir.join(RESOURCE_PACKS_DIR).join("shared.zip").exists());
        assert!(dir.join(RESOURCE_PACKS_DIR).join("private.zip").is_file());
    }

    #[test]
    fn detached_instances_are_excluded_from_sync() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path());
        let inst_a = make_instance(&game, "Synced", false, InstanceSyncExclusions::default());
        let inst_b = make_instance(&game, "Detached", true, InstanceSyncExclusions::default());

        let dir_a = inst_a.game_dir(&game).unwrap();
        let dir_b = inst_b.game_dir(&game).unwrap();
        std::fs::write(dir_a.join("options.txt"), "key_key.jump:key.keyboard.k\n").unwrap();
        std::fs::write(
            dir_b.join("options.txt"),
            "key_key.jump:key.keyboard.space\n",
        )
        .unwrap();

        let settings = SyncSettings {
            enabled: true,
            options: true,
            servers: false,
            ..SyncSettings::default()
        };

        let overview = sync_overview(&game, &settings);
        assert_eq!(overview.participating_instances, 1);
        assert_eq!(overview.detached_instances, 1);

        sync_apply(&game, &settings, SyncStrategy::Merge, None).unwrap();
        let b_opts = read_options_map(&dir_b.join("options.txt"));
        assert_eq!(
            b_opts.get("key_key.jump").map(String::as_str),
            Some("key.keyboard.space"),
            "脱离同步的实例不受影响"
        );
    }

    #[test]
    fn command_history_and_hotbar_sync_with_exclusions_and_backup() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path());
        let inst_a = make_instance(&game, "BuilderA", false, InstanceSyncExclusions::default());
        let inst_b = make_instance(
            &game,
            "BuilderB",
            false,
            InstanceSyncExclusions {
                hotbar: true,
                ..InstanceSyncExclusions::default()
            },
        );
        let dir_a = inst_a.game_dir(&game).unwrap();
        let dir_b = inst_b.game_dir(&game).unwrap();

        std::fs::write(
            dir_a.join(COMMAND_HISTORY_FILE),
            "/gamemode creative\n/time set day\n",
        )
        .unwrap();
        std::fs::write(
            dir_b.join(COMMAND_HISTORY_FILE),
            "/time set day\n/weather clear\n",
        )
        .unwrap();
        std::fs::write(dir_a.join(HOTBAR_FILE), b"hotbar-from-a").unwrap();
        std::fs::write(dir_b.join(HOTBAR_FILE), b"hotbar-from-b").unwrap();

        let settings = SyncSettings {
            enabled: true,
            options: false,
            servers: false,
            resource_packs: false,
            shader_packs: false,
            command_history: true,
            hotbar: true,
            ..SyncSettings::default()
        };

        let res = sync_apply(&game, &settings, SyncStrategy::Merge, None).unwrap();
        assert!(res.commands_merged >= 2);

        let a_cmds = read_command_lines(&dir_a.join(COMMAND_HISTORY_FILE));
        let b_cmds = read_command_lines(&dir_b.join(COMMAND_HISTORY_FILE));
        assert_eq!(a_cmds.len(), 3);
        assert_eq!(b_cmds.len(), 3);
        assert!(a_cmds.contains(&"/weather clear".to_owned()));
        assert!(b_cmds.contains(&"/gamemode creative".to_owned()));

        // BuilderB 单独排除了 hotbar，其 hotbar.nbt 必须保持原样
        assert_eq!(
            std::fs::read(dir_b.join(HOTBAR_FILE)).unwrap(),
            b"hotbar-from-b"
        );
    }
}
