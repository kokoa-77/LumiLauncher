//! 实例、启动、资源相关的命令。这一层只做：取参数 → 调核心库 → 登记任务 / 会话 → 发事件。
//! 界面只传实例 ID、项目 ID，路径全部在这里由核心库解析。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use launcher_core::content::curseforge as cf;
use launcher_core::content::modrinth::{self, PlanItem, SearchPage, SearchQuery, Target};
use launcher_core::content::{
    ContentItem, ContentKind, ContentSource, HistoryView, ImpactAction, InstanceContent,
    ModDepsGraph, ModImpactReport, RollbackOffer, SearchSort, TimelineView, UpdateCheckResult,
    UpdateInfo,
};
use launcher_core::install::loaders::{self, LoaderVersion};
use launcher_core::install::neoforge::{self, NeoForgeVersion};
use launcher_core::install::setup::{SetupProgress, create_instance};
use launcher_core::install::{InstallPhase, LoaderContext, LoaderProgress};
use launcher_core::instance::{self, Instance, LoaderKind, LoaderRef};
use launcher_core::java::{self, JavaContext, JavaRequest};
use launcher_core::jobs::{JobHandle, JobMeta, JobSource, JobState, ProgressUnit};
use launcher_core::launch::{
    GameEvent, GameState, LaunchOptions, LaunchOutcome, LogLine, MemoryChoice, PrepareContext,
    PrepareProgress, QuickPlay, QuickPlayRecord, RecordEnd, autocover, prepare, run_game,
};
use launcher_core::meta::{GameVersionEntry, fetch_manifest};
use launcher_core::modpack::ready::{self, PackReadiness};
use launcher_core::modpack::remote::{self, PackVersion};
use launcher_core::modpack::{
    self, ImportContext, ImportProgress, PackFormat, PackOrigin, PackPlan, PackPreview, curseforge,
    mrpack, multimc,
};
use launcher_core::net::{DownloadPriority, DownloadProgress, Downloader};
use launcher_core::paths::GameDir;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::downloads::{self, Retry};
use crate::error::AppError;
use crate::events::{GameLogBatch, SessionUpdated, emit};
use crate::records::{self, Recorder};
use crate::sessions::SessionSnapshot;
use crate::state::AppState;
use crate::turns::busy_error;

const LAUNCHER_NAME: &str = "Lumi";
/// 日志批量推送的间隔
const LOG_FLUSH: Duration = Duration::from_millis(150);
pub(crate) const SEARCH_PAGE: u32 = 20;

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// 批量下载器（游戏本体/整合包等大任务，遵守 75% Batch 配额不饿死交互请求）
pub(crate) fn downloader(state: &AppState) -> Downloader {
    let settings = state.settings.get();
    Downloader::from_engine(
        state.engine.clone(),
        DownloadPriority::Batch,
        settings.download.max_concurrency as usize,
    )
    .with_host_header(state.curseforge.download_header())
}

/// 绑定任务 [`PauseControl`] 与优先级的下载器。
pub(crate) fn downloader_for_job(
    state: &AppState,
    priority: DownloadPriority,
    job: &JobHandle,
) -> Downloader {
    let settings = state.settings.get();
    Downloader::from_engine(
        state.engine.clone(),
        priority,
        settings.download.max_concurrency as usize,
    )
    .with_host_header(state.curseforge.download_header())
    .with_pause(job.pause_control())
}

pub(crate) fn load_instance(state: &AppState, id: &str) -> Result<Instance, AppError> {
    instance::load(&state.game(), id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))
}

pub(crate) fn game_dir_of(state: &AppState, id: &str) -> Result<PathBuf, AppError> {
    let game = state.game();
    let instance = instance::load(&game, id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    Ok(instance.game_dir(&game)?)
}

pub(crate) fn content_of(state: &AppState, id: &str) -> Result<InstanceContent, AppError> {
    let instance = load_instance(state, id)?;
    let game = state.game();
    Ok(InstanceContent::for_game(&game, instance.game_dir(&game)?))
}

/// 下载进度 → 任务进度
/// 下载进度 → 任务进度（和计数说明一起节流，几千个文件也不会刷爆事件）
pub(crate) fn report(job: &JobHandle, label: &str, p: &DownloadProgress) {
    // 换了一步（「下载整合包」→「下载整合包文件」）就记进步骤里（下载历史展开看每一步）
    job.set_step(label);
    job.set_reused(p.reused_bytes, p.reused_files);
    job.set_download_progress(
        p.done_bytes as f64,
        Some(p.total_bytes as f64),
        p.received_bytes as f64,
        u32::try_from(p.fallbacks).unwrap_or(u32::MAX),
        format!("{label} {}/{}", p.done_files, p.total_files),
    );
}

/// 补全游戏文件的几段各叫什么（下载历史里一段一步，进度条不会在几段之间来回跳着看不懂）
pub(crate) fn phase_label(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::AssetIndex => "下载资源索引",
        InstallPhase::Files => "补全游戏文件",
        InstallPhase::Natives => "解压本地库",
    }
}

pub(crate) fn report_loader(job: &JobHandle, progress: &LoaderProgress) {
    match progress {
        LoaderProgress::GameJar(p) => report(job, "下载原版客户端", p),
        LoaderProgress::Installer(p) => report(job, "下载加载器安装器", p),
        LoaderProgress::Libraries(p) => report(job, "下载加载器依赖", p),
        LoaderProgress::Processor { index, total, task } => {
            job.set_message(format!("安装加载器 {index}/{total}：{task}"));
        }
    }
}

// ---------- 实例 ----------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSummary {
    pub id: String,
    pub name: String,
    pub game_version: String,
    /// 例如「NeoForge 21.1.248」；原版为空
    pub loader: Option<String>,
    /// 来自哪个整合包
    pub pack: Option<String>,
    /// 当前安装的整合包版本号
    pub pack_version: Option<String>,
    /// 若来自在线整合包平台（Modrinth / CurseForge 且带 project_id），标识其平台
    pub pack_source: Option<ContentSource>,
    /// 整合包在平台上的项目 ID（在线一键检查更新用）
    pub pack_project_id: Option<String>,
    /// 整合包在平台上的当前版本 ID
    pub pack_version_id: Option<String>,
    /// 是否存在可一键无损回滚的整合包更新记录
    pub pack_rollback: Option<launcher_core::modpack::update::PackRollbackSummary>,
    pub created_at: f64,
    pub last_played: Option<f64>,
    /// 实例库里的分组；没分组为空
    pub group: Option<String>,
    /// 置顶的时间；没置顶为空
    pub pinned_at: Option<f64>,
    /// 挂上的别家版本：是哪家的；本启动器自己的实例为空
    pub linked: Option<launcher_core::external::Launcher>,
    /// 实例级自定义封面（或整合包官方封面/图标 URL）；无自定义且非整合包时为空
    pub cover_url: Option<String>,
    /// 实例级自定义封面的显示方式
    pub cover_fit: launcher_core::saves::cover::ImageFit,
    /// 默认微光矢量封面预设（`<徽章>:<色板>`）；为空则按实例自动分配
    pub cover_preset: Option<String>,
}

fn summarize_instance(state: &AppState, game: &GameDir, instance: Instance) -> InstanceSummary {
    let c = instance.config;
    let pack_cover = c
        .source
        .as_ref()
        .and_then(|s| s.cover_url.clone())
        .filter(|u| u.starts_with("https://"));
    // 自定义封面放在 Lumi 自己的 `instances/<id>/Lumi/covers/`，关联实例也能安全保存不碰原启动器
    let local_cover = game
        .instance_dir(&instance.id)
        .ok()
        .and_then(|dir| launcher_core::saves::cover::instance_custom(&dir))
        .map(|(path, fit)| (state.images.url_for(&path), fit));
    let (cover_url, cover_fit) = match local_cover {
        Some((url, fit)) => (Some(url), fit),
        None => (pack_cover, launcher_core::saves::cover::ImageFit::Cover),
    };
    let pack_rollback = if c.link.is_none() {
        game.instance_dir(&instance.id)
            .ok()
            .and_then(|dir| launcher_core::modpack::update::rollback_summary(&dir))
    } else {
        None
    };
    let (pack, pack_version, pack_source, pack_project_id, pack_version_id) = match c.source {
        Some(s) => {
            let platform = match s.kind.as_str() {
                "mrpack" if s.project_id.is_some() => Some(ContentSource::Modrinth),
                "curseforge" if s.project_id.is_some() => Some(ContentSource::CurseForge),
                _ => None,
            };
            (
                Some(s.name),
                (!s.version.trim().is_empty()).then_some(s.version),
                platform,
                s.project_id,
                s.version_id,
            )
        }
        None => (None, None, None, None, None),
    };
    InstanceSummary {
        id: instance.id,
        name: c.name,
        game_version: c.game_version,
        loader: c
            .loader
            .map(|l| format!("{} {}", l.kind.label(), l.version)),
        pack,
        pack_version,
        pack_source,
        pack_project_id,
        pack_version_id,
        pack_rollback,
        created_at: c.created_at,
        last_played: c.last_played,
        group: c.group,
        pinned_at: c.pinned_at,
        linked: c.link.map(|l| l.launcher),
        cover_url,
        cover_fit,
        cover_preset: c.cover_preset,
    }
}

/// 全部实例：最近玩过的在前，没玩过的按创建时间。
#[tauri::command]
#[specta::specta]
pub fn instances_list(state: State<'_, AppState>) -> Vec<InstanceSummary> {
    let game = state.game();
    let mut list: Vec<InstanceSummary> = instance::list(&game)
        .into_iter()
        .map(|inst| summarize_instance(&state, &game, inst))
        .collect();
    list.sort_by(|a, b| {
        let key = |i: &InstanceSummary| i.last_played.unwrap_or(0.0).max(i.created_at / 1e6);
        b.last_played
            .is_some()
            .cmp(&a.last_played.is_some())
            .then_with(|| key(b).total_cmp(&key(a)))
            .then_with(|| b.created_at.total_cmp(&a.created_at))
    });
    list
}

/// 在资源管理器里打开实例的游戏文件夹（存档、模组、截图都在里面）。
/// 界面只传实例 ID，路径在这里由核心库解析，界面拿不到路径。
#[tauri::command]
#[specta::specta]
pub fn instances_open_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let instance = load_instance(&state, &instance_id)?;
    let dir = instance.game_dir(&state.game())?;
    tracing::info!(instance = %instance.id, "打开实例文件夹");
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| {
            AppError::new("instance.open_folder_failed", "打不开实例文件夹").with_detail(e)
        })
}

/// 新建实例（后台任务），返回任务 ID。`loader` 为空时建原版实例。
#[tauri::command]
#[specta::specta]
pub fn instances_create(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    game_version: String,
    loader: Option<LoaderRef>,
) -> Result<String, AppError> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(AppError::new("instance.name_empty", "请给实例起个名字"));
    }
    if let Some(loader) = &loader {
        loaders::validate(&game_version, loader)?;
    }
    let title = format!("新建实例 {name}");
    let retry = Retry::CreateInstance {
        name: name.clone(),
        game_version: game_version.clone(),
        loader: loader.clone(),
        neoforge: None,
    };
    let job = state.jobs.start_with(
        format!("instance-create:{name}"),
        title,
        ProgressUnit::Bytes,
        downloads::meta(JobSource::Game, None, None, Some(&retry)),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = async {
            let downloader = downloader_for_job(&state, DownloadPriority::Batch, &job);
            let java = pick_java_for(&state, &job, &downloader, &game_version).await?;
            let game = state.game();
            let work_dir = game.root().join(".mcl-tmp").join("create");
            let cancel = job.cancel_token();
            let ctx = LoaderContext {
                game: &game,
                client: &state.http_client(),
                downloader: &downloader,
                java: &java,
                work_dir: &work_dir,
                cancel: &cancel,
            };
            create_instance(&ctx, &name, &game_version, loader, |p| match &p {
                SetupProgress::Loader(l) => report_loader(&job, l),
                SetupProgress::Files(phase, d) => report(&job, phase_label(*phase), d),
            })
            .await
            .map_err(|e| state.capture_app_error(&job, "下载实例文件", e))
        }
        .await;
        match result {
            Ok(instance) => {
                tracing::info!(instance = %instance.id, "实例已创建");
                job.set_instance(instance.id);
                job.succeed();
            }
            Err(error) if job.is_cancelled() => {
                tracing::info!(error = %error.message, "新建实例已取消");
                job.cancelled();
            }
            Err(error) => state.finish_failed_job(job, error.message),
        }
    });
    Ok(job_id)
}

/// 游戏版本、库和资源由 Lumi 管理的实例共用。在启动登记和修复写入之间用同一把锁。
fn repair_key(game: &GameDir) -> PathBuf {
    crate::turns::directory_key(game.root()).join(".lumi-game-files")
}

fn repair_busy() -> AppError {
    AppError::new(
        "install.repair_busy",
        "正在修复游戏文件。请等任务完成后再启动游戏",
    )
}

/// 校验 Lumi 管理的游戏文件，补回缺少或校验失败的文件。
#[tauri::command]
#[specta::specta]
pub fn instances_repair_files(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, AppError> {
    let game = state.game();
    let instance = load_instance(&state, &instance_id)?;
    if instance.is_linked() {
        return Err(AppError::new(
            "install.repair_linked",
            "该实例来自其他启动器，请在原启动器中修复游戏文件",
        ));
    }
    let files_turn = state
        .turns
        .try_take(&repair_key(&game))
        .ok_or_else(repair_busy)?;
    if any_game_running(&state) {
        return Err(AppError::new(
            "install.repair_game_running",
            "请先退出正在运行的游戏",
        ));
    }
    let directory = instance.game_dir(&game)?;
    let turn = state.turns.try_take(&directory).ok_or_else(busy_error)?;
    let job = state.jobs.start_with(
        format!("repair:{instance_id}"),
        format!("修复游戏文件 · {}", instance.config.name),
        ProgressUnit::Bytes,
        JobMeta {
            source: JobSource::Game,
            instance_id: Some(instance_id),
            ..JobMeta::default()
        },
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let (_files_turn, _turn) = (files_turn, turn);
        let state = app.state::<AppState>();
        let cancel = job.cancel_token();
        let result: Result<(), AppError> = async {
            use launcher_core::install::setup::{repair_base_metadata, repair_version};
            let downloader =
                downloader_for_job(&state, DownloadPriority::Batch, &job).quick_verify(false);
            let config = &instance.config;
            let version_id = instance.version_id();
            job.set_step("检查版本文件");
            let java = if config.loader.as_ref().is_some_and(|loader| {
                loader.kind.uses_processor()
                    && loader.version_id(&config.game_version) == version_id
            }) {
                repair_base_metadata(&game, &config.game_version, &state.http_client()).await?;
                pick_java_for(&state, &job, &downloader, &config.game_version).await?
            } else {
                PathBuf::new()
            };
            let work_dir = game.root().join(".mcl-tmp");
            let ctx = LoaderContext {
                game: &game,
                client: &state.http_client(),
                downloader: &downloader,
                java: &java,
                work_dir: &work_dir,
                cancel: &cancel,
            };
            repair_version(
                &ctx,
                &config.game_version,
                config.loader.as_ref(),
                &version_id,
                |p| match &p {
                    SetupProgress::Loader(progress) => report_loader(&job, progress),
                    SetupProgress::Files(_, progress) => {
                        report(&job, "校验并补全游戏文件", progress)
                    }
                },
            )
            .await
            .map_err(|e| state.capture_app_error(&job, "校验并补全游戏文件", e))?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => job.succeed(),
            Err(error) if error.code == "job.cancelled" || cancel.is_cancelled() => job.cancelled(),
            Err(error) => {
                tracing::warn!(message = %error.message, detail = ?error.detail, "修复游戏文件失败");
                state.finish_failed_job(job, error.message);
            }
        }
    });
    Ok(job_id)
}

/// 运行安装处理器用的 Java：按游戏版本的要求选，电脑上没有就下载（进度记在任务上）
pub(crate) async fn pick_java_for(
    state: &AppState,
    job: &JobHandle,
    downloader: &Downloader,
    game_version: &str,
) -> Result<std::path::PathBuf, AppError> {
    let vanilla = launcher_core::install::load_or_fetch_version(
        &state.game(),
        game_version,
        &state.http_client(),
    )
    .await?;
    let request = JavaRequest::of(&vanilla);
    let locator = state.java_locator();
    let cancel = job.cancel_token();
    let ctx = JavaContext {
        locator: &locator,
        client: &state.http_client(),
        downloader,
        cancel: &cancel,
    };
    let label = format!("下载 Java {}", request.major);
    let found = java::find_or_install(&ctx, &request, |p| report(job, &label, &p)).await?;
    Ok(found.executable)
}

// ---------- 版本列表（新建实例向导用）----------

async fn populate_game_catalog(
    state: &State<'_, AppState>,
) -> Result<(Vec<String>, Vec<GameVersionEntry>), AppError> {
    let manifest = match fetch_manifest(&state.http_client()).await {
        Ok(m) => m,
        Err(_) if state.refresh_proxy_if_changed().is_some() => {
            fetch_manifest(&state.http_client()).await?
        }
        Err(err) => return Err(err.into()),
    };
    let entries = manifest.version_entries();
    let mut versions: Vec<String> = manifest
        .versions
        .into_iter()
        .filter(|v| v.is_release())
        .map(|v| v.id)
        .collect();
    if let Some(latest) = versions.iter().position(|v| v == &manifest.latest.release) {
        versions[..=latest].rotate_right(1);
    }
    *state
        .catalog
        .game_versions
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(versions.clone());
    *state
        .catalog
        .game_version_entries
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(entries.clone());
    Ok((versions, entries))
}

/// 正式版列表（新的在前）
#[tauri::command]
#[specta::specta]
pub async fn catalog_game_versions(state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    if let Some(cached) = state
        .catalog
        .game_versions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Ok(cached);
    }
    let (versions, _) = populate_game_catalog(&state).await?;
    Ok(versions)
}

/// 全量 Mojang 版本清单（含正式版 / 快照版 / 愚人节版 / 远古版分类与最新发布标记）
#[tauri::command]
#[specta::specta]
pub async fn catalog_game_version_entries(
    state: State<'_, AppState>,
) -> Result<Vec<GameVersionEntry>, AppError> {
    if let Some(cached) = state
        .catalog
        .game_version_entries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Ok(cached);
    }
    let (_, entries) = populate_game_catalog(&state).await?;
    Ok(entries)
}

/// 某个游戏版本可用的 NeoForge（新的在前）
#[tauri::command]
#[specta::specta]
pub async fn catalog_neoforge_versions(
    state: State<'_, AppState>,
    game_version: String,
) -> Result<Vec<NeoForgeVersion>, AppError> {
    let cached = state
        .catalog
        .neoforge
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let all = match cached {
        Some(all) => all,
        None => {
            let all =
                match neoforge::fetch_versions_with_doh(&state.http_client(), Some(&state.doh))
                    .await
                {
                    Ok(v) => v,
                    Err(_) if state.refresh_proxy_if_changed().is_some() => {
                        neoforge::fetch_versions_with_doh(&state.http_client(), Some(&state.doh))
                            .await?
                    }
                    Err(err) => return Err(err.into()),
                };
            *state
                .catalog
                .neoforge
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(all.clone());
            all
        }
    };
    Ok(all
        .into_iter()
        .filter(|v| v.game_version == game_version)
        .collect())
}

/// 当前游戏版本可选的加载器版本。None 表示 Lumi 尚未支持这个组合的安装。
#[tauri::command]
#[specta::specta]
pub async fn catalog_loader_versions(
    state: State<'_, AppState>,
    game_version: String,
    kind: LoaderKind,
) -> Result<Option<Vec<LoaderVersion>>, AppError> {
    if !kind.installable_for(&game_version) {
        return Ok(None);
    }
    if kind == LoaderKind::NeoForge {
        return Ok(Some(
            catalog_neoforge_versions(state, game_version)
                .await?
                .into_iter()
                .map(|v| LoaderVersion {
                    version: v.version,
                    stable: v.stable,
                })
                .collect(),
        ));
    }
    let versions = match loaders::fetch_versions(&state.http_client(), &game_version, kind).await {
        Ok(v) => v,
        Err(_) if state.refresh_proxy_if_changed().is_some() => {
            loaders::fetch_versions(&state.http_client(), &game_version, kind).await?
        }
        Err(err) => return Err(err.into()),
    };
    Ok(Some(versions))
}

// ---------- 启动 ----------

/// 启动实例。`world` 是要直接进入的存档文件夹名，`server` 是要直接进入的服务器地址（两个都给时进存档）。
/// `memory_mb` 是「这次给多少内存」（启动前检查发现内存不够、用户选了这次少给一点时传），平时为空。
/// 准备过程是一个任务，游戏状态通过事件推送。
#[tauri::command]
#[specta::specta]
pub fn launch_start(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    world: Option<String>,
    server: Option<String>,
    memory_mb: Option<u32>,
) -> Result<(), AppError> {
    let selection = crate::accounts::preflight(&state)?;
    let instance = load_instance(&state, &instance_id)?;
    let game = state.game();
    let directory = instance.game_dir(&game)?;
    let directory_key = crate::turns::directory_key(&directory);
    let _files_turn = if instance.is_linked() {
        None
    } else {
        Some(
            state
                .turns
                .try_take(&repair_key(&game))
                .ok_or_else(repair_busy)?,
        )
    };
    let auth_blocked = crate::events::LaunchAuthBlocked {
        instance_id: instance_id.clone(),
        world: world.clone(),
        server_address: server.clone(),
        memory_mb,
        code: String::new(),
        message: String::new(),
    };
    let quick_play = match world {
        Some(folder) => launcher_core::saves::supports_quick_play(&instance.config.game_version)
            .then_some(QuickPlay::World(folder)),
        None => server.map(QuickPlay::Server),
    };
    if state.sessions.running_in(&directory_key) {
        return Err(AppError::new(
            "launch.already_running",
            "同一个游戏目录已有实例在运行",
        ));
    }
    if instance_working(&state, &instance_id, &directory) {
        return Err(AppError::new(
            "launch.instance_busy",
            format!(
                "「{}」所在的游戏目录有任务进行中，等做完再启动（任务中心里能看到进度）",
                instance.config.name
            ),
        ));
    }
    let turn = state.turns.try_take(&directory).ok_or_else(busy_error)?;
    // 启动前的准备不进下载历史（每次进游戏都有一条，太多）
    let job = state.jobs.start_with(
        format!("launch:{instance_id}"),
        format!("准备启动 {}", instance.config.name),
        ProgressUnit::Bytes,
        JobMeta {
            source: JobSource::Game,
            instance_id: Some(instance_id.clone()),
            transient: true,
            ..JobMeta::default()
        },
    )?;
    let Some(stop) = state.sessions.begin(&instance_id, directory_key, now()) else {
        return Err(AppError::new(
            "launch.already_running",
            "同一个游戏目录已有实例在运行",
        ));
    };
    if let Some(snapshot) = state
        .sessions
        .list()
        .into_iter()
        .find(|s| s.instance_id == instance_id)
    {
        emit(&app, SessionUpdated(snapshot));
    }
    let quick_play_record = quick_play.as_ref().map(QuickPlayRecord::from);

    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _turn = turn;
        let notify_auth = |error: &AppError| {
            if error.code.starts_with("auth.") && error.code != "auth.cancelled" {
                emit(
                    &app,
                    crate::events::LaunchAuthBlocked {
                        code: error.code.clone(),
                        message: error.message.clone(),
                        ..auth_blocked.clone()
                    },
                );
            }
        };
        // 这一次的完整记录：读模组列表要一点时间，放在后台，不拖慢点击
        let recorder = records::begin(&state, &instance, quick_play_record);
        if let Some(record_id) = records::id(&recorder)
            && let Some(snapshot) = state.sessions.set_record(&instance_id, &record_id)
        {
            emit(&app, SessionUpdated(snapshot));
        }
        let settings = state.settings.get();
        let downloader = downloader_for_job(&state, DownloadPriority::Interactive, &job);
        let temp_root = game.root().join(".mcl-tmp");
        let cancel = job.cancel_token();
        let java_locator = state.java_locator();
        // 全局设置 + 这个实例自己的设置，没单独设的那组跟全局
        let resolved = instance::resolve(&settings.game, &instance.config.overrides);
        let hook_ctx = launcher_core::launch::hooks::HookContext {
            instance_id: instance.id.clone(),
            instance_name: instance.config.name.clone(),
            game_version: instance.config.game_version.clone(),
            loader: instance
                .config
                .loader
                .as_ref()
                .map(|l| format!("{} {}", l.kind.label(), l.version)),
            game_dir: directory.clone(),
        };
        let custom_title = launcher_core::launch::hooks::render_window_title(
            &resolved.custom_window_title,
            &hook_ctx,
            &selection.player_name,
        );
        let discord_enabled = settings.game.discord_rpc;
        let mut options = LaunchOptions::from_resolved(&resolved);
        let ui_lang = state.ui_language();
        options.default_language = settings
            .game
            .default_language
            .resolve(Some(ui_lang.as_str()))
            .map(str::to_owned);
        let memory = match memory_mb {
            Some(mb) => MemoryChoice::Manual(mb.clamp(512, 65536)),
            None => MemoryChoice::from_resolved(&resolved),
        };
        let verify_account = async || {
            job.set_step("核验 Java 版游玩权限");
            tokio::select! {
                _ = stop.cancelled() => Err(AppError::new("auth.cancelled", "已取消启动")),
                result = crate::accounts::ready(&state, &selection, &cancel) => result,
            }
        };
        let prepared = async {
            let account = verify_account().await?;
            // 启动前钩子：非空时先执行，失败中止启动
            if !resolved.hooks.pre_launch.is_empty() {
                job.set_step("执行启动前钩子");
                launcher_core::launch::hooks::run_hook(&resolved.hooks.pre_launch, &hook_ctx)
                    .await
                    .map_err(|e| {
                        AppError::new("launch.hook_failed", format!("启动前钩子失败：{e}"))
                            .with_detail(e)
                    })?;
            }
            // 获取许可后才回滚未完成的内容操作和准备游戏文件。
            settle_before_launch(&game, &instance, directory).await?;
            if let Some(QuickPlay::World(folder)) = &quick_play {
                crate::backups::before_play(&state, &job, &instance_id, folder).await;
            }
            // 启动前将本机共享同步库增量拉取到该实例（开启总开关且未脱离同步时生效）
            crate::sync::before_launch(&state, &instance).await;
            let prepare_account = async |account: &launcher_core::auth::LaunchAccount,
                                         quick_play| {
                let ctx = PrepareContext {
                    game: &game,
                    client: &state.http_client(),
                    downloader: &downloader,
                    temp_root: &temp_root,
                    cancel: &cancel,
                    account,
                    java: &java_locator,
                    memory,
                    options: &options,
                    launcher_name: LAUNCHER_NAME,
                    launcher_version: env!("CARGO_PKG_VERSION"),
                };
                prepare(&ctx, &instance_id, quick_play, |progress| match progress {
                    PrepareProgress::Loader(loader) => report_loader(&job, &loader),
                    PrepareProgress::Files(_, files) => report(&job, "检查游戏文件", &files),
                    PrepareProgress::Java(java) => report(&job, "下载 Java", &java),
                })
                .await
            };
            match prepare_account(&account, quick_play.clone()).await {
                Err(launcher_core::launch::LaunchError::Authorization(
                    launcher_core::auth::LaunchAuthorizationError::Expired,
                )) => {
                    let refreshed = verify_account().await?;
                    prepare_account(&refreshed, quick_play)
                        .await
                        .map_err(|e| state.capture_app_error(&job, "检查游戏文件", e))
                }
                result => result.map_err(|e| state.capture_app_error(&job, "检查游戏文件", e)),
            }
        }
        .await;
        let prepared = match prepared {
            Ok(prepared) => {
                job.succeed();
                prepared
            }
            Err(error) => {
                notify_auth(&error);
                records::with(&recorder, |writer| writer.set_failure_code(&error.code));
                records::finish(
                    &app,
                    &recorder,
                    &instance_id,
                    RecordEnd {
                        ended_at: 0.0,
                        outcome: LaunchOutcome::Failed,
                        exit_code: None,
                        reached_world: false,
                        error: Some(error.message.clone()),
                        crash_report: None,
                    },
                    None,
                );
                if let Some(snapshot) = state.sessions.fail(&instance_id, error.message.clone()) {
                    emit(&app, SessionUpdated(snapshot));
                }
                state.finish_failed_job(job, error.message);
                return;
            }
        };
        records::with(&recorder, |w| {
            w.set_launch(
                prepared.java(),
                prepared.java_version(),
                prepared.memory_mb(),
            )
        });
        crate::system::prefer_gpu(&state, prepared.java());
        tracing::info!(
            instance = %instance_id,
            java = %prepared.java_version(),
            memory_mb = prepared.memory_mb(),
            "开始启动游戏"
        );

        // 日志攒一小批再推给界面
        let pending: Arc<Mutex<Vec<LogLine>>> = Arc::default();
        let flusher = {
            let pending = pending.clone();
            let app = app.clone();
            let instance_id = instance_id.clone();
            let recorder = recorder.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(LOG_FLUSH).await;
                    flush_logs(&app, &instance_id, &pending, &recorder);
                }
            })
        };

        // 这次有没有进过世界（「永不玩坏」：进过世界并且正常退出，才算这个状态能玩）
        let reached_world = std::sync::atomic::AtomicBool::new(false);
        // 这次从什么时候开始玩（自动备份按它找这次玩过的世界）
        let started_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        // 自动封面：进程起来后开始跟着游戏状态截图，游戏结束时发送端一丢就停
        let (cover_state, cover_watch) = tokio::sync::watch::channel(GameState::Starting);
        let mut cover_watch = Some(cover_watch);
        let mut game_pid: Option<u32> = None;
        let mut was_playable = false;
        let mut maximized_once = false;
        let should_maximize = resolved.window.mode == launcher_core::config::WindowMode::Maximized;
        let mut release_id = None;
        let mut minimized_by_launch = false;
        // Discord 活动状态（默认关闭，纯本地管道，未运行 Discord 时静默跳过）
        let discord_task = if discord_enabled {
            let name = hook_ctx.instance_name.clone();
            let ver = hook_ctx.game_version.clone();
            let loader = hook_ctx.loader.clone();
            Some(tauri::async_runtime::spawn_blocking(move || {
                launcher_core::launch::discord::DiscordSession::connect(
                    &name,
                    &ver,
                    loader.as_deref(),
                )
            }))
        } else {
            None
        };
        let result = run_game(&prepared, &stop, |event| match event {
            GameEvent::Spawned(pid) => {
                game_pid = Some(pid);
                if let Some(watch) = cover_watch.take() {
                    let app = app.clone();
                    tauri::async_runtime::spawn(autocover::run(
                        prepared.game_dir().to_path_buf(),
                        pid,
                        watch,
                        move || app.state::<AppState>().settings.get().appearance.auto_cover,
                    ));
                }
            }
            GameEvent::Log(line) => pending.lock().unwrap_or_else(|e| e.into_inner()).push(line),
            GameEvent::State(game_state) => {
                cover_state.send_replace(game_state.clone());
                // 自定义游戏窗口标题：游戏窗口出现与进入主菜单时更新一次标题
                if let (Some(pid), Some(title)) = (game_pid, custom_title.as_ref())
                    && matches!(
                        game_state,
                        GameState::Loading | GameState::MainMenu | GameState::InWorld
                    )
                {
                    let title = title.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = winmem::set_process_window_title(pid, &title);
                    });
                }
                // 启动时最大化游戏窗口：窗口就绪后仅执行一次，绝不干扰玩家后续手动调整
                if should_maximize
                    && !maximized_once
                    && let Some(pid) = game_pid
                    && matches!(
                        game_state,
                        GameState::Loading | GameState::MainMenu | GameState::InWorld
                    )
                {
                    maximized_once = true;
                    tauri::async_runtime::spawn_blocking(move || {
                        let _ = winmem::maximize_process_window(pid);
                    });
                }
                if matches!(game_state, GameState::MainMenu | GameState::InWorld)
                    && !std::mem::replace(&mut was_playable, true)
                {
                    match state.settings.get().game.after_launch {
                        launcher_core::config::AfterLaunchBehavior::Keep => {}
                        launcher_core::config::AfterLaunchBehavior::Minimize => {
                            minimized_by_launch = true;
                            crate::window::minimize_main(&app);
                        }
                        launcher_core::config::AfterLaunchBehavior::Release => {
                            let id = crate::window::next_release_id();
                            release_id = Some(id);
                            crate::window::release_main(&app, id);
                        }
                    }
                }
                if game_state == GameState::InWorld {
                    reached_world.store(true, std::sync::atomic::Ordering::Relaxed);
                    records::with(&recorder, |w| w.mark_reached_world());
                }
                if let Some(snapshot) = state.sessions.set_state(&instance_id, game_state) {
                    emit(&app, SessionUpdated(snapshot));
                }
            }
        })
        .await;
        if let Some(task) = discord_task
            && let Ok(Some(session)) = task.await
        {
            let _ = tauri::async_runtime::spawn_blocking(move || session.clear()).await;
        }
        drop(cover_state);
        flusher.abort();
        flush_logs(&app, &instance_id, &pending, &recorder);
        let reached = reached_world.load(std::sync::atomic::Ordering::Relaxed);
        if let Err(error) = &result {
            let code = match error {
                launcher_core::launch::LaunchError::Authorization(_) => "auth.account_changed",
                launcher_core::launch::LaunchError::Parameters(_) => "launch.auth_parameters",
                _ => "launch.failed",
            };
            records::with(&recorder, |writer| writer.set_failure_code(code));
        }
        records::finish(
            &app,
            &recorder,
            &instance_id,
            record_end(&result, stop.is_cancelled(), reached),
            Some(&prepared),
        );
        if let Some(id) = release_id {
            crate::window::game_finished(&app, id);
        } else if minimized_by_launch || state.settings.get().game.refocus_on_exit {
            crate::window::open_main(&app);
        }
        // 退出后钩子：异步运行，失败仅记日志不阻塞退出收尾
        if !resolved.hooks.post_exit.is_empty() {
            let cmd = resolved.hooks.post_exit.clone();
            let ctx = hook_ctx.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = launcher_core::launch::hooks::run_hook(&cmd, &ctx).await {
                    tracing::warn!(%error, "退出后钩子执行失败");
                }
            });
        }
        match result {
            // 正常退出：把该实例改动的按键/设置与服务器增量回写共享库（即使只在主菜单改过设置也回写）
            Ok(GameState::Exited { code: Some(0) }) => {
                crate::sync::after_exit(&state, &instance_id).await;
                if reached_world.load(std::sync::atomic::Ordering::Relaxed) {
                    // 进过世界、正常退出：把现在的模组等内容记成「上次能玩的状态」，以后改坏了能一键回来
                    match content_of(&state, &instance_id) {
                        Ok(content) => {
                            if let Err(error) = content.mark_good().await {
                                tracing::warn!(%error, "记下能玩的状态失败");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error.message, "记下能玩的状态失败")
                        }
                    }
                    // 自动备份这次玩过的世界（设置里开着时）
                    crate::backups::after_game(&app, &instance_id, started_ms);
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "启动未完成");
                let error = AppError::from(error);
                notify_auth(&error);
                if let Some(snapshot) = state.sessions.fail(&instance_id, error.message) {
                    emit(&app, SessionUpdated(snapshot));
                }
            }
        }
    });
    Ok(())
}

/// 游戏结束时这一次记录的结局
fn record_end(
    result: &Result<GameState, launcher_core::launch::LaunchError>,
    stopped: bool,
    reached_world: bool,
) -> RecordEnd {
    let (outcome, exit_code, crash_report, error) = match result {
        Ok(GameState::Crashed { code, crash_report }) => {
            (LaunchOutcome::Crashed, *code, crash_report.clone(), None)
        }
        Ok(GameState::Exited { code }) if stopped => (LaunchOutcome::Stopped, *code, None, None),
        Ok(GameState::Exited { code }) => (LaunchOutcome::Exited, *code, None, None),
        Ok(_) => (LaunchOutcome::Exited, None, None, None),
        Err(error) => (LaunchOutcome::Failed, None, None, Some(error.to_string())),
    };
    RecordEnd {
        ended_at: 0.0,
        outcome,
        exit_code,
        reached_world,
        error,
        crash_report,
    }
}

fn flush_logs(
    app: &AppHandle,
    instance_id: &str,
    pending: &Mutex<Vec<LogLine>>,
    recorder: &Recorder,
) {
    let lines = std::mem::take(&mut *pending.lock().unwrap_or_else(|e| e.into_inner()));
    if lines.is_empty() {
        return;
    }
    records::append(recorder, &lines);
    app.state::<AppState>()
        .sessions
        .push_logs(instance_id, &lines);
    emit(
        app,
        GameLogBatch {
            instance_id: instance_id.to_owned(),
            lines,
        },
    );
}

/// 启动前极速本地预检：只读本地元信息，拦截缺前置、重复 modId、加载器错配和手动 Java 过旧。
#[tauri::command]
#[specta::specta]
pub async fn launch_preflight(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<launcher_core::launch::preflight::PreflightReport, AppError> {
    let game = state.game();
    let instance = load_instance(&state, &instance_id)?;
    let settings = state.settings.get();
    let resolved = instance::resolve(&settings.game, &instance.config.overrides);
    let pinned_java_id = resolved.java_id.clone();
    let locator = state.java_locator();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let selected_major = pinned_java_id.as_deref().and_then(|id| {
            locator
                .scan()
                .into_iter()
                .find(|install| install.id() == id)
                .map(|install| install.major)
        });
        launcher_core::launch::preflight::check(&game, &instance, selected_major)
    })
    .await
    .map_err(|e| AppError::new("launch.failed", "启动前检查失败").with_detail(e))?;
    if !report.issues.is_empty() {
        tracing::info!(
            instance = %instance_id,
            checked_mods = report.checked_mods,
            issues = report.issues.len(),
            elapsed_ms = report.elapsed_ms,
            "启动前预检发现风险项"
        );
    }
    Ok(report)
}

/// 结束游戏（强制）。返回游戏是否在运行。
#[tauri::command]
#[specta::specta]
pub fn launch_stop(state: State<'_, AppState>, instance_id: String) -> bool {
    state.sessions.stop(&instance_id)
}

#[tauri::command]
#[specta::specta]
pub fn launch_sessions(state: State<'_, AppState>) -> Vec<SessionSnapshot> {
    state.sessions.list()
}

/// 某个实例最近一次运行的日志（最多 5000 行）
#[tauri::command]
#[specta::specta]
pub fn launch_log(state: State<'_, AppState>, instance_id: String) -> Vec<LogLine> {
    state.sessions.log(&instance_id)
}

// ---------- 资源（Modrinth）----------

/// 搜索。给了实例时按实例的游戏版本和加载器筛选。
/// `category` 是 Modrinth 的分类名（例如 `technology`），只看这一类。
#[allow(clippy::too_many_arguments)] // 参数一一对应界面的调用
#[tauri::command]
#[specta::specta]
pub async fn modrinth_search(
    state: State<'_, AppState>,
    query: String,
    kind: ContentKind,
    instance_id: Option<String>,
    game_version: Option<String>,
    loader: Option<LoaderKind>,
    client_only: bool,
    sort: SearchSort,
    category: Option<String>,
    offset: u32,
) -> Result<SearchPage, AppError> {
    let instance = instance_id
        .map(|id| load_instance(&state, &id))
        .transpose()?;
    let resolved_gv = game_version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| instance.as_ref().map(|i| i.config.game_version.as_str()));
    let resolved_loader = loader.map(loader_slug).or_else(|| {
        instance
            .as_ref()
            .and_then(|i| i.config.loader.as_ref())
            .map(|l| loader_slug(l.kind))
    });
    let client_only = client_only && kind == ContentKind::Mod;
    let query = SearchQuery {
        text: &query,
        kind,
        game_version: resolved_gv,
        loader: resolved_loader,
        sort,
        category: category.as_deref(),
        client_only,
        offset,
        limit: SEARCH_PAGE,
    };
    let key = SearchKey {
        source: "modrinth",
        kind: &format!("{kind:?}"),
        text: query.text,
        game_version: query.game_version,
        loader: query.loader,
        client_only,
        sort,
        category: query.category,
        offset,
    }
    .build();
    cached_search(&state, key, modrinth::search(&state.http_client(), &query)).await
}

/// 搜索缓存的键：同样的条件才算同一次搜索
pub(crate) struct SearchKey<'a> {
    pub source: &'a str,
    pub kind: &'a str,
    pub text: &'a str,
    pub game_version: Option<&'a str>,
    pub loader: Option<&'a str>,
    pub client_only: bool,
    pub sort: SearchSort,
    pub category: Option<&'a str>,
    pub offset: u32,
}

impl SearchKey<'_> {
    /// 平台、类型、版本、加载器、仅客户端、排序、分类、第几页、关键词（关键词放最后，里面可能有分隔符）
    pub(crate) fn build(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.source,
            self.kind,
            self.game_version.unwrap_or(""),
            self.loader.unwrap_or(""),
            u8::from(self.client_only),
            self.sort.key(),
            self.category.unwrap_or(""),
            self.offset,
            self.text.trim().to_lowercase()
        )
    }
}

/// CurseForge 的分类编号（界面传字符串，和 Modrinth 的分类共用一个参数）
pub(crate) fn curseforge_category(category: Option<&str>) -> Result<Option<u32>, AppError> {
    category
        .map(|c| {
            c.parse()
                .map_err(|_| AppError::new("curseforge.bad_category", "CurseForge 分类编号不对"))
        })
        .transpose()
}

/// 先查缓存，没有再搜，搜到的放进缓存（失败的不放）
pub(crate) async fn cached_search(
    state: &AppState,
    key: String,
    search: impl std::future::Future<Output = Result<SearchPage, launcher_core::content::ContentError>>,
) -> Result<SearchPage, AppError> {
    let mut page = match state.search_cache.get(&key) {
        Some(page) => page,
        None => {
            let page = search.await?;
            state.search_cache.insert(key, page.clone());
            page
        }
    };
    // 中文名、另一个平台的编号每次现补（装了新模组、名表更新都能马上用上）
    state.learned_names().annotate(&mut page.hits);
    Ok(page)
}

fn loader_slug(kind: LoaderKind) -> &'static str {
    kind.slug()
}

/// 加载器在 CurseForge 上的编号
fn curseforge_loader(kind: LoaderKind) -> Option<u32> {
    match kind {
        LoaderKind::NeoForge => Some(cf::LOADER_NEOFORGE),
        LoaderKind::Forge => Some(cf::LOADER_FORGE),
        LoaderKind::Fabric => Some(cf::LOADER_FABRIC),
        LoaderKind::Quilt => Some(cf::LOADER_QUILT),
        LoaderKind::LegacyFabric | LoaderKind::Babric => None,
    }
}

/// 搜 CurseForge。给了实例时按实例的游戏版本和加载器筛选。
/// 启动器的 Key 没有搜索权限时返回 `curseforge.denied`（界面据此提示「CurseForge 搜索还没开通」）。
/// `category` 是 CurseForge 的分类编号。
#[allow(clippy::too_many_arguments)] // 参数一一对应界面的调用
#[tauri::command]
#[specta::specta]
pub async fn curseforge_search(
    state: State<'_, AppState>,
    query: String,
    kind: ContentKind,
    instance_id: Option<String>,
    game_version: Option<String>,
    loader: Option<LoaderKind>,
    sort: SearchSort,
    category: Option<String>,
    offset: u32,
) -> Result<SearchPage, AppError> {
    let instance = instance_id
        .map(|id| load_instance(&state, &id))
        .transpose()?;
    let resolved_gv = game_version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| instance.as_ref().map(|i| i.config.game_version.as_str()));
    let resolved_loader = loader
        .and_then(curseforge_loader)
        .or_else(|| {
            instance
                .as_ref()
                .and_then(|i| i.config.loader.as_ref())
                .and_then(|l| curseforge_loader(l.kind))
        })
        .filter(|_| kind == ContentKind::Mod);
    let query = cf::SearchQuery {
        text: &query,
        class_id: cf::class_for(kind),
        game_version: resolved_gv,
        loader: resolved_loader,
        sort,
        category: curseforge_category(category.as_deref())?,
        offset,
        limit: SEARCH_PAGE,
    };
    let loader_str = resolved_loader.map(|l| l.to_string());
    let key = SearchKey {
        source: "curseforge",
        kind: &format!("{kind:?}"),
        text: query.text,
        game_version: query.game_version,
        loader: loader_str.as_deref(),
        client_only: false,
        sort,
        category: category.as_deref(),
        offset,
    }
    .build();
    cached_search(&state, key, state.curseforge.search(&query)).await
}

pub(crate) fn curseforge_id(id: &str) -> Result<u32, AppError> {
    id.parse()
        .map_err(|_| AppError::new("curseforge.bad_id", "CurseForge 项目编号不对"))
}

/// 在资源管理器里选中这个文件（停用的模组也找得到）。
#[tauri::command]
#[specta::specta]
pub fn content_reveal(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
    file_name: String,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let path = content_of(&state, &instance_id)?.locate(kind, &file_name)?;
    app.opener().reveal_item_in_dir(&path).map_err(|e| {
        AppError::new("content.reveal_failed", "打不开文件所在的文件夹").with_detail(e)
    })
}

/// 一次删好几个：只记一条改动历史，一次撤销全部回来。返回删掉了几个。
#[tauri::command]
#[specta::specta]
pub async fn content_remove_many(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
    file_names: Vec<String>,
) -> Result<u32, AppError> {
    ensure_not_running(&state, &instance_id)?;
    let _turn = take_content_turn(&state, &instance_id)?;
    let content = content_of(&state, &instance_id)?;
    let count = content.remove_many(kind, &file_names).await?;
    tracing::info!(instance = %instance_id, count, "批量删除");
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// 解析安装指定版本的计划（含前置、光影加载器）。
#[tauri::command]
#[specta::specta]
pub async fn content_resolve_version(
    state: State<'_, AppState>,
    instance_id: String,
    project_id: String,
    version_id: String,
    kind: ContentKind,
    source: ContentSource,
) -> Result<Vec<PlanItem>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let game = state.game();
    let content = InstanceContent::for_game(&game, instance.game_dir(&game)?);
    if source == ContentSource::CurseForge {
        let mod_id = curseforge_id(&project_id)?;
        let file_id = curseforge_id(&version_id)?;
        let installed = content.installed_curseforge(&state.curseforge).await?;
        let target = cf::Target {
            game_version: &instance.config.game_version,
            loader: instance
                .config
                .loader
                .as_ref()
                .and_then(|l| curseforge_loader(l.kind)),
            installed: &installed,
        };
        return Ok(state
            .curseforge
            .resolve_version(&state.http_client(), &target, mod_id, file_id, kind)
            .await?);
    }
    let installed = content.installed_projects(&state.http_client()).await?;
    let target = Target {
        game_version: &instance.config.game_version,
        loader: instance.config.loader.as_ref().map(|l| loader_slug(l.kind)),
        installed: &installed,
    };
    Ok(modrinth::resolve_version(
        &state.http_client(),
        &target,
        &project_id,
        &version_id,
        kind,
    )
    .await?)
}

/// 锁定或取消锁定实例内某项内容的版本。
#[tauri::command]
#[specta::specta]
pub fn content_set_locked(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
    file_name: String,
    locked: bool,
) -> Result<bool, AppError> {
    let content = content_of(&state, &instance_id)?;
    let next = content.set_locked(kind, &file_name, locked)?;
    tracing::info!(instance = %instance_id, file = %file_name, locked = next, "切换内容版本锁定");
    Ok(next)
}

/// 解析安装计划（含前置、光影加载器），给用户确认。
#[tauri::command]
#[specta::specta]
pub async fn content_resolve(
    state: State<'_, AppState>,
    instance_id: String,
    project_id: String,
    kind: ContentKind,
    source: ContentSource,
) -> Result<Vec<PlanItem>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let game = state.game();
    let content = InstanceContent::for_game(&game, instance.game_dir(&game)?);
    if source == ContentSource::CurseForge {
        let mod_id = curseforge_id(&project_id)?;
        let installed = content.installed_curseforge(&state.curseforge).await?;
        let target = cf::Target {
            game_version: &instance.config.game_version,
            loader: instance
                .config
                .loader
                .as_ref()
                .and_then(|l| curseforge_loader(l.kind)),
            installed: &installed,
        };
        return Ok(state
            .curseforge
            .resolve(&state.http_client(), &target, mod_id, kind)
            .await?);
    }
    let installed = content.installed_projects(&state.http_client()).await?;
    let target = Target {
        game_version: &instance.config.game_version,
        loader: instance.config.loader.as_ref().map(|l| loader_slug(l.kind)),
        installed: &installed,
    };
    Ok(modrinth::resolve(&state.http_client(), &target, &project_id, kind).await?)
}

/// 是否有后台安装 / 更新 / 拖入 / 备份任务正在操作此游戏目录。
fn has_running_instance_job(
    state: &AppState,
    instance_id: &str,
    directory: &std::path::Path,
) -> bool {
    let prefixes = [
        format!("content:{instance_id}:"),
        format!("drop:{instance_id}:"),
        format!("map-install:{instance_id}:"),
        // 正在备份、恢复这个实例的世界：游戏一开就会写存档
        format!("backup:{instance_id}:"),
    ];
    let directory = crate::turns::directory_key(directory);
    state.jobs.list().into_iter().any(|job| {
        job.state == JobState::Running
            && (prefixes.iter().any(|p| job.key.starts_with(p))
                || (["backup:", "map-install:", "drop:", "content:"]
                    .iter()
                    .any(|p| job.key.starts_with(p))
                    && job
                        .instance_id
                        .as_deref()
                        .and_then(|id| game_dir_of(state, id).ok())
                        .is_some_and(|dir| crate::turns::directory_key(&dir) == directory)))
    })
}

/// 这个实例上有没有正在装的东西：正在改文件的（排到了的），加上还在下载、排队的安装和更新、
/// 拖进来正在装的、正在装的地图（下载期间不占实例的锁，但装完要放进实例，这时启动游戏会冲突）
fn instance_working(state: &AppState, instance_id: &str, directory: &std::path::Path) -> bool {
    state.turns.is_busy(directory) || has_running_instance_job(state, instance_id, directory)
}

/// 安装用户确认过的计划（后台任务），返回任务 ID。
#[tauri::command]
#[specta::specta]
pub fn content_install(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    plan: Vec<PlanItem>,
) -> Result<String, AppError> {
    ensure_not_running(&state, &instance_id)?;
    let content = content_of(&state, &instance_id)?;
    let directory = game_dir_of(&state, &instance_id)?;
    let title = match plan.first() {
        Some(first) if plan.len() > 1 => format!("安装 {} 等 {} 项", first.title, plan.len()),
        Some(first) => format!("安装 {}", first.title),
        None => return Err(AppError::new("content.empty_plan", "没有要安装的内容")),
    };
    let key = format!("content:{instance_id}:{}", plan[0].project_id);
    // 同一个东西已经在装了（连点了两下）：不报错，就是那个任务
    if let Some(running) = state
        .jobs
        .list()
        .into_iter()
        .find(|j| j.key == key && j.state == JobState::Running)
    {
        tracing::info!(job = %running.id, key, "已经在装了，不重复开任务");
        return Ok(running.id);
    }
    let retry = Retry::ContentInstall {
        instance_id: instance_id.clone(),
        plan: plan.clone(),
    };
    let job = state.jobs.start_with(
        key,
        title.clone(),
        ProgressUnit::Bytes,
        downloads::meta(
            downloads::content_source(plan[0].source),
            Some(&instance_id),
            plan[0].icon_url.clone(),
            Some(&retry),
        ),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let downloader = downloader_for_job(&state, DownloadPriority::Interactive, &job);
        let cancel = job.cancel_token();
        let result: Result<(), AppError> = async {
            let prepared = content
                .prepare_install(&state.http_client(), &state.curseforge, &plan)
                .await
                .map_err(|e| state.capture_app_error(&job, "下载", e))?;
            // 作者禁止第三方下载、Modrinth 上也没有同样文件的：开始补件，一边下载其他文件一边等
            let claim =
                crate::claims::start(&app, &state, &job, title.clone(), prepared.manual.clone());
            // 下载几个安装可以一起（各下到自己的临时文件夹）
            let staged = content
                .download_install(
                    &downloader,
                    prepared,
                    claim.as_ref().map(crate::claims::ClaimGuard::session),
                    &cancel,
                    |p| report(&job, "下载", &p),
                )
                .await
                .map_err(|e| state.capture_app_error(&job, "下载", e))?;
            drop(claim);
            // 放进实例要排队：同一个实例同一时间只有一个在改文件和来源记录
            let Some(_turn) = state.turns.wait(&directory, &job).await else {
                return Err(AppError::new("job.cancelled", "已取消"));
            };
            ensure_not_running(&state, &instance_id)?;
            job.set_message("放进实例");
            content.finish_install(staged).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => job.succeed(),
            Err(_) if job.is_cancelled() => job.cancelled(),
            Err(error) => state.finish_failed_job(job, error.message),
        }
    });
    Ok(job_id)
}

#[tauri::command]
#[specta::specta]
pub fn content_list(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
) -> Result<Vec<ContentItem>, AppError> {
    Ok(content_of(&state, &instance_id)?.list(kind)?)
}

#[tauri::command]
#[specta::specta]
pub async fn content_set_enabled(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
    file_name: String,
    enabled: bool,
) -> Result<(), AppError> {
    ensure_not_running(&state, &instance_id)?;
    let dir = game_dir_of(&state, &instance_id)?;
    // 真正有安装/更新/备份等后台任务在跑时直接拦住；
    // 若只是玩家连续快点多个开关导致上一个瞬时改名还没释放锁，则安静排队顺序执行，不误报「正在装东西」。
    let _turn = match state.turns.try_take(&dir) {
        Some(turn) => turn,
        None if has_running_instance_job(&state, &instance_id, &dir) => return Err(busy_error()),
        None => {
            let turn = state.turns.take(&dir).await;
            ensure_not_running(&state, &instance_id)?;
            turn
        }
    };
    let content = content_of(&state, &instance_id)?;
    // 上次改到一半的先退回
    content.recover().await?;
    // 启用 / 停用也是改动：还没有快照的话先记一份，改完后再写入时间线节点
    content.snapshot_before_change().await;
    let tl_guard = content.begin_timeline_change().await;
    content.set_enabled(kind, &file_name, enabled)?;
    let verb = if enabled { "启用" } else { "停用" };
    let stem = file_name
        .strip_suffix(".jar")
        .or_else(|| file_name.strip_suffix(".zip"))
        .unwrap_or(&file_name);
    content
        .finish_timeline_toggle(
            tl_guard,
            kind,
            &file_name,
            enabled,
            format!("{verb} {stem}"),
        )
        .await;
    Ok(())
}

/// 一次启用 / 停用好几个：要么全改、要么一个不改。返回真的改了的那些，
/// 界面上的「撤销」就是拿它反过来再调一次。
#[tauri::command]
#[specta::specta]
pub async fn content_set_many_enabled(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
    file_names: Vec<String>,
    enabled: bool,
) -> Result<Vec<String>, AppError> {
    ensure_not_running(&state, &instance_id)?;
    let _turn = take_content_turn(&state, &instance_id)?;
    let changed = content_of(&state, &instance_id)?
        .set_many_enabled(kind, &file_names, enabled)
        .await?;
    tracing::info!(instance = %instance_id, count = changed.len(), enabled, "批量启用 / 停用");
    Ok(changed)
}

/// 在启动持有目录锁时，先收尾上次中断的世界恢复及模组改动。
async fn settle_before_launch(
    game: &GameDir,
    instance: &Instance,
    directory: PathBuf,
) -> Result<(), AppError> {
    let content = InstanceContent::for_game(game, &directory);
    crate::backups::settle(directory).await?;
    if !content.has_pending() {
        return Ok(());
    }
    if let Some(recovered) = content.recover().await? {
        tracing::warn!(instance = %instance.id, summary = %recovered.summary, "启动前退回了上次没做完的改动");
    }
    Ok(())
}

/// 这个实例的游戏开着吗（准备启动、加载、在世界里都算）
pub(crate) fn instance_running(state: &AppState, instance_id: &str) -> bool {
    game_dir_of(state, instance_id).is_ok_and(|path| {
        state
            .sessions
            .running_in(&crate::turns::directory_key(&path))
    })
}

fn take_content_turn(state: &AppState, instance_id: &str) -> Result<crate::turns::Turn, AppError> {
    state
        .turns
        .try_take(&game_dir_of(state, instance_id)?)
        .ok_or_else(busy_error)
}

/// 游戏开着时不能动它的文件（游戏锁着文件，而且改了也不会生效）
fn ensure_not_running(state: &AppState, instance_id: &str) -> Result<(), AppError> {
    if instance_running(state, instance_id) {
        return Err(AppError::new(
            "content.game_running",
            "游戏正在运行，先退出游戏再改模组（游戏开着时改了也不会生效）",
        ));
    }
    Ok(())
}

/// 认出还不认识的文件（联网问 Modrinth，问不到就读文件自带的说明），返回最新列表。
#[tauri::command]
#[specta::specta]
pub async fn content_refresh(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
) -> Result<Vec<ContentItem>, AppError> {
    Ok(content_of(&state, &instance_id)?
        .refresh(&state.http_client(), &state.curseforge, kind)
        .await?)
}

/// 检查这一类内容的更新（同时支持 Modrinth 与 CurseForge，跳过已锁定项），并报告实际检查了多少项。
#[tauri::command]
#[specta::specta]
pub async fn content_check_updates(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
) -> Result<UpdateCheckResult, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let content = content_of(&state, &instance_id)?;
    let loader = instance.config.loader.as_ref().map(|l| loader_slug(l.kind));
    Ok(content
        .check_updates(
            &state.http_client(),
            &state.curseforge,
            kind,
            &instance.config.game_version,
            loader,
        )
        .await?)
}

fn start_update_job(
    app: AppHandle,
    state: &AppState,
    instance_id: String,
    updates: Vec<UpdateInfo>,
    title: String,
    verb: &'static str,
) -> Result<String, AppError> {
    ensure_not_running(state, &instance_id)?;
    let content = content_of(state, &instance_id)?;
    let directory = game_dir_of(state, &instance_id)?;
    if updates.is_empty() {
        return Err(AppError::new("content.empty_plan", "没有要更新的内容"));
    }
    let retry = Retry::ContentUpdate {
        instance_id: instance_id.clone(),
        updates: updates.clone(),
    };
    let source = if updates
        .iter()
        .all(|u| u.source == ContentSource::CurseForge)
    {
        JobSource::CurseForge
    } else {
        JobSource::Modrinth
    };
    let job = state.jobs.start_with(
        format!("content:{instance_id}:update"),
        title.clone(),
        ProgressUnit::Bytes,
        downloads::meta(source, Some(&instance_id), None, Some(&retry)),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let downloader = downloader_for_job(&state, DownloadPriority::Interactive, &job);
        let cancel = job.cancel_token();
        let result: Result<(), AppError> = async {
            let prepared = content
                .prepare_update(&state.http_client(), &state.curseforge, &updates)
                .await
                .map_err(|e| state.capture_app_error(&job, "下载新版本", e))?;
            let claim =
                crate::claims::start(&app, &state, &job, title.clone(), prepared.manual.clone());
            let staged = content
                .download_update(
                    &downloader,
                    prepared,
                    claim.as_ref().map(crate::claims::ClaimGuard::session),
                    &cancel,
                    |p| report(&job, "下载新版本", &p),
                )
                .await
                .map_err(|e| state.capture_app_error(&job, "下载新版本", e))?;
            drop(claim);
            let Some(_turn) = state.turns.wait(&directory, &job).await else {
                return Err(AppError::new("job.cancelled", "已取消"));
            };
            ensure_not_running(&state, &instance_id)?;
            job.set_message("替换文件");
            content.finish_update(staged, verb).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => job.succeed(),
            Err(_) if job.is_cancelled() => job.cancelled(),
            Err(error) => state.finish_failed_job(job, error.message),
        }
    });
    Ok(job_id)
}

/// 更新（后台任务，支持 Modrinth 与 CurseForge 含受限补件），返回任务 ID。旧文件进内容仓库，可在「改动历史」里撤销。
#[tauri::command]
#[specta::specta]
pub fn content_update(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    updates: Vec<UpdateInfo>,
) -> Result<String, AppError> {
    let title = match updates.first() {
        Some(first) if updates.len() > 1 => format!("更新 {} 等 {} 项", first.title, updates.len()),
        Some(first) => format!("更新 {}", first.title),
        None => return Err(AppError::new("content.empty_plan", "没有要更新的内容")),
    };
    start_update_job(app, &state, instance_id, updates, title, "更新")
}

/// 切换某项内容到指定版本（后台任务，记入改动历史可撤销）。
#[tauri::command]
#[specta::specta]
pub fn content_switch_version(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    update: UpdateInfo,
) -> Result<String, AppError> {
    let title = format!("切换 {} 到 {}", update.title, update.version_number);
    start_update_job(app, &state, instance_id, vec![update], title, "切换版本")
}

/// 删除（文件进内容仓库，可以撤销）。
#[tauri::command]
#[specta::specta]
pub async fn content_remove(
    state: State<'_, AppState>,
    instance_id: String,
    kind: ContentKind,
    file_name: String,
) -> Result<(), AppError> {
    ensure_not_running(&state, &instance_id)?;
    let _turn = take_content_turn(&state, &instance_id)?;
    Ok(content_of(&state, &instance_id)?
        .remove(kind, &file_name)
        .await?)
}

/// 改动历史（安装、更新、删除），新的在前。
#[tauri::command]
#[specta::specta]
pub fn content_history(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Vec<HistoryView>, AppError> {
    Ok(content_of(&state, &instance_id)?.history())
}

/// 撤销一次改动。
#[tauri::command]
#[specta::specta]
pub async fn content_rollback(
    state: State<'_, AppState>,
    instance_id: String,
    id: String,
) -> Result<(), AppError> {
    ensure_not_running(&state, &instance_id)?;
    let _turn = take_content_turn(&state, &instance_id)?;
    Ok(content_of(&state, &instance_id)?.rollback(&id).await?)
}

/// 游戏崩溃后问一句：和「能玩的时候」比，改过什么？没改过（或者还没有记下能玩的状态）返回 `None`。
#[tauri::command]
#[specta::specta]
pub async fn content_crash_offer(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Option<RollbackOffer>, AppError> {
    Ok(content_of(&state, &instance_id)?.crash_offer().await?)
}

/// 一键回到「能玩的时候」（或者第一次改动之前）。返回动了几个文件；这次放回记在改动历史里，可以撤销。
#[tauri::command]
#[specta::specta]
pub async fn content_restore_snapshot(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<u32, AppError> {
    ensure_not_running(&state, &instance_id)?;
    let _turn = take_content_turn(&state, &instance_id)?;
    Ok(content_of(&state, &instance_id)?.restore_snapshot().await?)
}

/// 「永不玩坏」融合时间线（改动快照节点 + 历史启动/崩溃记录，按时间倒序排列）。
#[tauri::command]
#[specta::specta]
pub async fn content_timeline(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<TimelineView, AppError> {
    let running = state.sessions.running_record(&instance_id);
    Ok(content_of(&state, &instance_id)?
        .timeline(running.as_deref())
        .await?)
}

/// 一键恢复到时间线上的任意历史快照节点（恢复前自动为当前态留存保护节点，随时可前滚回来）。
#[tauri::command]
#[specta::specta]
pub async fn content_restore_timeline_node(
    state: State<'_, AppState>,
    instance_id: String,
    node_id: String,
) -> Result<u32, AppError> {
    ensure_not_running(&state, &instance_id)?;
    let _turn = take_content_turn(&state, &instance_id)?;
    Ok(content_of(&state, &instance_id)?
        .restore_timeline_node(&node_id)
        .await?)
}

/// 离线读取实例的模组正/反向依赖图（带缓存，供内容列表展示「被 XX 依赖」与「孤儿前置」标记）。
#[tauri::command]
#[specta::specta]
pub async fn content_mod_deps(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<ModDepsGraph, AppError> {
    let content = content_of(&state, &instance_id)?;
    tauri::async_runtime::spawn_blocking(move || content.mod_deps())
        .await
        .map_err(|e| AppError::new("content.deps_failed", "分析模组依赖失败").with_detail(e))?
        .map_err(AppError::from)
}

/// 停用或卸载指定模组前的依赖影响分析（被依赖预警 + 卸载后将失去依赖方的孤儿前置列表）。
#[tauri::command]
#[specta::specta]
pub async fn content_mod_impact(
    state: State<'_, AppState>,
    instance_id: String,
    file_names: Vec<String>,
    action: ImpactAction,
) -> Result<ModImpactReport, AppError> {
    let content = content_of(&state, &instance_id)?;
    tauri::async_runtime::spawn_blocking(move || content.analyze_mod_impact(&file_names, action))
        .await
        .map_err(|e| AppError::new("content.impact_failed", "分析模组影响失败").with_detail(e))?
        .map_err(AppError::from)
}

// ---------- 游戏目录 ----------

/// 没设置时的默认游戏目录：应用数据目录下的 `minecraft`
pub fn default_game_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("minecraft")
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameDirInfo {
    /// 只用来显示；界面不会把它传回来
    pub path: String,
    /// 由开发用的环境变量指定，界面里不能改
    pub locked: bool,
}

fn game_dir_info_of(state: &AppState) -> GameDirInfo {
    GameDirInfo {
        path: state.game().root().display().to_string(),
        locked: state.game_dir_from_env,
    }
}

#[tauri::command]
#[specta::specta]
pub fn game_dir_info(state: State<'_, AppState>) -> GameDirInfo {
    game_dir_info_of(&state)
}

pub(crate) fn any_game_running(state: &AppState) -> bool {
    state.sessions.list().iter().any(|s| {
        !matches!(
            s.state,
            GameState::Exited { .. } | GameState::Crashed { .. }
        )
    })
}

/// 弹出系统的「选择文件夹」窗口换游戏目录。用户取消时返回 `None`。
#[tauri::command]
#[specta::specta]
pub async fn game_dir_choose(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<GameDirInfo>, AppError> {
    if state.game_dir_from_env {
        return Err(AppError::new(
            "game_dir.locked",
            "开发模式下游戏目录由环境变量 MCL_GAME_DIR 指定，不能在这里改",
        ));
    }
    if any_game_running(&state) {
        return Err(AppError::new(
            "game_dir.busy",
            "游戏正在运行，请先退出游戏再换目录",
        ));
    }
    let Some(path) = pick(&app, Pick::Folder).await? else {
        return Ok(None);
    };
    let _files_turn = state
        .turns
        .try_take(&repair_key(&state.game()))
        .ok_or_else(repair_busy)?;
    // 先确认这个文件夹能写，免得之后下载到一半才失败
    let probe = path.join(".launcher-write-test");
    std::fs::create_dir_all(&path)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| {
            AppError::new(
                "game_dir.not_writable",
                "这个文件夹不能写入，请换一个（比如 D 盘下新建一个）",
            )
            .with_detail(e)
        })?;
    let mut settings = state.settings.get();
    settings.game.directory = Some(path.display().to_string());
    state.settings.set(settings)?;
    let game = GameDir::new(&path);
    migrate_instances(&game);
    state.set_game(game);
    tracing::info!(dir = %path.display(), "游戏目录已更换");
    // 挂上的别家目录同步进新的游戏目录
    crate::linked::spawn_rescan(&app);
    Ok(Some(game_dir_info_of(&state)))
}

pub(crate) enum Pick {
    Folder,
    Modpack,
    /// 世界的背景图
    Image,
    /// 手动添加 Java
    Java,
    /// 挂上别家的 .minecraft
    GameRoot,
    /// 从别家启动器导入实例
    ForeignInstance,
}

/// 在后端弹出系统选择窗口（界面拿不到路径，只拿结果）
pub(crate) async fn pick(
    app: &AppHandle,
    kind: Pick,
) -> Result<Option<std::path::PathBuf>, AppError> {
    use tauri_plugin_dialog::DialogExt;
    let app = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app.dialog().file();
        if let Some(window) = app.get_webview_window(crate::window::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        match kind {
            Pick::Folder => dialog
                .set_title("选择游戏目录（放版本、模组、存档的地方）")
                .blocking_pick_folder(),
            Pick::Modpack => dialog
                .set_title("选择要导入的整合包")
                .add_filter(
                    "整合包（Modrinth .mrpack、CurseForge .zip）",
                    &["mrpack", "zip"],
                )
                .blocking_pick_file(),
            Pick::Image => dialog
                .set_title("选一张图当这个世界的背景")
                .add_filter("图片（PNG、JPG）", &["png", "jpg", "jpeg"])
                .blocking_pick_file(),
            Pick::Java => dialog
                .set_title("选择 Java（Java 文件夹里 bin 下面的 java.exe）")
                .add_filter("Java 程序（java.exe）", &["exe"])
                .blocking_pick_file(),
            Pick::GameRoot => dialog
                .set_title("选择别的启动器的 .minecraft 文件夹")
                .blocking_pick_folder(),
            Pick::ForeignInstance => dialog
                .set_title("选择别的启动器的实例文件夹（或者放实例的文件夹）")
                .blocking_pick_folder(),
        }
    })
    .await
    .map_err(|e| AppError::new("dialog.failed", "打开选择窗口失败").with_detail(e))?;
    picked
        .map(|p| p.into_path())
        .transpose()
        .map_err(|e| AppError::new("dialog.failed", "选中的路径无法使用").with_detail(e))
}

// ---------- 整合包：本地导入、在线安装 ----------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PackChoice {
    /// 一次性的令牌，确认导入时传回来
    pub token: String,
    pub preview: PackPreview,
    /// 还缺什么（游戏本体、加载器、Java）
    pub readiness: PackReadiness,
}

/// 扫描能用的 Java（只读文件，放到后台线程）
async fn scan_java(state: &AppState) -> Vec<launcher_core::java::JavaInstall> {
    let locator = state.java_locator();
    match tauri::async_runtime::spawn_blocking(move || locator.scan()).await {
        Ok(javas) => javas,
        Err(error) => {
            tracing::warn!(%error, "扫描 Java 的任务异常结束");
            Vec::new()
        }
    }
}

/// 在后台线程里跑读压缩包的活
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, launcher_core::modpack::ModpackError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::new("modpack.failed", "读取整合包失败").with_detail(e))?
        .map_err(AppError::from)
}

/// 读取并检查整合包：先认格式；CurseForge 包还要联网查清每个文件（要 API Key）
pub(crate) async fn inspect_file(state: &AppState, path: PathBuf) -> Result<PackPlan, AppError> {
    let probe = path.clone();
    match blocking(move || modpack::detect(&probe)).await? {
        PackFormat::Modrinth => blocking(move || mrpack::inspect(&path)).await,
        PackFormat::CurseForge => {
            let pack = blocking(move || curseforge::inspect(&path)).await?;
            Ok(curseforge::resolve(&state.http_client(), &state.curseforge, pack).await?)
        }
        PackFormat::MultiMc => blocking(move || multimc::inspect(&path)).await,
    }
}

/// 早先的实例（`versions/<实例名>/`）搬到 `instances/<实例名>/`（国际主流的实例布局）
pub(crate) fn migrate_instances(game: &GameDir) {
    let report = instance::migrate_legacy(game);
    if report.moved > 0 || report.renamed > 0 || report.failed > 0 {
        tracing::info!(
            moved = report.moved,
            renamed = report.renamed,
            failed = report.failed,
            dir = %game.root().display(),
            "实例搬家（老实例搬到 instances/，定名前的设置文件夹改名）"
        );
    }
}

/// 以前在线安装时下载的整合包放这里；现在放进每次导入自己的临时目录
fn legacy_packs_dir(game: &GameDir) -> PathBuf {
    game.root().join(".mcl-tmp").join("packs")
}

/// 启动器启动时清掉上次没删的临时文件（比如安装途中启动器被关掉）：以前的临时整合包、复制到一半的实例。
/// 导入到一半的整合包还能重试的留着（`keep`），见 [`crate::downloads::clean_imports`]。
/// 只在启动时清：那时没有任何安装在进行，不会误删正在用的东西。
pub(crate) fn clean_temp_packs(game: &GameDir, keep: &std::collections::HashSet<String>) {
    crate::downloads::clean_imports(game, keep, false);
    let temp = game.root().join(".mcl-tmp");
    let mut leftovers = vec![legacy_packs_dir(game)];
    if let Ok(entries) = std::fs::read_dir(&temp) {
        leftovers.extend(
            entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with(instance::COPY_TEMP_PREFIX) || name.starts_with("pack-update-")
                })
                .map(|e| e.path()),
        );
    }
    // 上次没装完的安装下载的文件
    leftovers.push(
        game.root()
            .join(launcher_core::content::store::STORE_DIR)
            .join(launcher_core::content::STAGING_DIR),
    );
    for dir in leftovers {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => tracing::info!(dir = %dir.display(), "已清理上次留下的临时文件"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, dir = %dir.display(), "清理临时文件失败"),
        }
    }
}

/// 弹出选择文件窗口，读取并检查整合包，返回预览给用户确认。用户取消时返回 `None`。
#[tauri::command]
#[specta::specta]
pub async fn modpack_pick(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<PackChoice>, AppError> {
    let Some(path) = pick(&app, Pick::Modpack).await? else {
        return Ok(None);
    };
    Ok(Some(pack_choice(&state, path).await?))
}

/// 检查一个本地整合包，给出预览和还缺什么（选择文件导入、拖进窗口都用它）
pub(crate) async fn pack_choice(state: &AppState, path: PathBuf) -> Result<PackChoice, AppError> {
    let plan = inspect_file(state, path).await?;
    let javas = scan_java(state).await;
    let readiness =
        ready::readiness(&state.game(), &state.http_client(), &plan.preview, &javas).await;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let preview = plan.preview.clone();
    state
        .pending_imports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), plan);
    Ok(PackChoice {
        token,
        preview,
        readiness,
    })
}

/// 用户关掉本地导入的预览、不导入了：丢掉检查结果
#[tauri::command]
#[specta::specta]
pub fn modpack_discard(state: State<'_, AppState>, token: String) {
    state
        .pending_imports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token);
}

/// 本地导入：确认后在后台导入，返回任务 ID。`name` 为空时用整合包的名字。
#[tauri::command]
#[specta::specta]
pub fn modpack_import(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    name: Option<String>,
) -> Result<String, AppError> {
    let plan = state
        .pending_imports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token)
        .ok_or_else(|| AppError::new("modpack.expired", "请重新选择整合包"))?;
    start_file_import(&app, &state, plan, name)
}

/// 本地整合包开始导入（选文件、拖进来的、重试都走这里）。临时目录按文件定（能接着装）
fn start_file_import(
    app: &AppHandle,
    state: &AppState,
    plan: PackPlan,
    name: Option<String>,
) -> Result<String, AppError> {
    let retry = Retry::ModpackFile {
        path: plan.pack_path().to_path_buf(),
        name: name.clone(),
    };
    let resume_key = modpack::file_resume_key(plan.pack_path());
    let job = state.jobs.start_with(
        format!("modpack-import:file:{}", plan.preview.name),
        format!("导入整合包 {}", plan.preview.name),
        ProgressUnit::Bytes,
        downloads::meta(JobSource::Local, None, None, Some(&retry)),
    )?;
    let job_id = job.id().to_owned();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = import_plan(&app, &state, &job, &plan, name.as_deref(), &resume_key).await;
        finish_import(&state, job, result);
    });
    Ok(job_id)
}

/// 重试本地导入：重新检查一遍文件再导入，下过的文件接着用
pub(crate) async fn import_file(
    app: &AppHandle,
    path: PathBuf,
    name: Option<String>,
) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let plan = inspect_file(&state, path).await?;
    start_file_import(app, &state, plan, name)
}

/// 搜整合包。支持按游戏版本、加载器与分类筛选。`category` 按平台是 Modrinth 的分类名或 CurseForge 的分类编号。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
#[specta::specta]
pub async fn modpack_search(
    state: State<'_, AppState>,
    query: String,
    source: ContentSource,
    game_version: Option<String>,
    loader: Option<LoaderKind>,
    sort: SearchSort,
    category: Option<String>,
    offset: u32,
) -> Result<SearchPage, AppError> {
    let gv = game_version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let loader_str = loader.map(loader_slug);
    let key = SearchKey {
        source: source_key(source),
        kind: "modpack",
        text: &query,
        game_version: gv,
        loader: loader_str,
        client_only: false,
        sort,
        category: category.as_deref(),
        offset,
    }
    .build();
    match source {
        ContentSource::Modrinth => {
            let client = state.http_client();
            let search = modrinth::search_modpacks(
                &client,
                &query,
                gv,
                loader_str,
                sort,
                category.as_deref(),
                offset,
                SEARCH_PAGE,
            );
            cached_search(&state, key, search).await
        }
        ContentSource::CurseForge => {
            let query = cf::SearchQuery {
                text: &query,
                class_id: cf::class::MODPACK,
                game_version: gv,
                loader: loader.and_then(curseforge_loader),
                sort,
                category: curseforge_category(category.as_deref())?,
                offset,
                limit: SEARCH_PAGE,
            };
            cached_search(&state, key, state.curseforge.search(&query)).await
        }
    }
}

/// 一个整合包能装的版本（新的在前，带推荐项）
#[tauri::command]
#[specta::specta]
pub async fn modpack_versions(
    state: State<'_, AppState>,
    project_id: String,
    source: ContentSource,
) -> Result<Vec<PackVersion>, AppError> {
    match source {
        ContentSource::Modrinth => Ok(remote::versions(&state.http_client(), &project_id).await?),
        ContentSource::CurseForge => {
            Ok(remote::curseforge_versions(&state.curseforge, curseforge_id(&project_id)?).await?)
        }
    }
}

/// 任务键里的来源名（资源中心按它找「安装中」的整合包）
fn source_key(source: ContentSource) -> &'static str {
    match source {
        ContentSource::Modrinth => "modrinth",
        ContentSource::CurseForge => "curseforge",
    }
}

/// 在线安装前的准备情况：整合包还没下载，只按游戏版本查（主要是看缺不缺 Java）
#[tauri::command]
#[specta::specta]
pub async fn modpack_readiness(
    state: State<'_, AppState>,
    game_version: String,
) -> Result<PackReadiness, AppError> {
    let javas = scan_java(&state).await;
    Ok(ready::for_game(
        &state.game(),
        &state.http_client(),
        &game_version,
        None,
        &javas,
    )
    .await)
}

/// 在线安装：点了就是一个后台任务，下载整合包 → 检查整个包 → 装加载器、下载模组、补全游戏 → 登记成新实例。
/// 全程在「实例」页和任务中心显示进度，可以随便切页面，可以在任务中心取消。
/// `title` 只用来在任务中心显示；`name` 为空时用整合包自己的名字。
/// `join_server`：服务器要求的整合包，装好后把这个服务器加进新实例的多人游戏列表。
// 参数一一对应界面的调用（命令参数就是前端函数的参数），合成一个结构只会让每个调用处多包一层
#[allow(clippy::too_many_arguments)]
#[tauri::command]
#[specta::specta]
pub fn modpack_install(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    version_id: String,
    title: String,
    name: Option<String>,
    source: ContentSource,
    join_server: Option<crate::discover::ServerJoin>,
) -> Result<String, AppError> {
    let cf_file = match source {
        ContentSource::CurseForge => Some(curseforge_id(&version_id)?),
        ContentSource::Modrinth => None,
    };
    let retry = Retry::ModpackInstall {
        project_id: project_id.clone(),
        version_id: version_id.clone(),
        title: title.clone(),
        name: name.clone(),
        source,
        join_server: join_server.clone(),
    };
    let resume_key = retry.resume_key().unwrap_or_else(|| version_id.clone());
    // 资源中心按项目找这个任务（显示「安装中」）；同一个整合包同时只能装一份
    let job = state.jobs.start_with(
        format!("modpack-import:{}:{project_id}", source_key(source)),
        format!("安装整合包 {title}"),
        ProgressUnit::Bytes,
        downloads::meta(downloads::content_source(source), None, None, Some(&retry)),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let downloader = downloader_for_job(&state, DownloadPriority::Batch, &job);
        let cancel = job.cancel_token();
        let result = async {
            // 整合包下到这次导入自己的临时目录里：没装完时留着，重试时校验通过就不重下
            let staging = modpack::staging_dir(&state.game().root().join(".mcl-tmp"), &resume_key);
            let dir = staging.join("pack");
            let progress = |p| report(&job, "下载整合包", &p);
            let path = match cf_file {
                Some(file_id) => remote::curseforge_download(
                    &state.curseforge,
                    &downloader,
                    file_id,
                    &dir,
                    &cancel,
                    progress,
                )
                .await
                .map_err(|e| state.capture_app_error(&job, "下载整合包", e))?,
                None => remote::download(
                    &state.http_client(),
                    &downloader,
                    &version_id,
                    &dir,
                    &cancel,
                    progress,
                )
                .await
                .map_err(|e| state.capture_app_error(&job, "下载整合包", e))?,
            };
            job.set_message("检查整合包");
            let mut plan = inspect_file(&state, path).await?;
            plan.origin = Some(PackOrigin {
                project_id: project_id.clone(),
                version_id,
            });
            let instance =
                import_plan(&app, &state, &job, &plan, name.as_deref(), &resume_key).await?;
            // 尽量补齐整合包官方宽屏画廊图或图标作为默认封面（失败静默跳过，不影响安装成功）
            attach_pack_cover(&state, &instance, source, &project_id).await;
            // 服务器要求的整合包：装好后把服务器加进这个实例的多人游戏列表
            if let Some(server) = &join_server {
                crate::discover::add_server_to(&state, &instance, server);
            }
            Ok(instance)
        }
        .await;
        finish_import(&state, job, result);
    });
    Ok(job_id)
}

/// 按检查过的计划导入（本地导入和在线安装共用）。
/// 有作者禁止第三方下载的文件时开始补件：一边下载其他文件，一边等用户在浏览器里下载。
async fn import_plan(
    app: &AppHandle,
    state: &AppState,
    job: &JobHandle,
    plan: &PackPlan,
    name: Option<&str>,
    resume_key: &str,
) -> Result<Instance, AppError> {
    let downloader = downloader_for_job(state, DownloadPriority::Batch, job);
    let java = pick_java_for(state, job, &downloader, &plan.preview.game_version).await?;
    let claim = crate::claims::start(
        app,
        state,
        job,
        plan.preview.name.clone(),
        plan.preview.manual_files.clone(),
    );
    let game = state.game();
    let temp_root = game.root().join(".mcl-tmp");
    let cancel = job.cancel_token();
    let ctx = ImportContext {
        loader: LoaderContext {
            game: &game,
            client: &state.http_client(),
            downloader: &downloader,
            java: &java,
            work_dir: &temp_root,
            cancel: &cancel,
        },
        temp_root: &temp_root,
        manual: claim.as_ref().map(crate::claims::ClaimGuard::session),
    };
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    modpack::import(&ctx, plan, name, resume_key, |p| match &p {
        ImportProgress::Loader(l) => report_loader(job, l),
        ImportProgress::Files(d) => report(job, "下载整合包文件", d),
        ImportProgress::WaitingManual(n) => {
            job.set_message(format!("等你补齐 {n} 个文件（作者不允许第三方启动器下载）"));
        }
        ImportProgress::Overrides => job.set_message("解压配置文件"),
        ImportProgress::Game(phase, d) => report(job, phase_label(*phase), d),
    })
    .await
    .map_err(|e| state.capture_app_error(job, "下载整合包文件", e))
}

fn finish_import(state: &AppState, job: JobHandle, result: Result<Instance, AppError>) {
    match result {
        Ok(instance) => {
            tracing::info!(instance = %instance.id, "整合包已导入");
            job.set_instance(instance.id);
            job.succeed();
        }
        Err(_) if job.is_cancelled() => job.cancelled(),
        Err(error) => state.finish_failed_job(job, error.message),
    }
}

/// 整合包安装完成后尝试写入官方宽屏宣传图或图标地址，供无截图时作为实例默认封面兜底。
async fn attach_pack_cover(
    state: &AppState,
    instance: &Instance,
    source: ContentSource,
    project_id: &str,
) {
    let client = state.http_client();
    let cover_url = match source {
        ContentSource::Modrinth => match modrinth::project_detail(&client, project_id).await {
            Ok(detail) => detail
                .hit
                .gallery_url
                .or_else(|| detail.gallery.first().map(|g| g.url.clone()))
                .or(detail.hit.icon_url),
            Err(error) => {
                tracing::debug!(%error, project_id, "拉取 Modrinth 整合包封面信息失败，已忽略");
                None
            }
        },
        ContentSource::CurseForge => {
            if let Ok(mod_id) = project_id.parse::<u32>() {
                match state.curseforge.project_detail(mod_id).await {
                    Ok(detail) => detail
                        .hit
                        .gallery_url
                        .or_else(|| detail.gallery.first().map(|g| g.url.clone()))
                        .or(detail.hit.icon_url),
                    Err(error) => {
                        tracing::debug!(%error, mod_id, "拉取 CurseForge 整合包封面信息失败，已忽略");
                        None
                    }
                }
            } else {
                None
            }
        }
    };
    let Some(url) = cover_url.filter(|u| u.starts_with("https://")) else {
        return;
    };
    let game = state.game();
    if let Err(error) = instance::update(&game, &instance.id, |config| {
        if let Some(pack) = &mut config.source {
            pack.cover_url = Some(url.clone());
        }
    }) {
        tracing::debug!(%error, instance = %instance.id, "写入整合包默认封面地址失败，已忽略");
    }
}
