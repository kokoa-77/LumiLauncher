//! 实例自己的设置、导出整合包、复制、删除。
//!
//! 界面只传实例 ID；导出的保存位置由后端弹系统的「另存为」窗口选，界面拿不到路径。
//! 复制、删除、导出前先看这个实例是不是正忙（游戏开着、正在往里装东西），忙就不动，说清楚为什么。

use std::path::PathBuf;

use launcher_core::instance::{self, CopyOptions, InstanceOverrides};
use launcher_core::java::JavaRequest;
use launcher_core::jobs::{JobState, ProgressUnit};
use launcher_core::modpack::export::{self, ExportOptions, ExportPreview};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::game::load_instance;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSettingsView {
    pub name: String,
    pub overrides: InstanceOverrides,
    /// 这个游戏版本要的 Java 大版本（版本描述读不到时为空）
    pub required_java: Option<u32>,
    /// 跟着全局时现在会用哪个 Java（按全局的规则挑出来的 ID）；电脑上一个合适的都没有时为空（启动时会下载）
    pub auto_java_id: Option<String>,
}

async fn settings_view(
    state: &AppState,
    instance: launcher_core::instance::Instance,
) -> InstanceSettingsView {
    let source = state.meta_source_now();
    // 挂上的别家版本：版本描述在别家的 .minecraft 里
    let required_java = match launcher_core::install::load_or_fetch_version(
        &instance.files(&state.game()),
        &instance.version_id(),
        &state.http_client(),
        source,
    )
    .await
    {
        Ok(version) => Some(JavaRequest::of(&version).major),
        Err(error) => {
            tracing::warn!(instance = %instance.id, %error, "读不到版本描述，不知道要哪个 Java");
            None
        }
    };
    let auto_java_id = match required_java {
        Some(required) => {
            let locator = state.java_locator();
            match tauri::async_runtime::spawn_blocking(move || {
                let installs = locator.scan();
                locator.pick(&installs, required).map(|j| j.id())
            })
            .await
            {
                Ok(id) => id,
                Err(error) => {
                    tracing::warn!(%error, "扫描 Java 的任务异常结束");
                    None
                }
            }
        }
        None => None,
    };
    InstanceSettingsView {
        name: instance.config.name,
        overrides: instance.config.overrides,
        required_java,
        auto_java_id,
    }
}

/// 实例「设置」页签里的内容。
#[tauri::command]
#[specta::specta]
pub async fn instances_settings_get(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<InstanceSettingsView, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    Ok(settings_view(&state, instance).await)
}

/// 保存实例的名字和单独设置（只改显示名，文件夹名不变）。改完下次启动生效。
#[tauri::command]
#[specta::specta]
pub async fn instances_settings_set(
    state: State<'_, AppState>,
    instance_id: String,
    name: String,
    overrides: InstanceOverrides,
) -> Result<InstanceSettingsView, AppError> {
    let updated = instance::update(&state.game(), &instance_id, |config| {
        config.name = name;
        config.overrides = overrides;
    })?
    .ok_or_else(not_found)?;
    tracing::info!(
        instance = %instance_id,
        overrides = ?updated.config.overrides,
        "实例设置已保存"
    );
    Ok(settings_view(&state, updated).await)
}

/// 把实例挪到一个分组（`None` 或空名字是不分组）。
#[tauri::command]
#[specta::specta]
pub fn instances_set_group(
    state: State<'_, AppState>,
    instance_id: String,
    group: Option<String>,
) -> Result<(), AppError> {
    let updated = instance::update(&state.game(), &instance_id, |config| config.group = group)?
        .ok_or_else(not_found)?;
    tracing::info!(instance = %instance_id, group = ?updated.config.group, "实例换了分组");
    Ok(())
}

/// 置顶或取消置顶。
#[tauri::command]
#[specta::specta]
pub fn instances_set_pinned(
    state: State<'_, AppState>,
    instance_id: String,
    pinned: bool,
) -> Result<(), AppError> {
    instance::update(&state.game(), &instance_id, |config| {
        config.pinned_at = match (pinned, config.pinned_at) {
            (true, Some(at)) => Some(at),
            (true, None) => Some(launcher_core::now_millis() as f64),
            (false, _) => None,
        };
    })?
    .ok_or_else(not_found)?;
    tracing::info!(instance = %instance_id, pinned, "实例置顶改了");
    Ok(())
}

/// 给分组改名（`to` 为空是解散这个分组，里面的实例变成没分组）。
#[tauri::command]
#[specta::specta]
pub fn instances_rename_group(state: State<'_, AppState>, from: String, to: Option<String>) -> u32 {
    let changed = instance::rename_group(&state.game(), &from, to.as_deref());
    tracing::info!(from = %from, to = ?to, changed, "分组改名");
    u32::try_from(changed).unwrap_or(u32::MAX)
}

fn not_found() -> AppError {
    AppError::new("instance.not_found", "找不到这个实例，可能已经被删除")
}

/// 这个实例现在能不能整个复制、删除、导出：游戏开着、或者正在往里装东西（安装、更新、拖进来的、地图）时不行。
pub(crate) fn ensure_idle(
    state: &AppState,
    id: &str,
    name: &str,
    action: &str,
) -> Result<(), AppError> {
    let running = crate::game::instance_running(state, id);
    if running {
        return Err(AppError::new(
            "instance.busy",
            format!("「{name}」正在运行，先退出游戏再{action}"),
        ));
    }
    let busy_keys = [
        format!("launch:{id}"),
        format!("instance-copy:{id}"),
        format!("instance-export:{id}"),
    ];
    let busy_prefixes = [
        format!("content:{id}:"),
        format!("map-install:{id}:"),
        format!("drop:{id}:"),
    ];
    let busy = state.jobs.list().into_iter().find(|job| {
        job.state == JobState::Running
            && (busy_keys.contains(&job.key)
                || busy_prefixes.iter().any(|p| job.key.starts_with(p)))
    });
    if busy.is_none()
        && crate::game::game_dir_of(state, id).is_ok_and(|dir| state.turns.is_busy(&dir))
    {
        return Err(AppError::new(
            "instance.busy",
            format!("「{name}」正在装东西，等它做完再{action}"),
        ));
    }
    if let Some(job) = busy {
        tracing::info!(instance = %id, job = %job.key, action, "实例正忙，先不动");
        return Err(AppError::new(
            "instance.busy",
            format!("「{name}」正在{}，等它做完再{action}", job_label(&job.key)),
        ));
    }
    Ok(())
}

fn job_label(key: &str) -> &'static str {
    if key.starts_with("launch:") {
        "准备启动"
    } else if key.starts_with("instance-copy:") {
        "复制"
    } else if key.starts_with("instance-export:") {
        "导出"
    } else {
        "安装内容"
    }
}

// ---------- 导出整合包 ----------

/// 导出弹窗打开时：列出会导出什么（读文件、联网查一次 Modrinth，要一两秒）。
#[tauri::command]
#[specta::specta]
pub async fn instances_export_preview(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<ExportPreview, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let plan = export::plan(&state.http_client(), &state.game(), &instance).await?;
    Ok(plan.preview())
}

/// 文件名里不能有的字符换成下划线（和实例文件夹名的规则一样）
fn file_stem(name: &str) -> String {
    instance::sanitize_id(name)
}

/// 后端弹「另存为」窗口（默认放在「下载」文件夹）。用户关掉窗口时返回空。
async fn save_dialog(app: &AppHandle, file_name: String) -> Result<Option<PathBuf>, AppError> {
    use tauri_plugin_dialog::DialogExt;
    let app = app.clone();
    let downloads = app.state::<AppState>().downloads_dir.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("导出整合包")
            .add_filter("Modrinth 整合包（.mrpack）", &["mrpack"])
            .set_file_name(file_name);
        if let Some(dir) = downloads {
            dialog = dialog.set_directory(dir);
        }
        if let Some(window) = app.get_webview_window(crate::window::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_save_file()
    })
    .await
    .map_err(|e| AppError::new("dialog.failed", "打开保存窗口失败").with_detail(e))?;
    let Some(path) = picked
        .map(|p| p.into_path())
        .transpose()
        .map_err(|e| AppError::new("dialog.failed", "选中的位置无法使用").with_detail(e))?
    else {
        return Ok(None);
    };
    // 用户把扩展名删了也补回来（别的启动器按扩展名认）
    let is_mrpack = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mrpack"));
    Ok(Some(if is_mrpack {
        path
    } else {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".mrpack");
        path.with_file_name(name)
    }))
}

/// 导出整合包：先弹「另存为」，选好后在后台导出（任务中心有进度），返回任务 ID；用户关掉保存窗口时返回空。
#[tauri::command]
#[specta::specta]
pub async fn instances_export(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    options: ExportOptions,
) -> Result<Option<String>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    ensure_idle(&state, &instance_id, &instance.config.name, "导出")?;
    let version = options.version.trim();
    let default_name = format!(
        "{}-{}.mrpack",
        file_stem(if options.name.trim().is_empty() {
            &instance.config.name
        } else {
            &options.name
        }),
        file_stem(if version.is_empty() { "1.0.0" } else { version })
    );
    let Some(dest) = save_dialog(&app, default_name).await? else {
        return Ok(None);
    };
    // 弹窗开着的时候可能开始了别的事，再看一次
    ensure_idle(&state, &instance_id, &instance.config.name, "导出")?;
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let job = state.jobs.start_with(
        format!("instance-export:{instance_id}"),
        format!("导出整合包 {file_name}"),
        ProgressUnit::Bytes,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::Local,
            Some(&instance_id),
            None,
            None,
        ),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        job.set_message("查 Modrinth 上的文件");
        let plan = match export::plan(&state.http_client(), &state.game(), &instance).await {
            Ok(plan) => plan,
            Err(error) => {
                job.fail(AppError::from(error).message);
                return;
            }
        };
        // 打包很慢：任务句柄交给后台线程，在那边报进度、结束任务
        let handle = app.clone();
        let finished = tauri::async_runtime::spawn_blocking(move || {
            let cancel = job.cancel_token();
            let written = plan.write(&options, &dest, &cancel, |p| {
                job.set_progress_detail(
                    p.done_bytes as f64,
                    Some(p.total_bytes as f64),
                    "打包".to_owned(),
                );
            });
            match written {
                Ok(summary) => {
                    handle
                        .state::<AppState>()
                        .exports
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(job.id().to_owned(), summary.path);
                    job.succeed();
                }
                Err(launcher_core::modpack::ModpackError::Cancelled) => job.cancelled(),
                Err(error) => job.fail(AppError::from(error).message),
            }
        })
        .await;
        if let Err(error) = finished {
            tracing::error!(%error, "导出整合包的任务异常结束");
        }
    });
    Ok(Some(job_id))
}

/// 导出完：在资源管理器里选中导出的文件。
#[tauri::command]
#[specta::specta]
pub fn instances_reveal_export(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let path = state
        .exports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&job_id)
        .cloned()
        .ok_or_else(|| AppError::new("export.not_found", "找不到导出的文件了"))?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| AppError::new("export.reveal_failed", "打不开所在的文件夹").with_detail(e))
}

// ---------- 复制、删除 ----------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CopyStarted {
    pub job_id: String,
    /// 复制出来的实例的 ID（复制完才有这个实例）
    pub instance_id: String,
}

/// 复制实例（后台任务，任务中心有进度）。
#[tauri::command]
#[specta::specta]
pub fn instances_copy(
    state: State<'_, AppState>,
    instance_id: String,
    name: String,
    include_worlds: bool,
) -> Result<CopyStarted, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    ensure_idle(&state, &instance_id, &instance.config.name, "复制")?;
    // 挂上的别家版本复制出来是 Lumi 自己的实例，加载器要自己装
    if instance.is_linked()
        && let Some(loader) = &instance.config.loader
        && !loader.kind.installable_for(&instance.config.game_version)
    {
        return Err(AppError::new(
            "instance.copy_unsupported",
            format!(
                "暂时无法复制 Minecraft {} 的 {} 为 Lumi 实例",
                instance.config.game_version,
                loader.kind.label()
            ),
        ));
    }
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(AppError::new("instance.name_empty", "请给实例起个名字"));
    }
    let game = state.game();
    let options = CopyOptions {
        id: instance::unique_id(&game, &name),
        name,
        include_worlds,
    };
    let job = state.jobs.start_with(
        format!("instance-copy:{instance_id}"),
        format!("复制实例 {}", options.name),
        ProgressUnit::Bytes,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::Local,
            Some(&options.id),
            None,
            None,
        ),
    )?;
    let started = CopyStarted {
        job_id: job.id().to_owned(),
        instance_id: options.id.clone(),
    };
    tauri::async_runtime::spawn(async move {
        let finished = tauri::async_runtime::spawn_blocking(move || {
            let cancel = job.cancel_token();
            let copied = instance::copy(&game, &instance_id, &options, &cancel, |p| {
                job.set_progress_detail(
                    p.done_bytes as f64,
                    Some(p.total_bytes as f64),
                    format!("复制文件 {}/{}", p.done_files, p.total_files),
                );
            });
            match copied {
                Ok(_) => job.succeed(),
                Err(launcher_core::instance::CopyError::Cancelled) => job.cancelled(),
                Err(error) => job.fail(AppError::from(error).message),
            }
        })
        .await;
        if let Err(error) = finished {
            tracing::error!(%error, "复制实例的任务异常结束");
        }
    });
    Ok(started)
}

/// 把实例移到系统回收站（存档、模组、配置都在里面，回收站里还能找回来）。
#[tauri::command]
#[specta::specta]
pub async fn instances_delete(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), AppError> {
    let instance = load_instance(&state, &instance_id)?;
    ensure_idle(&state, &instance_id, &instance.config.name, "删除")?;
    let game = state.game();
    if let Some(link) = &instance.config.link {
        // 挂上的别家版本：只从 Lumi 里移除，别家的文件不动。先记下来，下次重新扫不加回来
        state.linked.hide(&link.root, &link.version)?;
        instance::unlink(&game, &instance_id)?;
        return Ok(());
    }
    let id = instance_id.clone();
    tauri::async_runtime::spawn_blocking(move || instance::move_to_trash(&game, &id))
        .await
        .map_err(|e| AppError::new("instance.trash_failed", "移到回收站失败").with_detail(e))??;
    Ok(())
}
