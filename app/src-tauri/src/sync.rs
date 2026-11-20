//! 本机跨实例同步命令与启动/退出自动钩子（D26b）。

use std::collections::BTreeMap;

use launcher_core::instance::Instance;
use launcher_core::sync::{
    SharedFolderKind, SharedPackEntry, SharedServerEntry, SyncApplyResult, SyncError, SyncOverview,
    SyncPreview, SyncStrategy, add_shared_server, clear_shared_hotbar, import_instance_to_shared,
    list_shared_packs, list_shared_servers, read_shared_commands, read_shared_options,
    remove_shared_pack, remove_shared_server, shared_folder_path, sync_after_exit,
    sync_apply as core_sync_apply, sync_before_launch, sync_overview as core_sync_overview,
    sync_preview as core_sync_preview, write_shared_commands, write_shared_options,
};
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::state::AppState;

fn map_sync_error(error: SyncError) -> AppError {
    match error {
        SyncError::SourceNotFound => AppError::new("sync.source_not_found", "找不到指定的源实例"),
        other => AppError::new("sync.failed", "跨实例同步失败").with_detail(other),
    }
}

/// 获取本机跨实例同步总览（参与实例数、独立实例数与共享库统计）。
#[tauri::command]
#[specta::specta]
pub async fn sync_overview(state: State<'_, AppState>) -> Result<SyncOverview, AppError> {
    let game = state.game();
    let sync = state.settings.get().sync;
    tauri::async_runtime::spawn_blocking(move || core_sync_overview(&game, &sync))
        .await
        .map_err(|e| AppError::new("sync.failed", "读取同步状态失败").with_detail(e))
}

/// 预览各参与实例与共享库之间的设置/服务器/通用包差异。
#[tauri::command]
#[specta::specta]
pub async fn sync_preview(state: State<'_, AppState>) -> Result<SyncPreview, AppError> {
    let game = state.game();
    let sync = state.settings.get().sync;
    tauri::async_runtime::spawn_blocking(move || core_sync_preview(&game, &sync))
        .await
        .map_err(|e| AppError::new("sync.failed", "检查同步差异失败").with_detail(e))
}

/// 按所选策略立即执行一次跨实例同步（游戏运行时拒绝批量写入）。
#[tauri::command]
#[specta::specta]
pub async fn sync_apply(
    state: State<'_, AppState>,
    strategy: SyncStrategy,
    source_instance_id: Option<String>,
) -> Result<SyncApplyResult, AppError> {
    if crate::game::any_game_running(&state) {
        return Err(AppError::new(
            "sync.game_running",
            "游戏正在运行，请先退出游戏再执行批量同步",
        ));
    }
    let game = state.game();
    let sync = state.settings.get().sync;
    let result = tauri::async_runtime::spawn_blocking(move || {
        core_sync_apply(&game, &sync, strategy, source_instance_id.as_deref())
    })
    .await
    .map_err(|e| AppError::new("sync.failed", "执行同步任务失败").with_detail(e))?
    .map_err(map_sync_error)?;
    tracing::info!(
        updated = result.updated_instances,
        backups = result.backups_created,
        options = result.options_written,
        servers = result.servers_merged,
        packs = result.packs_synced,
        "已执行本机跨实例同步"
    );
    Ok(result)
}

/// 打开共享库文件夹（根目录 / 通用资源包 / 通用光影包）；不存在时自动创建。
#[tauri::command]
#[specta::specta]
pub fn sync_open_shared_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: SharedFolderKind,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let dir = shared_folder_path(&state.game(), kind);
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::new("folder.open_failed", "创建共享文件夹失败").with_detail(e))?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| AppError::new("folder.open_failed", "打不开共享文件夹").with_detail(e))
}

/// 读取共享库中的游戏设置键值对（供可视化编辑器使用，D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_options_get(
    state: State<'_, AppState>,
) -> Result<BTreeMap<String, String>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || read_shared_options(&game))
        .await
        .map_err(|e| AppError::new("sync.failed", "读取共享游戏设置失败").with_detail(e))
}

/// 保存共享库中的游戏设置键值对（供可视化编辑器使用，D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_options_set(
    state: State<'_, AppState>,
    entries: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || write_shared_options(&game, &entries))
        .await
        .map_err(|e| AppError::new("sync.failed", "保存共享游戏设置失败").with_detail(e))?
        .map_err(map_sync_error)
}

/// 读取共享库中的多人服务器列表（D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_servers_list(
    state: State<'_, AppState>,
) -> Result<Vec<SharedServerEntry>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || list_shared_servers(&game))
        .await
        .map_err(|e| AppError::new("sync.failed", "读取共享服务器列表失败").with_detail(e))
}

/// 向共享库新增一条多人服务器（D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_servers_add(
    state: State<'_, AppState>,
    name: String,
    address: String,
) -> Result<Vec<SharedServerEntry>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || add_shared_server(&game, &name, &address))
        .await
        .map_err(|e| AppError::new("sync.failed", "添加共享服务器失败").with_detail(e))?
        .map_err(|e| match e {
            SyncError::Servers(launcher_core::saves::servers::ServersError::BadAddress(_)) => {
                AppError::new("server.bad_address", "服务器地址格式不正确")
            }
            other => map_sync_error(other),
        })
}

/// 从共享库删除一条多人服务器（D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_servers_remove(
    state: State<'_, AppState>,
    address: String,
) -> Result<Vec<SharedServerEntry>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || remove_shared_server(&game, &address))
        .await
        .map_err(|e| AppError::new("sync.failed", "删除共享服务器失败").with_detail(e))?
        .map_err(map_sync_error)
}

/// 列出共享资源包或光影包目录里的全部包（D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_packs_list(
    state: State<'_, AppState>,
    kind: SharedFolderKind,
) -> Result<Vec<SharedPackEntry>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || list_shared_packs(&game, kind))
        .await
        .map_err(|e| AppError::new("sync.failed", "读取共享包列表失败").with_detail(e))
}

/// 删除共享资源包或光影包目录里的单个包（D26b 精修）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_packs_remove(
    state: State<'_, AppState>,
    kind: SharedFolderKind,
    name: String,
) -> Result<Vec<SharedPackEntry>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || remove_shared_pack(&game, kind, &name))
        .await
        .map_err(|e| AppError::new("sync.failed", "删除共享包失败").with_detail(e))?
        .map_err(map_sync_error)
}

/// 读取共享库中的命令历史记录（D26c）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_commands_get(state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || read_shared_commands(&game))
        .await
        .map_err(|e| AppError::new("sync.failed", "读取共享命令记录失败").with_detail(e))
}

/// 保存共享库中的命令历史记录（D26c）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_commands_set(
    state: State<'_, AppState>,
    lines: Vec<String>,
) -> Result<Vec<String>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || write_shared_commands(&game, &lines))
        .await
        .map_err(|e| AppError::new("sync.failed", "保存共享命令记录失败").with_detail(e))?
        .map_err(map_sync_error)
}

/// 清空共享库中的创造模式物品栏快照（D26c）。
#[tauri::command]
#[specta::specta]
pub async fn sync_shared_hotbar_clear(state: State<'_, AppState>) -> Result<(), AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || clear_shared_hotbar(&game))
        .await
        .map_err(|e| AppError::new("sync.failed", "清空共享创造物品栏失败").with_detail(e))?
        .map_err(map_sync_error)
}

/// 从指定实例导入设置、服务器、命令记录和/或创造模式物品栏到共享库（D26b/D26c）。
#[tauri::command]
#[specta::specta]
pub async fn sync_import_from_instance(
    state: State<'_, AppState>,
    instance_id: String,
    import_options: bool,
    import_servers: bool,
    import_commands: bool,
    import_hotbar: bool,
) -> Result<(), AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || {
        import_instance_to_shared(
            &game,
            &instance_id,
            import_options,
            import_servers,
            import_commands,
            import_hotbar,
        )
    })
    .await
    .map_err(|e| AppError::new("sync.failed", "从实例导入到共享库失败").with_detail(e))?
    .map_err(map_sync_error)
}

/// 游戏启动前：自动将共享库增量同步进该实例（失败只记日志，绝不阻断启动）。
pub(crate) async fn before_launch(state: &AppState, instance: &Instance) {
    let sync = state.settings.get().sync;
    if !sync.enabled {
        return;
    }
    let game = state.game();
    let instance = instance.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || sync_before_launch(&game, &sync, &instance))
            .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "启动前自动跨实例同步失败，照常启动"),
        Err(error) => tracing::warn!(%error, "启动前自动跨实例同步线程异常，照常启动"),
    }
}

/// 游戏正常退出后：自动将该实例改动的设置与新增服务器增量回写共享库。
pub(crate) async fn after_exit(state: &AppState, instance_id: &str) {
    let sync = state.settings.get().sync;
    if !sync.enabled {
        return;
    }
    let game = state.game();
    let Ok(Some(instance)) = launcher_core::instance::load(&game, instance_id) else {
        return;
    };
    let result =
        tauri::async_runtime::spawn_blocking(move || sync_after_exit(&game, &sync, &instance))
            .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "退出后自动回写共享同步库失败"),
        Err(error) => tracing::warn!(%error, "退出后自动回写共享同步库线程异常"),
    }
}
