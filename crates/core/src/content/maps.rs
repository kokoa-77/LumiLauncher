//! 地图：CurseForge 的 Worlds 分类（Modrinth 没有地图这一类，2026-11-12 实测搜不到）。
//!
//! 装一张地图 = 挑文件 → 下载（校验哈希和大小）→ 解压到实例的 `saves` 里。
//! 压缩包是网上来的，不可信：
//! - 存档根目录按 `level.dat` 的位置认（有的包把世界放在根上，有的套了一两层文件夹）
//! - 每个条目的路径都经过 [`safe_join`]，符号链接直接跳过
//! - 解压出来的总大小按真正写出的字节算，超过上限就停（防压缩炸弹，条目上写的大小可以造假）
//! - 先解到一个临时文件夹，全部成功才改名成最终的名字，失败不留半个世界

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use super::ContentError;
use super::curseforge::CurseForge;
use crate::fs::{UnsafePath, safe_join};
use crate::net::{DownloadError, DownloadItem, DownloadProgress, Downloader};

/// 解压出来最多多大（字节）
const MAX_TOTAL: u64 = 4 * 1024 * 1024 * 1024;
/// 最多多少个条目
const MAX_ENTRIES: usize = 200_000;

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error("下载地图失败")]
    Download(#[from] DownloadError),
    #[error("这张地图的作者不允许第三方启动器下载，请到 CurseForge 页面手动下载")]
    Manual { page_url: Option<String> },
    #[error("压缩包里没有找到存档（level.dat）")]
    NoWorld,
    #[error("地图解压出来太大了（超过 4 GB），停止安装")]
    TooLarge,
    #[error("地图压缩包里有不安全的路径")]
    Unsafe(#[from] UnsafePath),
    #[error("地图压缩包坏了：{0}")]
    BadArchive(String),
    #[error("读写存档文件夹失败")]
    Io(#[from] std::io::Error),
}

/// 装地图要用到的东西
pub struct MapInstall<'a> {
    pub curseforge: &'a CurseForge,
    pub downloader: &'a Downloader,
    /// 实例的游戏目录（里面有 `saves`）
    pub instance_dir: &'a Path,
    /// 放临时下载文件的地方
    pub temp_dir: &'a Path,
    pub game_version: &'a str,
    pub cancel: &'a CancellationToken,
}

/// 装一张地图，返回它在 `saves` 里的文件夹名（单人游戏列表里就是它）。
pub async fn install<F>(
    ctx: &MapInstall<'_>,
    mod_id: u32,
    title: &str,
    on_progress: F,
) -> Result<String, MapError>
where
    F: Fn(DownloadProgress) + Send + Sync,
{
    let file = ctx
        .curseforge
        .pick_world_file(mod_id, ctx.game_version)
        .await?;
    let Some(url) = file
        .download_url
        .clone()
        .filter(|u| u.starts_with("https://"))
    else {
        return Err(MapError::Manual { page_url: None });
    };
    std::fs::create_dir_all(ctx.temp_dir)?;
    let archive = ctx.temp_dir.join(format!("map-{mod_id}-{}.zip", file.id));
    let item = DownloadItem {
        urls: vec![url],
        path: archive.clone(),
        sha1: file.sha1(),
        size: Some(file.file_length),
    };
    ctx.downloader
        .download_all(std::slice::from_ref(&item), ctx.cancel, on_progress)
        .await?;
    let saves = ctx.instance_dir.join("saves");
    let fallback = title.to_owned();
    let archive_for_task = archive.clone();
    let result =
        tokio::task::spawn_blocking(move || extract_world(&archive_for_task, &saves, &fallback))
            .await
            .map_err(|e| MapError::BadArchive(e.to_string()))?;
    if let Err(error) = std::fs::remove_file(&archive) {
        tracing::warn!(%error, path = %archive.display(), "删除地图临时文件失败");
    }
    result
}

/// 文件夹名里不能有的字符换成空格，去掉首尾的点和空格；什么都不剩时叫「地图」
fn folder_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "地图".to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// `saves` 里不重名的文件夹名：重名时加「 (2)」「 (3)」……
fn unique_folder(saves: &Path, wanted: &str) -> String {
    if !saves.join(wanted).exists() {
        return wanted.to_owned();
    }
    (2..)
        .map(|n| format!("{wanted} ({n})"))
        .find(|name| !saves.join(name).exists())
        .unwrap_or_else(|| wanted.to_owned())
}

/// 把一个世界文件夹（拖进窗口的）复制到 `saves/<名字>`，返回名字。原文件夹不动。
/// 符号链接跳过；条目数和总大小有上限；先复制到临时文件夹，全部成功才改名。
pub fn copy_world_dir(source: &Path, saves: &Path) -> Result<String, MapError> {
    if !source.join("level.dat").is_file() {
        return Err(MapError::NoWorld);
    }
    let raw = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = unique_folder(saves, &folder_name(&raw));
    std::fs::create_dir_all(saves)?;
    let staging = saves.join(format!(".mcl-incoming-{}", uuid::Uuid::new_v4().simple()));
    let mut budget = (MAX_ENTRIES, MAX_TOTAL);
    if let Err(error) = copy_tree(source, &staging, &mut budget) {
        if let Err(cleanup) = std::fs::remove_dir_all(&staging) {
            tracing::warn!(%cleanup, path = %staging.display(), "清理没复制完的世界失败");
        }
        return Err(error);
    }
    std::fs::rename(&staging, saves.join(&name))?;
    Ok(name)
}

/// 递归复制（不跟随符号链接），`budget` 是剩下的条目数和字节数
fn copy_tree(from: &Path, to: &Path, budget: &mut (usize, u64)) -> Result<(), MapError> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            tracing::warn!(path = %entry.path().display(), "世界文件夹里有符号链接，跳过");
            continue;
        }
        budget.0 = budget.0.checked_sub(1).ok_or(MapError::TooLarge)?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, budget)?;
        } else if kind.is_file() {
            let size = entry.metadata()?.len();
            budget.1 = budget.1.checked_sub(size).ok_or(MapError::TooLarge)?;
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 把压缩包里的世界解压到 `saves/<名字>`，返回名字。
pub fn extract_world(
    archive: &Path,
    saves: &Path,
    fallback_name: &str,
) -> Result<String, MapError> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| MapError::BadArchive(e.to_string()))?;
    if zip.len() > MAX_ENTRIES {
        return Err(MapError::TooLarge);
    }
    // 存档根目录：最浅的那个 level.dat 所在的文件夹
    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| MapError::BadArchive(e.to_string()))?;
        // 文件名不是合法 UTF-8 的条目不可能是 level.dat，跳过
        if let Ok(name) = entry.name() {
            names.push(name.replace('\\', "/"));
        }
    }
    let root = names
        .iter()
        .filter(|n| {
            n.rsplit('/')
                .next()
                .is_some_and(|f| f.eq_ignore_ascii_case("level.dat"))
        })
        .min_by_key(|n| n.matches('/').count())
        .map(|n| n[..n.len() - "level.dat".len()].to_owned())
        .ok_or(MapError::NoWorld)?;
    let root_folder = root.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let name = unique_folder(
        saves,
        &folder_name(if root_folder.is_empty() {
            fallback_name
        } else {
            root_folder
        }),
    );

    std::fs::create_dir_all(saves)?;
    let staging = saves.join(format!(".mcl-incoming-{}", uuid::Uuid::new_v4().simple()));
    let outcome = unpack(&mut zip, &root, &staging);
    if let Err(error) = outcome {
        if let Err(cleanup) = std::fs::remove_dir_all(&staging) {
            tracing::warn!(%cleanup, path = %staging.display(), "清理没装完的地图失败");
        }
        return Err(error);
    }
    std::fs::rename(&staging, saves.join(&name))?;
    Ok(name)
}

fn unpack(zip: &mut zip::ZipArchive<File>, root: &str, dest: &PathBuf) -> Result<(), MapError> {
    std::fs::create_dir_all(dest)?;
    let mut written: u64 = 0;
    let mut buffer = vec![0u8; 64 * 1024];
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| MapError::BadArchive(e.to_string()))?;
        let name = match entry.name() {
            Ok(name) => name.replace('\\', "/"),
            Err(error) => {
                tracing::warn!(%error, "地图压缩包里有无法识别的文件名，跳过");
                continue;
            }
        };
        if entry.is_symlink() {
            tracing::warn!(entry = %name, "地图压缩包里有符号链接，跳过");
            continue;
        }
        let Some(relative) = name.strip_prefix(root) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        let target = safe_join(dest, relative.trim_end_matches('/'))?;
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target)?;
        loop {
            let n = entry
                .read(&mut buffer)
                .map_err(|e| MapError::BadArchive(e.to_string()))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > MAX_TOTAL {
                return Err(MapError::TooLarge);
            }
            out.write_all(&buffer[..n])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn archive(entries: &[(&str, &str)]) -> tempfile::NamedTempFile {
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(out.reopen().unwrap());
        let options = SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out
    }

    #[test]
    fn finds_the_world_inside_wrapper_folders() {
        let zip = archive(&[
            ("readme.txt", "看我"),
            ("Skyblock v2/Skyblock/level.dat", "nbt"),
            ("Skyblock v2/Skyblock/region/r.0.0.mca", "region"),
            ("Skyblock v2/Skyblock/datapacks/x/pack.mcmeta", "{}"),
        ]);
        let saves = tempfile::tempdir().unwrap();
        let name = extract_world(zip.path(), saves.path(), "空岛").unwrap();
        assert_eq!(name, "Skyblock");
        let world = saves.path().join("Skyblock");
        assert!(world.join("level.dat").exists());
        assert!(world.join("region/r.0.0.mca").exists());
        assert!(
            !world.join("readme.txt").exists(),
            "存档根目录外面的东西不要"
        );
        // 再装一次不覆盖，换个名字
        let again = extract_world(zip.path(), saves.path(), "空岛").unwrap();
        assert_eq!(again, "Skyblock (2)");
    }

    #[test]
    fn dropped_world_folders_are_copied_without_touching_the_original() {
        let source = tempfile::tempdir().unwrap();
        let world = source.path().join("空岛生存");
        std::fs::create_dir_all(world.join("region")).unwrap();
        std::fs::write(world.join("level.dat"), "nbt").unwrap();
        std::fs::write(world.join("region/r.0.0.mca"), "x").unwrap();
        let saves = tempfile::tempdir().unwrap();
        assert_eq!(copy_world_dir(&world, saves.path()).unwrap(), "空岛生存");
        assert!(saves.path().join("空岛生存/region/r.0.0.mca").is_file());
        assert!(world.join("level.dat").is_file(), "原文件夹还在");
        assert_eq!(
            copy_world_dir(&world, saves.path()).unwrap(),
            "空岛生存 (2)"
        );
        assert!(matches!(
            copy_world_dir(source.path(), saves.path()),
            Err(MapError::NoWorld)
        ));
    }

    #[test]
    fn world_at_the_root_uses_the_project_name() {
        let zip = archive(&[("level.dat", "nbt"), ("region/r.0.0.mca", "x")]);
        let saves = tempfile::tempdir().unwrap();
        let name = extract_world(zip.path(), saves.path(), "跑酷: 第一章?").unwrap();
        assert_eq!(name, "跑酷  第一章");
        assert!(saves.path().join(&name).join("level.dat").exists());
    }

    #[test]
    fn no_level_dat_and_unsafe_paths_are_rejected() {
        let saves = tempfile::tempdir().unwrap();
        let empty = archive(&[("readme.txt", "x")]);
        assert!(matches!(
            extract_world(empty.path(), saves.path(), "x"),
            Err(MapError::NoWorld)
        ));
        let evil = archive(&[("level.dat", "nbt"), ("../../evil.txt", "x")]);
        assert!(matches!(
            extract_world(evil.path(), saves.path(), "x"),
            Err(MapError::Unsafe(_))
        ));
        // 失败了不留半个世界
        let left: Vec<_> = std::fs::read_dir(saves.path()).unwrap().collect();
        assert!(left.is_empty(), "临时文件夹已经清掉");
    }
}
