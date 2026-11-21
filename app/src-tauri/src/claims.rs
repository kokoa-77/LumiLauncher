//! 补件（作者禁止第三方下载的文件由用户在浏览器里下载）：会话登记、拖文件进窗口、相关命令。
//!
//! 拖进窗口的文件由这里（后端）接收，路径直接交给核心库核对，**界面拿不到路径**。

use std::path::PathBuf;
use std::sync::Arc;

use launcher_core::content::claim::{ClaimParams, ClaimSession, ClaimSnapshot, ManualFile};
use launcher_core::content::store::{ContentStore, STORE_DIR};
use launcher_core::jobs::JobHandle;
use tauri::{AppHandle, DragDropEvent, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::events::{ClaimUpdated, FileDragHover, emit};
use crate::state::AppState;

/// 一次补件的生命周期：建好就开始盯下载文件夹；丢掉时结束补件、从登记表里删掉。
pub struct ClaimGuard {
    app: AppHandle,
    session: Arc<ClaimSession>,
    stop_watch: CancellationToken,
}

impl ClaimGuard {
    pub fn session(&self) -> &ClaimSession {
        &self.session
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        self.stop_watch.cancel();
        self.session.close();
        let state = self.app.state::<AppState>();
        state
            .claims
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| s.id() != self.session.id());
    }
}

/// 有要用户在浏览器里下载的文件时（整合包、单个模组都可能有），开始补件；没有就返回 `None`。
pub fn start(
    app: &AppHandle,
    state: &AppState,
    job: &JobHandle,
    title: String,
    files: Vec<ManualFile>,
) -> Option<ClaimGuard> {
    if files.is_empty() {
        return None;
    }
    let count = files.len();
    let handle = app.clone();
    let session = ClaimSession::new(
        ClaimParams {
            job_id: job.id().to_owned(),
            title: title.clone(),
            files,
            watch: crate::downloads::claim_folder(state).into_iter().collect(),
            store: ContentStore::new(state.game().root().join(STORE_DIR)),
            curseforge: state.curseforge.clone(),
            move_claimed: state.settings.get().download.move_claimed_files,
        },
        move |snapshot| emit(&handle, ClaimUpdated(snapshot)),
    );
    state
        .claims
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(session.clone());
    let stop_watch = job.cancel_token().child_token();
    {
        let session = session.clone();
        let stop = stop_watch.clone();
        tauri::async_runtime::spawn(async move { session.watch(&stop).await });
    }
    emit(app, ClaimUpdated(session.snapshot()));
    tracing::info!(%title, files = count, "开始补件");
    Some(ClaimGuard {
        app: app.clone(),
        session,
        stop_watch,
    })
}

fn find(state: &AppState, id: &str) -> Result<Arc<ClaimSession>, AppError> {
    state
        .claims
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|s| s.id() == id)
        .cloned()
        .ok_or_else(|| AppError::new("claim.not_found", "这次补件已经结束了"))
}

/// 最新的一次补件（拖进窗口的文件交给它）
fn newest(state: &AppState) -> Option<Arc<ClaimSession>> {
    state
        .claims
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last()
        .cloned()
}

/// 窗口的拖放事件：拖进来时通知界面高亮；放下时有正在进行的补件就交给补件，
/// 没有就当成「拖进来装」。
pub fn on_drag_drop(app: &AppHandle, event: &DragDropEvent) {
    match event {
        DragDropEvent::Enter { .. } => emit(app, FileDragHover(true)),
        DragDropEvent::Leave => emit(app, FileDragHover(false)),
        DragDropEvent::Drop { paths, .. } => {
            emit(app, FileDragHover(false));
            let state = app.state::<AppState>();
            match newest(&state) {
                Some(session) => {
                    let paths = paths.clone();
                    tauri::async_runtime::spawn(async move { session.offer(paths).await });
                }
                None => crate::drops::received(app, paths.clone()),
            }
        }
        _ => {}
    }
}

/// 正在进行的补件（界面启动时读一次，之后靠 `ClaimUpdated` 事件）。
#[tauri::command]
#[specta::specta]
pub fn claim_list(state: State<'_, AppState>) -> Vec<ClaimSnapshot> {
    state
        .claims
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|s| s.snapshot())
        .collect()
}

/// 跳过 / 取消跳过一个可选文件。
#[tauri::command]
#[specta::specta]
pub fn claim_skip(
    state: State<'_, AppState>,
    claim_id: String,
    file_id: u32,
    skipped: bool,
) -> Result<(), AppError> {
    let session = find(&state, &claim_id)?;
    if session.set_skipped(file_id, skipped) {
        Ok(())
    } else {
        Err(AppError::new(
            "claim.required",
            "这个文件是必需的，不能跳过",
        ))
    }
}

/// 弹出系统选择窗口选已经下载好的文件（和拖进窗口效果一样）。
#[tauri::command]
#[specta::specta]
pub async fn claim_pick(
    app: AppHandle,
    state: State<'_, AppState>,
    claim_id: String,
) -> Result<(), AppError> {
    use tauri_plugin_dialog::DialogExt;
    let session = find(&state, &claim_id)?;
    let dialog_app = app.clone();
    let start_dir = crate::downloads::claim_folder(&state);
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = dialog_app
            .dialog()
            .file()
            .set_title("选择下载好的文件（可以多选）")
            .add_filter("模组、材质包、光影", &["jar", "zip"]);
        if let Some(window) = dialog_app.get_webview_window(crate::window::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        if let Some(dir) = start_dir {
            dialog = dialog.set_directory(dir);
        }
        dialog.blocking_pick_files()
    })
    .await
    .map_err(|e| AppError::new("dialog.failed", "打开选择窗口失败").with_detail(e))?;
    let Some(picked) = picked else {
        return Ok(());
    };
    let paths: Vec<PathBuf> = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    session.offer(paths).await;
    Ok(())
}
