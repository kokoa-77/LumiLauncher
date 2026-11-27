//! 整合包一键无损更新与一键回滚命令（Modrinth / CurseForge 在线更新 + 本地 `.mrpack` / `.zip` 更新）。

use std::path::PathBuf;

use launcher_core::content::detail::BodyFormat;
use launcher_core::content::{ContentSource, modrinth};
use launcher_core::install::LoaderContext;
use launcher_core::jobs::{JobSource, ProgressUnit};
use launcher_core::modpack::remote::{self, PackVersion};
use launcher_core::modpack::update::{
    self, PackRollbackOutcome, PackUpdateOptions, PackUpdateOutcome, PackUpdatePreview,
    PackUpdateProgress,
};
use launcher_core::modpack::{self, PackOrigin};
use launcher_core::upgrade::ActiveClaim;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::game::{
    Pick, curseforge_id, downloader, inspect_file, load_instance, phase_label, pick, pick_java_for,
    report, report_loader,
};
use crate::instances::ensure_idle;
use crate::state::AppState;
use crate::turns::busy_error;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PackUpdateChoice {
    /// 一次性令牌，确认执行更新时传回
    pub token: String,
    pub preview: PackUpdatePreview,
}

fn pack_platform(kind: &str) -> Result<ContentSource, AppError> {
    match kind {
        "mrpack" => Ok(ContentSource::Modrinth),
        "curseforge" => Ok(ContentSource::CurseForge),
        _ => Err(AppError::new(
            "modpack.unsupported_source",
            "这个整合包不是来自在线平台，请选择本地整合包文件进行更新",
        )),
    }
}

/// 查询当前受管整合包在平台（Modrinth / CurseForge）上的全部可用版本（新的在前）。
#[tauri::command]
#[specta::specta]
pub async fn modpack_update_versions(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Vec<PackVersion>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let source = instance
        .config
        .source
        .as_ref()
        .ok_or_else(|| AppError::new("modpack.not_managed", "这个实例不是从整合包安装的"))?;
    let project_id = source.project_id.as_deref().ok_or_else(|| {
        AppError::new(
            "modpack.no_project_id",
            "这个整合包是从本地文件导入的，没有绑定平台项目，请使用「从本地文件更新」",
        )
    })?;
    match pack_platform(&source.kind)? {
        ContentSource::Modrinth => Ok(remote::versions(&state.http_client(), project_id).await?),
        ContentSource::CurseForge => {
            Ok(remote::curseforge_versions(&state.curseforge, curseforge_id(project_id)?).await?)
        }
    }
}

/// 从在线平台下载选定版本到暂存目录，完成三方比对并返回更新预览。
#[tauri::command]
#[specta::specta]
pub async fn modpack_update_preview_remote(
    state: State<'_, AppState>,
    instance_id: String,
    version_id: String,
) -> Result<PackUpdateChoice, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    if instance.config.link.is_some() {
        return Err(AppError::new(
            "modpack.linked_instance",
            "外部关联实例不能直接更新整合包，请先「复制成 Lumi 的实例」后再更新",
        ));
    }
    let source = instance
        .config
        .source
        .as_ref()
        .ok_or_else(|| AppError::new("modpack.not_managed", "这个实例不是从整合包安装的"))?;
    let project_id = source
        .project_id
        .as_deref()
        .ok_or_else(|| AppError::new("modpack.no_project_id", "该整合包未绑定平台项目"))?
        .to_owned();
    let platform = pack_platform(&source.kind)?;

    let game = state.game();
    let temp_dir = game
        .root()
        .join(".mcl-tmp")
        .join(format!("pack-update-dl-{}", uuid::Uuid::new_v4().simple()));
    let dl = downloader(&state);
    let cancel = tokio_util::sync::CancellationToken::new();

    let (pack_path, changelog, changelog_format) = match platform {
        ContentSource::Modrinth => {
            let client = state.http_client();
            let changelog = modrinth::fetch_version(&client, &version_id)
                .await
                .ok()
                .and_then(|v| v.changelog)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            let path =
                remote::download(&client, &dl, &version_id, &temp_dir, &cancel, |_| {}).await?;
            (path, changelog, BodyFormat::Markdown)
        }
        ContentSource::CurseForge => {
            let mod_id = curseforge_id(&project_id)?;
            let file_id = curseforge_id(&version_id)?;
            let changelog = state
                .curseforge
                .file_changelog(mod_id, file_id)
                .await
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            let path = remote::curseforge_download(
                &state.curseforge,
                &dl,
                file_id,
                &temp_dir,
                &cancel,
                |_| {},
            )
            .await?;
            (path, changelog, BodyFormat::Html)
        }
    };

    let inspected = inspect_file(&state, pack_path).await;
    let mut target_plan = match inspected {
        Ok(plan) => plan,
        Err(error) => {
            modpack::discard_staging(&temp_dir).await;
            return Err(error);
        }
    };
    target_plan.origin = Some(PackOrigin {
        project_id,
        version_id,
    });

    let update_plan = match update::preview(
        &game,
        &instance,
        target_plan,
        changelog,
        changelog_format,
        Some(temp_dir.clone()),
    )
    .await
    {
        Ok(plan) => plan,
        Err(error) => {
            modpack::discard_staging(&temp_dir).await;
            return Err(error.into());
        }
    };

    Ok(cache_plan(&state, update_plan))
}

/// 选择本地 `.mrpack` / `.zip` 整合包文件，与当前实例进行三方比对并返回预览。用户取消时返回 `None`。
#[tauri::command]
#[specta::specta]
pub async fn modpack_update_preview_file(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Option<PackUpdateChoice>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    if instance.config.link.is_some() {
        return Err(AppError::new(
            "modpack.linked_instance",
            "外部关联实例不能直接更新整合包，请先「复制成 Lumi 的实例」后再更新",
        ));
    }
    let Some(path) = pick(&app, Pick::Modpack).await? else {
        return Ok(None);
    };
    let target_plan = inspect_file(&state, path).await?;
    let game = state.game();
    let update_plan = update::preview(
        &game,
        &instance,
        target_plan,
        None,
        BodyFormat::Markdown,
        None,
    )
    .await?;
    Ok(Some(cache_plan(&state, update_plan)))
}

fn cache_plan(state: &AppState, plan: update::PackUpdatePlan) -> PackUpdateChoice {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let preview = plan.preview.clone();
    let mut cache = state
        .pending_pack_updates
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let evicted: Vec<PathBuf> = if cache.len() >= 12 {
        cache
            .drain()
            .filter_map(|(_, old)| old.temp_pack_dir)
            .collect()
    } else {
        Vec::new()
    };
    cache.insert(token.clone(), plan);
    drop(cache);
    if !evicted.is_empty() {
        tauri::async_runtime::spawn(async move {
            for dir in evicted {
                modpack::discard_staging(&dir).await;
            }
        });
    }
    PackUpdateChoice { token, preview }
}

/// 放弃预检计划并清理可能已下载到暂存目录的整合包文件。
#[tauri::command]
#[specta::specta]
pub fn modpack_update_discard(state: State<'_, AppState>, token: String) {
    let removed = state
        .pending_pack_updates
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token);
    if let Some(temp_dir) = removed.and_then(|p| p.temp_pack_dir) {
        tauri::async_runtime::spawn(async move {
            modpack::discard_staging(&temp_dir).await;
        });
    }
}

/// 启动整合包无损更新后台任务，返回任务 ID。
#[tauri::command]
#[specta::specta]
pub fn modpack_update_start(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    options: PackUpdateOptions,
) -> Result<String, AppError> {
    let plan = state
        .pending_pack_updates
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token)
        .ok_or_else(|| {
            AppError::new(
                "modpack.update_expired",
                "更新预览已过期，请重新检查整合包更新",
            )
        })?;

    let instance_id = plan.preview.instance_id.clone();
    let instance = load_instance(&state, &instance_id)?;
    ensure_idle(&state, &instance_id, &instance.config.name, "更新整合包")?;

    let game = state.game();
    let directory = instance.game_dir(&game)?;
    let turn = state.turns.try_take(&directory).ok_or_else(busy_error)?;

    let title = format!(
        "更新整合包 {} → {}",
        plan.preview.pack_name, plan.preview.target_pack_version
    );
    let job = state.jobs.start_with(
        format!("modpack-update:{instance_id}"),
        title,
        ProgressUnit::Bytes,
        crate::downloads::meta(JobSource::Game, Some(&instance_id), None, None),
    )?;
    let job_id = job.id().to_owned();

    tauri::async_runtime::spawn(async move {
        let _turn = turn;
        let state = app.state::<AppState>();
        let cancel = job.cancel_token();
        let result: Result<PackUpdateOutcome, AppError> = async {
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
                && !plan.target_plan.skip_loader_install()
            {
                pick_java_for(&state, &job, &dl, &plan.preview.target_game_version).await?
            } else {
                PathBuf::new()
            };
            let temp_root = game.root().join(".mcl-tmp");
            let ctx = LoaderContext {
                game: &game,
                client: &state.http_client(),
                downloader: &dl,
                java: &java,
                work_dir: &temp_root,
                cancel: &cancel,
            };
            let outcome = update::execute(
                &ctx,
                &temp_root,
                &plan,
                &options,
                |p| match &p {
                    PackUpdateProgress::Loader(l) => report_loader(&job, l),
                    PackUpdateProgress::Game(phase, d) => report(&job, phase_label(*phase), d),
                    PackUpdateProgress::Files(d) => report(&job, "下载新版整合包文件", d),
                    PackUpdateProgress::WaitingManual(n) => {
                        job.set_step("等待手动补件");
                        job.set_message(format!(
                            "等你补齐 {n} 个文件（作者不允许第三方启动器下载）"
                        ));
                    }
                    PackUpdateProgress::Overrides => {
                        job.set_step("解压新版配置");
                        job.set_message("正在解压新版覆盖文件");
                    }
                    PackUpdateProgress::Applying => {
                        job.set_step("应用无损更新");
                        job.set_message("正在备份旧文件并应用整合包更新");
                    }
                },
                |claim_title, manual_files| {
                    let guard =
                        crate::claims::start(&app, &state, &job, claim_title, manual_files)?;
                    let session = guard.session_arc();
                    Some(ActiveClaim::new(session, guard))
                },
            )
            .await
            .map_err(|e| state.capture_app_error(&job, "更新整合包", e))?;
            Ok(outcome)
        }
        .await;

        match result {
            Ok(outcome) => {
                let mut parts = Vec::new();
                if outcome.added > 0 {
                    parts.push(format!("新增 {}", outcome.added));
                }
                if outcome.updated > 0 {
                    parts.push(format!("更新 {}", outcome.updated));
                }
                if outcome.removed > 0 {
                    parts.push(format!("移除 {}", outcome.removed));
                }
                if outcome.preserved_player_added > 0 {
                    parts.push(format!("保留自加 {}", outcome.preserved_player_added));
                }
                if outcome.preserved_player_modified > 0 {
                    parts.push(format!("保留修改 {}", outcome.preserved_player_modified));
                }
                if outcome.skipped_manual > 0 {
                    parts.push(format!("跳过补件 {}", outcome.skipped_manual));
                }
                if !parts.is_empty() {
                    job.set_message(parts.join(" · "));
                }
                job.set_instance(outcome.instance_id);
                job.succeed();
            }
            Err(error) if job.is_cancelled() => {
                tracing::info!(error = %error.message, "整合包更新已取消");
                job.cancelled();
            }
            Err(error) => state.finish_failed_job(job, error.message),
        }
    });

    Ok(job_id)
}

/// 一键无损回滚最近一次整合包更新。
#[tauri::command]
#[specta::specta]
pub async fn modpack_update_rollback(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<PackRollbackOutcome, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    ensure_idle(&state, &instance_id, &instance.config.name, "回滚整合包")?;
    let game = state.game();
    let directory = instance.game_dir(&game)?;
    let _turn = state.turns.try_take(&directory).ok_or_else(busy_error)?;
    Ok(update::rollback(&game, &instance_id).await?)
}
