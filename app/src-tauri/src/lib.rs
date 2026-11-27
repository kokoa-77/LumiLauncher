//! 启动器桌面应用壳。核心逻辑都在 `launcher-core`，这里只负责窗口、命令、事件和日志。

mod accounts;
mod app_dirs;
mod backups;
mod claims;
mod commands;
mod discover;
mod downloads;
mod drops;
mod error;
mod events;
mod game;
mod images;
mod installer;
mod instances;
mod linked;
mod logging;
mod pack_update;
mod portable;
mod records;
mod recovery;
mod sessions;
mod shortcuts;
mod state;
mod storage;
mod sync;
mod system;
mod turns;
mod updates;
mod window;
mod window_lifecycle;
mod worlds;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, RwLock};

use launcher_core::config::SettingsStore;
use launcher_core::content::cache::TtlCache;
use launcher_core::content::curseforge::CurseForge;
use launcher_core::content::favorites::Favorites;
use launcher_core::java::JavaPrefsStore;
use launcher_core::jobs::JobRegistry;
use launcher_core::net::{
    Doh, DownloadEngine, ProxyWatcher, TransferMetricEventKind, clean_orphan_part_files,
    is_metered_network,
};
use launcher_core::paths::GameDir;
#[cfg(any(debug_assertions, test))]
use specta_typescript::Typescript;
use tauri::Manager;
use tauri_specta::{collect_commands, collect_events};

use crate::sessions::Sessions;
use crate::state::{AppState, Catalog};

// 只在开发时（调试构建和测试）导出前端类型，正式版不需要
/// 前端类型文件的位置（提交进仓库，前端类型检查直接用它）。
#[cfg(any(debug_assertions, test))]
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");
#[cfg(any(debug_assertions, test))]
const BINDINGS_HEADER: &str = "// 由 tauri-specta 根据 Rust 代码自动生成，不要手改。\n// 重新生成：cargo test -p launcher-app";

fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        // 前端总是收发完整对象，不需要区分「输出 / 输入」两种形状（输入形状里所有字段都会变成可选）
        .disable_serde_phases()
        .commands(collect_commands![
            commands::app_info,
            commands::app_ready,
            commands::app_set_ui_language,
            drops::drops_shell_ready,
            commands::app_log,
            updates::updates_mode,
            updates::updates_check,
            updates::updates_install,
            commands::settings_get,
            commands::settings_set,
            commands::settings_export,
            commands::settings_import,
            commands::jobs_list,
            commands::jobs_cancel,
            storage::storage_scan,
            storage::storage_clean_cache,
            storage::storage_unused_versions,
            storage::storage_clean_unused_versions,
            sync::sync_overview,
            sync::sync_preview,
            sync::sync_apply,
            sync::sync_open_shared_folder,
            sync::sync_shared_options_get,
            sync::sync_shared_options_set,
            sync::sync_shared_servers_list,
            sync::sync_shared_servers_add,
            sync::sync_shared_servers_remove,
            sync::sync_shared_packs_list,
            sync::sync_shared_packs_remove,
            sync::sync_shared_commands_get,
            sync::sync_shared_commands_set,
            sync::sync_shared_hotbar_clear,
            sync::sync_import_from_instance,
            linked::linked_roots_list,
            linked::linked_roots_discover,
            linked::linked_roots_add,
            linked::linked_roots_pick,
            linked::linked_roots_remove,
            linked::linked_roots_restore,
            linked::foreign_list,
            linked::foreign_pick,
            linked::foreign_import,
            downloads::jobs_history,
            downloads::jobs_retry,
            downloads::jobs_retry_failed,
            downloads::jobs_pause,
            downloads::jobs_resume,
            downloads::jobs_remove,
            downloads::jobs_clear_history,
            downloads::net_doh_test,
            downloads::net_diagnostics,
            downloads::net_runtime_status,
            downloads::net_test_proxy,
            downloads::net_apply_quick_fix,
            downloads::claim_folder_info,
            downloads::claim_folder_choose,
            downloads::claim_folder_reset,
            game::instances_list,
            game::instances_open_folder,
            game::instances_create,
            game::instances_repair_files,
            game::catalog_game_versions,
            game::catalog_game_version_entries,
            game::catalog_neoforge_versions,
            game::catalog_loader_versions,
            game::launch_start,
            game::launch_preflight,
            game::launch_stop,
            game::launch_sessions,
            game::launch_log,
            game::modrinth_search,
            game::curseforge_search,
            game::content_resolve,
            game::content_install,
            game::content_list,
            game::content_set_enabled,
            game::content_refresh,
            game::content_check_updates,
            game::content_update,
            game::content_remove,
            game::content_history,
            game::content_rollback,
            game::content_crash_offer,
            game::content_restore_snapshot,
            game::content_timeline,
            game::content_restore_timeline_node,
            game::content_mod_deps,
            game::content_mod_impact,
            game::content_set_many_enabled,
            recovery::content_recovered,
            backups::backups_list,
            backups::backups_create,
            backups::backups_restore,
            backups::backups_delete,
            backups::backups_reveal,
            backups::backups_decide,
            backups::backups_usage,
            backups::backups_clear,
            backups::backups_dir_info,
            backups::backups_dir_choose,
            backups::backups_dir_reset,
            worlds::worlds_notes_get,
            worlds::worlds_notes_set,
            game::game_dir_info,
            game::game_dir_choose,
            game::modpack_pick,
            game::modpack_search,
            game::modpack_versions,
            game::modpack_readiness,
            game::modpack_install,
            game::modpack_discard,
            game::modpack_import,
            pack_update::modpack_update_versions,
            pack_update::modpack_update_preview_remote,
            pack_update::modpack_update_preview_file,
            pack_update::modpack_update_discard,
            pack_update::modpack_update_start,
            pack_update::modpack_update_rollback,
            claims::claim_list,
            claims::claim_skip,
            claims::claim_pick,
            worlds::worlds_list,
            worlds::worlds_recent,
            worlds::worlds_recent_servers,
            discover::discover_project,
            discover::discover_versions,
            discover::discover_version_changelog,
            discover::favorites_list,
            discover::names_match,
            discover::favorites_toggle,
            discover::maps_search,
            discover::maps_install,
            discover::servers_search,
            discover::servers_add,
            discover::servers_list,
            discover::servers_status,
            game::content_reveal,
            game::content_remove_many,
            game::content_set_locked,
            game::content_switch_version,
            game::content_resolve_version,
            worlds::instances_screenshots,
            worlds::instances_open_screenshots,
            worlds::worlds_open_folder,
            worlds::worlds_background_pick,
            worlds::worlds_background_fit,
            worlds::worlds_background_clear,
            worlds::worlds_auto_covers_list,
            worlds::worlds_auto_cover_lock,
            worlds::worlds_auto_cover_unlock,
            worlds::worlds_save_share_card,
            records::launch_records,
            records::launch_record_log,
            records::launch_record_crash,
            records::launch_record_diagnosis,
            records::launch_record_diagnostic_export,
            records::launch_record_reveal,
            records::launch_record_upload_log,
            drops::drops_check,
            drops::drops_install,
            drops::drops_modpack,
            drops::drops_discard,
            system::memory_status,
            system::memory_check,
            system::memory_programs,
            system::memory_clean,
            system::java_list,
            system::java_add,
            system::java_install,
            system::java_set_default,
            system::java_remove,
            system::java_delete,
            instances::instances_settings_get,
            instances::instances_settings_set,
            instances::instances_notes_get,
            instances::instances_notes_set,
            instances::instances_export_preview,
            instances::instances_export_match,
            instances::instances_export_discard,
            instances::instances_export,
            instances::instances_reveal_export,
            instances::instances_copy,
            instances::instances_delete,
            instances::instances_cover_pick,
            instances::instances_cover_set_preset,
            instances::instances_cover_clear,
            instances::instances_upgrade_preview,
            instances::instances_upgrade_cancel_preview,
            instances::instances_upgrade_discard,
            instances::instances_upgrade_start,
            instances::instances_set_group,
            instances::instances_set_pinned,
            instances::instances_rename_group,
            shortcuts::shortcut_take,
            shortcuts::shortcut_create,
            shortcuts::window_progress,
            shortcuts::window_show,
            accounts::accounts_list,
            accounts::accounts_skin,
            accounts::accounts_cape_texture,
            accounts::accounts_select,
            accounts::accounts_remove,
            accounts::accounts_finish_onboarding,
            accounts::accounts_restart_onboarding,
            accounts::accounts_microsoft_login,
            accounts::accounts_microsoft_reopen_browser,
            accounts::accounts_microsoft_copy_url,
            accounts::accounts_microsoft_cancel,
            accounts::accounts_prewarm,
            accounts::accounts_skin_refresh,
            accounts::accounts_skin_upload,
            accounts::accounts_skin_reset,
            accounts::accounts_cape_set,
        ])
        .events(collect_events![
            events::JobUpdated,
            events::TransferMetricsUpdated,
            events::SessionUpdated,
            events::GameLogBatch,
            events::ClaimUpdated,
            events::FileDragHover,
            events::FilesReading,
            events::FilesDropped,
            events::LaunchRecordSaved,
            events::LaunchAuthBlocked,
            events::AuthProgressUpdated,
            events::AccountHealthUpdated,
            events::InstancesChanged,
            updates::UpdateProgress,
            backups::BackupAsk,
            shortcuts::ShortcutPlay
        ])
}

#[cfg(any(debug_assertions, test))]
fn export_bindings(builder: &tauri_specta::Builder<tauri::Wry>) -> Result<(), String> {
    builder
        .export(Typescript::default().header(BINDINGS_HEADER), BINDINGS_PATH)
        .map_err(|e| e.to_string())
}

/// 以管理员身份被再开一次、只做一件事的模式（现在只有整理内存）。不是这种模式返回 `None`。
pub fn run_helper() -> Option<i32> {
    let first = std::env::args().nth(1)?;
    match first.as_str() {
        system::CLEAN_MEMORY_ARG => Some(system::clean_memory_helper()),
        installer::SCAN_ARG => Some(installer::scan()),
        _ => None,
    }
}

pub fn run() {
    let specta = specta_builder();

    #[cfg(debug_assertions)]
    if let Err(error) = export_bindings(&specta) {
        eprintln!("导出前端类型失败：{error}");
    }

    let mut context = tauri::generate_context!();
    let portable = match portable::PortableProfile::detect() {
        Ok(profile) => profile,
        Err(error) => {
            show_portable_error(context, error);
            return;
        }
    };
    let portable_dir = portable
        .as_ref()
        .map(|profile| profile.data_dir().to_path_buf());
    let is_portable = portable.is_some();
    // 定名前的设置、日志、界面偏好搬到新 ID 下：要在窗口（WebView2）创建之前
    let isolated_data = if cfg!(debug_assertions) {
        std::env::var_os("LUMI_DEV_DATA_DIR").map(std::path::PathBuf::from)
    } else {
        None
    };
    if let Some(root) = portable_dir.as_ref().or(isolated_data.as_ref()) {
        use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides};
        context.config_mut().app.app_directories_override =
            Some(AppDirectoriesOverride::Directories(AppDirectoryOverrides {
                data: Some(root.clone()),
                config: Some(root.clone()),
                local_data: Some(root.join("webview")),
                cache: Some(root.join("caches")),
                log: Some(root.join("logs")),
            }));
    }
    let moved_dirs = if isolated_data.is_none() && !is_portable {
        app_dirs::migrate(&context.config().identifier)
    } else {
        Vec::new()
    };

    let mut builder = tauri::Builder::default().manage(drops::ShellPack::from_args());
    if !is_portable {
        // 安装版的窗口按应用标识转发。便携版按数据目录上的操作系统锁互斥。
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            shortcuts::on_second_launch(app, &argv)
        }));
    }
    if !is_portable && context.config().plugins.0.contains_key("updater") {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let builder = builder
        // 记住窗口位置和大小；不恢复「可见」状态，窗口由前端就绪后再显示
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window::saved_state_flags())
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        // 只在后端用（选游戏目录、选整合包文件），界面没有它的权限
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(specta.invoke_handler())
        // 本地图片（存档截图）交给界面：界面只拿到一次性编号，读不到登记以外的文件
        .register_asynchronous_uri_scheme_protocol(images::SCHEME, images::handle)
        // 拖进窗口的文件由后端接收（补件），界面拿不到路径
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(drop) = event {
                claims::on_drag_drop(window.app_handle(), drop);
            }
        })
        .setup(move |app| {
            specta.mount_events(app);
            app.manage(updates::Updates::new(app, is_portable));

            let log_dir = match &isolated_data {
                Some(directory) => directory.join("logs"),
                None => app.path().app_log_dir()?,
            };
            let log_guard = logging::init(&log_dir)?;
            app.manage(log_guard);
            app.manage(shortcuts::PendingPlay::from_args());
            app.manage(backups::BackupPrompts::default());
            app.manage(window::MainWindow::default());
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "启动器启动");
            app_dirs::log(&moved_dirs);

            let main_window = app
                .get_webview_window(window::MAIN_WINDOW)
                .ok_or("找不到主窗口")?;
            let backdrop = window::apply_backdrop(&main_window);

            let data_dir = match &isolated_data {
                Some(directory) => directory.clone(),
                None => app.path().app_data_dir()?,
            };
            // 下载历史：关掉再打开还在
            let jobs = JobRegistry::with_history(data_dir.join("jobs-history.json"));
            events::forward_job_events(app.handle().clone(), &jobs);

            // 有设置的旧数据视为已完成引导，安装器可单独标记本次安装需要引导。
            let settings_existed = data_dir.join("settings.json").is_file();
            let settings = SettingsStore::open(data_dir.join("settings.json"));
            if !is_portable {
                installer::apply_game_location(&settings, &data_dir);
            }
            // 游戏目录：环境变量（开发时指向测试目录）> 设置里选的 > 应用数据目录下的 minecraft。
            // 开发版的桌面快捷方式带 `--game-dir`（双击时没有环境变量），只有开发版认它
            let from_env = if is_portable {
                None
            } else {
                std::env::var_os("MCL_GAME_DIR")
                    .map(std::path::PathBuf::from)
                    .or_else(shortcuts::dev_game_dir)
            };
            let game_dir_from_env = from_env.is_some();
            let game_root = from_env
                .or_else(|| settings.get().game.directory.map(std::path::PathBuf::from))
                .unwrap_or_else(|| game::default_game_dir(&data_dir));
            tracing::info!(dir = %game_root.display(), from_env = game_dir_from_env, "游戏目录");
            // 早先的实例在 versions/<实例名>/：搬到 instances/。只是改名，瞬间完成；
            // 要在下面的清理、退回没做完的改动之前做（它们按新位置找实例）
            game::migrate_instances(&GameDir::new(&game_root));
            // 上次没删掉的临时文件（安装途中被关掉之类），放后台清，不耽误首屏；
            // 还能重试的整合包导入留着接着装
            {
                let game = GameDir::new(&game_root);
                let keep = downloads::kept_staging(&jobs);
                tauri::async_runtime::spawn_blocking(move || game::clean_temp_packs(&game, &keep));
            }
            drops::clean_leftovers(&game_root);
            let ignore_system_java = std::env::var_os("MCL_IGNORE_SYSTEM_JAVA").is_some();
            if ignore_system_java {
                tracing::warn!(
                    "MCL_IGNORE_SYSTEM_JAVA 已设置：不用电脑上的 Java，只用启动器下载的"
                );
            }
            // 下载域名先问 RFC 8484 纯 IP DoH，并挂载 ProxyWatcher + 全局三池下载引擎
            let current_download = settings.get().download;
            let proxy_watcher = std::sync::Arc::new(ProxyWatcher::new());
            let doh = std::sync::Arc::new(Doh::new(current_download.doh_enabled)?);
            let snap = proxy_watcher.snapshot();
            let engine = match doh
                .sync_proxy_with_snapshot(&current_download.proxy, Some(&snap))
                .and_then(|()| {
                    DownloadEngine::new_with_paths(
                        doh.clone(),
                        proxy_watcher.clone(),
                        &current_download.proxy,
                        data_dir.clone(),
                        game_root.clone(),
                    )
                }) {
                Ok(engine) => std::sync::Arc::new(engine),
                Err(error) => {
                    tracing::warn!(%error, "启动时自定义代理无效，回退为系统代理");
                    let fallback = launcher_core::config::ProxySettings::default();
                    let _ = doh.sync_proxy_with_snapshot(&fallback, Some(&snap));
                    std::sync::Arc::new(DownloadEngine::new_with_paths(
                        doh.clone(),
                        proxy_watcher.clone(),
                        &fallback,
                        data_dir.clone(),
                        game_root.clone(),
                    )?)
                }
            };
            engine.apply_settings(&current_download);
            engine.set_metered(is_metered_network());
            let linked = linked::Linked::open(data_dir.join("linked-roots.json"));
            engine.set_external_roots(linked.root_paths());
            events::spawn_metrics_emitter(app.handle().clone(), engine.clone(), jobs.clone());
            // 启动 15s 后低优先级清理超过 72h 的孤儿分段碎片
            {
                let gc_root = game_root.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        let (bytes, files) = clean_orphan_part_files(
                            &gc_root,
                            std::time::Duration::from_secs(72 * 3600),
                        );
                        if files > 0 {
                            tracing::info!(files, bytes, "启动后台清理孤儿分段碎片完成");
                        }
                    })
                    .await;
                });
            }
            let client = engine.h2_client();
            let auth_client = match launcher_core::net::build_auth_client(
                doh.clone(),
                &current_download.proxy,
                Some(&snap),
            ) {
                Ok(client) => client,
                Err(error) => {
                    tracing::warn!(%error, "认证客户端使用默认系统代理回退");
                    launcher_core::net::build_auth_client(
                        doh.clone(),
                        &launcher_core::config::ProxySettings::default(),
                        Some(&snap),
                    )?
                }
            };
            let curseforge = CurseForge::from_env(client.clone());
            if !curseforge.has_key() {
                tracing::info!(
                    "未配置 CurseForge API Key：CurseForge 在线服务暂不启用（Modrinth 正常可用）"
                );
            }
            let accounts = accounts::Accounts::open(
                &data_dir,
                settings_existed,
                isolated_data.is_some(),
                accounts::install_onboarding_marker(is_portable),
            );
            app.manage(AppState {
                data_dir: data_dir.clone(),
                settings,
                jobs,
                ui_ready: AtomicBool::new(false),
                ui_language: Mutex::new("zh-CN".to_owned()),
                backdrop: Mutex::new(backdrop),
                game_dir: Mutex::new(GameDir::new(game_root)),
                game_dir_from_env,
                ignore_system_java,
                client: RwLock::new(client),
                auth_client: RwLock::new(auth_client),
                engine,
                proxy_watcher,
                doh,
                curseforge,
                downloads_dir: app.path().download_dir().ok(),
                claims: Mutex::default(),
                images: images::Images::default(),
                sessions: Sessions::default(),
                catalog: Catalog::default(),
                pending_imports: Mutex::default(),
                pending_upgrades: Mutex::default(),
                upgrade_preview_cancels: Mutex::default(),
                pending_pack_updates: Mutex::default(),
                pending_exports: Mutex::default(),
                drops: Mutex::default(),
                search_cache: TtlCache::new(state::DISCOVER_CACHE_TTL, 300),
                detail_cache: TtlCache::new(state::DISCOVER_CACHE_TTL, 100),
                favorites: Favorites::open(data_dir.join("favorites.json")),
                java_prefs: JavaPrefsStore::open(data_dir.join("java.json")),
                gpu_ledger: data_dir.join("gpu-preferences.json"),
                exports: Mutex::default(),
                turns: turns::InstanceTurns::default(),
                recovery: recovery::RecoveryNotes::default(),
                names: launcher_core::content::names::NameBook::default(),
                linked,
                accounts,
                failure_receipts: Mutex::default(),
            });
            // 挂上的别家目录：别家新装、删掉的版本跟着变，放后台不耽误首屏
            linked::spawn_rescan(app.handle());
            // 上次改到一半被关掉的实例：后台退回改之前
            recovery::spawn(app.handle());

            // 每 1 秒热感知系统代理、计费网络状态与休眠唤醒，瞬时重置 DoH/Fake-IP 租约并重建三池连接
            let watch_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    let state = watch_app.state::<AppState>();
                    state.engine.set_metered(is_metered_network());
                    let proxy = state.settings.get().download.proxy;
                    if let Some((epoch, snap, woke)) = state.proxy_watcher.poll(&proxy) {
                        tracing::info!(
                            epoch,
                            woke,
                            sys_enabled = snap.enabled,
                            "检测到网络或系统代理变更，热重建连接池并重置 DoH"
                        );
                        state.engine.metrics().push_event(
                            TransferMetricEventKind::RouteSwitched,
                            if woke {
                                "系统休眠唤醒，已丢弃老化连接并重建连接池"
                            } else {
                                "系统代理发生切换，已热切换连接池"
                            },
                        );
                        if woke {
                            state.accounts.clear_warm_sessions();
                        }
                        if let Err(err) = state.doh.sync_proxy_with_snapshot(&proxy, Some(&snap)) {
                            tracing::warn!(%err, "热更新 DoH 客户端代理失败");
                        }
                        state.doh.on_network_epoch_change();
                        match state.engine.rebuild_clients(&proxy, Some(&snap)) {
                            Ok(next) => {
                                *state.client.write().unwrap_or_else(|e| e.into_inner()) =
                                    next.clone();
                                state.curseforge.set_client(next);
                            }
                            Err(err) => {
                                tracing::warn!(%err, "热重建 HTTP 客户端池失败");
                            }
                        }
                        if let Ok(next_auth) = launcher_core::net::build_auth_client(
                            state.doh.clone(),
                            &proxy,
                            Some(&snap),
                        ) {
                            *state.auth_client.write().unwrap_or_else(|e| e.into_inner()) =
                                next_auth;
                        }
                    }
                }
            });

            window::spawn_show_fallback(app.handle());
            Ok(())
        });

    match builder.build(context) {
        Ok(app) => {
            if let Some(profile) = portable {
                app.manage(profile);
            }
            app.run(window::on_event);
        }
        Err(error) => {
            tracing::error!(%error, "启动器无法运行");
            eprintln!("启动器无法运行：{error}");
            std::process::exit(1);
        }
    }
}

fn show_portable_error(mut context: tauri::Context<tauri::Wry>, error: std::io::Error) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    context.config_mut().app.windows.clear();
    let text = format!("无法打开 Lumi 便携版。{error}");
    if let Err(error) = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            app.dialog()
                .message(text.clone())
                .title("Lumi")
                .kind(MessageDialogKind::Error)
                .show(move |_| handle.exit(1));
            Ok(())
        })
        .run(context)
    {
        eprintln!("无法显示便携版错误：{error}");
    }
}

#[cfg(test)]
mod tests {
    /// 运行 `cargo test -p launcher-app` 时顺便重新生成前端类型文件。
    #[test]
    fn export_typescript_bindings() {
        super::export_bindings(&super::specta_builder()).unwrap();
    }
}
