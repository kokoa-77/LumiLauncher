//! 世界备份的命令和自动备份（D17j）。
//!
//! 设置里打开「自动备份」以后：
//! - 正常退出游戏（进过世界、退出码 0）后，备份这次玩过的世界（崩溃退出的不备，免得坏掉的世界挤掉好的备份）；
//! - 从首页「继续」进世界之前，世界在上次备份之后变过（上次崩溃了、在别的启动器里玩过）就先备一份；
//! - 世界没变就跳过；超过 1 GB 的世界第一次不备，先问用户（[`BackupAsk`]），记住选择。
//!
//! 备份、恢复以入口实例和世界登记任务，写入按实际目录排队。共享目录的游戏启动、
//! 世界恢复和不同世界的备份共用同一把锁（索引与恢复记录属于整个目录）。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

use launcher_core::jobs::{JobHandle, JobState, ProgressUnit};
use launcher_core::saves;
use launcher_core::saves::backup::{
    AutoCheck, BackupError, BackupInfo, BackupReason, Backups, migrate_backups_root,
};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::events::emit;
use crate::state::AppState;

/// 大世界第一次自动备份前问用户
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct BackupAsk {
    pub instance_id: String,
    pub folder: String,
    pub world_name: String,
    /// 世界多大（字节）
    pub bytes: f64,
}

/// WebView 重建时先保留还未送出的确认，等游戏事件监听重新装好后再推送。
#[derive(Default)]
pub struct BackupPrompts(Mutex<PromptState>);

#[derive(Default)]
struct PromptState {
    listening: bool,
    waiting: Vec<BackupAsk>,
}

pub fn pause_prompts(app: &AppHandle) {
    app.state::<BackupPrompts>()
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .listening = false;
}

pub fn resume_prompts(app: &AppHandle) {
    let prompts = app.state::<BackupPrompts>();
    let mut prompts = prompts
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prompts.listening = true;
    for prompt in std::mem::take(&mut prompts.waiting) {
        emit(app, prompt);
    }
}

fn ask_backup(app: &AppHandle, prompt: BackupAsk) {
    let prompts = app.state::<BackupPrompts>();
    let mut prompts = prompts
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if prompts.listening {
        emit(app, prompt);
    } else {
        prompts.waiting.push(prompt);
    }
}

/// 所有实例的世界备份一共占多少
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BackupUsage {
    pub bytes: f64,
    pub count: u32,
}

/// 世界备份目录信息（D26b）
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BackupDirInfo {
    /// 自定义目录完整路径；使用默认位置（随各实例存放）时为 `None`
    pub path: Option<String>,
    pub custom: bool,
}

/// 切换世界备份目录后的迁移结果（D26b）
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BackupDirChangeResult {
    pub info: BackupDirInfo,
    pub moved_files: u32,
    pub moved_bytes: f64,
}

fn job_key(instance_id: &str, folder: &str) -> String {
    format!("backup:{instance_id}:{folder}")
}

fn custom_backup_root(state: &AppState) -> Option<PathBuf> {
    state.settings.get().worlds.backup_dir.map(PathBuf::from)
}

fn backup_dir_info_of(state: &AppState) -> BackupDirInfo {
    let custom_dir = state.settings.get().worlds.backup_dir;
    BackupDirInfo {
        custom: custom_dir.is_some(),
        path: custom_dir,
    }
}

/// 实例的游戏目录（实例要在）
fn instance_dir(state: &AppState, instance_id: &str) -> Result<PathBuf, AppError> {
    crate::game::game_dir_of(state, instance_id)
}

fn backups_of(state: &AppState, instance_id: &str) -> Result<Backups, AppError> {
    let root = custom_backup_root(state);
    Ok(Backups::with_root(
        instance_dir(state, instance_id)?,
        root.as_deref(),
    ))
}

/// 游戏开着时不能备份、恢复这个实例的世界（游戏正在写存档）
fn ensure_not_running(state: &AppState, instance_id: &str) -> Result<(), AppError> {
    if crate::game::instance_running(state, instance_id) {
        return Err(AppError::new(
            "backup.game_running",
            "游戏正在运行，先退出游戏再备份或恢复（游戏开着时存档还在变）",
        ));
    }
    Ok(())
}

fn keep(state: &AppState) -> usize {
    state.settings.get().worlds.keep_backups as usize
}

/// 在后台备份一个世界，返回任务 ID
fn spawn_backup(
    app: &AppHandle,
    instance_id: &str,
    folder: &str,
    reason: BackupReason,
) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let dir = instance_dir(&state, instance_id)?;
    let name = saves::world_name(&dir, folder);
    let job = state.jobs.start_with(
        job_key(instance_id, folder),
        format!("备份世界「{name}」"),
        ProgressUnit::Bytes,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::Local,
            Some(instance_id),
            None,
            None,
        ),
    )?;
    let job_id = job.id().to_owned();
    let keep = keep(&state);
    let root = custom_backup_root(&state);
    let folder = folder.to_owned();
    run_blocking(app, dir, job, move |job, dir| {
        Backups::with_root(dir, root.as_deref())
            .create(&folder, reason, keep, &job.cancel_token(), |done, total| {
                job.set_progress(done as f64, Some(total as f64));
            })
            .map(|_| ())
    });
    Ok(job_id)
}

/// 在后台线程里做完一件备份的事，按结果结束任务
fn run_blocking(
    app: &AppHandle,
    dir: PathBuf,
    job: JobHandle,
    work: impl FnOnce(&JobHandle, PathBuf) -> Result<(), BackupError> + Send + 'static,
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let Some(turn) = state.turns.wait(&dir, &job).await else {
            job.cancelled();
            return;
        };
        let finished = tauri::async_runtime::spawn_blocking(move || {
            let result = work(&job, dir);
            drop(turn);
            match result {
                Ok(()) => job.succeed(),
                Err(BackupError::Cancelled) => job.cancelled(),
                Err(error) => job.fail(AppError::from(error).message),
            }
        })
        .await;
        if let Err(error) = finished {
            tracing::error!(%error, "备份的后台线程出错");
        }
    });
}

/// 游戏正常退出后调用：自动备份开着的话，备份这次玩过的世界（`started_ms` 是这次启动的时间）
pub fn after_game(app: &AppHandle, instance_id: &str, started_ms: u64) {
    let state = app.state::<AppState>();
    if !state.settings.get().worlds.auto_backup {
        return;
    }
    let Ok(dir) = instance_dir(&state, instance_id) else {
        return;
    };
    let root = custom_backup_root(&state);
    let backups = Backups::with_root(&dir, root.as_deref());
    for folder in launcher_core::saves::backup::played_since(&dir, started_ms) {
        match backups.auto_check(&folder) {
            Ok(AutoCheck::Go) => {
                if let Err(error) = spawn_backup(app, instance_id, &folder, BackupReason::AfterPlay)
                {
                    tracing::warn!(error = %error.message, folder, "开始自动备份失败");
                }
            }
            Ok(AutoCheck::AskLarge { bytes }) => ask_backup(
                app,
                BackupAsk {
                    instance_id: instance_id.to_owned(),
                    world_name: saves::world_name(&dir, &folder),
                    folder,
                    bytes: bytes as f64,
                },
            ),
            Ok(AutoCheck::Unchanged | AutoCheck::SkipLarge) => {}
            Err(error) => tracing::warn!(%error, folder, "看世界要不要备份时出错"),
        }
    }
}

/// 从首页进世界之前（启动任务里调用）：世界在上次备份之后变过就先备一份。出错不挡启动，只记日志
pub async fn before_play(state: &AppState, job: &JobHandle, instance_id: &str, folder: &str) {
    if !state.settings.get().worlds.auto_backup {
        return;
    }
    let Ok(dir) = instance_dir(state, instance_id) else {
        return;
    };
    let root = custom_backup_root(state);
    let backups = std::sync::Arc::new(Backups::with_root(dir, root.as_deref()));
    let check = {
        let (backups, folder) = (backups.clone(), folder.to_owned());
        tauri::async_runtime::spawn_blocking(move || backups.auto_check(&folder)).await
    };
    // 大世界还没问过、没变、不要自动备份的：这次不备（大世界玩完再问）
    match check {
        Ok(Ok(AutoCheck::Go)) => {}
        Ok(Ok(_) | Err(BackupError::NoWorld)) => return,
        Ok(Err(error)) => {
            tracing::warn!(%error, "看世界要不要备份时出错，照样启动");
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "看世界要不要备份的后台线程出错，照样启动");
            return;
        }
    }
    job.set_message("进世界前先备份一份");
    let (folder, keep, cancel) = (folder.to_owned(), keep(state), job.cancel_token());
    let result = tauri::async_runtime::spawn_blocking(move || {
        backups.create(&folder, BackupReason::BeforePlay, keep, &cancel, |_, _| {})
    })
    .await;
    match result {
        Ok(Ok(_)) => tracing::info!(instance = %instance_id, "进世界前备份好了"),
        Ok(Err(error)) => tracing::warn!(%error, "进世界前备份失败，照样启动"),
        Err(error) => tracing::warn!(%error, "进世界前备份的后台线程出错，照样启动"),
    }
}

/// 一个世界的备份，新的在前
#[tauri::command]
#[specta::specta]
pub fn backups_list(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<Vec<BackupInfo>, AppError> {
    Ok(backups_of(&state, &instance_id)?.list(&folder))
}

/// 现在备份一份（后台任务），返回任务 ID
#[tauri::command]
#[specta::specta]
pub fn backups_create(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<String, AppError> {
    ensure_not_running(&state, &instance_id)?;
    spawn_backup(&app, &instance_id, &folder, BackupReason::Manual)
}

/// 恢复到某一份备份（后台任务：先把现在的世界备一份，再换上去），返回任务 ID
#[tauri::command]
#[specta::specta]
pub fn backups_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    backup_id: String,
) -> Result<String, AppError> {
    ensure_not_running(&state, &instance_id)?;
    let dir = instance_dir(&state, &instance_id)?;
    let name = saves::world_name(&dir, &folder);
    let job = state.jobs.start_with(
        job_key(&instance_id, &folder),
        format!("恢复世界「{name}」"),
        ProgressUnit::Bytes,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::Local,
            Some(&instance_id),
            None,
            None,
        ),
    )?;
    let job_id = job.id().to_owned();
    let keep = keep(&state);
    let root = custom_backup_root(&state);
    run_blocking(&app, dir, job, move |job, dir| {
        Backups::with_root(dir, root.as_deref()).restore(
            &folder,
            &backup_id,
            keep,
            &job.cancel_token(),
            |done, total| {
                job.set_progress(done as f64, Some(total as f64));
            },
        )
    });
    Ok(job_id)
}

/// 删一份备份
#[tauri::command]
#[specta::specta]
pub fn backups_delete(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    backup_id: String,
) -> Result<(), AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    let _turn = state.turns.try_take(&dir).ok_or_else(backup_busy)?;
    let root = custom_backup_root(&state);
    Ok(Backups::with_root(dir, root.as_deref()).delete(&folder, &backup_id)?)
}

/// 在资源管理器里选中这份备份
#[tauri::command]
#[specta::specta]
pub fn backups_reveal(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    backup_id: String,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let path = backups_of(&state, &instance_id)?.locate(&folder, &backup_id)?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| AppError::new("backup.reveal_failed", "打不开备份所在的文件夹").with_detail(e))
}

/// 用户对大世界的回答：`auto` 为真时记住「要自动备份」并马上备一份（返回任务 ID）
#[tauri::command]
#[specta::specta]
pub fn backups_decide(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    auto: bool,
) -> Result<Option<String>, AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    {
        let _turn = state.turns.try_take(&dir).ok_or_else(backup_busy)?;
        let root = custom_backup_root(&state);
        Backups::with_root(dir, root.as_deref()).set_auto_large(&folder, auto)?;
    }
    tracing::info!(instance = %instance_id, folder, auto, "大世界要不要自动备份");
    if !auto || crate::game::instance_running(&state, &instance_id) {
        return Ok(None);
    }
    spawn_backup(&app, &instance_id, &folder, BackupReason::AfterPlay).map(Some)
}

fn backup_busy() -> AppError {
    AppError::new(
        "backup.busy",
        "这个游戏目录正在使用，等游戏退出或相关任务做完再改备份",
    )
}

pub(crate) fn directories(game: &launcher_core::paths::GameDir) -> BTreeSet<PathBuf> {
    launcher_core::instance::list(game)
        .into_iter()
        .filter_map(|i| i.game_dir(game).ok()?.canonicalize().ok())
        .collect()
}

/// 调用者持有实际目录的锁。只在有中断记录时启用后台恢复。
pub(crate) async fn settle(dir: PathBuf) -> Result<(), AppError> {
    let backups = Backups::new(dir);
    if backups.has_pending_restore() {
        tauri::async_runtime::spawn_blocking(move || backups.settle())
            .await
            .map_err(|e| {
                AppError::new("backup.failed", "处理上次中断的世界恢复失败").with_detail(e)
            })??;
    }
    Ok(())
}

/// 所有实例的世界备份一共占多少（设置里显示）
#[tauri::command]
#[specta::specta]
pub async fn backups_usage(state: State<'_, AppState>) -> Result<BackupUsage, AppError> {
    let game = state.game();
    let root = custom_backup_root(&state);
    let (bytes, count) = tauri::async_runtime::spawn_blocking(move || {
        directories(&game)
            .into_iter()
            .map(|dir| Backups::with_root(dir, root.as_deref()).usage())
            .fold((0, 0), |(b, c), (bytes, count)| (b + bytes, c + count))
    })
    .await
    .map_err(|e| AppError::new("backup.failed", "统计备份占用失败").with_detail(e))?;
    Ok(BackupUsage {
        bytes: bytes as f64,
        count: u32::try_from(count).unwrap_or(u32::MAX),
    })
}

/// 删掉所有实例的世界备份，返回腾出了多少字节。正在备份、恢复的时候不让删
#[tauri::command]
#[specta::specta]
pub async fn backups_clear(state: State<'_, AppState>) -> Result<f64, AppError> {
    ensure_no_backup_job(&state)?;
    let dirs = directories(&state.game());
    let turns: Vec<_> = dirs
        .iter()
        .map(|dir| state.turns.try_take(dir).ok_or_else(backup_busy))
        .collect::<Result<_, _>>()?;
    let root = custom_backup_root(&state);
    let freed = tauri::async_runtime::spawn_blocking(move || {
        let _turns = turns;
        let mut freed = 0;
        for dir in dirs {
            freed += Backups::with_root(dir, root.as_deref()).clear()?;
        }
        Ok::<u64, BackupError>(freed)
    })
    .await
    .map_err(|e| AppError::new("backup.failed", "删除备份失败").with_detail(e))??;
    tracing::info!(freed, "删掉了所有世界备份");
    Ok(freed as f64)
}

fn ensure_no_backup_job(state: &AppState) -> Result<(), AppError> {
    let busy = state
        .jobs
        .list()
        .into_iter()
        .any(|j| j.state == JobState::Running && j.key.starts_with("backup:"));
    if busy {
        return Err(AppError::new(
            "backup.busy",
            "正在备份或恢复世界，请等任务完成后再操作",
        ));
    }
    Ok(())
}

/// 当前世界备份存放目录信息（D26b）
#[tauri::command]
#[specta::specta]
pub fn backups_dir_info(state: State<'_, AppState>) -> BackupDirInfo {
    backup_dir_info_of(&state)
}

/// 选择自定义世界备份目录并安全迁移既有备份（D26b）
#[tauri::command]
#[specta::specta]
pub async fn backups_dir_choose(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<BackupDirChangeResult>, AppError> {
    if crate::game::any_game_running(&state) {
        return Err(AppError::new(
            "backup.game_running",
            "请先退出正在运行的游戏，再更改世界备份目录",
        ));
    }
    ensure_no_backup_job(&state)?;
    let Some(chosen) = crate::game::pick(&app, crate::game::Pick::Folder).await? else {
        return Ok(None);
    };
    let probe = chosen.join(format!(".lumi-write-probe-{}", launcher_core::now_millis()));
    std::fs::write(&probe, b"ok").map_err(|e| {
        AppError::new(
            "backup.dir_not_writable",
            "无法写入所选文件夹，请选择其他有写入权限的位置",
        )
        .with_detail(e)
    })?;
    let _ = std::fs::remove_file(&probe);

    let from_root = custom_backup_root(&state);
    if from_root.as_deref() == Some(chosen.as_path()) {
        return Ok(Some(BackupDirChangeResult {
            info: backup_dir_info_of(&state),
            moved_files: 0,
            moved_bytes: 0.0,
        }));
    }

    let dirs: Vec<PathBuf> = directories(&state.game()).into_iter().collect();
    let turns: Vec<_> = dirs
        .iter()
        .map(|dir| state.turns.try_take(dir).ok_or_else(backup_busy))
        .collect::<Result<_, _>>()?;
    let to_root = chosen.clone();
    let migrated = tauri::async_runtime::spawn_blocking(move || {
        let _turns = turns;
        migrate_backups_root(&dirs, from_root.as_deref(), Some(to_root.as_path()))
    })
    .await
    .map_err(|e| AppError::new("backup.failed", "迁移世界备份失败").with_detail(e))??;

    let mut settings = state.settings.get();
    settings.worlds.backup_dir = Some(chosen.display().to_string());
    state.settings.set(settings)?;
    tracing::info!(
        dir = %chosen.display(),
        files = migrated.moved_files,
        "已更改世界备份目录并完成迁移"
    );
    Ok(Some(BackupDirChangeResult {
        info: backup_dir_info_of(&state),
        moved_files: migrated.moved_files,
        moved_bytes: migrated.moved_bytes,
    }))
}

/// 恢复世界备份到默认位置（各实例自己的 `Lumi/backups/`）并安全迁回既有备份（D26b）
#[tauri::command]
#[specta::specta]
pub async fn backups_dir_reset(
    state: State<'_, AppState>,
) -> Result<BackupDirChangeResult, AppError> {
    if crate::game::any_game_running(&state) {
        return Err(AppError::new(
            "backup.game_running",
            "请先退出正在运行的游戏，再更改世界备份目录",
        ));
    }
    ensure_no_backup_job(&state)?;
    let from_root = custom_backup_root(&state);
    if from_root.is_none() {
        return Ok(BackupDirChangeResult {
            info: backup_dir_info_of(&state),
            moved_files: 0,
            moved_bytes: 0.0,
        });
    }
    let dirs: Vec<PathBuf> = directories(&state.game()).into_iter().collect();
    let turns: Vec<_> = dirs
        .iter()
        .map(|dir| state.turns.try_take(dir).ok_or_else(backup_busy))
        .collect::<Result<_, _>>()?;
    let migrated = tauri::async_runtime::spawn_blocking(move || {
        let _turns = turns;
        migrate_backups_root(&dirs, from_root.as_deref(), None)
    })
    .await
    .map_err(|e| AppError::new("backup.failed", "迁回世界备份失败").with_detail(e))??;

    let mut settings = state.settings.get();
    settings.worlds.backup_dir = None;
    state.settings.set(settings)?;
    tracing::info!(files = migrated.moved_files, "已将世界备份恢复至默认目录");
    Ok(BackupDirChangeResult {
        info: backup_dir_info_of(&state),
        moved_files: migrated.moved_files,
        moved_bytes: migrated.moved_bytes,
    })
}
