//! 实例 Minecraft / 加载器版本切换与升级向导。
//!
//! 流程分两段：
//! 1. [`preview`]：校验目标版本与加载器，扫描当前实例模组并结合 Modrinth / CurseForge 判定每项状态
//!    （可保留 / 可自动升级 / 不兼容 / 已锁定 / 未知来源 / 已停用），同时识别四类高风险信号
//!    （自定义补丁、跨版本降级、自定义 Java 覆盖、受管整合包脱离），支持中途取消；
//! 2. [`execute`]：按用户选择的 [`UpgradeMode`]（原地升级或克隆副本后升级）与单条模组覆写动作执行：
//!    - **先安装目标加载器与补全目标游戏文件，再克隆/迁移模组（走 `journal.rs` 可恢复事务）**；
//!    - CurseForge 作者受限模组无缝接入既有补件认领会话（[`ActiveClaim`]）；
//!    - 升级模组后可选自动解析并补装新版新增的必需前置依赖；
//!    - 最后原子更新 `instance.json`（跨版本/跨加载器切换时自动清理失效旧补丁并解除旧整合包绑定）。

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::content::claim::{ClaimSession, ManualFile};
use crate::content::curseforge::{self, CurseForge};
use crate::content::modrinth::{self, PlanItem, PlanReason};
use crate::content::{
    ContentError, ContentItem, ContentKind, ContentSource, Delivery, InstanceContent, UpdateInfo,
};
use crate::fs::UnsafePath;
use crate::install::processors::LoaderContext;
use crate::install::{
    InstallError, InstallPhase, LoaderProgress, install_files, load_or_fetch_version, loaders,
};
use crate::instance::{
    self, CopyError, CopyOptions, CopyProgress, Instance, InstanceError, LoaderRef, META_DIR,
};
use crate::meta::{Environment, patches};
use crate::net::DownloadProgress;
use crate::paths::GameDir;

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error("外部关联实例不能原地修改，请使用「复制并升级为 Lumi 实例」")]
    LinkedInstance,
    #[error("目标版本和加载器与当前一致，无需切换")]
    Unchanged,
    #[error("找不到这个实例，可能已经被删除")]
    NotFound,
    #[error("请给新实例起个名字")]
    EmptyCloneName,
    #[error("已取消")]
    Cancelled,
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    Copy(#[from] CopyError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
}

/// 持有一次进行中的手动补件会话及其宿主守卫（丢弃时自动结束目录监听并注销会话）。
pub struct ActiveClaim {
    pub session: Arc<ClaimSession>,
    _guard: Box<dyn Any + Send + Sync>,
}

impl ActiveClaim {
    pub fn new(session: Arc<ClaimSession>, guard: impl Any + Send + Sync + 'static) -> Self {
        Self {
            session,
            _guard: Box::new(guard),
        }
    }
}

/// 升级模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum UpgradeMode {
    /// 原地切换（升级前自动打快照，模组改动进改动历史可撤销）
    InPlace,
    /// 先克隆副本再升级，原实例保持不变
    Clone,
}

/// 单个模组在目标版本/加载器下的兼容状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ModUpgradeStatus {
    /// 已兼容目标版本，保持原样
    Compatible,
    /// 平台有适配目标版本的新版，可自动升级
    Upgradable,
    /// 目标为原版或平台未找到适配版本（建议自动停用）
    Incompatible,
    /// 已锁定版本（跳过自动升级）
    Locked,
    /// 未识别来源的本地模组（建议升级后人工核对）
    Unknown,
    /// 原本即已停用
    Disabled,
}

/// 用户对单条模组的处理动作（支持在预检页逐条覆写默认策略）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ModUpgradeAction {
    /// 升级到适配目标版本的新版
    Upgrade,
    /// 保持当前文件不动
    Keep,
    /// 停用（重命名为 `.disabled`）
    Disable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ModUpgradeItem {
    pub file_name: String,
    pub title: String,
    pub title_zh: Option<String>,
    pub icon_url: Option<String>,
    pub page_url: Option<String>,
    pub current_version: Option<String>,
    pub target_version: Option<String>,
    pub status: ModUpgradeStatus,
    pub delivery: Option<Delivery>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UpgradeCounts {
    pub compatible: u32,
    pub upgradable: u32,
    pub incompatible: u32,
    pub locked: u32,
    pub unknown: u32,
    pub disabled: u32,
}

/// 给前端展示的兼容性预检结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UpgradePreview {
    pub instance_id: String,
    pub instance_name: String,
    pub current_game_version: String,
    pub current_loader: Option<LoaderRef>,
    pub target_game_version: String,
    pub target_loader: Option<LoaderRef>,
    pub same_game_version: bool,
    pub same_loader_kind: bool,
    /// 平台查询是否失败（失败时无法准确给出可升级清单，但仍允许继续切换）
    pub lookup_failed: bool,
    /// 可升级模组中，属于 CurseForge 作者禁止第三方下载、需通过补件认领的数量
    pub manual_count: u32,
    /// 当前实例是否带有 MultiMC / Prism 自定义运行库/参数补丁（`Lumi/patches.json`）
    pub has_patches: bool,
    /// 补丁显示名列表（供风险提示展示）
    pub patch_names: Vec<String>,
    /// 目标 Minecraft 版本是否低于当前版本（存在原位打开高版本存档毁档风险）
    pub is_downgrade: bool,
    /// 当前实例是否设置了自定义 Java 覆盖且正在切换 Minecraft 版本
    pub has_custom_java: bool,
    /// 若实例来自受管整合包，给出原整合包名称（手动切版本将脱离原包更新线）
    pub managed_pack_name: Option<String>,
    /// 是否为外部启动器关联实例（仅支持复制为新 Lumi 实例后升级，绝不改动原启动器目录）
    pub is_linked: bool,
    /// 实例当前是否包含单人世界存档（跨版本升级时提示世界格式不可逆迁移）
    pub has_worlds: bool,
    pub counts: UpgradeCounts,
    pub mods: Vec<ModUpgradeItem>,
}

/// 后端暂存的完整升级计划（前端只拿一次性 token，不接触内部更新明细）。
#[derive(Debug, Clone)]
pub struct UpgradePlan {
    pub preview: UpgradePreview,
    /// 所有可升级模组的完整更新信息（含直连与 CurseForge 补件，键为小写文件名）
    pub updates_by_file: HashMap<String, UpdateInfo>,
}

fn default_true() -> bool {
    true
}

/// 执行升级时的用户选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UpgradeOptions {
    pub mode: UpgradeMode,
    /// 仅 `Clone` 模式使用；留空则自动生成名称
    pub clone_name: Option<String>,
    /// 仅 `InPlace` 模式使用；若非空则同步更新实例显示名（如自动把名称里的旧版本号替换为新版本）
    #[serde(default)]
    pub new_name: Option<String>,
    /// 仅 `Clone` 模式使用；是否一并复制世界存档
    pub include_worlds: bool,
    /// 是否默认将可升级模组升级至适配目标版本的最新版
    pub update_mods: bool,
    /// 是否默认停用不兼容模组
    pub disable_incompatible: bool,
    /// 升级模组后是否自动解析并补装新版模组新增的必需前置依赖
    #[serde(default = "default_true")]
    pub install_dependencies: bool,
    /// 用户在预检列表里对单条模组手动覆写的动作（键为 `file_name`）
    #[serde(default)]
    pub mod_actions: HashMap<String, ModUpgradeAction>,
}

#[derive(Debug, Clone)]
pub enum UpgradeProgress {
    Cloning(CopyProgress),
    Loader(LoaderProgress),
    Game(InstallPhase, DownloadProgress),
    UpdatingMods(DownloadProgress),
    ResolvingDependencies,
    InstallingDependencies(DownloadProgress),
    DisablingMods,
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct UpgradeOutcome {
    pub instance_id: String,
    pub upgraded_mods: u32,
    pub installed_deps: u32,
    pub disabled_mods: u32,
    pub skipped_manual: u32,
}

/// 解析正式版 Minecraft 版本号为可比较元组，支持经典 `1.x[.y]` 与年份版 `26.x[.y]`。
fn parse_release_tuple(version: &str) -> Option<(u32, u32, u32)> {
    let trimmed = version.trim();
    if trimmed.is_empty() || trimmed.contains('-') || trimmed.contains('w') {
        return None;
    }
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse::<u32>().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// 粗略判断目标正式版是否低于当前正式版（用于原位降级毁档提示）。
pub fn is_version_downgrade(current: &str, target: &str) -> bool {
    match (parse_release_tuple(current), parse_release_tuple(target)) {
        (Some(cur), Some(tgt)) => tgt < cur,
        _ => false,
    }
}

/// 预检实例切换到目标游戏版本与加载器后的模组兼容性与潜在风险。
#[allow(clippy::too_many_arguments)]
pub async fn preview(
    game: &GameDir,
    instance: &Instance,
    client: &Client,
    curseforge: &CurseForge,
    target_game_version: String,
    target_loader: Option<LoaderRef>,
    cancel: &CancellationToken,
) -> Result<UpgradePlan, UpgradeError> {
    if target_game_version == instance.config.game_version
        && target_loader == instance.config.loader
    {
        return Err(UpgradeError::Unchanged);
    }
    if cancel.is_cancelled() {
        return Err(UpgradeError::Cancelled);
    }
    if let Some(loader) = &target_loader {
        loaders::validate(&target_game_version, loader)?;
    }

    let same_game_version = target_game_version == instance.config.game_version;
    let same_loader_kind =
        target_loader.as_ref().map(|l| l.kind) == instance.config.loader.as_ref().map(|l| l.kind);

    let game_dir = instance.game_dir(game)?;
    let has_worlds = std::fs::read_dir(game_dir.join("saves"))
        .ok()
        .is_some_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        });
    let patch_names = patches::load_for_instance(&game_dir)
        .ok()
        .flatten()
        .filter(|bundle| !bundle.is_empty())
        .map(|bundle| bundle.display_names())
        .unwrap_or_default();

    let content = InstanceContent::for_game(game, game_dir);
    let items = content.list(ContentKind::Mod)?;

    // 仅升级同 MC 同类加载器的小版本（如 NeoForge 21.1.x -> 21.1.y）：现有模组天然兼容，无需联网全量扫描
    if same_game_version && same_loader_kind {
        return Ok(build_plan(
            instance,
            target_game_version,
            target_loader,
            true,
            true,
            false,
            false,
            false,
            has_worlds,
            patch_names,
            &items,
            &HashMap::new(),
            &HashSet::new(),
        ));
    }

    // 目标为原版：所有已启用模组均标记为不兼容
    let Some(loader_ref) = &target_loader else {
        return Ok(build_plan(
            instance,
            target_game_version,
            None,
            same_game_version,
            same_loader_kind,
            false,
            false,
            false,
            has_worlds,
            patch_names,
            &items,
            &HashMap::new(),
            &HashSet::new(),
        ));
    };

    let check = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(UpgradeError::Cancelled),
        res = content.check_updates(
            client,
            curseforge,
            ContentKind::Mod,
            &target_game_version,
            Some(loader_ref.kind.slug()),
        ) => res?,
    };

    if cancel.is_cancelled() {
        return Err(UpgradeError::Cancelled);
    }

    let update_map: HashMap<String, UpdateInfo> = check
        .updates
        .into_iter()
        .map(|u| (u.file_name.to_lowercase(), u))
        .collect();
    let compatible_set: HashSet<String> = check
        .compatible_files
        .into_iter()
        .map(|name| name.to_lowercase())
        .collect();

    Ok(build_plan(
        instance,
        target_game_version,
        target_loader,
        same_game_version,
        same_loader_kind,
        check.lookup_failed,
        check.modrinth_failed,
        check.curseforge_failed,
        has_worlds,
        patch_names,
        &items,
        &update_map,
        &compatible_set,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_plan(
    instance: &Instance,
    target_game_version: String,
    target_loader: Option<LoaderRef>,
    same_game_version: bool,
    same_loader_kind: bool,
    lookup_failed: bool,
    modrinth_failed: bool,
    curseforge_failed: bool,
    has_worlds: bool,
    patch_names: Vec<String>,
    items: &[ContentItem],
    update_map: &HashMap<String, UpdateInfo>,
    compatible_set: &HashSet<String>,
) -> UpgradePlan {
    let mut counts = UpgradeCounts::default();
    let mut mods = Vec::with_capacity(items.len());
    let mut updates_by_file = HashMap::new();
    let mut manual_count = 0u32;

    for item in items {
        let key = item.file_name.to_lowercase();
        let platform_failed = lookup_failed
            || match item.source {
                Some(crate::content::IdentitySource::CurseForge) => curseforge_failed,
                Some(crate::content::IdentitySource::Modrinth) => modrinth_failed,
                Some(crate::content::IdentitySource::File) | None => {
                    modrinth_failed || curseforge_failed
                }
            };
        let (status, target_version, delivery, update_page) = if !item.enabled {
            counts.disabled += 1;
            (ModUpgradeStatus::Disabled, None, None, None)
        } else if target_loader.is_none() {
            counts.incompatible += 1;
            (ModUpgradeStatus::Incompatible, None, None, None)
        } else if same_game_version && same_loader_kind {
            if item.locked {
                counts.locked += 1;
                (ModUpgradeStatus::Locked, None, None, None)
            } else {
                counts.compatible += 1;
                (ModUpgradeStatus::Compatible, None, None, None)
            }
        } else if item.locked {
            counts.locked += 1;
            (ModUpgradeStatus::Locked, None, None, None)
        } else if let Some(update) = update_map.get(&key) {
            counts.upgradable += 1;
            if update.delivery == Delivery::Manual {
                manual_count += 1;
            }
            updates_by_file.insert(key.clone(), update.clone());
            (
                ModUpgradeStatus::Upgradable,
                Some(update.version_number.clone()),
                Some(update.delivery),
                update.page_url.clone(),
            )
        } else if compatible_set.contains(&key) {
            counts.compatible += 1;
            (ModUpgradeStatus::Compatible, None, None, None)
        } else if (item.project_id.is_none() && item.source.is_none()) || platform_failed {
            counts.unknown += 1;
            (ModUpgradeStatus::Unknown, None, None, None)
        } else {
            counts.incompatible += 1;
            (ModUpgradeStatus::Incompatible, None, None, None)
        };

        mods.push(ModUpgradeItem {
            file_name: item.file_name.clone(),
            title: item.title.clone(),
            title_zh: item.title_zh.clone(),
            icon_url: item.icon_url.clone(),
            page_url: update_page.or_else(|| item.page_url.clone()),
            current_version: item.version_number.clone(),
            target_version,
            status,
            delivery,
        });
    }

    // 排序：不兼容、可升级在前，方便用户一眼看到变化
    mods.sort_by_key(|m| match m.status {
        ModUpgradeStatus::Incompatible => 0,
        ModUpgradeStatus::Upgradable => 1,
        ModUpgradeStatus::Locked => 2,
        ModUpgradeStatus::Unknown => 3,
        ModUpgradeStatus::Compatible => 4,
        ModUpgradeStatus::Disabled => 5,
    });

    let is_downgrade = is_version_downgrade(&instance.config.game_version, &target_game_version);
    let has_custom_java = !same_game_version && instance.config.overrides.java_id.is_some();
    let has_patches = !patch_names.is_empty();
    let managed_pack_name = instance.config.source.as_ref().map(|s| s.name.clone());

    UpgradePlan {
        preview: UpgradePreview {
            instance_id: instance.id.clone(),
            instance_name: instance.config.name.clone(),
            current_game_version: instance.config.game_version.clone(),
            current_loader: instance.config.loader.clone(),
            target_game_version,
            target_loader,
            same_game_version,
            same_loader_kind,
            lookup_failed,
            manual_count,
            has_patches,
            patch_names,
            is_downgrade,
            has_custom_java,
            managed_pack_name,
            is_linked: instance.is_linked(),
            has_worlds,
            counts,
            mods,
        },
        updates_by_file,
    }
}

/// 根据全局选项与单条覆写解析出最终要升级和要停用的模组清单。
pub fn resolve_actions(
    plan: &UpgradePlan,
    options: &UpgradeOptions,
) -> (Vec<UpdateInfo>, Vec<String>) {
    let overrides: HashMap<String, ModUpgradeAction> = options
        .mod_actions
        .iter()
        .map(|(k, v)| (k.to_lowercase(), *v))
        .collect();

    let mut updates = Vec::new();
    let mut disables = Vec::new();

    for item in &plan.preview.mods {
        if item.status == ModUpgradeStatus::Disabled {
            continue;
        }
        let key = item.file_name.to_lowercase();
        let action = overrides.get(&key).copied().unwrap_or(match item.status {
            ModUpgradeStatus::Upgradable if options.update_mods => ModUpgradeAction::Upgrade,
            ModUpgradeStatus::Incompatible if options.disable_incompatible => {
                ModUpgradeAction::Disable
            }
            _ => ModUpgradeAction::Keep,
        });
        match action {
            ModUpgradeAction::Upgrade => {
                if let Some(info) = plan.updates_by_file.get(&key) {
                    updates.push(info.clone());
                }
            }
            ModUpgradeAction::Disable => disables.push(item.file_name.clone()),
            ModUpgradeAction::Keep => {}
        }
    }

    (updates, disables)
}

/// 计算克隆模式下的新实例名称（供任务开始前预分配）。
pub fn clone_target_name(preview: &UpgradePreview, custom: Option<&str>) -> String {
    if let Some(name) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return name.to_owned();
    }
    let suffix = match &preview.target_loader {
        Some(loader) => format!("{} {}", preview.target_game_version, loader.kind.label()),
        None => preview.target_game_version.clone(),
    };
    format!("{} ({suffix})", preview.instance_name)
}

/// 为刚刚升级到目标版本的模组解析新增的必需前置依赖（单条查询失败记日志跳过，不阻断整体升级）。
async fn resolve_missing_dependencies(
    content: &InstanceContent,
    client: &Client,
    curseforge: &CurseForge,
    target_game_version: &str,
    target_loader: &LoaderRef,
    upgraded: &[UpdateInfo],
    cancel: &CancellationToken,
) -> Result<Vec<PlanItem>, UpgradeError> {
    if upgraded.is_empty() {
        return Ok(Vec::new());
    }

    let mut installed_mr = content
        .installed_projects(client)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "读取已安装 Modrinth 项目列表失败，继续解析前置");
            HashSet::new()
        });
    let mut installed_cf = if curseforge.usable() {
        content
            .installed_curseforge(curseforge)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "读取已安装 CurseForge 项目列表失败，继续解析前置");
                HashSet::new()
            })
    } else {
        HashSet::new()
    };

    for u in upgraded {
        match u.source {
            ContentSource::Modrinth => {
                if let Some(pid) = &u.project_id {
                    installed_mr.insert(pid.clone());
                }
            }
            ContentSource::CurseForge => {
                if let Some(Ok(mod_id)) = u.project_id.as_deref().map(str::parse::<u32>) {
                    installed_cf.insert(mod_id);
                }
            }
        }
    }

    let mr_target = modrinth::Target {
        game_version: target_game_version,
        loader: Some(target_loader.kind.slug()),
        installed: &installed_mr,
    };
    let cf_target = curseforge::Target {
        game_version: target_game_version,
        loader: curseforge::loader_id(target_loader.kind.slug()),
        installed: &installed_cf,
    };

    let mut deps = Vec::new();
    let mut seen: HashSet<(ContentSource, String)> = HashSet::new();

    for u in upgraded {
        if cancel.is_cancelled() {
            return Err(UpgradeError::Cancelled);
        }
        let resolved = match u.source {
            ContentSource::Modrinth => {
                let Some(pid) = &u.project_id else {
                    continue;
                };
                match modrinth::resolve_version(
                    client,
                    &mr_target,
                    pid,
                    &u.version_id,
                    ContentKind::Mod,
                )
                .await
                {
                    Ok(items) => items,
                    Err(error) => {
                        tracing::warn!(%error, mod_title = %u.title, "解析 Modrinth 模组新增前置失败，已跳过");
                        continue;
                    }
                }
            }
            ContentSource::CurseForge => {
                if !curseforge.usable() {
                    continue;
                }
                let (Some(Ok(mod_id)), Ok(file_id)) = (
                    u.project_id.as_deref().map(str::parse::<u32>),
                    u.version_id.parse::<u32>(),
                ) else {
                    continue;
                };
                match curseforge
                    .resolve_version(client, &cf_target, mod_id, file_id, ContentKind::Mod)
                    .await
                {
                    Ok(items) => items,
                    Err(error) => {
                        tracing::warn!(%error, mod_title = %u.title, "解析 CurseForge 模组新增前置失败，已跳过");
                        continue;
                    }
                }
            }
        };

        for item in resolved {
            if item.reason != PlanReason::Dependency {
                continue;
            }
            if seen.insert((item.source, item.project_id.clone())) {
                deps.push(item);
            }
        }
    }

    Ok(deps)
}

/// 执行实例版本切换或升级。
pub async fn execute<F, C>(
    ctx: &LoaderContext<'_>,
    curseforge: &CurseForge,
    plan: &UpgradePlan,
    options: &UpgradeOptions,
    on_progress: F,
    on_claim: C,
) -> Result<UpgradeOutcome, UpgradeError>
where
    F: Fn(UpgradeProgress) + Send + Sync,
    C: Fn(String, Vec<ManualFile>) -> Option<ActiveClaim> + Send + Sync,
{
    let game = ctx.game;
    let source = instance::load(game, &plan.preview.instance_id)?.ok_or(UpgradeError::NotFound)?;
    if source.is_linked() && options.mode == UpgradeMode::InPlace {
        return Err(UpgradeError::LinkedInstance);
    }

    // 1. 先安装目标加载器与补全目标游戏本体（失败或取消时不碰任何实例文件与配置）
    let target_version_id = if let Some(loader) = &plan.preview.target_loader {
        loaders::install(ctx, &plan.preview.target_game_version, loader, |p| {
            on_progress(UpgradeProgress::Loader(p));
        })
        .await?
    } else {
        plan.preview.target_game_version.clone()
    };

    let version = load_or_fetch_version(game, &target_version_id, ctx.client).await?;
    install_files(
        version,
        game,
        &Environment::current(),
        ctx.downloader,
        ctx.cancel,
        |phase, p| on_progress(UpgradeProgress::Game(phase, p)),
    )
    .await?;

    if ctx.cancel.is_cancelled() {
        return Err(UpgradeError::Cancelled);
    }

    // 2. 确定目标实例（原地或先克隆；克隆过程实时回传字节进度）
    let target_instance = match options.mode {
        UpgradeMode::InPlace => source,
        UpgradeMode::Clone => {
            let clone_name = clone_target_name(&plan.preview, options.clone_name.as_deref());
            if clone_name.trim().is_empty() {
                return Err(UpgradeError::EmptyCloneName);
            }
            let clone_id = instance::unique_id(game, &clone_name);
            let copy_opts = CopyOptions {
                name: clone_name,
                id: clone_id,
                include_worlds: options.include_worlds,
            };
            let game_clone = game.clone();
            let src_id = plan.preview.instance_id.clone();
            let cancel = ctx.cancel.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CopyProgress>();
            let mut handle = tokio::task::spawn_blocking(move || {
                instance::copy(&game_clone, &src_id, &copy_opts, &cancel, |p| {
                    let _ = tx.send(p);
                })
            });
            loop {
                tokio::select! {
                    Some(p) = rx.recv() => on_progress(UpgradeProgress::Cloning(p)),
                    res = &mut handle => {
                        while let Ok(p) = rx.try_recv() {
                            on_progress(UpgradeProgress::Cloning(p));
                        }
                        break res.map_err(|e| UpgradeError::Instance(InstanceError::Io(e.to_string())))??;
                    }
                }
            }
        }
    };

    let target_dir = target_instance.game_dir(game)?;
    let content = InstanceContent::for_game(game, target_dir.clone());

    // 3. 原地模式下在修改模组前先落永不玩坏快照与时间线内存基线
    let tl_guard = if options.mode == UpgradeMode::InPlace {
        content.snapshot_before_change().await;
        content.begin_timeline_change().await
    } else {
        None
    };

    // 4. 解析每条模组的最终动作（支持逐条覆写）
    let (updates_to_apply, files_to_disable) = resolve_actions(plan, options);

    // 4a-5 包裹在同一事务内：任一步出错或取消时自动回滚（原地模式恢复快照；克隆模式删除半成品副本）
    let tx_res: Result<(u32, u32, u32, u32, String), UpgradeError> = async {
        // 4a. 模组自动升级（支持 CurseForge 受限文件自动接入补件认领）
        let mut upgraded_mods = 0u32;
        let mut skipped_manual = 0u32;
        let mut applied_updates = Vec::new();
        if !updates_to_apply.is_empty() {
            let prepared = content
                .prepare_update(ctx.client, curseforge, &updates_to_apply)
                .await?;
            let claim_guard = if !prepared.manual.is_empty() {
                on_claim(
                    format!("版本切换补件 · {}", target_instance.config.name),
                    prepared.manual.clone(),
                )
            } else {
                None
            };

            if !prepared.manual.is_empty() && claim_guard.is_none() {
                // 无交互补件宿主的场景（如 CLI/测试）：仅执行可直连更新，其余计入跳过
                let direct_only: Vec<UpdateInfo> = updates_to_apply
                    .iter()
                    .filter(|u| u.delivery != Delivery::Manual)
                    .cloned()
                    .collect();
                skipped_manual =
                    u32::try_from(updates_to_apply.len().saturating_sub(direct_only.len()))
                        .unwrap_or(u32::MAX);
                if !direct_only.is_empty() {
                    content
                        .update(
                            curseforge,
                            ctx.downloader,
                            &direct_only,
                            None,
                            ctx.cancel,
                            |p| on_progress(UpgradeProgress::UpdatingMods(p)),
                        )
                        .await?;
                    upgraded_mods = u32::try_from(direct_only.len()).unwrap_or(u32::MAX);
                    applied_updates = direct_only;
                }
            } else {
                let staged = content
                    .download_update(
                        ctx.downloader,
                        prepared,
                        claim_guard.as_ref().map(|c| c.session.as_ref()),
                        ctx.cancel,
                        |p| on_progress(UpgradeProgress::UpdatingMods(p)),
                    )
                    .await?;
                drop(claim_guard);
                content.finish_update(staged, "切换版本升级").await?;
                upgraded_mods = u32::try_from(updates_to_apply.len()).unwrap_or(u32::MAX);
                applied_updates = updates_to_apply;
            }
        }

        // 4b. 自动补装新版模组新增的必需前置依赖
        let mut installed_deps = 0u32;
        if options.install_dependencies
            && !applied_updates.is_empty()
            && let Some(target_loader) = &plan.preview.target_loader
        {
            on_progress(UpgradeProgress::ResolvingDependencies);
            let deps = resolve_missing_dependencies(
                &content,
                ctx.client,
                curseforge,
                &plan.preview.target_game_version,
                target_loader,
                &applied_updates,
                ctx.cancel,
            )
            .await?;
            if !deps.is_empty() {
                let prepared_deps = content
                    .prepare_install(ctx.client, curseforge, &deps)
                    .await?;
                let dep_claim = if !prepared_deps.manual.is_empty() {
                    on_claim(
                        format!("前置依赖补件 · {}", target_instance.config.name),
                        prepared_deps.manual.clone(),
                    )
                } else {
                    None
                };
                if prepared_deps.manual.is_empty() || dep_claim.is_some() {
                    let staged_deps = content
                        .download_install(
                            ctx.downloader,
                            prepared_deps,
                            dep_claim.as_ref().map(|c| c.session.as_ref()),
                            ctx.cancel,
                            |p| on_progress(UpgradeProgress::InstallingDependencies(p)),
                        )
                        .await?;
                    drop(dep_claim);
                    content.finish_install(staged_deps).await?;
                    installed_deps = u32::try_from(deps.len()).unwrap_or(u32::MAX);
                } else {
                    skipped_manual = skipped_manual
                        .saturating_add(u32::try_from(prepared_deps.manual.len()).unwrap_or(0));
                }
            }
        }

        // 4c. 停用不兼容或用户指定停用的模组
        let mut disabled_mods = 0u32;
        if !files_to_disable.is_empty() {
            on_progress(UpgradeProgress::DisablingMods);
            let changed = content
                .set_many_enabled(ContentKind::Mod, &files_to_disable, false)
                .await?;
            disabled_mods = u32::try_from(changed.len()).unwrap_or(u32::MAX);
        }

        // 5. 跨游戏版本或跨加载器切换时：移除不再适用的旧版本补丁文件，并解除原整合包跟踪
        on_progress(UpgradeProgress::Finalizing);
        let major_switch = !plan.preview.same_game_version || !plan.preview.same_loader_kind;
        if major_switch {
            let patch_path = target_dir.join(META_DIR).join(patches::PATCHES_FILE);
            if let Err(error) = std::fs::remove_file(&patch_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%error, path = %patch_path.display(), "清理旧版本补丁文件失败");
            }
        }

        let rename = if options.mode == UpgradeMode::InPlace {
            options
                .new_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        } else {
            None
        };
        let updated = instance::update(game, &target_instance.id, |config| {
            if let Some(next_name) = &rename {
                config.name = next_name.clone();
            }
            config.game_version = plan.preview.target_game_version.clone();
            config.loader = plan.preview.target_loader.clone();
            config.version_id = target_version_id.clone();
            if major_switch {
                config.source = None;
            }
        })?
        .ok_or(UpgradeError::NotFound)?;

        if options.mode == UpgradeMode::InPlace {
            let loader_label = updated
                .config
                .loader
                .as_ref()
                .map(|l| format!(" · {} {}", l.kind.label(), l.version))
                .unwrap_or_default();
            content
                .finish_timeline_change(
                    tl_guard,
                    format!("切换版本至 {}{loader_label}", updated.config.game_version),
                )
                .await;
        }

        tracing::info!(
            instance = %updated.id,
            mode = ?options.mode,
            game_version = %updated.config.game_version,
            loader = ?updated.config.loader,
            upgraded_mods,
            installed_deps,
            disabled_mods,
            skipped_manual,
            "实例版本切换完成"
        );

        Ok((
            upgraded_mods,
            installed_deps,
            disabled_mods,
            skipped_manual,
            updated.id,
        ))
    }
    .await;

    let (upgraded_mods, installed_deps, disabled_mods, skipped_manual, updated_id) = match tx_res {
        Ok(ok) => ok,
        Err(err) => {
            match options.mode {
                UpgradeMode::InPlace => {
                    if let Err(rb_err) = content.restore_snapshot().await {
                        tracing::warn!(%rb_err, instance = %target_instance.id, "版本切换失败后恢复模组快照失败");
                    }
                }
                UpgradeMode::Clone => {
                    if let Err(rm_err) = std::fs::remove_dir_all(&target_dir) {
                        tracing::warn!(%rm_err, dir = %target_dir.display(), "清理半成品克隆实例失败");
                    }
                }
            }
            return Err(err);
        }
    };

    Ok(UpgradeOutcome {
        instance_id: updated_id,
        upgraded_mods,
        installed_deps,
        disabled_mods,
        skipped_manual,
    })
}

#[cfg(test)]
mod tests {
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::content::ContentSource;
    use crate::instance::{InstanceConfig, LoaderKind, PackSource};
    use crate::meta::patches::{InstancePatches, MetaPatch};

    fn dummy_item(
        file_name: &str,
        enabled: bool,
        locked: bool,
        project_id: Option<&str>,
    ) -> ContentItem {
        ContentItem {
            kind: ContentKind::Mod,
            file_name: file_name.to_owned(),
            title: file_name.trim_end_matches(".jar").to_owned(),
            enabled,
            size: 1024.0,
            project_id: project_id.map(str::to_owned),
            version_id: Some("v1".to_owned()),
            version_number: Some("1.0.0".to_owned()),
            locked,
            icon_url: Some("https://cdn.modrinth.com/demo.png".to_owned()),
            source: project_id.map(|_| crate::content::IdentitySource::Modrinth),
            is_folder: false,
            author: None,
            author_url: None,
            page_url: project_id.map(|id| format!("https://modrinth.com/project/{id}")),
            modified: 0.0,
            title_zh: Some(format!("中文-{file_name}")),
            twin_id: None,
        }
    }

    fn dummy_instance(mc: &str, loader: Option<LoaderRef>) -> Instance {
        Instance {
            id: "demo".to_owned(),
            config: InstanceConfig::new("Demo", mc, loader),
        }
    }

    #[test]
    fn detects_mc_version_downgrade_across_classic_and_year_schemes() {
        assert!(is_version_downgrade("1.21.1", "1.20.1"));
        assert!(is_version_downgrade("1.21.11", "1.21.4"));
        assert!(is_version_downgrade("26.1", "1.21.11"));
        assert!(!is_version_downgrade("1.20.1", "1.21.1"));
        assert!(!is_version_downgrade("1.21.1", "1.21.1"));
        assert!(!is_version_downgrade("1.21.1", "26.1"));
        assert!(!is_version_downgrade("25w10a", "1.21.4"));
    }

    #[test]
    fn classifies_mods_and_applies_per_mod_action_overrides() {
        let mut inst = dummy_instance(
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.200".to_owned(),
            }),
        );
        inst.config.overrides.java_id = Some("custom-java".to_owned());
        inst.config.source = Some(PackSource {
            kind: "mrpack".to_owned(),
            name: "测试整合包".to_owned(),
            version: "1.0.0".to_owned(),
            project_id: None,
            version_id: None,
            cover_url: None,
        });

        let items = vec![
            dummy_item("sodium.jar", true, false, Some("AANobbMI")),
            dummy_item("waystones.jar", true, false, Some("LOpKHB2A")),
            dummy_item("old-only.jar", true, false, Some("old123")),
            dummy_item("pinned.jar", true, true, Some("pin1")),
            dummy_item("custom-local.jar", true, false, None),
            dummy_item("off.jar", false, false, Some("off1")),
        ];
        let mut updates = HashMap::new();
        updates.insert(
            "sodium.jar".to_owned(),
            UpdateInfo {
                kind: ContentKind::Mod,
                file_name: "sodium.jar".to_owned(),
                title: "Sodium".to_owned(),
                current_version: Some("1.0.0".to_owned()),
                version_id: "v2".to_owned(),
                version_number: "2.0.0".to_owned(),
                source: ContentSource::Modrinth,
                project_id: Some("AANobbMI".to_owned()),
                delivery: Delivery::Direct,
                page_url: None,
            },
        );
        let compatible = HashSet::from(["waystones.jar".to_owned()]);

        let plan = build_plan(
            &inst,
            "1.20.1".to_owned(),
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "47.1.106".to_owned(),
            }),
            false,
            true,
            false,
            false,
            false,
            true,
            vec!["Custom Patch".to_owned()],
            &items,
            &updates,
            &compatible,
        );

        assert!(plan.preview.is_downgrade);
        assert!(plan.preview.has_custom_java);
        assert!(plan.preview.has_patches);
        assert_eq!(
            plan.preview.managed_pack_name.as_deref(),
            Some("测试整合包")
        );
        assert_eq!(plan.preview.counts.upgradable, 1);
        assert_eq!(plan.preview.counts.compatible, 1);
        assert_eq!(plan.preview.counts.incompatible, 1);
        assert_eq!(plan.preview.counts.locked, 1);
        assert_eq!(plan.preview.counts.unknown, 1);
        assert_eq!(plan.preview.counts.disabled, 1);
        assert_eq!(plan.preview.mods[0].status, ModUpgradeStatus::Incompatible);
        assert!(plan.preview.mods[0].title_zh.is_some());
        assert!(plan.preview.mods[0].icon_url.is_some());

        // 覆盖单条动作：sodium 改为保留、old-only 改为保留、custom-local 改为停用
        let opts = UpgradeOptions {
            mode: UpgradeMode::Clone,
            clone_name: None,
            new_name: None,
            include_worlds: true,
            update_mods: true,
            disable_incompatible: true,
            install_dependencies: true,
            mod_actions: HashMap::from([
                ("sodium.jar".to_owned(), ModUpgradeAction::Keep),
                ("old-only.jar".to_owned(), ModUpgradeAction::Keep),
                ("custom-local.jar".to_owned(), ModUpgradeAction::Disable),
            ]),
        };
        let (to_update, to_disable) = resolve_actions(&plan, &opts);
        assert!(to_update.is_empty());
        assert_eq!(to_disable, vec!["custom-local.jar"]);
    }

    #[test]
    fn switching_to_vanilla_marks_all_enabled_mods_incompatible() {
        let inst = dummy_instance(
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::Fabric,
                version: "0.16.10".to_owned(),
            }),
        );
        let items = vec![
            dummy_item("a.jar", true, false, None),
            dummy_item("b.jar", true, true, Some("p1")),
            dummy_item("c.jar", false, false, None),
        ];
        let plan = build_plan(
            &inst,
            "1.21.1".to_owned(),
            None,
            true,
            false,
            false,
            false,
            false,
            false,
            Vec::new(),
            &items,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(plan.preview.counts.incompatible, 2);
        assert_eq!(plan.preview.counts.disabled, 1);
        let (_, disables) = resolve_actions(
            &plan,
            &UpgradeOptions {
                mode: UpgradeMode::InPlace,
                clone_name: None,
                new_name: None,
                include_worlds: false,
                update_mods: true,
                disable_incompatible: true,
                install_dependencies: true,
                mod_actions: HashMap::new(),
            },
        );
        assert_eq!(disables.len(), 2);
    }

    #[test]
    fn same_mc_loader_bump_keeps_mods_compatible_without_network() {
        let inst = dummy_instance(
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.200".to_owned(),
            }),
        );
        let items = vec![
            dummy_item("a.jar", true, false, Some("p1")),
            dummy_item("b.jar", true, true, Some("p2")),
        ];
        let plan = build_plan(
            &inst,
            "1.21.1".to_owned(),
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.219".to_owned(),
            }),
            true,
            true,
            false,
            false,
            false,
            false,
            Vec::new(),
            &items,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(plan.preview.counts.compatible, 1);
        assert_eq!(plan.preview.counts.locked, 1);
        assert!(!plan.preview.is_downgrade);
        assert!(!plan.preview.has_custom_java);
    }

    #[tokio::test]
    async fn execute_clone_and_in_place_upgrade_clears_stale_patches_and_reports_progress() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path());

        for ver in ["1.20.1", "1.21.1"] {
            let ver_dir = game.version_dir(ver).unwrap();
            std::fs::create_dir_all(&ver_dir).unwrap();
            std::fs::write(
                ver_dir.join(format!("{ver}.json")),
                format!(
                    r#"{{"id":"{ver}","type":"release","mainClass":"net.minecraft.client.main.Main","libraries":[]}}"#
                ),
            )
            .unwrap();
            std::fs::write(ver_dir.join(format!("{ver}.jar")), b"fake-client").unwrap();
        }

        let inst_dir = game.instance_dir("srv").unwrap();
        std::fs::create_dir_all(inst_dir.join("mods")).unwrap();
        std::fs::write(inst_dir.join("mods").join("demo.jar"), b"mod-bytes").unwrap();
        // 写入自定义补丁，验证跨版本切换时克隆副本和原地目标都会清理旧补丁、原实例保持不动
        let patch = MetaPatch {
            uid: "org.example.patch".to_owned(),
            name: Some("Demo Patch".to_owned()),
            ..MetaPatch::default()
        };
        patches::save_for_instance(&inst_dir, &InstancePatches::new(vec![patch], false)).unwrap();

        let mut config = InstanceConfig::new(
            "生存服",
            "1.20.1",
            Some(LoaderRef {
                kind: LoaderKind::Fabric,
                version: "0.15.11".to_owned(),
            }),
        );
        config.source = Some(PackSource {
            kind: "mrpack".to_owned(),
            name: "原整合包".to_owned(),
            version: "1.0".to_owned(),
            project_id: None,
            version_id: None,
            cover_url: None,
        });
        let inst = instance::register(&game, "srv", &config).unwrap();

        let client = Client::new();
        let cf = CurseForge::new(client.clone(), None);
        let downloader = crate::net::Downloader::new(client.clone(), 4);
        let cancel = CancellationToken::new();
        let work_dir = temp.path().join("tmp");
        let loader_ctx = LoaderContext {
            game: &game,
            client: &client,
            downloader: &downloader,
            java: std::path::Path::new(""),
            work_dir: &work_dir,
            cancel: &cancel,
        };

        let plan = preview(
            &game,
            &inst,
            &client,
            &cf,
            "1.21.1".to_owned(),
            None,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(plan.preview.counts.incompatible, 1);
        assert!(plan.preview.has_patches);
        assert_eq!(plan.preview.patch_names, vec!["Demo Patch"]);
        assert_eq!(plan.preview.managed_pack_name.as_deref(), Some("原整合包"));

        // 1) 克隆后升级：原实例分毫未动，新实例版本切为 1.21.1 原版，旧补丁与旧包绑定已清理，且收到真实克隆进度
        let clone_events = std::sync::atomic::AtomicU32::new(0);
        let clone_outcome = execute(
            &loader_ctx,
            &cf,
            &plan,
            &UpgradeOptions {
                mode: UpgradeMode::Clone,
                clone_name: Some("生存服-1.21.1".to_owned()),
                new_name: None,
                include_worlds: true,
                update_mods: true,
                disable_incompatible: true,
                install_dependencies: true,
                mod_actions: HashMap::new(),
            },
            |p| {
                if matches!(p, UpgradeProgress::Cloning(_)) {
                    clone_events.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            },
            |_, _| None,
        )
        .await
        .unwrap();
        assert!(clone_events.load(std::sync::atomic::Ordering::Relaxed) >= 1);
        assert_eq!(clone_outcome.disabled_mods, 1);

        let orig = instance::load(&game, "srv").unwrap().unwrap();
        assert_eq!(orig.config.game_version, "1.20.1");
        assert!(orig.config.source.is_some());
        assert!(patches::load_for_instance(&inst_dir).unwrap().is_some());

        let cloned = instance::load(&game, &clone_outcome.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(cloned.config.game_version, "1.21.1");
        assert!(cloned.config.loader.is_none());
        assert!(cloned.config.source.is_none());
        let cloned_dir = cloned.game_dir(&game).unwrap();
        assert!(cloned_dir.join("mods").join("demo.jar.disabled").is_file());
        assert!(patches::load_for_instance(&cloned_dir).unwrap().is_none());

        // 2) 原地升级：原实例版本切为 1.21.1 原版、重命名、旧补丁与旧包绑定已清理
        let inplace_outcome = execute(
            &loader_ctx,
            &cf,
            &plan,
            &UpgradeOptions {
                mode: UpgradeMode::InPlace,
                clone_name: None,
                new_name: Some("生存服 1.21.1".to_owned()),
                include_worlds: true,
                update_mods: true,
                disable_incompatible: true,
                install_dependencies: true,
                mod_actions: HashMap::new(),
            },
            |_| {},
            |_, _| None,
        )
        .await
        .unwrap();
        assert_eq!(inplace_outcome.instance_id, "srv");
        assert_eq!(inplace_outcome.disabled_mods, 1);
        let after = instance::load(&game, "srv").unwrap().unwrap();
        assert_eq!(after.config.name, "生存服 1.21.1");
        assert_eq!(after.config.game_version, "1.21.1");
        assert!(after.config.loader.is_none());
        assert!(after.config.source.is_none());
        assert!(patches::load_for_instance(&inst_dir).unwrap().is_none());
        assert!(inst_dir.join("mods").join("demo.jar.disabled").is_file());
    }

    #[tokio::test]
    async fn linked_instance_supports_clone_upgrade_and_blocks_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path().join("lumi"));
        let ext_root = temp.path().join("pcl").join(".minecraft");
        let ext_ver_dir = ext_root.join("versions").join("1.20.1-Fabric");
        std::fs::create_dir_all(ext_ver_dir.join("mods")).unwrap();
        std::fs::create_dir_all(ext_ver_dir.join("saves").join("MyWorld")).unwrap();
        std::fs::write(ext_ver_dir.join("mods").join("demo.jar"), b"mod-bytes").unwrap();

        let ver_dir = game.version_dir("1.21.1").unwrap();
        std::fs::create_dir_all(&ver_dir).unwrap();
        std::fs::write(
            ver_dir.join("1.21.1.json"),
            r#"{"id":"1.21.1","type":"release","mainClass":"net.minecraft.client.main.Main","libraries":[]}"#,
        )
        .unwrap();
        std::fs::write(ver_dir.join("1.21.1.jar"), b"fake-client").unwrap();

        let mut config = InstanceConfig::new(
            "意外农场",
            "1.20.1",
            Some(LoaderRef {
                kind: LoaderKind::Fabric,
                version: "0.15.6".to_owned(),
            }),
        );
        config.link = Some(crate::instance::Link {
            root: ext_root.clone(),
            version: "1.20.1-Fabric".to_owned(),
            game_dir: ext_ver_dir.clone(),
            launcher: crate::external::Launcher::Pcl,
        });
        std::fs::create_dir_all(game.instance_dir("ext-farm").unwrap()).unwrap();
        let inst = instance::register(&game, "ext-farm", &config).unwrap();

        let client = Client::new();
        let cf = CurseForge::new(client.clone(), None);
        let downloader = crate::net::Downloader::new(client.clone(), 4);
        let cancel = CancellationToken::new();
        let work_dir = temp.path().join("tmp");
        let loader_ctx = LoaderContext {
            game: &game,
            client: &client,
            downloader: &downloader,
            java: std::path::Path::new(""),
            work_dir: &work_dir,
            cancel: &cancel,
        };

        let plan = preview(
            &game,
            &inst,
            &client,
            &cf,
            "1.21.1".to_owned(),
            None,
            &cancel,
        )
        .await
        .unwrap();
        assert!(plan.preview.is_linked);
        assert!(plan.preview.has_worlds);
        assert_eq!(plan.preview.counts.incompatible, 1);

        // 原地升级必须拒绝，保护外部启动器目录
        let err = execute(
            &loader_ctx,
            &cf,
            &plan,
            &UpgradeOptions {
                mode: UpgradeMode::InPlace,
                clone_name: None,
                new_name: None,
                include_worlds: true,
                update_mods: true,
                disable_incompatible: true,
                install_dependencies: true,
                mod_actions: HashMap::new(),
            },
            |_| {},
            |_, _| None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpgradeError::LinkedInstance));

        // 复制并升级为 Lumi 实例：成功生成独立实例且原 PCL 目录分毫未动
        let outcome = execute(
            &loader_ctx,
            &cf,
            &plan,
            &UpgradeOptions {
                mode: UpgradeMode::Clone,
                clone_name: Some("意外农场 1.21.1".to_owned()),
                new_name: None,
                include_worlds: true,
                update_mods: true,
                disable_incompatible: true,
                install_dependencies: true,
                mod_actions: HashMap::new(),
            },
            |_| {},
            |_, _| None,
        )
        .await
        .unwrap();
        let cloned = instance::load(&game, &outcome.instance_id)
            .unwrap()
            .unwrap();
        assert!(!cloned.is_linked());
        assert_eq!(cloned.config.game_version, "1.21.1");
        let cloned_dir = cloned.game_dir(&game).unwrap();
        assert!(cloned_dir.join("mods").join("demo.jar.disabled").is_file());
        // 外部原目录保持原样
        assert!(ext_ver_dir.join("mods").join("demo.jar").is_file());
    }

    #[test]
    fn partial_curseforge_failure_marks_only_cf_items_unknown_not_incompatible() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = instance::register(
            &game,
            "cf-partial",
            &InstanceConfig::new(
                "CF Partial",
                "1.20.1",
                Some(LoaderRef {
                    kind: LoaderKind::Fabric,
                    version: "0.15.0".to_owned(),
                }),
            ),
        )
        .unwrap();

        let mut cf_item = dummy_item("cf-mod.jar", true, false, Some("12345"));
        cf_item.source = Some(crate::content::IdentitySource::CurseForge);
        let mr_item = dummy_item("mr-mod.jar", true, false, Some("AANobbMI"));

        let plan = build_plan(
            &instance,
            "1.21.1".to_owned(),
            Some(LoaderRef {
                kind: LoaderKind::Fabric,
                version: "0.16.0".to_owned(),
            }),
            false,
            true,
            false,
            false,
            true,
            false,
            Vec::new(),
            &[cf_item, mr_item],
            &HashMap::new(),
            &HashSet::new(),
        );
        let cf_res = plan
            .preview
            .mods
            .iter()
            .find(|m| m.file_name == "cf-mod.jar")
            .unwrap();
        let mr_res = plan
            .preview
            .mods
            .iter()
            .find(|m| m.file_name == "mr-mod.jar")
            .unwrap();
        assert_eq!(cf_res.status, ModUpgradeStatus::Unknown);
        assert_eq!(mr_res.status, ModUpgradeStatus::Incompatible);
    }
}
