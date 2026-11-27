//! 下载中心：下载历史、重试、DoH 测试、连通性检测、补件盯的文件夹。
//!
//! 每种能重试的任务开始时带一份 [`Retry`] 配方（存在历史文件里，界面拿不到：本地整合包的配方里有路径）。
//! 「重试」按配方再开一个同样的任务，新任务接替历史里的旧记录；下载过的文件校验通过不重下，
//! 整合包导入的临时目录按 [`Retry::resume_key`] 固定，失败、取消后留着，重试时接着用。

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use launcher_core::config::{
    ConcurrencyMode, DiskWriteMode, ProxyMode, ProxySettings, SegmentMode,
};
use launcher_core::content::ContentSource;
use launcher_core::content::UpdateInfo;
use launcher_core::content::modrinth::PlanItem;
use launcher_core::instance::{LoaderKind, LoaderRef};
use launcher_core::jobs::{JobMeta, JobRegistry, JobSnapshot, JobSource, JobState, ProgressUnit};
use launcher_core::modpack;
use launcher_core::net::http::USER_AGENT;
use launcher_core::net::proxy_watch::apply_proxy_settings;
use launcher_core::net::{
    DnsProbe, DownloadError, DownloadPriority, QuickFixAction, TransferMetrics,
};
use launcher_core::paths::GameDir;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::discover::ServerJoin;
use crate::error::AppError;
use crate::state::AppState;

/// 没做完的整合包导入留多久（过了就删，重试时从头来）
const KEEP_UNFINISHED: Duration = Duration::from_secs(7 * 24 * 3600);

/// 「重试」配方：怎么把这个任务再开一次
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Retry {
    #[serde(rename_all = "camelCase")]
    CreateInstance {
        name: String,
        game_version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loader: Option<LoaderRef>,
        // 早先的任务只有 NeoForge 版本号。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        neoforge: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ContentInstall {
        instance_id: String,
        plan: Vec<PlanItem>,
    },
    #[serde(rename_all = "camelCase")]
    ContentUpdate {
        instance_id: String,
        updates: Vec<UpdateInfo>,
    },
    #[serde(rename_all = "camelCase")]
    MapInstall {
        instance_id: String,
        project_id: String,
        title: String,
    },
    #[serde(rename_all = "camelCase")]
    ModpackInstall {
        project_id: String,
        version_id: String,
        title: String,
        name: Option<String>,
        source: ContentSource,
        join_server: Option<ServerJoin>,
    },
    /// 本地整合包文件（路径只在后端）
    #[serde(rename_all = "camelCase")]
    ModpackFile {
        path: PathBuf,
        name: Option<String>,
    },
    Java {
        major: u32,
    },
}

impl Retry {
    pub fn to_value(&self) -> Option<serde_json::Value> {
        match serde_json::to_value(self) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(%error, "重试配方存不下来，这个任务不能重试");
                None
            }
        }
    }

    /// 整合包导入接着装用的键（临时目录名）；别的任务没有
    pub fn resume_key(&self) -> Option<String> {
        match self {
            Retry::ModpackInstall {
                version_id, source, ..
            } => Some(match source {
                ContentSource::Modrinth => format!("mr-{version_id}"),
                ContentSource::CurseForge => format!("cf-{version_id}"),
            }),
            Retry::ModpackFile { path, .. } => Some(modpack::file_resume_key(path)),
            _ => None,
        }
    }
}

/// 开任务时的说明：来源、实例、图标、重试配方
pub(crate) fn meta(
    source: JobSource,
    instance_id: Option<&str>,
    icon_url: Option<String>,
    retry: Option<&Retry>,
) -> JobMeta {
    JobMeta {
        source,
        instance_id: instance_id.map(str::to_owned),
        icon_url,
        retry: retry.and_then(Retry::to_value),
        transient: false,
    }
}

pub(crate) fn content_source(source: ContentSource) -> JobSource {
    match source {
        ContentSource::Modrinth => JobSource::Modrinth,
        ContentSource::CurseForge => JobSource::CurseForge,
    }
}

/// 下载历史：正在进行的在前，然后新的在前（关掉启动器再打开还在）
#[tauri::command]
#[specta::specta]
pub fn jobs_history(state: State<'_, AppState>) -> Vec<JobSnapshot> {
    state.jobs.history()
}

/// 清空下载历史（正在进行的不动）。没做完的整合包导入的临时文件一起删掉（没法重试了）
#[tauri::command]
#[specta::specta]
pub fn jobs_clear_history(app: AppHandle, state: State<'_, AppState>) {
    state.jobs.clear_finished();
    state
        .failure_receipts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    state.engine.metrics().set_diagnosis(None);
    crate::events::emit(
        &app,
        crate::events::TransferMetricsUpdated(state.engine.runtime_metrics()),
    );
    let game = state.game();
    let keep = kept_staging(&state.jobs);
    tauri::async_runtime::spawn_blocking(move || clean_imports(&game, &keep, true));
}

async fn spawn_retry_recipe(app: AppHandle, retry: Retry) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    match retry {
        Retry::CreateInstance {
            name,
            game_version,
            loader,
            neoforge,
        } => {
            let loader = loader.or_else(|| {
                neoforge.map(|version| LoaderRef {
                    kind: LoaderKind::NeoForge,
                    version,
                })
            });
            crate::game::instances_create(app.clone(), state, name, game_version, loader)
        }
        Retry::ContentInstall { instance_id, plan } => {
            crate::game::content_install(app.clone(), state, instance_id, plan)
        }
        Retry::ContentUpdate {
            instance_id,
            updates,
        } => crate::game::content_update(app.clone(), state, instance_id, updates),
        Retry::MapInstall {
            instance_id,
            project_id,
            title,
        } => crate::discover::maps_install(app.clone(), state, instance_id, project_id, title),
        Retry::ModpackInstall {
            project_id,
            version_id,
            title,
            name,
            source,
            join_server,
        } => crate::game::modpack_install(
            app.clone(),
            state,
            project_id,
            version_id,
            title,
            name,
            source,
            join_server,
        ),
        Retry::ModpackFile { path, name } => {
            if !path.is_file() {
                return Err(AppError::new(
                    "jobs.pack_missing",
                    "整合包文件不在原来的位置了，请重新导入",
                ));
            }
            crate::game::import_file(&app, path, name).await
        }
        Retry::Java { major } => crate::system::java_install(app.clone(), state, major),
    }
}

fn strip_partial_retry_prefix(mut title: &str) -> &str {
    while let Some(rest) = title.strip_prefix("补齐失败文件 (")
        && let Some((_, original)) = rest.split_once(") · ")
    {
        title = original;
    }
    title
}

fn emit_metrics_snapshot(app: &AppHandle, state: &AppState) {
    crate::events::emit(
        app,
        crate::events::TransferMetricsUpdated(state.engine.runtime_metrics()),
    );
}

/// 重试一个失败或取消的任务，返回新任务的编号。新任务接替历史里的旧记录
#[tauri::command]
#[specta::specta]
pub async fn jobs_retry(app: AppHandle, id: String) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let (job, recipe) = state
        .jobs
        .find(&id)
        .ok_or_else(|| AppError::new("jobs.not_found", "找不到这个任务，可能已经清掉了"))?;
    if !matches!(
        job.state,
        JobState::Failed | JobState::Cancelled | JobState::Interrupted
    ) {
        return Err(AppError::new(
            "jobs.not_retryable",
            "只有失败或取消的任务能重试",
        ));
    }
    let retry: Retry = recipe
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| AppError::new("jobs.not_retryable", "这个任务不能重试，请重新操作一次"))?;
    tracing::info!(job = %id, title = %job.title, "重试任务");
    let new_id = spawn_retry_recipe(app.clone(), retry).await?;
    state.jobs.remove(&id);
    state
        .failure_receipts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    state.engine.metrics().set_diagnosis(None);
    emit_metrics_snapshot(&app, &state);
    Ok(new_id)
}

/// 从下载历史里删掉一条已结束的（正在进行或已暂停的删不掉）
#[tauri::command]
#[specta::specta]
pub fn jobs_remove(app: AppHandle, state: State<'_, AppState>, id: String) -> bool {
    let removed = state.jobs.remove(&id);
    if removed {
        state
            .failure_receipts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let has_remaining_diagnosis = state
            .jobs
            .list()
            .iter()
            .any(|j| j.state == JobState::Failed && j.diagnosis.is_some());
        if !has_remaining_diagnosis {
            state.engine.metrics().set_diagnosis(None);
            emit_metrics_snapshot(&app, &state);
        }
    }
    removed
}

/// 暂停正在进行的任务（立即释放网络与磁盘并发许可，保留已下分片供恢复）。
#[tauri::command]
#[specta::specta]
pub fn jobs_pause(state: State<'_, AppState>, id: String) -> bool {
    state.jobs.pause(&id)
}

/// 恢复已暂停的任务。
#[tauri::command]
#[specta::specta]
pub fn jobs_resume(state: State<'_, AppState>, id: String) -> bool {
    state.jobs.resume(&id)
}

/// 仅重试上次失败的条目（依赖后端 `PartialFailureReceipt`；无回执时自动回退整单重试）。
#[tauri::command]
#[specta::specta]
pub async fn jobs_retry_failed(app: AppHandle, id: String) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let receipt = state
        .failure_receipts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|(r, _, _)| r.clone());

    let Some(receipt) = receipt.filter(|r| !r.failed_entries.is_empty()) else {
        return jobs_retry(app, id).await;
    };

    let (old_snap, old_retry) = state
        .jobs
        .find(&id)
        .ok_or_else(|| AppError::new("jobs.not_found", "找不到这个任务，可能已经清掉了"))?;

    let items: Vec<_> = receipt
        .failed_entries
        .iter()
        .map(|e| e.item.clone())
        .collect();
    let count = items.len();
    let follow_up: Option<Retry> = old_retry
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let base_title = strip_partial_retry_prefix(&old_snap.title);
    let job = state.jobs.start_with(
        format!("retry-failed:{id}"),
        format!("补齐失败文件 ({count}) · {base_title}"),
        ProgressUnit::Bytes,
        JobMeta {
            source: old_snap.source,
            instance_id: old_snap.instance_id.clone(),
            icon_url: old_snap.icon_url.clone(),
            retry: old_retry,
            transient: false,
        },
    )?;
    let new_id = job.id().to_owned();
    state.jobs.remove(&id);
    state
        .failure_receipts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    state.engine.metrics().set_diagnosis(None);
    emit_metrics_snapshot(&app, &state);

    let step_label = if receipt.step_label.is_empty() {
        "补齐失败文件".to_owned()
    } else {
        receipt.step_label.clone()
    };
    let app_bg = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app_bg.state::<AppState>();
        let dl = crate::game::downloader_for_job(&state, DownloadPriority::Interactive, &job);
        let cancel = job.cancel_token();
        match dl
            .download_all(&items, &cancel, |p| {
                crate::game::report(&job, &step_label, &p);
            })
            .await
        {
            Ok(_) => {
                if let Some(recipe) = follow_up
                    && let Err(err) = spawn_retry_recipe(app_bg.clone(), recipe).await
                {
                    tracing::warn!(
                        code = %err.code,
                        message = %err.message,
                        job = %job.id(),
                        "补齐失败文件后继续执行后续步骤失败"
                    );
                    job.fail(err.message);
                    return;
                }
                job.succeed();
            }
            Err(DownloadError::Cancelled) => job.cancelled(),
            Err(DownloadError::Failed(failures)) => {
                let msg = format!("仍有 {} 个文件下载失败", failures.len());
                state.fail_job_with_receipt(job, &step_label, msg, Some(&failures));
            }
        }
    });

    Ok(new_id)
}

/// 读取当前运行期网络、磁盘与限速实时状态摘要（供设置页与驾驶舱首帧显示）。
#[tauri::command]
#[specta::specta]
pub fn net_runtime_status(state: State<'_, AppState>) -> TransferMetrics {
    state.engine.runtime_metrics()
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub ok: bool,
    pub latency_ms: Option<u32>,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

/// 5 秒超时实测当前输入/选择的代理配置是否可连通官方 Mojang 端点。
#[tauri::command]
#[specta::specta]
pub async fn net_test_proxy(proxy: ProxySettings) -> Result<ProxyTestResult, AppError> {
    let normalized = proxy.normalized();
    let builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(5));
    let client = apply_proxy_settings(builder, &normalized, None)
        .map_err(|e| AppError::new("network.invalid_proxy", e.to_string()))?
        .build()
        .map_err(|e| AppError::new("network.test_failed", e.to_string()))?;

    let started = Instant::now();
    match client
        .head("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
        .send()
        .await
    {
        Ok(resp) => {
            let ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            let status = resp.status().as_u16();
            let ok = resp.status().is_success() || resp.status().is_redirection();
            Ok(ProxyTestResult {
                ok,
                latency_ms: Some(ms),
                http_status: Some(status),
                error: (!ok).then(|| format!("返回状态码 {status}")),
            })
        }
        Err(err) => Ok(ProxyTestResult {
            ok: false,
            latency_ms: None,
            http_status: None,
            error: Some(launcher_core::redact::redact(&err.to_string()).into_owned()),
        }),
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct QuickFixResult {
    pub applied: bool,
    pub message: String,
    pub open_settings_section: Option<String>,
    pub focus_setting_id: Option<String>,
    pub retried_job_id: Option<String>,
}

/// 驾驶舱诊断卡一键自愈执行器：自动调优配置，并在有可恢复失败任务时直接重跑失败项。
#[tauri::command]
#[specta::specta]
pub async fn net_apply_quick_fix(
    app: AppHandle,
    state: State<'_, AppState>,
    action: QuickFixAction,
    job_id: Option<String>,
) -> Result<QuickFixResult, AppError> {
    let mut settings = state.settings.get();
    let mut should_retry = false;
    let mut section: Option<String> = None;
    let mut focus: Option<String> = None;
    let message: String;

    match action {
        QuickFixAction::EnableDohAndRetry => {
            settings.download.doh_enabled = true;
            let saved = state.settings.set(settings)?;
            state.apply_download_settings(&saved.download);
            state.engine.metrics().set_diagnosis(None);
            should_retry = true;
            message = "已开启 DoH 防污染解析并准备重试".to_owned();
        }
        QuickFixAction::SwitchDirectAndRetry => {
            settings.download.proxy.mode = ProxyMode::Direct;
            state
                .apply_proxy(&settings.download.proxy)
                .map_err(|e| AppError::new("network.invalid_proxy", e.to_string()))?;
            let saved = state.settings.set(settings)?;
            state.apply_download_settings(&saved.download);
            state.engine.metrics().set_diagnosis(None);
            should_retry = true;
            message = "已切换为直连并重建连接池".to_owned();
        }
        QuickFixAction::SwitchOfficialCdnAndRetry => {
            state.engine.metrics().set_diagnosis(None);
            should_retry = true;
            message = "已标记故障节点避让并重试下载".to_owned();
        }
        QuickFixAction::LowerDiskPressureAndRetry => {
            settings.download.disk_write_mode = DiskWriteMode::Manual;
            settings.download.max_disk_writers = 2;
            settings.download.segment_mode = SegmentMode::Off;
            let saved = state.settings.set(settings)?;
            state.apply_download_settings(&saved.download);
            state.engine.metrics().set_diagnosis(None);
            should_retry = true;
            message = "已将并发磁盘写入降至 2 路并关闭大文件分段".to_owned();
        }
        QuickFixAction::LowerConcurrencyAndRetry => {
            settings.download.concurrency_mode = ConcurrencyMode::Manual;
            settings.download.max_concurrency = settings.download.max_concurrency.min(12);
            settings.download.segment_mode = SegmentMode::Auto;
            settings.download.max_segments_per_file =
                settings.download.max_segments_per_file.min(4);
            let saved = state.settings.set(settings)?;
            state.apply_download_settings(&saved.download);
            state.engine.metrics().set_diagnosis(None);
            should_retry = true;
            message = "已收敛全局并发与分段数，避开限流阈值".to_owned();
        }
        QuickFixAction::OpenDateTimeSettings => {
            let _ = app
                .opener()
                .open_url("ms-settings:dateandtime", None::<&str>);
            message = "已尝试打开 Windows 日期与时间设置".to_owned();
        }
        QuickFixAction::OpenStorageClean => {
            section = Some("worlds".to_owned());
            focus = Some("storage".to_owned());
            message = "已定位到存储清理设置".to_owned();
        }
        QuickFixAction::OpenProxySettings => {
            section = Some("download".to_owned());
            focus = Some("proxy".to_owned());
            message = "已定位到网络代理设置".to_owned();
        }
        QuickFixAction::OpenDiagnostics => {
            section = Some("download".to_owned());
            focus = Some("netdiag".to_owned());
            message = "已定位到网络连通性诊断".to_owned();
        }
        QuickFixAction::EnableMirrorFallbackRetry => {
            section = Some("download".to_owned());
            focus = Some("netdiag".to_owned());
            message = "已定位到网络连通性诊断".to_owned();
        }
    }

    let retried_job_id = if should_retry && let Some(jid) = job_id {
        jobs_retry_failed(app.clone(), jid).await.ok()
    } else {
        None
    };

    emit_metrics_snapshot(&app, &state);

    Ok(QuickFixResult {
        applied: true,
        message,
        open_settings_section: section,
        focus_setting_id: focus,
        retried_job_id,
    })
}

/// 正在用、或者还能重试的整合包导入的临时目录名（这些不删）
pub(crate) fn kept_staging(jobs: &JobRegistry) -> HashSet<String> {
    jobs.history()
        .into_iter()
        .filter(|job| {
            job.state == JobState::Running
                || job.state == JobState::Paused
                || (job.retryable
                    && matches!(
                        job.state,
                        JobState::Failed | JobState::Cancelled | JobState::Interrupted
                    ))
        })
        .filter_map(|job| jobs.find(&job.id))
        .filter_map(|(_, recipe)| serde_json::from_value::<Retry>(recipe?).ok())
        .filter_map(|retry| retry.resume_key())
        .filter_map(|key| {
            modpack::staging_dir(std::path::Path::new(""), &key)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect()
}

/// 清掉整合包导入的临时目录：不在 `keep` 里的删掉；`all` 为假时（启动时）还在 `keep` 里但放了太久的也删
pub(crate) fn clean_imports(game: &GameDir, keep: &HashSet<String>, all: bool) {
    let temp = game.root().join(".mcl-tmp");
    let Ok(entries) = std::fs::read_dir(&temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(modpack::STAGING_PREFIX) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > KEEP_UNFINISHED);
        if keep.contains(&name) && !(stale && !all) {
            continue;
        }
        let dir = entry.path();
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => tracing::info!(dir = %dir.display(), "已清理没做完的整合包导入"),
            Err(error) => {
                tracing::warn!(%error, dir = %dir.display(), "清理整合包导入的临时目录失败")
            }
        }
    }
}

/// 设置里的「测试 DoH」：每个 DoH 服务和系统 DNS 各问一次
#[tauri::command]
#[specta::specta]
pub async fn net_doh_test(state: State<'_, AppState>) -> Result<Vec<DnsProbe>, AppError> {
    Ok(state.doh.test().await)
}

/// 设置里的「网络连通性检测」：并行探测核心官方服务端点
#[tauri::command]
#[specta::specta]
pub async fn net_diagnostics(
    state: State<'_, AppState>,
) -> Result<Vec<launcher_core::net::EndpointProbe>, AppError> {
    let client = state.http_client();
    Ok(launcher_core::net::probe_endpoints(&client).await)
}

/// 补件时盯的文件夹（显示用）
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClaimFolderInfo {
    /// 完整路径；系统的「下载」文件夹也找不到时为空
    pub path: Option<String>,
    /// 是自己选的（不是系统的「下载」文件夹）
    pub custom: bool,
}

/// 补件时真正盯的文件夹：设置里选的（还在的话），不然系统的「下载」文件夹
pub(crate) fn claim_folder(state: &AppState) -> Option<PathBuf> {
    state
        .settings
        .get()
        .download
        .claim_folder
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| state.downloads_dir.clone())
}

fn claim_folder_info_of(state: &AppState) -> ClaimFolderInfo {
    let custom = state.settings.get().download.claim_folder.is_some();
    ClaimFolderInfo {
        path: claim_folder(state).map(|p| p.display().to_string()),
        custom,
    }
}

#[tauri::command]
#[specta::specta]
pub fn claim_folder_info(state: State<'_, AppState>) -> ClaimFolderInfo {
    claim_folder_info_of(&state)
}

/// 弹出「选择文件夹」换补件盯的文件夹；用户取消时返回 `None`
#[tauri::command]
#[specta::specta]
pub async fn claim_folder_choose(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ClaimFolderInfo>, AppError> {
    let Some(path) = crate::game::pick(&app, crate::game::Pick::Folder).await? else {
        return Ok(None);
    };
    let mut settings = state.settings.get();
    settings.download.claim_folder = Some(path.display().to_string());
    state.settings.set(settings)?;
    tracing::info!(dir = %path.display(), "补件改盯这个文件夹");
    Ok(Some(claim_folder_info_of(&state)))
}

/// 改回系统的「下载」文件夹
#[tauri::command]
#[specta::specta]
pub fn claim_folder_reset(state: State<'_, AppState>) -> Result<ClaimFolderInfo, AppError> {
    let mut settings = state.settings.get();
    settings.download.claim_folder = None;
    state.settings.set(settings)?;
    Ok(claim_folder_info_of(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_round_trip_and_know_their_resume_keys() {
        let retry = Retry::ModpackInstall {
            project_id: "p".to_owned(),
            version_id: "v1".to_owned(),
            title: "整合包".to_owned(),
            name: None,
            source: ContentSource::Modrinth,
            join_server: None,
        };
        let value = retry.to_value().unwrap();
        assert_eq!(value["kind"], "modpackInstall");
        let back: Retry = serde_json::from_value(value).unwrap();
        assert_eq!(back, retry);
        assert_eq!(retry.resume_key().as_deref(), Some("mr-v1"));
        assert_eq!(Retry::Java { major: 21 }.resume_key(), None);
    }

    #[test]
    fn cleaning_keeps_resumable_imports_until_they_are_stale() {
        let root =
            std::env::temp_dir().join(format!("mcl-clean-{}", uuid::Uuid::new_v4().simple()));
        let game = GameDir::new(&root);
        let temp = root.join(".mcl-tmp");
        for name in ["import-mr-a", "import-mr-b", "other"] {
            std::fs::create_dir_all(temp.join(name)).unwrap();
        }
        let keep: HashSet<String> = ["import-mr-a".to_owned()].into();
        clean_imports(&game, &keep, false);
        assert!(temp.join("import-mr-a").exists(), "还能重试的留着");
        assert!(!temp.join("import-mr-b").exists(), "没人要的删掉");
        assert!(temp.join("other").exists(), "别的临时文件不归这里管");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn strip_partial_retry_prefix_avoids_nesting() {
        assert_eq!(
            strip_partial_retry_prefix("安装整合包 XYZ"),
            "安装整合包 XYZ"
        );
        assert_eq!(
            strip_partial_retry_prefix("补齐失败文件 (3) · 安装整合包 XYZ"),
            "安装整合包 XYZ"
        );
        assert_eq!(
            strip_partial_retry_prefix("补齐失败文件 (1) · 补齐失败文件 (3) · 安装整合包 XYZ"),
            "安装整合包 XYZ"
        );
    }
}
