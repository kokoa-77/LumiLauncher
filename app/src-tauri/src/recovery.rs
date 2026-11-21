//! 启动时把上次没做完的改动退回。
//!
//! 一次改好几个文件的操作（安装、更新、删除、撤销、回到快照……）做到一半启动器被关掉，
//! 实例里会留着改动日志。启动时在后台挨个看实例，有日志就退回改之前；界面打开后来问一次，给用户弹提示。
//! 改实例之前、启动实例之前核心库也会再看一眼，所以这里慢一点、漏一个也不会出错。

use std::sync::Arc;

use launcher_core::content::InstanceContent;
use launcher_core::instance::{self, Instance};
use launcher_core::paths::GameDir;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::AppError;
use crate::state::AppState;

/// 退回了（或者没退回去）的一次改动，给界面弹提示
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryNote {
    pub instance_id: String,
    pub instance_name: String,
    /// 那次在做什么（「更新 Sodium 等 3 项」）
    pub summary: String,
    /// 退回去了；没退回去时 `message` 说为什么（改这个实例之前、启动之前、下次打开都会再试）
    pub ok: bool,
    pub message: Option<String>,
}

/// 启动时恢复的结果。恢复做完之前一直锁着，界面来拿时会等它做完
pub type RecoveryNotes = Arc<AsyncMutex<Vec<RecoveryNote>>>;

/// 启动时调用（`AppState` 托管之后）
pub fn spawn(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(mut notes) = state.recovery.clone().try_lock_owned() else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let game = state.game();
        let starting_game = game.clone();
        let (world_dirs, found) = tauri::async_runtime::spawn_blocking(move || {
            let world_dirs = crate::backups::directories(&starting_game)
                .into_iter()
                .filter(|dir| launcher_core::saves::backup::Backups::new(dir).has_pending_restore())
                .collect::<Vec<_>>();
            (world_dirs, pending(&starting_game))
        })
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "查找没做完的改动失败");
            (Vec::new(), Vec::new())
        });
        for dir in world_dirs {
            let _turn = state.turns.take(&dir).await;
            if let Err(error) = crate::backups::settle(dir).await {
                tracing::warn!(error = %error.message, "启动时收尾上次世界恢复失败");
            }
        }
        for (instance, content) in found {
            // 和别的改动排队（启动时没别人，保险）
            let Ok(dir) = instance.game_dir(&game) else {
                continue;
            };
            let _turn = state.turns.take(&dir).await;
            let note = match content.recover().await {
                Ok(Some(recovered)) => RecoveryNote {
                    instance_id: instance.id,
                    instance_name: instance.config.name,
                    summary: recovered.summary,
                    ok: true,
                    message: None,
                },
                Ok(None) => continue,
                Err(error) => {
                    let error = AppError::from(error);
                    RecoveryNote {
                        instance_id: instance.id,
                        instance_name: instance.config.name,
                        summary: String::new(),
                        ok: false,
                        message: Some(error.message),
                    }
                }
            };
            tracing::info!(instance = %note.instance_name, ok = note.ok, "启动时处理了没做完的改动");
            notes.push(note);
        }
    });
}

/// 有改动日志留着的实例
fn pending(game: &GameDir) -> Vec<(Instance, InstanceContent)> {
    instance::list(game)
        .into_iter()
        .filter_map(|instance| {
            let content = InstanceContent::for_game(game, instance.game_dir(game).ok()?);
            content.has_pending().then_some((instance, content))
        })
        .collect()
}

/// 启动时退回了哪些没做完的改动（拿一次就清空；启动时的检查还没做完会等它做完）
#[tauri::command]
#[specta::specta]
pub async fn content_recovered(state: State<'_, AppState>) -> Result<Vec<RecoveryNote>, AppError> {
    Ok(std::mem::take(&mut *state.recovery.lock().await))
}
