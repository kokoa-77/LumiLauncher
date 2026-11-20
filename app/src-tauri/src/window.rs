//! 主窗口：无边框 + 透明。Windows 11 上加云母（Mica）背景，Windows 10 用实心背景。
//! 窗口一开始是隐藏的，等前端第一帧画好调用 `app_ready` 再显示，避免白屏；
//! 如果前端迟迟没报告（比如脚本加载失败），超时后强制显示，并记一条警告方便排查。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

use crate::state::AppState;
use crate::window_lifecycle::{Lifecycle, OpenAction};

pub const MAIN_WINDOW: &str = "main";
const PLAYING_TRAY: &str = "playing";
const SHOW_FALLBACK: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct MainWindow(Mutex<Lifecycle>);

impl MainWindow {
    fn lock(&self) -> MutexGuard<'_, Lifecycle> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub fn saved_state_flags() -> StateFlags {
    StateFlags::all().difference(StateFlags::VISIBLE)
}

pub fn next_release_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Backdrop {
    /// Windows 11 云母，界面用半透明表面
    Mica,
    /// 实心背景
    Solid,
}

pub fn apply_backdrop(window: &WebviewWindow) -> Backdrop {
    #[cfg(windows)]
    {
        use tauri::window::{Effect, EffectsBuilder};

        let build = windows_version::OsVersion::current().build;
        if build >= 22_000 {
            match window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build()) {
                Ok(()) => return Backdrop::Mica,
                Err(error) => tracing::warn!(%error, build, "云母效果设置失败，改用实心背景"),
            }
        } else {
            tracing::info!(build, "系统不支持云母效果，使用实心背景");
        }
    }
    #[cfg(not(windows))]
    let _ = window;
    Backdrop::Solid
}

pub fn show_main(handle: &AppHandle) {
    let Some(window) = handle.get_webview_window(MAIN_WINDOW) else {
        tracing::error!("找不到主窗口");
        return;
    };
    for (action, result) in [
        ("取消最小化", window.unminimize()),
        ("显示", window.show()),
        ("聚焦", window.set_focus()),
    ] {
        if let Err(error) = result {
            tracing::warn!(%error, action, "主窗口操作失败");
        }
    }
}

/// 启动游戏后最小化主窗口（D26a）。
pub fn minimize_main(handle: &AppHandle) {
    on_main_thread(handle, |handle| {
        if let Some(window) = handle.get_webview_window(MAIN_WINDOW)
            && let Err(error) = window.minimize()
        {
            tracing::warn!(%error, "最小化主窗口失败");
        }
    });
}

pub fn spawn_show_fallback(handle: &AppHandle) {
    let generation = handle.state::<MainWindow>().lock().generation;
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SHOW_FALLBACK).await;
        on_main_thread(&handle, move |handle| {
            let lifecycle = handle.state::<MainWindow>();
            let ready = handle.state::<AppState>().ui_ready.load(Ordering::SeqCst);
            if !ready && lifecycle.lock().generation == generation {
                tracing::warn!("界面没有在规定时间内报告就绪，强制显示窗口（前端可能加载失败）");
                show_main(handle);
            }
        });
    });
}

/// 向主线程交接与窗口生命周期相关的操作，避免同时处理销毁、托盘点击和游戏退出。
fn on_main_thread(handle: &AppHandle, action: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = handle.clone();
    if let Err(error) = handle.run_on_main_thread(move || action(&app)) {
        tracing::warn!(%error, "窗口操作没有送到主线程");
    }
}

pub fn release_main(handle: &AppHandle, game: u64) {
    // 目前 Lumi 发布 Windows 安装包；要能点托盘图标重新打开，才可以释放主窗口。
    if !cfg!(windows) {
        return;
    }
    on_main_thread(handle, move |handle| {
        let Some(window) = handle.get_webview_window(MAIN_WINDOW) else {
            return;
        };
        let main = handle.state::<MainWindow>();
        if main.lock().keep_running() || !handle.state::<AppState>().ui_ready.load(Ordering::SeqCst)
        {
            return;
        }
        if let Err(error) = show_playing_tray(handle) {
            tracing::warn!(%error, "托盘图标无法显示，保留启动器界面");
            return;
        }
        if let Err(error) = handle.save_window_state(saved_state_flags()) {
            tracing::warn!(%error, "释放界面前保存窗口状态失败");
        }
        if !main.lock().begin_release(game) {
            return;
        }
        crate::shortcuts::pause_requests(handle);
        crate::backups::pause_prompts(handle);
        if let Err(error) = window.destroy() {
            main.lock().finish_rebuild(true);
            hide_playing_tray(handle);
            crate::shortcuts::resume_requests(handle);
            crate::backups::resume_prompts(handle);
            tracing::warn!(%error, "释放启动器界面失败");
        }
    });
}

fn show_playing_tray(handle: &AppHandle) -> tauri::Result<()> {
    if let Some(tray) = handle.tray_by_id(PLAYING_TRAY) {
        return tray.set_visible(true);
    }
    let icon = handle
        .default_window_icon()
        .ok_or(tauri::Error::AssetNotFound("Lumi".to_owned()))?;
    TrayIconBuilder::with_id(PLAYING_TRAY)
        .icon(icon.clone())
        .tooltip(handle.package_info().name.clone())
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                open_main(tray.app_handle());
            }
        })
        .build(handle)?;
    Ok(())
}

fn hide_playing_tray(handle: &AppHandle) {
    if let Some(tray) = handle.tray_by_id(PLAYING_TRAY)
        && let Err(error) = tray.set_visible(false)
    {
        tracing::warn!(%error, "隐藏托盘图标失败");
    }
}

pub fn is_released(handle: &AppHandle) -> bool {
    handle
        .try_state::<MainWindow>()
        .is_some_and(|main| main.lock().keep_running())
}

pub fn open_main(handle: &AppHandle) {
    request_open(handle, None);
}

pub fn game_finished(handle: &AppHandle, game: u64) {
    request_open(handle, Some(game));
}

fn request_open(handle: &AppHandle, game: Option<u64>) {
    on_main_thread(handle, move |handle| {
        let action = handle
            .try_state::<MainWindow>()
            .map_or(OpenAction::Show, |main| main.lock().request_open(game));
        apply_open(handle, action);
    });
}

pub fn on_event(handle: &AppHandle, event: RunEvent) {
    match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } if is_released(handle) => {
            // 销毁最后一个窗口会触发隐式退出，托盘模式中保留任务和游戏会话。
            api.prevent_exit();
        }
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } if label == MAIN_WINDOW => {
            let main = handle.state::<MainWindow>();
            let action = main.lock().window_destroyed();
            apply_open(handle, action);
        }
        _ => {}
    }
}

fn apply_open(handle: &AppHandle, action: OpenAction) {
    match action {
        OpenAction::Show => show_main(handle),
        OpenAction::Wait => {}
        OpenAction::Rebuild => {
            handle
                .state::<AppState>()
                .ui_ready
                .store(false, Ordering::SeqCst);
            let handle = handle.clone();
            // Windows 上从同步命令或事件处理器创建 WebView2 窗口可能死锁。
            tauri::async_runtime::spawn_blocking(move || {
                let result = rebuild_main(&handle);
                on_main_thread(&handle, move |handle| {
                    let main = handle.state::<MainWindow>();
                    main.lock().finish_rebuild(result.is_ok());
                    match result {
                        Ok(()) => {
                            hide_playing_tray(handle);
                            spawn_show_fallback(handle);
                            tracing::info!("启动器界面已重建");
                        }
                        Err(error) => tracing::error!(%error, "重建启动器界面失败，可从托盘再试"),
                    }
                });
            });
        }
    }
}

fn rebuild_main(handle: &AppHandle) -> tauri::Result<()> {
    let config = handle
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == MAIN_WINDOW)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let window = WebviewWindowBuilder::from_config(handle, config)?.build()?;
    let backdrop = apply_backdrop(&window);
    *handle
        .state::<AppState>()
        .backdrop
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = backdrop;
    Ok(())
}
