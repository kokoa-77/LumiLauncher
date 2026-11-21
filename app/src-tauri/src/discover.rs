//! 资源中心：项目详情、收藏夹。搜索命令在 `game.rs`（三个搜索都走 [`AppState::search_cache`]）。

use launcher_core::content::curseforge as cf;
use launcher_core::content::detail::{ContentVersion, ProjectDetail};
use launcher_core::content::favorites::{Favorite, FavoriteKind};
use launcher_core::content::maps::{self, MapInstall};
use launcher_core::content::modrinth::SearchPage;
use launcher_core::content::modrinth::{self, ProjectHit};
use launcher_core::content::names::NameMatch;
use launcher_core::content::{ContentSource, SearchSort};
use launcher_core::instance::Instance;
use launcher_core::jobs::ProgressUnit;
use launcher_core::net::status::{self, ServerStatus};
use launcher_core::saves::servers;
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::game::{
    SEARCH_PAGE, SearchKey, cached_search, curseforge_category, curseforge_id, downloader,
    load_instance, report,
};
use crate::state::AppState;

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// 一个项目的详情：介绍原文（界面负责清洗后再显示）、图片、链接。10 分钟内再打开直接用缓存。
#[tauri::command]
#[specta::specta]
pub async fn discover_project(
    state: State<'_, AppState>,
    source: ContentSource,
    project_id: String,
) -> Result<ProjectDetail, AppError> {
    let key = format!("{source:?}|{project_id}");
    let learned = state.learned_names();
    if let Some(mut detail) = state.detail_cache.get(&key) {
        learned.annotate(std::slice::from_mut(&mut detail.hit));
        return Ok(detail);
    }
    let mut detail = match source {
        ContentSource::Modrinth => {
            modrinth::project_detail(&state.http_client(), &project_id).await?
        }
        ContentSource::CurseForge => {
            state
                .curseforge
                .project_detail(curseforge_id(&project_id)?)
                .await?
        }
    };
    state.detail_cache.insert(key, detail.clone());
    learned.annotate(std::slice::from_mut(&mut detail.hit));
    Ok(detail)
}

/// 列出项目的版本列表（项目「版本」页签与实例「切换版本」共用）。
#[tauri::command]
#[specta::specta]
pub async fn discover_versions(
    state: State<'_, AppState>,
    source: ContentSource,
    project_id: String,
    game_version: Option<String>,
    loader: Option<String>,
) -> Result<Vec<ContentVersion>, AppError> {
    let gv = game_version.as_deref().filter(|s| !s.trim().is_empty());
    let loader_slug = loader.as_deref().filter(|s| !s.trim().is_empty());
    let versions = match source {
        ContentSource::Modrinth => {
            modrinth::project_versions(&state.http_client(), &project_id, gv, loader_slug).await?
        }
        ContentSource::CurseForge => {
            let mod_id = curseforge_id(&project_id)?;
            let cf_loader = loader_slug.and_then(cf::loader_id);
            state
                .curseforge
                .project_versions(mod_id, gv, cf_loader)
                .await?
        }
    };
    Ok(versions)
}

/// 懒加载 CurseForge 单个版本的更新日志（HTML 原文，前端清洗后展示）。
#[tauri::command]
#[specta::specta]
pub async fn discover_version_changelog(
    state: State<'_, AppState>,
    source: ContentSource,
    project_id: String,
    version_id: String,
) -> Result<String, AppError> {
    match source {
        ContentSource::Modrinth => {
            let version = modrinth::fetch_version(&state.http_client(), &version_id).await?;
            Ok(version.changelog.unwrap_or_default())
        }
        ContentSource::CurseForge => {
            let mod_id = curseforge_id(&project_id)?;
            let file_id = curseforge_id(&version_id)?;
            Ok(state.curseforge.file_changelog(mod_id, file_id).await?)
        }
    }
}

/// 收藏夹里的全部项目，后收藏的在前。不用联网。
#[tauri::command]
#[specta::specta]
pub fn favorites_list(state: State<'_, AppState>) -> Vec<Favorite> {
    let mut list = state.favorites.list();
    let learned = state.learned_names();
    for favorite in &mut list {
        learned.annotate(std::slice::from_mut(&mut favorite.hit));
    }
    list
}

/// 中文关键词查中文名表：资源中心拿第一个的英文名去平台搜，其余的给玩家点。不用联网。
#[tauri::command]
#[specta::specta]
pub fn names_match(query: String) -> Vec<NameMatch> {
    launcher_core::content::names::table().search(&query, 6)
}

/// 收藏 / 取消收藏一个项目，返回之后是不是在收藏夹里。
#[tauri::command]
#[specta::specta]
pub fn favorites_toggle(
    state: State<'_, AppState>,
    kind: FavoriteKind,
    hit: ProjectHit,
) -> Result<bool, AppError> {
    let title = hit.title.clone();
    let added = state
        .favorites
        .toggle(kind, hit, now_ms())
        .map_err(|error| {
            tracing::error!(%error, "保存收藏夹失败");
            AppError::new(
                "favorites.save_failed",
                "保存收藏夹失败，请检查磁盘空间是否足够",
            )
            .with_detail(error)
        })?;
    tracing::info!(project = %title, added, "收藏夹已更新");
    Ok(added)
}

// ---------- 地图（CurseForge Worlds） ----------

/// 搜地图。给了实例时只列适用于它的游戏版本的（老地图在新版本也能开，所以装的时候还会再放宽一次）。
/// `category` 是 CurseForge 的分类编号（冒险、跑酷、解谜……）。
#[tauri::command]
#[specta::specta]
pub async fn maps_search(
    state: State<'_, AppState>,
    query: String,
    instance_id: Option<String>,
    sort: SearchSort,
    category: Option<String>,
    offset: u32,
) -> Result<SearchPage, AppError> {
    let instance = instance_id
        .map(|id| load_instance(&state, &id))
        .transpose()?;
    let game_version = instance.as_ref().map(|i| i.config.game_version.as_str());
    let search = cf::SearchQuery {
        text: &query,
        class_id: cf::class::WORLD,
        game_version,
        loader: None,
        sort,
        category: curseforge_category(category.as_deref())?,
        offset,
        limit: SEARCH_PAGE,
    };
    let key = SearchKey {
        source: "curseforge",
        kind: "map",
        text: &query,
        game_version,
        loader: None,
        sort,
        category: category.as_deref(),
        offset,
    }
    .build();
    cached_search(&state, key, state.curseforge.search(&search)).await
}

/// 装一张地图到实例里（后台任务），返回任务编号。装好后单人游戏列表里就有它。
#[tauri::command]
#[specta::specta]
pub fn maps_install(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    project_id: String,
    title: String,
) -> Result<String, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let mod_id = curseforge_id(&project_id)?;
    let retry = crate::downloads::Retry::MapInstall {
        instance_id: instance_id.clone(),
        project_id: project_id.clone(),
        title: title.clone(),
    };
    let job = state.jobs.start_with(
        format!("map-install:{instance_id}:{project_id}"),
        format!("安装地图 {title}"),
        ProgressUnit::Bytes,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::CurseForge,
            Some(&instance_id),
            None,
            Some(&retry),
        ),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = async {
            let game = state.game();
            let dir = instance.game_dir(&game)?;
            let Some(_turn) = state.turns.wait(&dir, &job).await else {
                return Err(AppError::new("job.cancelled", "已取消"));
            };
            let temp = game.root().join(".mcl-tmp").join("maps");
            let downloader = downloader(&state);
            let cancel = job.cancel_token();
            let ctx = MapInstall {
                curseforge: &state.curseforge,
                downloader: &downloader,
                instance_dir: &dir,
                temp_dir: &temp,
                game_version: &instance.config.game_version,
                cancel: &cancel,
            };
            maps::install(&ctx, mod_id, &title, |p| report(&job, "下载地图", &p))
                .await
                .map_err(AppError::from)
        }
        .await;
        match result {
            Ok(folder) => {
                tracing::info!(instance = %instance.id, world = %folder, "地图已装好");
                job.succeed();
            }
            Err(_) if job.is_cancelled() => job.cancelled(),
            Err(error) => job.fail(error.message),
        }
    });
    Ok(job_id)
}

// ---------- 多人服务器（Modrinth 的服务器列表） ----------

/// 搜多人服务器。`category` 是 Modrinth 的服务器分类名（生存、空岛、角色扮演……）。
#[tauri::command]
#[specta::specta]
pub async fn servers_search(
    state: State<'_, AppState>,
    query: String,
    sort: SearchSort,
    category: Option<String>,
    offset: u32,
) -> Result<SearchPage, AppError> {
    let key = SearchKey {
        source: "modrinth",
        kind: "server",
        text: &query,
        game_version: None,
        loader: None,
        sort,
        category: category.as_deref(),
        offset,
    }
    .build();
    let client = state.http_client();
    let search = launcher_core::content::modrinth::search_servers(
        &client,
        &query,
        sort,
        category.as_deref(),
        offset,
        SEARCH_PAGE,
    );
    cached_search(&state, key, search).await
}

/// 「装完整合包顺便加服务器」时界面传过来的服务器（也记在重试配方里）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerJoin {
    pub name: String,
    pub address: String,
}

/// 整合包装好后加服务器：失败只记日志，不算整合包没装好
pub(crate) fn add_server_to(state: &AppState, instance: &Instance, server: &ServerJoin) {
    let result = instance
        .game_dir(&state.game())
        .map_err(|e| e.to_string())
        .and_then(|dir| {
            servers::add(&dir, &server.name, &server.address).map_err(|e| e.to_string())
        });
    match result {
        Ok(_) => {
            tracing::info!(instance = %instance.id, server = %server.address, "服务器已加进多人游戏列表")
        }
        Err(error) => {
            tracing::warn!(%error, instance = %instance.id, "整合包装好了，但加服务器失败")
        }
    }
}

/// 把服务器加进实例的多人游戏列表（最前面）。已经有同一个地址时不重复加，返回 false。
#[tauri::command]
#[specta::specta]
pub fn servers_add(
    state: State<'_, AppState>,
    instance_id: String,
    name: String,
    address: String,
) -> Result<bool, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let dir = instance.game_dir(&state.game())?;
    let added = servers::add(&dir, &name, &address)?;
    tracing::info!(instance = %instance_id, server = %address, added, "加服务器");
    Ok(added)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntryView {
    pub name: String,
    pub address: String,
}

/// 实例多人游戏列表里的服务器（资源中心判断「已加入」用）。
#[tauri::command]
#[specta::specta]
pub fn servers_list(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Vec<ServerEntryView>, AppError> {
    let instance = load_instance(&state, &instance_id)?;
    let dir = instance.game_dir(&state.game())?;
    Ok(servers::list(&dir)
        .into_iter()
        .map(|s| ServerEntryView {
            name: s.name,
            address: s.address,
        })
        .collect())
}

/// 同一个地址多久之内不重复问
const STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(30);
/// 最多同时问几个服务器（资源中心一页二十个也不会一下子开二十个连接）
const STATUS_PARALLEL: usize = 8;

struct StatusCache {
    entries:
        std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, ServerStatus)>>,
    permits: tokio::sync::Semaphore,
}

fn status_cache() -> &'static StatusCache {
    static CACHE: std::sync::OnceLock<StatusCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| StatusCache {
        entries: std::sync::Mutex::new(std::collections::HashMap::new()),
        permits: tokio::sync::Semaphore::new(STATUS_PARALLEL),
    })
}

/// 问一下服务器现在的样子（在线人数、延迟、简介、图标）。30 秒内问过的直接用上次的，`fresh` 时重新问。
#[tauri::command]
#[specta::specta]
pub async fn servers_status(address: String, fresh: bool) -> ServerStatus {
    let key = address.trim().to_ascii_lowercase();
    let cache = status_cache();
    let cached = |cache: &StatusCache| {
        let entries = cache
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .get(&key)
            .filter(|(at, _)| at.elapsed() < STATUS_TTL)
            .map(|(_, status)| status.clone())
    };
    if !fresh && let Some(status) = cached(cache) {
        return status;
    }
    let _permit = cache.permits.acquire().await;
    // 排队的时候别人可能已经问过了
    if !fresh && let Some(status) = cached(cache) {
        return status;
    }
    let status = status::ping(&key).await;
    let mut entries = cache
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if entries.len() > 256 {
        entries.retain(|_, (at, _)| at.elapsed() < STATUS_TTL);
    }
    entries.insert(key, (std::time::Instant::now(), status.clone()));
    status
}
