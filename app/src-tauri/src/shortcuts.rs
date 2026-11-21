//! 桌面快捷方式：实例、世界的「⋮」里「创建桌面快捷方式」，双击直接进那个世界。
//!
//! 快捷方式运行的是启动器自己，带 `--play <实例> [--world <存档文件夹>]`：
//! - 启动器没开：窗口最小化在任务栏（任务栏图标上有进度），界面一就绪就开始启动；出错才弹出来
//! - 启动器开着：单实例插件把参数转过来，发 [`ShortcutPlay`] 给界面，直接启动，不把窗口拉到前面

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use launcher_core::saves;
use launcher_core::shortcut::{self, PlayRequest, Shortcut, ShortcutError};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::events::emit;
use crate::game::load_instance;
use crate::state::AppState;
use crate::window::{MAIN_WINDOW, is_released, open_main, show_main};

/// 启动时带来的 `--play`（界面就绪后取走）
#[derive(Default)]
pub struct PendingPlay {
    inbox: Mutex<PlayInbox>,
    /// 这次是从快捷方式启动的：窗口最小化显示
    start_minimized: AtomicBool,
}

#[derive(Default)]
struct PlayInbox {
    listening: bool,
    requests: Vec<PlayRequest>,
}

impl PendingPlay {
    pub fn from_args() -> Self {
        let request = shortcut::parse_play_args(std::env::args());
        if let Some(request) = &request {
            tracing::info!(instance = %request.instance_id, world = ?request.world, "从桌面快捷方式启动");
        }
        Self {
            start_minimized: AtomicBool::new(request.is_some()),
            inbox: Mutex::new(PlayInbox {
                requests: request.into_iter().collect(),
                ..PlayInbox::default()
            }),
        }
    }

    fn resume(&self) -> Vec<PlayRequest> {
        let mut inbox = self
            .inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inbox.listening = true;
        std::mem::take(&mut inbox.requests)
    }
}

/// 开发版的快捷方式带的 `--game-dir <测试游戏目录>`（正式版不认，永远是 `None`）。
pub fn dev_game_dir() -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let args: Vec<String> = std::env::args().collect();
    let at = args.iter().position(|a| a == DEV_GAME_DIR_ARG)?;
    args.get(at + 1).map(std::path::PathBuf::from)
}

const DEV_GAME_DIR_ARG: &str = "--game-dir";

/// 启动器已经开着时又双击了快捷方式。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ShortcutPlay(pub PlayRequest);

/// 单实例插件收到第二次启动的参数：带 `--play` 的交给界面去启动，别的照旧把窗口拉到前面。
pub fn on_second_launch(app: &AppHandle, argv: &[String]) {
    if crate::drops::shell_open(app, argv) {
        open_main(app);
        return;
    }
    match shortcut::parse_play_args(argv) {
        Some(request) => {
            tracing::info!(instance = %request.instance_id, world = ?request.world, "启动器开着时双击了快捷方式");
            let Some(pending) = app.try_state::<PendingPlay>() else {
                return;
            };
            let mut inbox = pending
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inbox.listening {
                emit(app, ShortcutPlay(request));
            } else {
                inbox.requests.push(request);
                drop(inbox);
                if is_released(app) {
                    open_main(app);
                }
            }
        }
        None => open_main(app),
    }
}

pub fn pause_requests(app: &AppHandle) {
    crate::drops::pause_shell(app);
    app.state::<PendingPlay>()
        .inbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .listening = false;
}

pub fn resume_requests(app: &AppHandle) {
    for request in app.state::<PendingPlay>().resume() {
        emit(app, ShortcutPlay(request));
    }
}

/// 装好游戏事件监听后取走待处理的 `--play`，并恢复实时推送。
#[tauri::command]
#[specta::specta]
pub fn shortcut_take(app: AppHandle, pending: State<'_, PendingPlay>) -> Option<PlayRequest> {
    let mut requests = pending.resume().into_iter();
    let first = requests.next();
    for request in requests {
        emit(&app, ShortcutPlay(request));
    }
    crate::backups::resume_prompts(&app);
    first
}

/// 在桌面上建快捷方式，返回快捷方式的名字（「生存 · 新的世界」）。同名的换成新的。
#[tauri::command]
#[specta::specta]
pub fn shortcut_create(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    world: Option<String>,
) -> Result<String, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let instance_dir = instance.game_dir(&state.game())?;
    let mut icon = None;
    let name = match &world {
        Some(folder) => {
            let dir = saves::world_dir(&instance_dir, folder)?;
            if !dir.join("level.dat").is_file() {
                return Err(AppError::new(
                    "shortcut.no_world",
                    "找不到这个世界，可能已经被删除",
                ));
            }
            let world_name = saves::world_name(&instance_dir, folder);
            let name = shortcut::file_name(&format!("{} · {world_name}", instance.config.name));
            let png = dir.join("icon.png");
            if png.is_file() {
                let icons = app.path().app_data_dir()?.join("shortcut-icons");
                std::fs::create_dir_all(&icons).map_err(|e| failure(&e))?;
                let ico = icons.join(format!("{name}.ico"));
                match shortcut::world_icon(&png, &ico) {
                    Ok(()) => icon = Some(ico),
                    // 图标坏了就用启动器的图标，不挡着建快捷方式
                    Err(error) => tracing::warn!(%error, "世界图标转不了，用启动器的图标"),
                }
            }
            name
        }
        None => shortcut::file_name(&instance.config.name),
    };
    let exe = std::env::current_exe().map_err(|e| failure(&e))?;
    let working_dir = exe
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let desktop = app.path().desktop_dir()?;
    let path = desktop.join(format!("{name}.lnk"));
    let mut args = shortcut::play_args(&PlayRequest {
        instance_id: instance.id.clone(),
        world: world.clone(),
    });
    // 开发版：双击时没有 MCL_GAME_DIR，把测试游戏目录写进参数，不然会去默认目录找实例
    if cfg!(debug_assertions)
        && let Some(dir) = std::env::var_os("MCL_GAME_DIR")
    {
        args.extend([
            DEV_GAME_DIR_ARG.to_owned(),
            dir.to_string_lossy().into_owned(),
        ]);
    }
    let description = match &world {
        Some(_) => format!("进入「{name}」"),
        None => format!("启动「{name}」"),
    };
    shortcut::write_lnk(
        &path,
        &Shortcut {
            target: &exe,
            args: &args,
            working_dir: &working_dir,
            icon: icon.as_deref(),
            description: &description,
        },
    )?;
    tracing::info!(instance = %instance.id, world = ?world, shortcut = %name, "建好了桌面快捷方式");
    Ok(name)
}

fn failure(error: &std::io::Error) -> AppError {
    tracing::error!(%error, "建快捷方式失败");
    AppError::new("shortcut.failed", "建快捷方式失败").with_detail(error.to_string())
}

impl From<ShortcutError> for AppError {
    fn from(error: ShortcutError) -> Self {
        tracing::error!(%error, "建快捷方式失败");
        AppError::new("shortcut.failed", error.to_string())
    }
}

/// 任务栏图标上的进度：`None` 不显示，小于 0 是一直转的那种，0 到 1 是进度。
#[tauri::command]
#[specta::specta]
pub fn window_progress(app: AppHandle, progress: Option<f64>) {
    use tauri::window::{ProgressBarState, ProgressBarStatus};
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let state = match progress {
        None => ProgressBarState {
            status: Some(ProgressBarStatus::None),
            progress: None,
        },
        Some(p) if p < 0.0 => ProgressBarState {
            status: Some(ProgressBarStatus::Indeterminate),
            progress: None,
        },
        Some(p) => ProgressBarState {
            status: Some(ProgressBarStatus::Normal),
            progress: Some((p.clamp(0.0, 1.0) * 100.0).round() as u64),
        },
    };
    if let Err(error) = window.set_progress_bar(state) {
        tracing::debug!(%error, "设置任务栏进度失败");
    }
}

/// 把窗口拉到前面（快捷方式启动出错时，界面要让玩家看到）。
#[tauri::command]
#[specta::specta]
pub fn window_show(app: AppHandle) {
    show_main(&app);
}

/// 界面就绪时：从快捷方式启动的只最小化显示（出现在任务栏），不弹到前面。
pub fn show_on_ready(app: &AppHandle) {
    let minimized = app
        .state::<PendingPlay>()
        .start_minimized
        .swap(false, Ordering::SeqCst);
    if !minimized {
        show_main(app);
        return;
    }
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => {
            // 隐藏着的窗口只最小化不会出现在任务栏：先最小化再显示（显示出来还是最小化的，不抢到前面）
            if let Err(error) = window.minimize().and_then(|()| window.show()) {
                tracing::warn!(%error, "最小化显示失败，照常显示");
                show_main(app);
            }
        }
        None => tracing::error!("找不到主窗口"),
    }
}
