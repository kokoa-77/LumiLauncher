//! 前端能调用的全部命令。命令名格式 `<域>_<动作>`；这一层只做参数解析、调用核心库、转换错误。
//! 新增命令后要登记到 `lib.rs` 的 `specta_builder`，再运行 `cargo test -p launcher-app` 重新生成前端类型。

use std::sync::atomic::Ordering;

use launcher_core::config::Settings;
use launcher_core::jobs::JobSnapshot;
use launcher_core::redact::redact;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::error::AppError;
use crate::state::AppState;
use crate::window::{Backdrop, MAIN_WINDOW};

/// 前端日志单条最长字符数，防止刷爆日志文件。
const MAX_FRONTEND_LOG_CHARS: usize = 4000;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub backdrop: Backdrop,
    /// 电脑总内存（MB），界面判断要不要自动用「流畅」画面效果；读不到为空
    pub memory_mb: Option<u32>,
}

#[tauri::command]
#[specta::specta]
pub fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        backdrop: *state
            .backdrop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        memory_mb: launcher_core::launch::memory::system_memory()
            .map(|m| u32::try_from(m.total_mb).unwrap_or(u32::MAX)),
    }
}

/// 前端第一帧画好后调用：显示主窗口（只生效一次）。
#[tauri::command]
#[specta::specta]
pub fn app_ready(app: AppHandle, state: State<'_, AppState>) {
    if state.ui_ready.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::info!("界面已就绪，显示窗口");
    crate::shortcuts::show_on_ready(&app);
}

#[derive(Debug, Clone, Copy, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum FrontendLogLevel {
    Info,
    Warn,
    Error,
}

/// 前端把需要留档的信息（尤其是未捕获的错误）写进启动器日志。
#[tauri::command]
#[specta::specta]
pub fn app_log(level: FrontendLogLevel, message: String) {
    let truncated: String = message.chars().take(MAX_FRONTEND_LOG_CHARS).collect();
    let message = redact(&truncated);
    match level {
        FrontendLogLevel::Info => tracing::info!(target: "frontend", "{message}"),
        FrontendLogLevel::Warn => tracing::warn!(target: "frontend", "{message}"),
        FrontendLogLevel::Error => tracing::error!(target: "frontend", "{message}"),
    }
}

/// 前端界面语言变更时同步给后端（供「默认游戏语言：跟随启动器」使用，D26a）。
#[tauri::command]
#[specta::specta]
pub fn app_set_ui_language(state: State<'_, AppState>, language: String) {
    let trimmed = language.trim();
    if !trimmed.is_empty() && trimmed.len() <= 32 {
        *state
            .ui_language
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = trimmed.to_owned();
    }
}

#[tauri::command]
#[specta::specta]
pub fn settings_get(state: State<'_, AppState>) -> Settings {
    state.settings.get()
}

/// 保存整份设置，返回规范化后的结果（越界的值会被拉回合法范围）。
#[tauri::command]
#[specta::specta]
pub fn settings_set(state: State<'_, AppState>, settings: Settings) -> Result<Settings, AppError> {
    let prev = state.settings.get();
    let gpu_was_on = prev.game.high_performance_gpu;
    let normalized = settings.normalized();
    if normalized.download.proxy != prev.download.proxy {
        state.apply_proxy(&normalized.download.proxy).map_err(|e| {
            AppError::new(
                "network.invalid_proxy",
                "自定义代理地址无效，请检查主机名与端口",
            )
            .with_detail(e)
        })?;
    }
    let saved = state.settings.set(normalized)?;
    state.doh.set_enabled(saved.download.doh_enabled);
    // Modrinth、CurseForge 两行下载源立刻生效（D18b）
    launcher_core::content::mirror::set_policies(
        saved.download.modrinth_source,
        saved.download.curseforge_source,
    );
    // 关掉「高性能显卡」：撤回我们写过的 Windows 显卡设置（D17h）
    if gpu_was_on && !saved.game.high_performance_gpu {
        crate::system::undo_gpu(&state);
    }
    Ok(saved)
}

/// 导出可迁移的偏好设置 JSON（自动剥离本机路径，绝不含任何登录凭据，D26a）。
#[tauri::command]
#[specta::specta]
pub async fn settings_export(app: AppHandle, state: State<'_, AppState>) -> Result<bool, AppError> {
    let json =
        state.settings.get().export_portable().map_err(|e| {
            AppError::new("settings.export_failed", "序列化设置失败").with_detail(e)
        })?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("导出 Lumi 偏好设置")
            .set_file_name("lumi-settings.json")
            .add_filter("JSON 文件", &["json"]);
        if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        let Some(picked) = dialog.blocking_save_file() else {
            return Ok(false);
        };
        let path = picked.into_path().map_err(|e| {
            AppError::new("settings.export_failed", "无法写入所选路径").with_detail(e)
        })?;
        launcher_core::fs::write_atomic(&path, json.as_bytes()).map_err(|e| {
            AppError::new("settings.export_failed", "导出设置文件失败").with_detail(e)
        })?;
        Ok(true)
    })
    .await
    .map_err(|e| AppError::new("dialog.failed", "打开保存窗口失败").with_detail(e))?
}

/// 导入偏好设置 JSON（自动保留本机游戏目录与补件目录，绝不覆盖账号数据，D26a）。
#[tauri::command]
#[specta::specta]
pub async fn settings_import(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<Settings>, AppError> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("导入 Lumi 偏好设置")
            .add_filter("JSON 文件", &["json"]);
        if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_pick_file()
    })
    .await
    .map_err(|e| AppError::new("dialog.failed", "打开选择窗口失败").with_detail(e))?;
    let Some(file) = picked else {
        return Ok(None);
    };
    let path = file
        .into_path()
        .map_err(|e| AppError::new("settings.import_failed", "无法读取所选文件").with_detail(e))?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| AppError::new("settings.import_failed", "读取设置文件失败").with_detail(e))?;
    let current = state.settings.get();
    let imported = current.import_portable(&raw).map_err(|e| {
        AppError::new(
            "settings.import_invalid",
            "设置文件格式无效，请选择由 Lumi 导出的 JSON 文件",
        )
        .with_detail(e)
    })?;
    if imported.download.proxy != current.download.proxy {
        state.apply_proxy(&imported.download.proxy).map_err(|e| {
            AppError::new(
                "network.invalid_proxy",
                "导入文件中的代理配置无效，已取消导入",
            )
            .with_detail(e)
        })?;
    }
    let saved = state.settings.set(imported)?;
    state.doh.set_enabled(saved.download.doh_enabled);
    launcher_core::content::mirror::set_policies(
        saved.download.modrinth_source,
        saved.download.curseforge_source,
    );
    Ok(Some(saved))
}

#[tauri::command]
#[specta::specta]
pub fn jobs_list(state: State<'_, AppState>) -> Vec<JobSnapshot> {
    state.jobs.list()
}

/// 请求取消任务；返回任务是否存在且仍在运行。
#[tauri::command]
#[specta::specta]
pub fn jobs_cancel(state: State<'_, AppState>, id: String) -> bool {
    state.jobs.cancel(&id)
}
