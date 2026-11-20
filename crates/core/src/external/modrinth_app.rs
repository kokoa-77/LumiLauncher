//! 读 Modrinth App 的实例清单（D19）。
//!
//! Modrinth App 把实例信息存在自己的数据库 `%APPDATA%\ModrinthApp\app.db`（SQLite）里，
//! 实例文件夹在 `<数据文件夹>/profiles/<路径>/`（数据文件夹可以在它的设置里换，记在 `settings.custom_dir`）。
//! 新版（2026 年起）是 `instances` 表 + `instance_content_sets` 表（游戏版本、加载器），老版本是一张 `profiles` 表，两种都认。
//!
//! 只读打开（Modrinth App 开着也不影响它），只查实例的名字、文件夹、游戏版本、加载器这几列。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::Launcher;
use super::foreign::ForeignInstance;
use crate::instance::{LoaderKind, LoaderRef};

/// Modrinth App 的数据库
fn database() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let path = PathBuf::from(appdata).join("ModrinthApp").join("app.db");
    path.is_file().then_some(path)
}

/// 电脑上 Modrinth App 的全部实例；没装、读不了时是空的（读不了记日志）
pub fn instances() -> Vec<ForeignInstance> {
    let Some(db) = database() else {
        return Vec::new();
    };
    let Some(data_dir) = db.parent() else {
        return Vec::new();
    };
    match read(&db, data_dir) {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!(%error, "读不了 Modrinth App 的实例清单");
            Vec::new()
        }
    }
}

/// 一行实例：文件夹（相对 profiles/）、名字、游戏版本、加载器、加载器版本
type Row = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn read(db: &Path, data_dir: &Path) -> rusqlite::Result<Vec<ForeignInstance>> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    let has_table = |name: &str| -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .is_ok()
    };
    let custom_dir: Option<String> = if has_table("settings") {
        conn.query_row("SELECT custom_dir FROM settings LIMIT 1", [], |r| r.get(0))
            .unwrap_or(None)
    } else {
        None
    };
    let base = custom_dir
        .filter(|d| !d.trim().is_empty())
        .map_or_else(|| data_dir.to_path_buf(), PathBuf::from)
        .join("profiles");

    let sql = if has_table("instances") && has_table("instance_content_sets") {
        "SELECT i.path, i.name, s.game_version, s.loader, s.loader_version
         FROM instances i LEFT JOIN instance_content_sets s ON s.id = i.applied_content_set_id"
    } else if has_table("profiles") {
        "SELECT path, name, game_version, mod_loader, mod_loader_version FROM profiles"
    } else {
        return Ok(Vec::new());
    };
    let mut statement = conn.prepare(sql)?;
    let rows: Vec<Row> = statement
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|row| to_instance(&base, row))
        .collect())
}

fn to_instance(
    base: &Path,
    (path, name, game_version, loader, loader_version): Row,
) -> Option<ForeignInstance> {
    // 文件夹名来自别人的数据库：只认单层的名字，不让它指到 profiles/ 外面
    let folder_name = crate::fs::safe_file_name(&path).ok()?;
    let folder = base.join(folder_name);
    if !folder.is_dir() {
        return None;
    }
    let game_version = game_version.filter(|v| !v.is_empty())?;
    let kind = match loader.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("neoforge") => Some(LoaderKind::NeoForge),
        Some("forge") => Some(LoaderKind::Forge),
        Some("fabric") => Some(LoaderKind::Fabric),
        Some("quilt") => Some(LoaderKind::Quilt),
        _ => None,
    };
    let loader = kind.and_then(|kind| {
        let version = loader_version.filter(|v| !v.is_empty())?;
        Some(LoaderRef { kind, version })
    });
    Some(ForeignInstance {
        launcher: Launcher::ModrinthApp,
        name: if name.trim().is_empty() { path } else { name },
        game_version,
        loader,
        game_dir: folder.clone(),
        folder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_new_and_old_layouts() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("app.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (custom_dir TEXT);
             INSERT INTO settings VALUES (NULL);
             CREATE TABLE instances (id TEXT, path TEXT, name TEXT, applied_content_set_id TEXT);
             CREATE TABLE instance_content_sets (id TEXT, game_version TEXT, loader TEXT, loader_version TEXT);
             INSERT INTO instances VALUES ('1', '生存', '我的生存', 'a'), ('2', '..', '坏的', 'b'), ('3', '没文件夹', 'x', 'c');
             INSERT INTO instance_content_sets VALUES ('a', '1.21.1', 'neoforge', '21.1.77'), ('b', '1.21.1', 'vanilla', NULL), ('c', '1.20.1', 'fabric', '0.15.3');",
        )
        .unwrap();
        drop(conn);
        std::fs::create_dir_all(temp.path().join("profiles").join("生存")).unwrap();

        let found = read(&db, temp.path()).unwrap();
        assert_eq!(found.len(), 1, "跑到外面的、文件夹不在的不要");
        assert_eq!(found[0].name, "我的生存");
        assert_eq!(found[0].launcher, Launcher::ModrinthApp);
        assert_eq!(
            found[0].loader,
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.77".into()
            })
        );

        let old = temp.path().join("old.db");
        let conn = Connection::open(&old).unwrap();
        conn.execute_batch(
            "CREATE TABLE profiles (path TEXT, name TEXT, game_version TEXT, mod_loader TEXT, mod_loader_version TEXT);
             INSERT INTO profiles VALUES ('生存', '老的', '1.20.1', 'vanilla', NULL);",
        )
        .unwrap();
        drop(conn);
        let found = read(&old, temp.path()).unwrap();
        assert_eq!(found[0].name, "老的");
        assert_eq!(found[0].loader, None);
    }
}
