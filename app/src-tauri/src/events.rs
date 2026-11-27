//! 后端推给前端的事件。

use std::sync::Arc;
use std::time::Duration;

use launcher_core::auth::microsoft::AuthStage;
use launcher_core::content::claim::ClaimSnapshot;
use launcher_core::jobs::{JobRegistry, JobSnapshot};
use launcher_core::launch::LogLine;
use launcher_core::net::{DownloadEngine, TransferTelemetry};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::broadcast::error::RecvError;

use crate::accounts::AccountHealthStatus;
use crate::sessions::SessionSnapshot;

/// 某个任务的状态或进度变了（核心库已做节流）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct JobUpdated(pub JobSnapshot);

/// Microsoft 桌面登录当前所处阶段，供界面四阶段进度条实时更新。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct AuthProgressUpdated {
    pub stage: AuthStage,
    pub expires_in_secs: Option<u32>,
}

/// 账号在线会话预热/健康状态变化通知。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct AccountHealthUpdated {
    pub account_id: String,
    pub status: AccountHealthStatus,
    pub verified_at_ms: Option<f64>,
    pub error_code: Option<String>,
}

/// 游戏会话状态变了（启动中 → 加载中 → 主菜单 → 进入世界 → 退出 / 崩溃）。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct SessionUpdated(pub SessionSnapshot);

/// 一批新的游戏日志（攒 150 毫秒发一次，避免一行一个事件）。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct GameLogBatch {
    pub instance_id: String,
    pub lines: Vec<LogLine>,
}

/// 一次启动的记录写完了（游戏退出、崩溃或没启动起来），「日志」页签据此刷新列表。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecordSaved {
    pub instance_id: String,
    pub record_id: String,
}

/// 启动时官验失败。保留原请求供界面在用户决定后重试。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct LaunchAuthBlocked {
    pub instance_id: String,
    pub world: Option<String>,
    pub server_address: Option<String>,
    pub memory_mb: Option<u32>,
    pub code: String,
    pub message: String,
}

/// 补件的进度变了（认领了一个文件、拖错了、结束了……）。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct ClaimUpdated(pub ClaimSnapshot);

/// 有文件被拖到窗口上方（`true`）或拖走、放下（`false`），界面据此高亮放置区。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct FileDragHover(pub bool);

/// 放下了文件、后台正在认（一包东西要先拆开，可能要几秒），界面显示「正在看看是什么」；
/// 认完发 [`FilesDropped`]。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct FilesReading(pub bool);

/// 拖进窗口的文件认好了（没有正在进行的补件时），界面弹出「装到哪」。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct FilesDropped {
    pub items: Vec<crate::drops::DropItem>,
    /// 一包东西里没拆的别的文件，每包一条
    pub rests: Vec<crate::drops::BundleRest>,
}

/// 10 Hz 独立实时传输遥测事件（仅在有活跃传输或任务时推送，空闲时 0 Hz 零唤醒）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct TransferTelemetryUpdated(pub TransferTelemetry);

/// 实例列表在后台变了（重新扫挂上的别家目录时加了、改了、移除了实例），界面重新拿一次。
#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
pub struct InstancesChanged;

/// 发事件；失败只记日志（窗口关了等情况）。
pub fn emit<E: Event + Serialize + Clone>(handle: &AppHandle, event: E) {
    if let Err(error) = event.emit(handle) {
        tracing::warn!(%error, "推送事件失败");
    }
}

/// 把任务注册表的变化转发成前端事件。
pub fn forward_job_events(handle: AppHandle, jobs: &JobRegistry) {
    let mut rx = jobs.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(snapshot) => emit(&handle, JobUpdated(snapshot)),
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "任务事件积压，跳过了部分中间进度");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// 启动 10 Hz `TransferTelemetryUpdated` 遥测脉冲推送（严格只在活跃传输期间触发，空闲 0 Hz）。
pub fn spawn_telemetry_emitter(handle: AppHandle, engine: Arc<DownloadEngine>, jobs: JobRegistry) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut was_active = false;
        loop {
            interval.tick().await;
            let active = engine.telemetry().is_active() || jobs.has_active_jobs();
            if active {
                let snap = engine.runtime_telemetry();
                emit(&handle, TransferTelemetryUpdated(snap));
                was_active = true;
            } else if was_active {
                let snap = engine.runtime_telemetry();
                emit(&handle, TransferTelemetryUpdated(snap));
                was_active = false;
            }
        }
    });
}
