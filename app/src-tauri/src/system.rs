//! 这台电脑的内存、Java、显卡。
//!
//! - 内存：设置里的内存条、启动前检查、谁占着内存、整理内存（以管理员身份再开一次自己，只做这一件事）
//! - Java：列表、手动添加、每个大版本选默认、删除启动器下载的
//! - 显卡：设置里关掉「高性能显卡」时撤回我们写过的 Windows 显卡设置
//!
//! 界面只传 Java 的 ID，路径都在这里解析。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use launcher_core::java::{self, JavaContext, JavaInstall, JavaOrigin, JavaOverview, JavaRequest};
use launcher_core::jobs::ProgressUnit;
use launcher_core::launch::MemoryChoice;
use launcher_core::launch::gpu;
use launcher_core::launch::memory::{self, MemoryCheck, MemoryStatus, ProgramMemory};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::game::{Pick, any_game_running, pick, report};
use crate::state::AppState;

/// 以管理员身份再开一次启动器时带的参数：只整理内存，然后退出
pub const CLEAN_MEMORY_ARG: &str = "--clean-memory";
/// 「谁占着内存」最多列几个
const PROGRAM_LIMIT: usize = 6;
/// 等管理员确认 + 整理最多等多久（用户可能盯着确认框想一会儿）
const CLEAN_TIMEOUT: Duration = Duration::from_secs(120);

/// 整理内存进程的退出码
const EXIT_OK: u32 = 0;
const EXIT_NOT_ADMIN: u32 = 2;
const EXIT_FAILED: u32 = 3;
const EXIT_UNSUPPORTED: u32 = 4;

/// 同一时间只整理一次
static CLEANING: AtomicBool = AtomicBool::new(false);

/// 以管理员身份被再开一次时走这里：整理内存，返回退出码（不开窗口、不写日志文件）。
pub fn clean_memory_helper() -> i32 {
    let code = match winmem::trim() {
        Ok(()) => EXIT_OK,
        Err(winmem::TrimError::NotAdmin) => EXIT_NOT_ADMIN,
        Err(winmem::TrimError::Failed(code)) => {
            eprintln!("内存优化失败：{code:#x}");
            EXIT_FAILED
        }
        Err(winmem::TrimError::Unsupported) => EXIT_UNSUPPORTED,
    };
    i32::try_from(code).unwrap_or(1)
}

/// 这个实例这次要按什么给内存：实例单独设了就用实例的，没设跟全局
fn memory_choice(
    state: &AppState,
    instance: Option<&launcher_core::instance::Instance>,
) -> MemoryChoice {
    let global = state.settings.get().game;
    let overrides = instance
        .map(|i| i.config.overrides.clone())
        .unwrap_or_default();
    MemoryChoice::from_resolved(&launcher_core::instance::resolve(&global, &overrides))
}

/// 设置里内存条的数字（每隔两秒读一次）。读不到电脑内存时为空。
#[tauri::command]
#[specta::specta]
pub fn memory_status() -> Option<MemoryStatus> {
    memory::status()
}

/// 启动前检查：这次想给游戏的内存现在空得出来吗。读不到电脑内存时为空（不拦）。
#[tauri::command]
#[specta::specta]
pub fn memory_check(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Option<MemoryCheck>, AppError> {
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?;
    let game_dir = match &instance {
        Some(instance) => instance.game_dir(&game)?,
        None => game.root().to_path_buf(),
    };
    let recommended = instance
        .as_ref()
        .and_then(|i| i.config.recommended_memory_mb);
    let check = memory::check(
        memory_choice(&state, instance.as_ref()),
        &game_dir,
        recommended,
    );
    if let Some(check) = check.filter(|c| !c.enough) {
        tracing::info!(
            instance = %instance_id,
            wanted_mb = check.wanted_mb,
            spare_mb = check.spare_mb,
            "启动前检查：空闲内存不够"
        );
    }
    Ok(check)
}

/// 占内存最多的、带窗口的程序（只在内存不够的提示里用）。
#[tauri::command]
#[specta::specta]
pub async fn memory_programs() -> Vec<ProgramMemory> {
    match tauri::async_runtime::spawn_blocking(|| memory::top_programs(PROGRAM_LIMIT)).await {
        Ok(programs) => programs,
        Err(error) => {
            tracing::warn!(%error, "读进程列表的任务异常结束");
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCleanResult {
    /// 腾出了多少（整理后的「可用」减去整理前的，没变多时为 0）
    pub freed_mb: u32,
    /// 整理后的「可用」
    pub available_mb: u32,
}

struct CleaningGuard;

impl Drop for CleaningGuard {
    fn drop(&mut self) {
        CLEANING.store(false, Ordering::SeqCst);
    }
}

/// 内存优化：以管理员身份再开一次启动器（Windows 会弹出管理员确认，每次都问），整理完对比前后的「可用」。
#[tauri::command]
#[specta::specta]
pub async fn memory_clean() -> Result<MemoryCleanResult, AppError> {
    if CLEANING.swap(true, Ordering::SeqCst) {
        return Err(AppError::new("memory.clean_busy", "正在优化内存，请稍等"));
    }
    let _guard = CleaningGuard;
    let exe = std::env::current_exe()
        .map_err(|e| AppError::new("memory.clean_failed", "内存优化失败").with_detail(e))?;
    let before = memory::system_memory().map_or(0, |m| m.available_mb);
    tracing::info!(available_mb = before, "开始内存优化（等管理员确认）");
    let result = tauri::async_runtime::spawn_blocking(move || {
        winmem::run_elevated(&exe, &[CLEAN_MEMORY_ARG], CLEAN_TIMEOUT)
    })
    .await
    .map_err(|e| AppError::new("memory.clean_failed", "内存优化失败").with_detail(e))?;
    match result {
        Ok(EXIT_OK) => {}
        Ok(EXIT_NOT_ADMIN) => {
            return Err(AppError::new(
                "memory.clean_not_admin",
                "没拿到管理员权限，没能优化",
            ));
        }
        Ok(code) => {
            return Err(AppError::new("memory.clean_failed", "内存优化失败")
                .with_detail(format!("退出码 {code}")));
        }
        Err(winmem::ElevateError::Cancelled) => {
            tracing::info!("用户没给管理员权限，没做内存优化");
            return Err(AppError::new(
                "memory.clean_cancelled",
                "没给管理员权限，已取消",
            ));
        }
        Err(winmem::ElevateError::Timeout) => {
            return Err(AppError::new(
                "memory.clean_failed",
                "内存优化太久没结束，先不等了",
            ));
        }
        Err(error) => {
            return Err(AppError::new("memory.clean_failed", "内存优化失败")
                .with_detail(format!("{error:?}")));
        }
    }
    let after = memory::system_memory().map_or(0, |m| m.available_mb);
    let freed = after.saturating_sub(before);
    tracing::info!(before, after, freed, "内存优化完成");
    Ok(MemoryCleanResult {
        freed_mb: u32::try_from(freed).unwrap_or(u32::MAX),
        available_mb: u32::try_from(after).unwrap_or(u32::MAX),
    })
}

// ---------- Java ----------

/// 扫描能用的 Java（读文件和注册表，放到后台线程）
async fn scan(state: &AppState) -> Vec<JavaInstall> {
    let locator = state.java_locator();
    match tauri::async_runtime::spawn_blocking(move || locator.scan()).await {
        Ok(javas) => javas,
        Err(error) => {
            tracing::warn!(%error, "扫描 Java 的任务异常结束");
            Vec::new()
        }
    }
}

async fn overview(state: &AppState) -> JavaOverview {
    let installs = scan(state).await;
    java::overview(&installs, &state.java_prefs.get(), state.ignore_system_java)
}

fn not_found() -> AppError {
    AppError::new("java.not_found", "找不到这个 Java 了，请刷新列表")
}

fn save_failed(error: impl std::fmt::Display) -> AppError {
    AppError::new("java.save_failed", "保存 Java 设置失败").with_detail(error)
}

/// 设置里的 Java 列表。
#[tauri::command]
#[specta::specta]
pub async fn java_list(state: State<'_, AppState>) -> Result<JavaOverview, AppError> {
    Ok(overview(&state).await)
}

/// 手动添加一个 Java：在后端弹出选文件窗口（选 `bin` 里的 java.exe）。用户关掉窗口时返回空。
#[tauri::command]
#[specta::specta]
pub async fn java_add(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<JavaOverview>, AppError> {
    let Some(picked) = pick(&app, Pick::Java).await? else {
        return Ok(None);
    };
    let home = java::home_of(&picked);
    let checked = home.clone();
    let found =
        tauri::async_runtime::spawn_blocking(move || java::inspect(&checked, JavaOrigin::Manual))
            .await
            .ok()
            .flatten();
    let Some(found) = found else {
        return Err(AppError::new(
            "java.invalid",
            "这不是能用的 Java。请选 Java 文件夹里 bin 下面的 java.exe",
        )
        .with_detail(home.display()));
    };
    state
        .java_prefs
        .update(|p| p.add_manual(home.clone()))
        .map_err(save_failed)?;
    tracing::info!(home = %home.display(), version = %found.version, "手动添加了 Java");
    Ok(Some(overview(&state).await))
}

/// 给一个大版本选默认的 Java；`id` 为空时变回自动挑。
#[tauri::command]
#[specta::specta]
pub async fn java_set_default(
    state: State<'_, AppState>,
    major: u32,
    id: Option<String>,
) -> Result<JavaOverview, AppError> {
    let home: Option<PathBuf> = match id {
        None => None,
        Some(id) => {
            let installs = scan(&state).await;
            let install = installs
                .into_iter()
                .find(|j| j.id() == id && j.major == major)
                .ok_or_else(not_found)?;
            Some(install.home)
        }
    };
    state
        .java_prefs
        .update(|p| p.set_default(major, home))
        .map_err(save_failed)?;
    Ok(overview(&state).await)
}

/// 「安装推荐版本」：下载这个大版本的 Mojang 官方 Java（和官方启动器用的是同一份，放在游戏目录的 `.mcl-java`），
/// 电脑上已经有别的 Java 也照样下。后台任务，进度在任务中心；返回任务 ID，界面等它结束后刷新列表。
/// 已经下过的文件校验通过就跳过，所以重复点只会很快地检查一遍。
#[tauri::command]
#[specta::specta]
pub fn java_install(
    app: AppHandle,
    state: State<'_, AppState>,
    major: u32,
) -> Result<String, AppError> {
    let label = format!("下载 Java {major}");
    let job = state.jobs.start_with(
        format!("java:{major}"),
        label.clone(),
        ProgressUnit::Bytes,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::Java,
            None,
            None,
            Some(&crate::downloads::Retry::Java { major }),
        ),
    )?;
    let id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let downloader = crate::game::downloader_for_job(
            &state,
            launcher_core::net::DownloadPriority::Interactive,
            &job,
        );
        let locator = state.java_locator();
        let cancel = job.cancel_token();
        let ctx = JavaContext {
            locator: &locator,
            client: &state.http_client(),
            downloader: &downloader,
            cancel: &cancel,
        };
        let request = JavaRequest {
            major,
            component: None,
        };
        match java::runtime::install(&ctx, &request, |p| report(&job, &label, &p)).await {
            Ok(found) => {
                tracing::info!(major, version = %found.version, "装好了推荐的 Java");
                job.succeed();
            }
            Err(_) if cancel.is_cancelled() => job.cancelled(),
            Err(error) => {
                let app_err = state.capture_app_error(&job, &label, error);
                state.finish_failed_job(job, app_err.message);
            }
        }
    });
    Ok(id)
}

/// 不再列出一个手动添加的 Java（不删文件）。
#[tauri::command]
#[specta::specta]
pub async fn java_remove(state: State<'_, AppState>, id: String) -> Result<JavaOverview, AppError> {
    let prefs = state.java_prefs.get();
    let home = prefs
        .manual
        .iter()
        .find(|home| java::java_id(home) == id)
        .cloned()
        .ok_or_else(not_found)?;
    state
        .java_prefs
        .update(|p| p.forget(&home))
        .map_err(save_failed)?;
    tracing::info!(home = %home.display(), "移除了手动添加的 Java");
    Ok(overview(&state).await)
}

/// 删掉一个启动器下载的 Java（真的删文件；需要时启动器会再下载）。游戏开着时不删。
#[tauri::command]
#[specta::specta]
pub async fn java_delete(state: State<'_, AppState>, id: String) -> Result<JavaOverview, AppError> {
    if any_game_running(&state) {
        return Err(AppError::new(
            "java.busy",
            "游戏正在运行，请先退出游戏再删除 Java",
        ));
    }
    let installs = scan(&state).await;
    let install = installs
        .into_iter()
        .find(|j| j.id() == id)
        .ok_or_else(not_found)?;
    let locator = state.java_locator();
    let target = install.clone();
    let deleted = tauri::async_runtime::spawn_blocking(move || locator.delete_downloaded(&target))
        .await
        .map_err(|e| AppError::new("java.delete_failed", "删除 Java 失败").with_detail(e))?;
    match deleted {
        Ok(()) => {}
        Err(error @ java::JavaError::Io { .. }) => {
            tracing::warn!(%error, "删除 Java 失败");
            return Err(AppError::new(
                "java.delete_failed",
                "删除 Java 失败：有文件正被别的程序用着，关掉它再试",
            )
            .with_detail(format!("{error:?}")));
        }
        Err(error) => return Err(error.into()),
    }
    state
        .java_prefs
        .update(|p| p.forget(&install.home))
        .map_err(save_failed)?;
    Ok(overview(&state).await)
}

// ---------- 显卡 ----------

/// 启动前：设置开着时让这次用的 Java 用高性能显卡。失败只记日志，不耽误启动。
pub fn prefer_gpu(state: &AppState, java: &std::path::Path) {
    if !state.settings.get().game.high_performance_gpu {
        return;
    }
    if let Err(error) = gpu::prefer_high_performance(java, &state.gpu_ledger) {
        tracing::warn!(%error, "设置高性能显卡失败");
    }
}

/// 设置里关掉「高性能显卡」：撤回我们写过的。失败只记日志（下次关的时候再试）。
pub fn undo_gpu(state: &AppState) {
    if let Err(error) = gpu::undo_all(&state.gpu_ledger) {
        tracing::warn!(%error, "撤回高性能显卡设置失败");
    }
}
