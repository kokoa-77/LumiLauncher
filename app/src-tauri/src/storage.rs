//! 存储占用统计、安全缓存清理与闲置版本清理命令。

use std::collections::HashSet;
use std::path::PathBuf;

use launcher_core::storage::{
    CacheCleanResult, StorageReport, UnusedVersion, UnusedVersionsCleanResult, clean_cache,
    clean_unused_versions, find_unused_versions, scan_storage_with_backups,
};
use tauri::State;

use crate::error::AppError;
use crate::state::AppState;

fn protected_staging_paths(state: &AppState) -> HashSet<PathBuf> {
    let root = state.game().root().to_path_buf();
    crate::downloads::kept_staging(&state.jobs)
        .into_iter()
        .map(|name| root.join(name))
        .collect()
}

/// 扫描当前游戏目录与数据目录的磁盘占用分布。
#[tauri::command]
#[specta::specta]
pub async fn storage_scan(state: State<'_, AppState>) -> Result<StorageReport, AppError> {
    let game = state.game();
    let data_dir = state.data_dir.clone();
    let keep = protected_staging_paths(&state);
    let custom_backup_dir = state.settings.get().worlds.backup_dir.map(PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || {
        scan_storage_with_backups(&game, &data_dir, &keep, custom_backup_dir.as_deref())
    })
    .await
    .map_err(|e| AppError::new("storage.scan_failed", "统计存储占用失败").with_detail(e))
}

/// 清理可安全再生的缓存、缩略图、残留临时目录和旧日志。
#[tauri::command]
#[specta::specta]
pub async fn storage_clean_cache(state: State<'_, AppState>) -> Result<CacheCleanResult, AppError> {
    let game = state.game();
    let data_dir = state.data_dir.clone();
    let keep = protected_staging_paths(&state);
    tauri::async_runtime::spawn_blocking(move || clean_cache(&game, &data_dir, &keep))
        .await
        .map_err(|e| AppError::new("storage.clean_failed", "清理缓存失败").with_detail(e))
}

/// 扫描 `versions/` 中已没有任何实例引用的闲置版本目录。
#[tauri::command]
#[specta::specta]
pub async fn storage_unused_versions(
    state: State<'_, AppState>,
) -> Result<Vec<UnusedVersion>, AppError> {
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || find_unused_versions(&game))
        .await
        .map_err(|e| AppError::new("storage.scan_failed", "扫描闲置游戏版本失败").with_detail(e))
}

/// 清理已选中的闲置游戏版本目录（游戏运行时拒绝执行）。
#[tauri::command]
#[specta::specta]
pub async fn storage_clean_unused_versions(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<UnusedVersionsCleanResult, AppError> {
    if crate::game::any_game_running(&state) {
        return Err(AppError::new(
            "storage.game_running",
            "游戏正在运行，请先退出游戏再清理闲置版本",
        ));
    }
    let game = state.game();
    tauri::async_runtime::spawn_blocking(move || clean_unused_versions(&game, &ids))
        .await
        .map_err(|e| AppError::new("storage.clean_failed", "清理闲置版本失败").with_detail(e))
}
