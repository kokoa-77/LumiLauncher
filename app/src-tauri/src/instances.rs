//! 实例自己的设置、导出整合包、复制、删除。
//!
//! 界面只传实例 ID；导出的保存位置由后端弹系统的「另存为」窗口选，界面拿不到路径。
//! 复制、删除、导出前先看这个实例是不是正忙（游戏开着、正在往里装东西），忙就不动，说清楚为什么。

use std::path::PathBuf;

use launcher_core::install::LoaderContext;
use launcher_core::instance::{self, CopyOptions, InstanceOverrides, LoaderRef};
use launcher_core::java::JavaRequest;
use launcher_core::jobs::{JobSource, JobState, ProgressUnit};
use launcher_core::modpack::export::{
    self, ExportFormat, ExportMatchReport, ExportOptions, ExportPreview,
};
use launcher_core::upgrade::{self, UpgradeMode, UpgradeOptions, UpgradePreview, UpgradeProgress};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::game::{load_instance, phase_label, pick_java_for, report, report_loader};
use crate::state::AppState;
use crate::turns::busy_error;

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
    // 挂上的别家版本：版本描述在别家的 .minecraft 里
    let required_java = match launcher_core::install::load_or_fetch_version(
        &instance.files(&state.game()),
        &instance.version_id(),
        &state.http_client(),
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

/// 读取实例级笔记（`instances/<ID>/Lumi/notes.md`）。
#[tauri::command]
#[specta::specta]
pub fn instances_notes_get(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, AppError> {
    let _instance = load_instance(&state, &instance_id)?;
    Ok(instance::read_notes(&state.game(), &instance_id)?)
}

/// 原子保存实例级笔记（纯空白时删除文件），返回规范化后的内容。
#[tauri::command]
#[specta::specta]
pub fn instances_notes_set(
    state: State<'_, AppState>,
    instance_id: String,
    content: String,
) -> Result<String, AppError> {
    let _instance = load_instance(&state, &instance_id)?;
    let saved = instance::write_notes(&state.game(), &instance_id, &content)?;
    tracing::info!(
        instance = %instance_id,
        bytes = saved.len(),
        "实例笔记已保存"
    );
    Ok(saved)
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
        format!("instance-upgrade:{id}"),
        format!("modpack-update:{id}"),
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
    } else if key.starts_with("instance-upgrade:") {
        "切换版本"
    } else if key.starts_with("modpack-update:") {
        "更新整合包"
    } else {
        "安装内容"
    }
}

// ---------- 导出整合包 ----------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportChoice {
    /// 导出会话令牌（用于后台外链匹配、最终导出和关窗释放缓存）
    pub token: String,
    pub preview: ExportPreview,
}

/// 导出弹窗打开时：纯本地快速扫描实例文件并缓存计划，立即返回预览，不阻塞界面。
#[tauri::command]
#[specta::specta]
pub async fn instances_export_preview(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<ExportChoice, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let plan = export::scan_plan(&state.game(), &instance).await?;
    let preview = plan.preview(state.curseforge.usable());
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut cache = state
        .pending_exports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 8 {
        cache.clear();
    }
    cache.insert(
        token.clone(),
        crate::state::PendingExport {
            instance_id,
            plan,
            matched: false,
        },
    );
    Ok(ExportChoice { token, preview })
}

/// 导出弹窗打开后在后台调用：异步并行匹配 Modrinth（SHA-1）与 CurseForge（MurmurHash2）外链。
#[tauri::command]
#[specta::specta]
pub async fn instances_export_match(
    state: State<'_, AppState>,
    token: String,
) -> Result<ExportMatchReport, AppError> {
    let mut plan = {
        let cache = state
            .pending_exports
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entry) = cache.get(&token) else {
            return Err(AppError::new(
                "export.expired",
                "导出预览已过期，请重新打开导出弹窗",
            ));
        };
        entry.plan.clone()
    };
    let report = export::match_platforms(&state.http_client(), &state.curseforge, &mut plan).await;
    let mut cache = state
        .pending_exports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.get_mut(&token) {
        entry.plan = plan;
        entry.matched = true;
    }
    Ok(report)
}

/// 关闭导出弹窗时释放缓存的扫描计划。
#[tauri::command]
#[specta::specta]
pub fn instances_export_discard(state: State<'_, AppState>, token: String) {
    state
        .pending_exports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token);
}

/// 文件名里不能有的字符换成下划线（和实例文件夹名的规则一样）
fn file_stem(name: &str) -> String {
    instance::sanitize_id(name)
}

/// 后端弹「另存为」窗口（默认放在「下载」文件夹，按选定格式匹配扩展名）。用户关掉窗口时返回空。
async fn save_dialog(
    app: &AppHandle,
    file_name: String,
    format: ExportFormat,
) -> Result<Option<PathBuf>, AppError> {
    use tauri_plugin_dialog::DialogExt;
    let app = app.clone();
    let downloads = app.state::<AppState>().downloads_dir.clone();
    let ext = format.extension();
    let label = format.filter_label();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("导出整合包")
            .add_filter(label, &[ext])
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
    let has_ext = path
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(ext));
    Ok(Some(if has_ext {
        path
    } else {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".");
        name.push(ext);
        path.with_file_name(name)
    }))
}

/// 导出整合包：先弹「另存为」，选好后在后台按指定格式导出（任务中心有进度），返回任务 ID；用户关掉保存窗口时返回空。
#[tauri::command]
#[specta::specta]
pub async fn instances_export(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    token: Option<String>,
    options: ExportOptions,
) -> Result<Option<String>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    ensure_idle(&state, &instance_id, &instance.config.name, "导出")?;
    let version = options.version.trim();
    let default_name = format!(
        "{}-{}.{}",
        file_stem(if options.name.trim().is_empty() {
            &instance.config.name
        } else {
            &options.name
        }),
        file_stem(if version.is_empty() { "1.0.0" } else { version }),
        options.format.extension()
    );
    let Some(dest) = save_dialog(&app, default_name, options.format).await? else {
        return Ok(None);
    };
    // 弹窗开着的时候可能开始了别的事，再看一次
    ensure_idle(&state, &instance_id, &instance.config.name, "导出")?;

    // 取出缓存计划（若匹配仍在后台或缓存失效，在后台任务中按需补全）
    let cached = token.and_then(|key| {
        let mut map = state
            .pending_exports
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let matches_instance = map.get(&key).is_some_and(|e| e.instance_id == instance_id);
        if matches_instance {
            map.remove(&key)
        } else {
            None
        }
    });

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
    let needs_platform_match = matches!(
        options.format,
        ExportFormat::Modrinth | ExportFormat::CurseForge
    );
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let (mut plan, matched) = match cached {
            Some(entry) => (entry.plan, entry.matched),
            None => {
                job.set_message("扫描实例文件");
                match export::scan_plan(&state.game(), &instance).await {
                    Ok(plan) => (plan, false),
                    Err(error) => {
                        job.fail(AppError::from(error).message);
                        return;
                    }
                }
            }
        };
        if needs_platform_match && !matched {
            job.set_message("正在匹配 Modrinth / CurseForge 外链");
            let _ =
                export::match_platforms(&state.http_client(), &state.curseforge, &mut plan).await;
        }
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
                    let note = if summary.linked > 0 {
                        format!(
                            "已导出（外链 {} 项，内嵌 {} 项）",
                            summary.linked, summary.bundled
                        )
                    } else {
                        format!("已导出（共 {} 个文件）", summary.bundled)
                    };
                    job.set_message(note);
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

// ---------- 实例默认封面与预设 ----------

fn clean_cover_preset(raw: Option<String>) -> Option<String> {
    let value = raw?.trim().to_owned();
    let valid = !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b':' | b'-' | b'_')
        });
    valid.then_some(value)
}

/// 给实例挑选本地自定义封面（PNG / JPG），保存在 `instances/<id>/Lumi/covers/`（关联实例也不改动原启动器目录）。
#[tauri::command]
#[specta::specta]
pub async fn instances_cover_pick(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<bool, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let meta_dir = state.game().instance_dir(&instance.id)?;
    let Some(source) = crate::game::pick(&app, crate::game::Pick::Image).await? else {
        return Ok(false);
    };
    let now_ms = launcher_core::now_millis();
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::saves::cover::set_instance_custom(&meta_dir, &source, now_ms)
    })
    .await
    .map_err(|e| AppError::new("background.save_failed", "保存实例封面失败").with_detail(e))?
    .map_err(|e| AppError::new("background.invalid", e.to_string()))?;
    Ok(true)
}

/// 切换实例默认微光矢量封面预设（`<徽章>:<色板>`）；同时清除手动上传的实例封面图。
#[tauri::command]
#[specta::specta]
pub fn instances_cover_set_preset(
    state: State<'_, AppState>,
    instance_id: String,
    preset: Option<String>,
) -> Result<(), AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let game = state.game();
    let meta_dir = game.instance_dir(&instance.id)?;
    let _ = launcher_core::saves::cover::clear_instance_custom(&meta_dir);
    let cleaned = clean_cover_preset(preset);
    instance::update(&game, &instance.id, |config| {
        config.cover_preset = cleaned;
    })?;
    Ok(())
}

/// 清除实例自定义封面与预设，恢复默认自动封面。
#[tauri::command]
#[specta::specta]
pub fn instances_cover_clear(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let game = state.game();
    let meta_dir = game.instance_dir(&instance.id)?;
    let _ = launcher_core::saves::cover::clear_instance_custom(&meta_dir);
    instance::update(&game, &instance.id, |config| {
        config.cover_preset = None;
    })?;
    Ok(())
}

// ---------- 版本切换 / 升级向导 ----------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeChoice {
    /// 一次性令牌，确认执行升级时传回
    pub token: String,
    pub preview: UpgradePreview,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStarted {
    pub job_id: String,
    /// 目标实例 ID（原地模式为原 ID，克隆模式为预分配的新 ID）
    pub instance_id: String,
}

/// 预检实例切换到目标游戏版本/加载器后的模组兼容性（支持传 `request_id` 中途取消）。
#[tauri::command]
#[specta::specta]
pub async fn instances_upgrade_preview(
    state: State<'_, AppState>,
    instance_id: String,
    game_version: String,
    loader: Option<LoaderRef>,
    request_id: Option<String>,
) -> Result<UpgradeChoice, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_key = request_id.filter(|id| !id.trim().is_empty());
    if let Some(key) = &cancel_key {
        let mut map = state
            .upgrade_preview_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if map.len() >= 16 {
            map.clear();
        }
        map.insert(key.clone(), cancel.clone());
    }

    let plan_result = upgrade::preview(
        &state.game(),
        &instance,
        &state.http_client(),
        &state.curseforge,
        game_version,
        loader,
        &cancel,
    )
    .await;

    if let Some(key) = &cancel_key {
        state
            .upgrade_preview_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    let plan = plan_result?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let preview = plan.preview.clone();
    let mut cache = state
        .pending_upgrades
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(token.clone(), plan);
    Ok(UpgradeChoice { token, preview })
}

/// 取消正在进行中的版本兼容性预检请求。
#[tauri::command]
#[specta::specta]
pub fn instances_upgrade_cancel_preview(state: State<'_, AppState>, request_id: String) {
    if let Some(cancel) = state
        .upgrade_preview_cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&request_id)
    {
        cancel.cancel();
    }
}

/// 取消升级向导时释放已缓存的预检计划。
#[tauri::command]
#[specta::specta]
pub fn instances_upgrade_discard(state: State<'_, AppState>, token: String) {
    state
        .pending_upgrades
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token);
}

/// 启动实例版本切换 / 升级后台任务（支持原地事务升级与克隆后升级双模式）。
#[tauri::command]
#[specta::specta]
pub fn instances_upgrade_start(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    options: UpgradeOptions,
) -> Result<UpgradeStarted, AppError> {
    let plan = state
        .pending_upgrades
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token)
        .ok_or_else(|| AppError::new("upgrade.expired", "预检结果已过期，请重新检查版本兼容性"))?;

    let start_setup = (|| -> Result<_, AppError> {
        let instance_id = plan.preview.instance_id.clone();
        let instance = load_instance(&state, &instance_id)?;
        ensure_idle(&state, &instance_id, &instance.config.name, "切换版本")?;

        let game = state.game();
        let directory = instance.game_dir(&game)?;
        let turn = state.turns.try_take(&directory).ok_or_else(busy_error)?;

        let target_label = match &plan.preview.target_loader {
            Some(loader) => format!(
                "{} · {} {}",
                plan.preview.target_game_version,
                loader.kind.label(),
                loader.version
            ),
            None => plan.preview.target_game_version.clone(),
        };
        let (title, expected_instance_id) = match options.mode {
            UpgradeMode::InPlace => (
                format!("切换版本 {} → {target_label}", instance.config.name),
                instance_id.clone(),
            ),
            UpgradeMode::Clone => {
                let clone_name =
                    upgrade::clone_target_name(&plan.preview, options.clone_name.as_deref());
                let predicted_id = instance::unique_id(&game, &clone_name);
                (
                    format!("克隆并升级 {clone_name} → {target_label}"),
                    predicted_id,
                )
            }
        };

        let job = state.jobs.start_with(
            format!("instance-upgrade:{instance_id}"),
            title,
            ProgressUnit::Bytes,
            crate::downloads::meta(JobSource::Game, Some(&expected_instance_id), None, None),
        )?;
        Ok((game, turn, expected_instance_id, job))
    })();

    let (game, turn, expected_instance_id, job) = match start_setup {
        Ok(ok) => ok,
        Err(err) => {
            state
                .pending_upgrades
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(token, plan);
            return Err(err);
        }
    };
    let job_id = job.id().to_owned();
    let started = UpgradeStarted {
        job_id: job_id.clone(),
        instance_id: expected_instance_id,
    };

    tauri::async_runtime::spawn(async move {
        let _turn = turn;
        let state = app.state::<AppState>();
        let cancel = job.cancel_token();
        let result: Result<upgrade::UpgradeOutcome, AppError> = async {
            let dl = crate::game::downloader_for_job(
                &state,
                launcher_core::net::DownloadPriority::Batch,
                &job,
            );
            let java = if plan
                .preview
                .target_loader
                .as_ref()
                .is_some_and(|l| l.kind.uses_processor())
            {
                pick_java_for(&state, &job, &dl, &plan.preview.target_game_version).await?
            } else {
                PathBuf::new()
            };
            let work_dir = game.root().join(".mcl-tmp").join("upgrade");
            let ctx = LoaderContext {
                game: &game,
                client: &state.http_client(),
                downloader: &dl,
                java: &java,
                work_dir: &work_dir,
                cancel: &cancel,
            };
            let outcome = upgrade::execute(
                &ctx,
                &state.curseforge,
                &plan,
                &options,
                |p| match &p {
                    UpgradeProgress::Cloning(d) => {
                        job.set_step("克隆实例");
                        job.set_progress_detail(
                            d.done_bytes as f64,
                            Some(d.total_bytes as f64),
                            format!("复制文件 {}/{}", d.done_files, d.total_files),
                        );
                    }
                    UpgradeProgress::Loader(l) => report_loader(&job, l),
                    UpgradeProgress::Game(phase, d) => report(&job, phase_label(*phase), d),
                    UpgradeProgress::UpdatingMods(d) => report(&job, "升级适配模组", d),
                    UpgradeProgress::ResolvingDependencies => {
                        job.set_step("检查新增前置");
                        job.set_message("正在检查新版模组需要的前置依赖");
                    }
                    UpgradeProgress::InstallingDependencies(d) => {
                        report(&job, "补装前置依赖", d);
                    }
                    UpgradeProgress::DisablingMods => {
                        job.set_step("停用不兼容模组");
                        job.set_message("正在停用不兼容模组");
                    }
                    UpgradeProgress::Finalizing => {
                        job.set_step("保存实例配置");
                        job.set_message("正在更新版本设置");
                    }
                },
                |claim_title, manual_files| {
                    let guard =
                        crate::claims::start(&app, &state, &job, claim_title, manual_files)?;
                    let session = guard.session_arc();
                    Some(upgrade::ActiveClaim::new(session, guard))
                },
            )
            .await
            .map_err(|e| state.capture_app_error(&job, "升级实例", e))?;
            Ok(outcome)
        }
        .await;

        match result {
            Ok(outcome) => {
                let mut parts = Vec::new();
                if outcome.upgraded_mods > 0 {
                    parts.push(format!("升级 {} 个模组", outcome.upgraded_mods));
                }
                if outcome.installed_deps > 0 {
                    parts.push(format!("补装 {} 个前置", outcome.installed_deps));
                }
                if outcome.disabled_mods > 0 {
                    parts.push(format!("停用 {} 个不兼容模组", outcome.disabled_mods));
                }
                if outcome.skipped_manual > 0 {
                    parts.push(format!("{} 个需手动补件", outcome.skipped_manual));
                }
                if !parts.is_empty() {
                    job.set_message(parts.join(" · "));
                }
                job.set_instance(outcome.instance_id);
                job.succeed();
            }
            Err(error) if job.is_cancelled() => {
                tracing::info!(error = %error.message, "切换版本已取消");
                job.cancelled();
            }
            Err(error) => state.finish_failed_job(job, error.message),
        }
    });

    Ok(started)
}
