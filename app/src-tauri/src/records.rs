//! 每次启动的完整记录（D17c）：启动时开一份、边玩边写、结束时补上结局。界面在实例页「日志」页签里翻。
//! 记录本身的读写在核心库 `launch::record`，这里只负责接到启动流程上、给界面开命令。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use launcher_core::content::{ContentKind, InstanceContent};
use launcher_core::instance::Instance;
use launcher_core::launch::record::{self, read_crash_report, read_log};
use launcher_core::launch::{LaunchRecord, LogLine, QuickPlayRecord, RecordEnd, RecordLog};
use launcher_core::launch::{PreparedLaunch, RecordWriter};
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::events::{LaunchRecordSaved, emit};
use crate::game::load_instance;
use crate::state::AppState;

/// 界面一次最多看最后这么多条（再多的在文件里，「在文件夹中显示」能找到）
const MAX_SHOWN_LINES: usize = 50_000;

/// 正在写的这一次（启动流程、日志推送、状态变化几处共用）。开不了记录时是 `None`，不挡启动。
pub(crate) type Recorder = Arc<Mutex<Option<RecordWriter>>>;

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// 开始记这一次：记下游戏版本、加载器、直接进哪里、当时装着的模组。失败只记日志。
pub(crate) fn begin(
    state: &AppState,
    instance: &Instance,
    quick_play: Option<QuickPlayRecord>,
) -> Recorder {
    let game = state.game();
    let writer = instance
        .game_dir(&game)
        .map_err(|e| e.to_string())
        .and_then(|dir| {
            let mods = InstanceContent::for_game(&game, dir.clone())
                .list(ContentKind::Mod)
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "启动记录：读不到模组列表，这一次不记模组");
                    Vec::new()
                });
            let config = &instance.config;
            let record = LaunchRecord::start(
                now(),
                config.game_version.clone(),
                config
                    .loader
                    .as_ref()
                    .map(|l| format!("{} {}", l.kind.label(), l.version)),
                quick_play,
                &mods,
            );
            RecordWriter::begin(&dir, record).map_err(|e| e.to_string())
        });
    match writer {
        Ok(writer) => {
            tracing::info!(instance = %instance.id, record = writer.id(), "开始记录这次启动");
            Arc::new(Mutex::new(Some(writer)))
        }
        Err(error) => {
            tracing::warn!(%error, instance = %instance.id, "开不了启动记录，这一次不记");
            Arc::default()
        }
    }
}

/// 对正在写的记录做点什么（没有记录时什么也不做）
pub(crate) fn with(recorder: &Recorder, f: impl FnOnce(&mut RecordWriter)) {
    if let Some(writer) = recorder.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        f(writer);
    }
}

pub(crate) fn id(recorder: &Recorder) -> Option<String> {
    recorder
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|w| w.id().to_owned())
}

pub(crate) fn append(recorder: &Recorder, lines: &[LogLine]) {
    with(recorder, |w| w.append(lines));
}

/// 写完这一次，告诉界面刷新
pub(crate) fn finish(
    app: &AppHandle,
    recorder: &Recorder,
    instance_id: &str,
    mut end: RecordEnd,
    prepared: Option<&PreparedLaunch>,
) {
    let Some(writer) = recorder.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    end.ended_at = now();
    let record = match prepared {
        Some(launch) => writer.finish_with_launch(end, launch),
        None => writer.finish(end),
    };
    tracing::info!(
        instance = %instance_id,
        record = %record.id,
        outcome = ?record.outcome,
        lines = record.log_lines,
        "启动记录已保存"
    );
    emit(
        app,
        LaunchRecordSaved {
            instance_id: instance_id.to_owned(),
            record_id: record.id,
        },
    );
}

fn instance_dir(state: &AppState, instance_id: &str) -> Result<PathBuf, AppError> {
    let instance = load_instance(state, instance_id)?;
    Ok(instance.game_dir(&state.game())?)
}

fn read_failed(error: impl std::fmt::Display) -> AppError {
    AppError::new("launch.record_read_failed", "读取这次启动的记录失败").with_detail(error)
}

/// 这个实例最近的启动记录（最多 20 次），新的在前。
#[tauri::command]
#[specta::specta]
pub fn launch_records(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Vec<LaunchRecord>, AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    let running = state.sessions.running_record(&instance_id);
    Ok(record::list(&dir, running.as_deref()))
}

/// 某一次的游戏输出（最多最后 5 万条）。
#[tauri::command]
#[specta::specta]
pub async fn launch_record_log(
    state: State<'_, AppState>,
    instance_id: String,
    record_id: String,
) -> Result<RecordLog, AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    tauri::async_runtime::spawn_blocking(move || read_log(&dir, &record_id, MAX_SHOWN_LINES))
        .await
        .map_err(read_failed)?
        .map_err(read_failed)
}

/// 某一次复制下来的崩溃报告；没崩溃过时为空。
#[tauri::command]
#[specta::specta]
pub async fn launch_record_crash(
    state: State<'_, AppState>,
    instance_id: String,
    record_id: String,
) -> Result<Option<String>, AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    tauri::async_runtime::spawn_blocking(move || read_crash_report(&dir, &record_id))
        .await
        .map_err(read_failed)?
        .map_err(read_failed)
}

/// 对指定的历史记录判断故障类型。读取在后台执行，旧记录也能重新判断。
#[tauri::command]
#[specta::specta]
pub async fn launch_record_diagnosis(
    state: State<'_, AppState>,
    instance_id: String,
    record_id: String,
) -> Result<launcher_core::launch::diagnostics::CrashKind, AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::launch::diagnostics::diagnose(&dir, &record_id)
    })
    .await
    .map_err(read_failed)?
    .map_err(read_failed)
}

fn export_failed(error: impl std::fmt::Display) -> AppError {
    AppError::new("launch.diagnostic_export_failed", "无法保存诊断包").with_detail(error)
}

/// 在用户选择的位置保存 ZIP。取消保存返回 false。
#[tauri::command]
#[specta::specta]
pub async fn launch_record_diagnostic_export(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    record_id: String,
) -> Result<bool, AppError> {
    use tauri::Manager;
    use tauri_plugin_dialog::DialogExt;

    let dir = instance_dir(&state, &instance_id)?;
    let home = app.path().home_dir().ok();
    let name = format!("Lumi-diagnostic-{record_id}.zip");
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = launcher_core::launch::diagnostics::export(&dir, &record_id, home.as_deref())
            .map_err(export_failed)?;
        let mut dialog = app
            .dialog()
            .file()
            .set_title("保存 Lumi 诊断包")
            .set_file_name(name)
            .add_filter("ZIP 诊断包", &["zip"]);
        if let Some(window) = app.get_webview_window(crate::window::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        let Some(path) = dialog.blocking_save_file() else {
            return Ok(false);
        };
        let path = path.into_path().map_err(export_failed)?;
        launcher_core::fs::write_atomic(&path, &bytes).map_err(export_failed)?;
        Ok(true)
    })
    .await
    .map_err(export_failed)?
}

/// 在资源管理器里选中这一次的输出文件（想发给别人看时用）。
#[tauri::command]
#[specta::specta]
pub fn launch_record_reveal(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    record_id: String,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let dir = instance_dir(&state, &instance_id)?;
    let path = record::log_file(&dir, &record_id)?;
    app.opener().reveal_item_in_dir(&path).map_err(|e| {
        AppError::new("launch.record_reveal_failed", "打不开记录所在的文件夹").with_detail(e)
    })
}

/// 手动将指定启动记录的脱敏日志（含崩溃报告，如有）上传至 mclo.gs，返回分享链接（D26c）。
#[tauri::command]
#[specta::specta]
pub async fn launch_record_upload_log(
    state: State<'_, AppState>,
    instance_id: String,
    record_id: String,
) -> Result<launcher_core::net::MclogsUpload, AppError> {
    let dir = instance_dir(&state, &instance_id)?;
    let combined = tauri::async_runtime::spawn_blocking(move || {
        let crash = read_crash_report(&dir, &record_id).ok().flatten();
        let log = read_log(&dir, &record_id, 10_000).ok();
        let mut parts: Vec<String> = Vec::new();
        if let Some(report) = crash
            && !report.trim().is_empty()
        {
            parts.push("===== Crash Report =====".to_owned());
            parts.push(report);
        }
        if let Some(log) = log
            && !log.lines.is_empty()
        {
            if !parts.is_empty() {
                parts.push(String::new());
                parts.push("===== Game Output =====".to_owned());
            }
            for line in log.lines {
                let prefix = match &line.thread {
                    Some(thread) if !thread.is_empty() => format!("[{thread}/{:?}] ", line.level),
                    _ => format!("[{:?}] ", line.level),
                };
                parts.push(format!("{prefix}{}", line.message));
            }
        }
        parts.join("\n")
    })
    .await
    .map_err(read_failed)?;

    let client = state.http_client();
    launcher_core::net::upload_log(&client, &combined)
        .await
        .map_err(|e| match e {
            launcher_core::net::MclogsError::Empty => {
                AppError::new("launch.log_upload_empty", "此次启动没有可上传的日志内容")
            }
            other => AppError::new("launch.log_upload_failed", "上传日志到 mclo.gs 失败")
                .with_detail(other),
        })
}
