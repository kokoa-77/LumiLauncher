//! 收藏夹：资源中心里点了「收藏」的项目。
//!
//! 存在启动器的数据目录（`favorites.json`，和设置放在一起），换游戏目录也还在。
//! 每一项原样存一份搜索结果（名字、图标、作者……），打开收藏夹不用联网。
//! 文件坏了就备份成 `favorites.json.corrupt-<时间>`，从空的开始，不让启动器打不开。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::ContentSource;
use super::detail::https_only;
use super::modrinth::ProjectHit;
use crate::fs::{AtomicWriteError, ReadJsonError, read_json, write_json_atomic};

/// 收藏的是哪一类（决定点开后装到哪里、怎么装）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum FavoriteKind {
    Modpack,
    Mod,
    ResourcePack,
    Shader,
    /// 地图（CurseForge 的 Worlds）
    Map,
    /// 多人服务器
    Server,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub kind: FavoriteKind,
    pub hit: ProjectHit,
    /// 收藏的时间（毫秒时间戳）
    pub added_at: f64,
}

const SCHEMA: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FavoritesFile {
    schema_version: u32,
    #[serde(default)]
    items: Vec<Favorite>,
}

#[derive(Debug, thiserror::Error)]
pub enum FavoritesError {
    #[error("保存收藏夹失败")]
    Write(#[from] AtomicWriteError),
}

pub struct Favorites {
    path: PathBuf,
    items: Mutex<Vec<Favorite>>,
}

/// 同一个项目：平台 + 平台上的编号
fn same(a: &ProjectHit, source: ContentSource, project_id: &str) -> bool {
    a.source == source && a.project_id == project_id
}

/// 界面传回来的数据：地址只留 https 的（以后会放进图片、交给浏览器）
fn sanitized(mut hit: ProjectHit) -> ProjectHit {
    hit.icon_url = hit.icon_url.and_then(https_only);
    hit.page_url = hit.page_url.and_then(https_only);
    hit.gallery_url = hit.gallery_url.and_then(https_only);
    hit.author_url = hit.author_url.and_then(https_only);
    hit
}

impl Favorites {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let items = load_or_recover(&path);
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Favorite>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 全部收藏，后收藏的在前
    pub fn list(&self) -> Vec<Favorite> {
        let mut items = self.lock().clone();
        items.sort_by(|a, b| b.added_at.total_cmp(&a.added_at));
        items
    }

    /// 收藏 / 取消收藏，返回之后是不是在收藏夹里。保存失败时内存里的也不变。
    pub fn toggle(
        &self,
        kind: FavoriteKind,
        hit: ProjectHit,
        now_ms: f64,
    ) -> Result<bool, FavoritesError> {
        let mut items = self.lock();
        let mut next = items.clone();
        let existed = next
            .iter()
            .any(|f| same(&f.hit, hit.source, &hit.project_id));
        if existed {
            next.retain(|f| !same(&f.hit, hit.source, &hit.project_id));
        } else {
            next.push(Favorite {
                kind,
                hit: sanitized(hit),
                added_at: now_ms,
            });
        }
        write_json_atomic(
            &self.path,
            &FavoritesFile {
                schema_version: SCHEMA,
                items: next.clone(),
            },
        )?;
        *items = next;
        Ok(!existed)
    }
}

fn load_or_recover(path: &Path) -> Vec<Favorite> {
    match read_json::<FavoritesFile>(path) {
        Ok(None) => Vec::new(),
        Ok(Some(file)) => file.items,
        Err(ReadJsonError::Parse { source, .. }) => {
            let backup = path.with_file_name(format!(
                "favorites.json.corrupt-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            ));
            match std::fs::rename(path, &backup) {
                Ok(()) => tracing::warn!(
                    backup = %backup.display(),
                    reason = %source,
                    "收藏夹文件坏了，已备份，从空的收藏夹开始"
                ),
                Err(error) => tracing::error!(
                    path = %path.display(),
                    %error,
                    "收藏夹文件坏了，备份也失败，本次从空的收藏夹开始"
                ),
            }
            Vec::new()
        }
        Err(error @ ReadJsonError::Io { .. }) => {
            tracing::error!(path = %path.display(), %error, "读取收藏夹失败，本次从空的收藏夹开始");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, source: ContentSource) -> ProjectHit {
        ProjectHit {
            project_id: id.into(),
            slug: id.into(),
            title: format!("项目 {id}"),
            description: String::new(),
            author: "a".into(),
            icon_url: Some("javascript:alert(1)".into()),
            downloads: 1.0,
            display_categories: Vec::new(),
            source,
            side: None,
            page_url: Some("https://modrinth.com/mod/x".into()),
            gallery_url: None,
            author_url: None,
            updated: None,
            published: None,
            server: None,
            manual_only: false,
            title_zh: None,
            twin_id: None,
        }
    }

    #[test]
    fn toggle_adds_removes_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("favorites.json");
        let store = Favorites::open(&path);
        assert!(store.list().is_empty());

        assert!(
            store
                .toggle(FavoriteKind::Mod, hit("a", ContentSource::Modrinth), 1.0)
                .unwrap()
        );
        assert!(
            store
                .toggle(
                    FavoriteKind::Modpack,
                    hit("b", ContentSource::CurseForge),
                    2.0
                )
                .unwrap()
        );
        // 同一个编号、不同平台是两个项目
        assert!(
            store
                .toggle(FavoriteKind::Mod, hit("a", ContentSource::CurseForge), 3.0)
                .unwrap()
        );

        let reopened = Favorites::open(&path);
        let list = reopened.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].added_at, 3.0, "后收藏的在前");
        // 不是 https 的地址存之前就去掉了
        assert_eq!(list[0].hit.icon_url, None);
        assert!(list[0].hit.page_url.is_some());

        assert!(
            !reopened
                .toggle(FavoriteKind::Mod, hit("a", ContentSource::Modrinth), 4.0)
                .unwrap()
        );
        assert_eq!(Favorites::open(&path).list().len(), 2);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("favorites.json");
        std::fs::write(&path, "{ 坏了").unwrap();
        let store = Favorites::open(&path);
        assert!(store.list().is_empty());
        assert!(!path.exists(), "坏文件被改名备份了");
        let backups = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(backups, 1);
    }
}
