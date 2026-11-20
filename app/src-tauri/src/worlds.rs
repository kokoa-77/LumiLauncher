//! 存档相关命令（招牌特色 1「直达你的世界」）。

use std::collections::HashMap;

use launcher_core::saves::cover::{self, CoverError, ImageFit};
use launcher_core::saves::notes::{self, NoteLine, WorldNotes};
use launcher_core::saves::{self, GameMode, ImageKind, World};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::state::AppState;

/// 给界面看的世界信息：图片换成启动器图片协议的地址，不给路径。
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WorldView {
    /// 存档文件夹名：启动时传回来，用来直接进这个存档
    pub folder: String,
    pub name: String,
    /// 上次游玩（毫秒时间戳）
    pub last_played: f64,
    /// 游戏里第几天（从 1 开始）
    pub day: f64,
    pub game_mode: Option<GameMode>,
    pub hardcore: bool,
    pub version: Option<String>,
    /// 玩了多久（秒）
    pub play_seconds: Option<f64>,
    pub image_url: Option<String>,
    pub image_kind: Option<ImageKind>,
    /// 铺满还是完整显示（只有自己选的图可能是完整显示）
    pub image_fit: ImageFit,
    /// 世界种子（文字：64 位整数界面放不下）
    pub seed: Option<String>,
    /// 世界笔记在首页显示的那一行（D17j）
    pub note: Option<NoteLine>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WorldList {
    /// 这个实例的游戏版本支不支持启动时直接进存档（1.20 起）
    pub quick_play: bool,
    /// 最近玩过的在前
    pub worlds: Vec<WorldView>,
}

/// 一个实例最近玩的世界（首页换世界、实例封面用）。
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorld {
    pub instance_id: String,
    /// 这个实例支不支持直接进存档
    pub quick_play: bool,
    pub world: WorldView,
}

/// 一个实例 `servers.dat` 里的多人服务器（首页「直达你的世界与服务器」用，D25d）。
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentServer {
    pub instance_id: String,
    /// 这个实例的游戏版本支不支持启动时直接连服（1.20 起）
    pub quick_play: bool,
    pub name: String,
    pub address: String,
}

/// 供精选弹窗展示的一张自动封面（D25d）；图片走启动器协议地址，不暴露路径。
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoCoverView {
    /// 锁定封面时传回的条目文件名（仅作标识，由核心库经 `safe_join` 校验）
    pub file: String,
    pub url: String,
    /// 截图时间（毫秒）
    pub taken_at: f64,
    pub active: bool,
    pub locked: bool,
}

fn view(state: &AppState, world: World) -> WorldView {
    WorldView {
        image_url: world.image.as_ref().map(|i| state.images.url_for(&i.path)),
        image_kind: world.image.as_ref().map(|i| i.kind),
        image_fit: world.image.as_ref().map_or(ImageFit::Cover, |i| i.fit),
        folder: world.folder,
        name: world.name,
        last_played: world.last_played,
        day: world.day,
        game_mode: world.game_mode,
        hardcore: world.hardcore,
        version: world.version,
        play_seconds: world.play_seconds,
        seed: world.seed,
        note: world.note,
    }
}

/// 一个实例里的单人世界（读 `level.dat`、游玩统计、截图），最近玩过的在前。
#[tauri::command]
#[specta::specta]
pub async fn worlds_list(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<WorldList, AppError> {
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = instance.game_dir(&game)?;
    let player = crate::accounts::current_uuid(&state).to_string();
    let worlds = tauri::async_runtime::spawn_blocking(move || saves::list(&dir, &player))
        .await
        .map_err(|e| AppError::new("worlds.failed", "读取存档失败").with_detail(e))?;
    Ok(WorldList {
        quick_play: saves::supports_quick_play(&instance.config.game_version),
        worlds: worlds.into_iter().map(|w| view(&state, w)).collect(),
    })
}

/// 每个实例最近玩的那个世界，最近玩过的在前；没有存档的实例不在里面。
/// 首页「换一个世界」和实例库的封面都用它。只读存档，不改任何文件。
#[tauri::command]
#[specta::specta]
pub async fn worlds_recent(state: State<'_, AppState>) -> Result<Vec<RecentWorld>, AppError> {
    let game = state.game();
    let player = crate::accounts::current_uuid(&state).to_string();
    let found = tauri::async_runtime::spawn_blocking(move || {
        let mut by_dir = HashMap::new();
        launcher_core::instance::list(&game)
            .into_iter()
            .filter_map(|instance| {
                let dir = instance.game_dir(&game).ok()?;
                let key = crate::turns::directory_key(&dir);
                let world = by_dir
                    .entry(key)
                    .or_insert_with(|| saves::recent(&dir, &player))
                    .clone()?;
                let quick_play = saves::supports_quick_play(&instance.config.game_version);
                Some((instance.id, quick_play, world))
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| AppError::new("worlds.failed", "读取存档失败").with_detail(e))?;
    let mut recent: Vec<RecentWorld> = found
        .into_iter()
        .map(|(instance_id, quick_play, world)| RecentWorld {
            instance_id,
            quick_play,
            world: view(&state, world),
        })
        .collect();
    recent.sort_by(|a, b| b.world.last_played.total_cmp(&a.world.last_played));
    Ok(recent)
}

/// 各实例 `servers.dat` 里的多人服务器，按实例最近游玩顺序排（同地址同实例去重，D25d）。
#[tauri::command]
#[specta::specta]
pub async fn worlds_recent_servers(
    state: State<'_, AppState>,
) -> Result<Vec<RecentServer>, AppError> {
    let game = state.game();
    let found = tauri::async_runtime::spawn_blocking(move || {
        let mut instances = launcher_core::instance::list(&game);
        instances.sort_by(|a, b| {
            let key = |i: &launcher_core::instance::Instance| {
                i.config
                    .last_played
                    .unwrap_or(0.0)
                    .max(i.config.created_at / 1e6)
            };
            b.config
                .last_played
                .is_some()
                .cmp(&a.config.last_played.is_some())
                .then_with(|| key(b).total_cmp(&key(a)))
        });
        let mut by_dir = HashMap::new();
        let mut out = Vec::new();
        for instance in instances {
            let Ok(dir) = instance.game_dir(&game) else {
                continue;
            };
            let key = crate::turns::directory_key(&dir);
            let entries = by_dir
                .entry(key)
                .or_insert_with(|| saves::servers::list(&dir));
            let quick_play = saves::supports_quick_play(&instance.config.game_version);
            for entry in entries.iter().take(6) {
                out.push(RecentServer {
                    instance_id: instance.id.clone(),
                    quick_play,
                    name: entry.name.clone(),
                    address: entry.address.clone(),
                });
            }
        }
        out
    })
    .await
    .map_err(|e| AppError::new("servers.failed", "读取服务器列表失败").with_detail(e))?;
    Ok(found)
}

/// 实例的一张截图：图片换成启动器图片协议的地址，不给路径。
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotView {
    pub url: String,
    pub name: String,
    /// 截图时间（毫秒）
    pub taken_at: f64,
}

/// 实例的截图，新的在前。
#[tauri::command]
#[specta::specta]
pub async fn instances_screenshots(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Vec<ScreenshotView>, AppError> {
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = instance.game_dir(&game)?;
    let shots = tauri::async_runtime::spawn_blocking(move || saves::list_screenshots(&dir))
        .await
        .map_err(|e| AppError::new("screenshots.failed", "读取截图失败").with_detail(e))?;
    Ok(shots
        .into_iter()
        .map(|s| ScreenshotView {
            url: state.images.url_for(&s.path),
            name: s.name,
            taken_at: s.taken_at,
        })
        .collect())
}

fn open_dir(app: &AppHandle, dir: &std::path::Path, what: &str) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| AppError::new("folder.open_failed", format!("打不开{what}")).with_detail(e))
}

/// 打开实例的截图文件夹（还没有就建一个）。
#[tauri::command]
#[specta::specta]
pub fn instances_open_screenshots(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), AppError> {
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = instance.game_dir(&game)?.join("screenshots");
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::new("folder.open_failed", "打不开截图文件夹").with_detail(e))?;
    open_dir(&app, &dir, "截图文件夹")
}

/// 打开一个存档的文件夹。
#[tauri::command]
#[specta::specta]
pub fn worlds_open_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<(), AppError> {
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = saves::world_dir(&instance.game_dir(&game)?, &folder)?;
    if !dir.is_dir() {
        return Err(AppError::new(
            "world.not_found",
            "找不到这个存档，可能已经被删除",
        ));
    }
    open_dir(&app, &dir, "存档文件夹")
}

// ---------- 世界笔记（D17j） ----------

/// 一个世界的笔记（没写过是空的）
#[tauri::command]
#[specta::specta]
pub fn worlds_notes_get(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<WorldNotes, AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    Ok(notes::load(&dir, &folder))
}

/// 存一个世界的笔记，返回整理后的样子（空的待办去掉、太长的截短）
#[tauri::command]
#[specta::specta]
pub fn worlds_notes_set(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    notes: WorldNotes,
) -> Result<WorldNotes, AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    let saved = notes::save(&dir, &folder, notes)?;
    tracing::info!(instance = %instance_id, folder, "存了世界笔记");
    Ok(saved)
}

// ---------- 自定义背景（D17d） ----------

/// 一个世界所在实例的游戏目录（存档名是界面传回来的：先确认这个存档真的在）
fn world_owner(
    state: &AppState,
    instance_id: &str,
    folder: &str,
) -> Result<std::path::PathBuf, AppError> {
    let game = state.game();
    let instance = launcher_core::instance::load(&game, instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = instance.game_dir(&game)?;
    if !saves::world_dir(&dir, folder)?.is_dir() {
        return Err(AppError::new(
            "world.not_found",
            "找不到这个存档，可能已经被删除",
        ));
    }
    Ok(dir)
}

fn background_error(error: CoverError) -> AppError {
    let code = match &error {
        CoverError::TooLarge => "background.too_large",
        CoverError::Unsupported => "background.unsupported",
        CoverError::TooSmall => "background.too_small",
        CoverError::Image(_) | CoverError::Io(_) => "background.unreadable",
        CoverError::Write(_) | CoverError::Read(_) => "background.save_failed",
    };
    tracing::warn!(%error, "换背景图失败");
    AppError::new(code, error.to_string()).with_detail(&error)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// 给一个世界换背景图：在后端弹出选图片的窗口（界面拿不到路径），检查后复制一份到实例自己的文件夹。
/// 选了返回 `true`，取消了返回 `false`。首页、实例卡片、实例页横幅都会换成这张图，主色跟着变。
#[tauri::command]
#[specta::specta]
pub async fn worlds_background_pick(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<bool, AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    let Some(source) = crate::game::pick(&app, crate::game::Pick::Image).await? else {
        return Ok(false);
    };
    let fit = tauri::async_runtime::spawn_blocking(move || {
        cover::set_custom(&dir, &folder, &source, now_ms())
    })
    .await
    .map_err(|e| AppError::new("background.save_failed", "保存背景图失败").with_detail(e))?
    .map_err(background_error)?;
    tracing::info!(instance = %instance_id, ?fit, "世界换了背景图");
    Ok(true)
}

/// 自己选的背景图换个显示方式：铺满 / 完整显示。
#[tauri::command]
#[specta::specta]
pub fn worlds_background_fit(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    fit: ImageFit,
) -> Result<(), AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    if cover::set_fit(&dir, &folder, fit).map_err(background_error)? {
        Ok(())
    } else {
        Err(AppError::new("background.none", "这个世界还没有选背景图"))
    }
}

/// 恢复默认背景：删掉自己选的图，回到截图 / 自动封面 / 世界图标。
#[tauri::command]
#[specta::specta]
pub fn worlds_background_clear(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<(), AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    cover::clear_custom(&dir, &folder).map_err(background_error)?;
    Ok(())
}

/// 列出一个世界最近保存的自动封面（最多 5 张，新的在前，D25d）。
#[tauri::command]
#[specta::specta]
pub async fn worlds_auto_covers_list(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<Vec<AutoCoverView>, AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    let items = tauri::async_runtime::spawn_blocking(move || cover::list_auto(&dir, &folder))
        .await
        .map_err(|e| AppError::new("background.unreadable", "读取自动封面失败").with_detail(e))?;
    Ok(items
        .into_iter()
        .map(|item| AutoCoverView {
            file: item.file,
            url: state.images.url_for(&item.path),
            taken_at: item.time as f64,
            active: item.active,
            locked: item.locked,
        })
        .collect())
}

/// 锁定某张自动封面为这个世界的固定封面（锁定期间自动封面不再覆盖，D25d）。
#[tauri::command]
#[specta::specta]
pub fn worlds_auto_cover_lock(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
    file: String,
) -> Result<(), AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    if cover::lock_auto(&dir, &folder, &file).map_err(background_error)? {
        tracing::info!(instance = %instance_id, folder, file, "锁定了自动封面");
        Ok(())
    } else {
        Err(AppError::new(
            "background.none",
            "找不到这张自动封面，可能已被替换",
        ))
    }
}

/// 解除自动封面锁定，恢复游玩时自动更新（D25d）。
#[tauri::command]
#[specta::specta]
pub fn worlds_auto_cover_unlock(
    state: State<'_, AppState>,
    instance_id: String,
    folder: String,
) -> Result<(), AppError> {
    let dir = world_owner(&state, &instance_id, &folder)?;
    cover::unlock_auto(&dir, &folder).map_err(background_error)?;
    tracing::info!(instance = %instance_id, folder, "已恢复自动封面更新");
    Ok(())
}

/// 分享卡片 PNG 上限（16 MiB，正常 1200×675 卡片约 0.5–2 MiB）
const MAX_SHARE_CARD_BYTES: usize = 16 * 1024 * 1024;
const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn decode_png_base64(input: &str) -> Result<Vec<u8>, AppError> {
    let raw = input
        .trim()
        .strip_prefix("data:image/png;base64,")
        .unwrap_or(input.trim());
    let mut out = Vec::with_capacity((raw.len() / 4) * 3);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for b in raw.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            break;
        }
        let val: u32 = match b {
            b'A'..=b'Z' => u32::from(b - b'A'),
            b'a'..=b'z' => u32::from(b - b'a' + 26),
            b'0'..=b'9' => u32::from(b - b'0' + 52),
            b'+' => 62,
            b'/' => 63,
            _ => {
                return Err(AppError::new(
                    "share_card.invalid",
                    "分享卡片图片数据格式无效",
                ));
            }
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
            if out.len() > MAX_SHARE_CARD_BYTES {
                return Err(AppError::new("share_card.too_large", "分享卡片图片过大"));
            }
        }
    }
    if !out.starts_with(PNG_MAGIC) {
        return Err(AppError::new(
            "share_card.invalid",
            "生成的分享卡片不是有效的 PNG 图片",
        ));
    }
    Ok(out)
}

fn sanitize_card_file_name(suggested: &str) -> String {
    let cleaned: String = suggested
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let stem = if trimmed.is_empty() {
        "lumi-world-card"
    } else {
        trimmed
    };
    if stem.to_ascii_lowercase().ends_with(".png") {
        stem.to_owned()
    } else {
        format!("{stem}.png")
    }
}

/// 将前端 Canvas 渲染的世界分享卡片 PNG 通过系统「另存为」保存到磁盘（界面不接触路径，D25e）。
#[tauri::command]
#[specta::specta]
pub async fn worlds_save_share_card(
    app: AppHandle,
    suggested_name: String,
    png_base64: String,
) -> Result<bool, AppError> {
    use tauri::Manager;
    use tauri_plugin_dialog::DialogExt;

    let bytes = decode_png_base64(&png_base64)?;
    let file_name = sanitize_card_file_name(&suggested_name);
    let app_clone = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app_clone
            .dialog()
            .file()
            .set_title("保存世界分享卡片")
            .set_file_name(&file_name)
            .add_filter("PNG 图片", &["png"]);
        if let Some(window) = app_clone.get_webview_window(crate::window::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_save_file()
    })
    .await
    .map_err(|e| AppError::new("dialog.failed", "打开保存窗口失败").with_detail(e))?;
    let Some(file_path) = picked else {
        return Ok(false);
    };
    let path = file_path
        .into_path()
        .map_err(|e| AppError::new("dialog.failed", "选中的保存路径无效").with_detail(e))?;
    tauri::async_runtime::spawn_blocking(move || launcher_core::fs::write_atomic(&path, &bytes))
        .await
        .map_err(|e| AppError::new("share_card.save_failed", "保存分享卡片失败").with_detail(e))?
        .map_err(|e| AppError::new("share_card.save_failed", "保存分享卡片失败").with_detail(e))?;
    tracing::info!("已保存世界分享卡片 PNG");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_valid_png_base64_and_rejects_bad_data() {
        // 8-byte PNG signature in base64: iVBORw0KGgo=
        let decoded = decode_png_base64("data:image/png;base64,iVBORw0KGgo=").unwrap();
        assert_eq!(decoded, PNG_MAGIC);
        assert!(decode_png_base64("not-a-png").is_err());
        assert_eq!(
            sanitize_card_file_name("我的世界:冒险/1"),
            "我的世界_冒险_1.png"
        );
        assert_eq!(sanitize_card_file_name("  ...  "), "lumi-world-card.png");
    }
}
