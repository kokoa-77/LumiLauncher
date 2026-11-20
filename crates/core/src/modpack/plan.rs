//! 导入计划和导入流程（Modrinth、CurseForge 两种格式共用）。
//!
//! 流程（方案 5.3 节第 7 条）：
//! 1. 各格式先**检查整个包**，任何一项不过就拒绝，什么都不写，得到 [`PackPlan`]
//! 2. 在独立的临时目录里组装：装加载器 → 下载文件并校验 → 放入用户补的文件 → 解压覆盖文件
//! 3. 补全游戏文件
//! 4. 全部成功后才把临时目录改名成 `versions/<实例名>/` 并登记；失败只清理临时目录，不碰已有实例和存档

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::ModpackError;
use crate::content::claim::{ClaimSession, ManualFile};
use crate::content::store::{ContentStore, STORE_DIR};
use crate::fs::{safe_join, write_atomic_from};
use crate::install::processors::LoaderContext;
use crate::install::{InstallPhase, LoaderProgress, install_files, load_or_fetch_version, loaders};
use crate::instance::{self, Instance, InstanceConfig, LoaderRef, PackSource};
use crate::meta::Environment;
use crate::net::{DownloadItem, DownloadProgress};

/// 整合包格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum PackFormat {
    /// `.mrpack`
    Modrinth,
    /// `.zip`，里面有 `manifest.json`
    CurseForge,
}

impl PackFormat {
    /// 写进实例信息的来源类型
    fn source_kind(self) -> &'static str {
        match self {
            PackFormat::Modrinth => "mrpack",
            PackFormat::CurseForge => "curseforge",
        }
    }
}

/// 导入前给用户看的信息。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackPreview {
    pub format: PackFormat,
    pub name: String,
    pub version: String,
    pub summary: Option<String>,
    pub game_version: String,
    pub loader: Option<LoaderRef>,
    /// 启动器会自动下载的文件数（已去掉客户端不需要的）
    pub file_count: u32,
    pub download_bytes: f64,
    /// 覆盖文件（配置等）个数
    pub override_count: u32,
    /// 标成「可选」的文件（Modrinth：默认也装；CurseForge：装上但默认停用）
    pub optional_files: Vec<String>,
    /// 作者不允许第三方启动器下载、要用户在浏览器里下载的文件（补件）
    pub manual_files: Vec<ManualFile>,
    /// 作者不允许第三方下载、但 Modrinth 上有一模一样的文件，改从 Modrinth 自动下载的个数
    /// （已经算在 `file_count` 里）
    pub from_modrinth: u32,
    /// 作者推荐的内存（MB）：CurseForge 整合包的清单里有，Modrinth 整合包没有这个字段
    pub recommended_memory_mb: Option<u32>,
}

/// 整合包在平台上的出处（从平台下载的才有）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackOrigin {
    pub project_id: String,
    pub version_id: String,
}

/// 检查通过后的导入计划。
#[derive(Debug, Clone)]
pub struct PackPlan {
    pub preview: PackPreview,
    /// 调用方填：从平台下载的包记下出处，导入后写进实例信息（以后做整合包更新用）
    pub origin: Option<PackOrigin>,
    pub(super) pack_path: PathBuf,
    pub(super) downloads: Vec<PlannedFile>,
    /// 压缩包条目名 → 实例里的相对路径（按覆盖顺序）
    pub(super) overrides: Vec<(String, String)>,
}

impl PackPlan {
    /// 整合包文件在哪（只给后端用：重试本地导入时记下来，D18）
    pub fn pack_path(&self) -> &Path {
        &self.pack_path
    }
}

#[derive(Debug, Clone)]
pub(super) struct PlannedFile {
    pub(super) path: String,
    pub(super) urls: Vec<String>,
    pub(super) sha1: String,
    pub(super) size: Option<u64>,
}

/// 导入需要的环境。
pub struct ImportContext<'a> {
    pub loader: LoaderContext<'a>,
    /// 放临时目录的地方（和游戏目录在同一个盘，组装好后直接改名，不用复制）
    pub temp_root: &'a Path,
    /// 有要用户补的文件时必须给：在这里等用户补齐
    pub manual: Option<&'a ClaimSession>,
}

/// 导入进度。
#[derive(Debug, Clone)]
pub enum ImportProgress {
    Loader(LoaderProgress),
    /// 下载整合包里的文件
    Files(DownloadProgress),
    /// 等用户补文件（还缺几个）
    WaitingManual(u32),
    /// 解压覆盖文件
    Overrides,
    /// 补全游戏本体文件
    Game(InstallPhase, DownloadProgress),
}

/// 按计划导入，返回新实例。`name` 不填就用整合包的名字。
///
/// 能接着装（D18）：临时目录按 `resume_key` 起名（[`super::staging_dir`]），**只在成功后删掉**；
/// 失败、取消时留着，用同一个 `resume_key` 再导入一次，下过的文件校验通过就不重下。
/// 同一个整合包同一时间只能导入一份（调用方用任务键保证），不然两个导入会共用一个临时目录。
pub async fn import<F>(
    ctx: &ImportContext<'_>,
    plan: &PackPlan,
    name: Option<&str>,
    resume_key: &str,
    on_progress: F,
) -> Result<Instance, ModpackError>
where
    F: Fn(ImportProgress) + Send + Sync,
{
    let staging = super::staging_dir(ctx.temp_root, resume_key);
    let result = assemble_and_register(ctx, plan, name, &staging, &on_progress).await;
    if result.is_ok() {
        super::discard_staging(&staging).await;
    } else {
        tracing::info!(dir = %staging.display(), "导入没做完，临时目录留着，重试时接着用");
    }
    result
}

async fn assemble_and_register<F>(
    ctx: &ImportContext<'_>,
    plan: &PackPlan,
    name: Option<&str>,
    staging: &Path,
    on_progress: &F,
) -> Result<Instance, ModpackError>
where
    F: Fn(ImportProgress) + Send + Sync,
{
    let game = ctx.loader.game;
    let content = staging.join("instance");
    tokio::fs::create_dir_all(&content)
        .await
        .map_err(|source| ModpackError::Io {
            path: content.clone(),
            source,
        })?;
    tracing::info!(pack = %plan.preview.name, version = %plan.preview.version, "开始导入整合包");

    // 1. 加载器（已装过的瞬间跳过）
    let version_id = if let Some(loader) = &plan.preview.loader {
        let loader_work = staging.join("loader");
        let loader_ctx = LoaderContext {
            work_dir: &loader_work,
            ..ctx.loader
        };
        loaders::install(&loader_ctx, &plan.preview.game_version, loader, |p| {
            on_progress(ImportProgress::Loader(p))
        })
        .await?
    } else {
        plan.preview.game_version.clone()
    };

    // 2. 整合包里的文件
    let items = plan
        .downloads
        .iter()
        .map(|file| {
            Ok(DownloadItem {
                urls: file.urls.clone(),
                path: safe_join(&content, &file.path)?,
                sha1: Some(file.sha1.clone()),
                size: file.size,
            })
        })
        .collect::<Result<Vec<_>, ModpackError>>()?;
    ctx.loader
        .downloader
        .download_all(&items, ctx.loader.cancel, |p| {
            on_progress(ImportProgress::Files(p))
        })
        .await?;

    // 3. 用户补的文件（补件会话在后台一直盯着下载文件夹，这里等它补齐）
    if !plan.preview.manual_files.is_empty() {
        let session = ctx.manual.ok_or_else(|| {
            ModpackError::Invalid("有需要手动下载的文件，但没有开始补件".to_owned())
        })?;
        let waiting = session
            .snapshot()
            .entries
            .iter()
            .filter(|e| e.state != crate::content::claim::ClaimState::Claimed)
            .count();
        on_progress(ImportProgress::WaitingManual(
            u32::try_from(waiting).unwrap_or(u32::MAX),
        ));
        let local = session.wait(ctx.loader.cancel).await?;
        let store = ContentStore::new(game.root().join(STORE_DIR));
        for file in local {
            let dest = safe_join(&content, &file.path)?;
            store
                .restore(&file.sha1, &dest)
                .await
                .map_err(|source| ModpackError::Io { path: dest, source })?;
        }
    }

    // 4. 覆盖文件（按覆盖顺序）
    on_progress(ImportProgress::Overrides);
    {
        let pack = plan.pack_path.clone();
        let overrides = plan.overrides.clone();
        let content = content.clone();
        tokio::task::spawn_blocking(move || extract_overrides(&pack, &overrides, &content))
            .await
            .map_err(|e| ModpackError::Invalid(format!("解压任务异常结束：{e}")))??;
    }

    // 5. 游戏本体文件（导入完就能直接玩）
    let version =
        load_or_fetch_version(game, &version_id, ctx.loader.client, ctx.loader.meta_source).await?;
    install_files(
        version,
        game,
        &Environment::current(),
        ctx.loader.downloader,
        ctx.loader.source,
        ctx.loader.cancel,
        |phase, p| on_progress(ImportProgress::Game(phase, p)),
    )
    .await?;

    // 6. 全部成功：改名成正式实例并登记（实例指向 versions/ 里共用的版本，D18a）
    let display_name = name.unwrap_or(&plan.preview.name);
    let id = instance::unique_id(game, display_name);
    let target = game.instance_dir(&id)?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| ModpackError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    tokio::fs::rename(&content, &target)
        .await
        .map_err(|source| ModpackError::Io {
            path: target.clone(),
            source,
        })?;
    let mut config = InstanceConfig::new(
        display_name,
        &plan.preview.game_version,
        plan.preview.loader.clone(),
    );
    config.source = Some(PackSource {
        kind: plan.preview.format.source_kind().to_owned(),
        name: plan.preview.name.clone(),
        version: plan.preview.version.clone(),
        project_id: plan.origin.as_ref().map(|o| o.project_id.clone()),
        version_id: plan.origin.as_ref().map(|o| o.version_id.clone()),
    });
    config.recommended_memory_mb = plan.preview.recommended_memory_mb;
    config.version_id = version_id;
    let registered = instance::register(game, &id, &config);
    if registered.is_err() {
        // 登记失败就把刚改名的文件夹删掉，不留半个实例
        if let Err(error) = tokio::fs::remove_dir_all(&target).await {
            tracing::warn!(%error, dir = %target.display(), "清理未登记的实例目录失败");
        }
    }
    let instance = registered?;
    tracing::info!(instance = %instance.id, "整合包导入完成");
    Ok(instance)
}

fn extract_overrides(
    pack: &Path,
    overrides: &[(String, String)],
    content: &Path,
) -> Result<(), ModpackError> {
    let mut archive = super::check::open_archive(pack)?;
    for (entry_name, relative) in overrides {
        let mut entry = archive
            .by_name(entry_name)
            .map_err(|e| ModpackError::Invalid(format!("读取 {entry_name} 失败：{e}")))?;
        // 边解压边写（D17l，外部审阅指出）：整合包里的大文件（世界、材质包）不先整个读进内存
        write_atomic_from(&safe_join(content, relative)?, &mut entry)?;
    }
    Ok(())
}
