//! 从平台拿整合包：列出能装的版本、下载整合包文件（校验哈希）。Modrinth 是 `.mrpack`，CurseForge 是 `.zip`。
//!
//! 下载完和本地导入走同一条路：先认格式、检查整个包，再导入。

use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::ModpackError;
use crate::content::curseforge::{self, CurseForge};
use crate::content::detail::BodyFormat;
use crate::content::modrinth::{self, PACK_LOADERS, Version};
use crate::instance::LoaderKind;
use crate::net::{DownloadItem, DownloadProgress, Downloader};

/// 整合包文件只从 Modrinth 自己的 CDN 下
const PACK_HOST: &str = "https://cdn.modrinth.com/";

/// 整合包的一个可装版本。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackVersion {
    pub id: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    /// 该版本使用的加载器（如 `["Fabric"]`、`["NeoForge"]`；未标明时为空）
    pub loaders: Vec<String>,
    /// 正式版（不是测试版）
    pub release: bool,
    /// `.mrpack` 文件本身的大小（不含要下载的模组）
    pub size: f64,
    /// 默认选它：最新的正式版，没有正式版就是最新的
    pub recommended: bool,
    /// 发布时间（ISO 8601）
    pub published: Option<String>,
    /// 更新日志（Modrinth 直接携带 Markdown；CurseForge 需懒加载 HTML）
    pub changelog: Option<String>,
    /// 更新日志格式（Markdown / HTML）
    pub changelog_format: BodyFormat,
}

/// 列出能装的版本（新的在前）。没有能装的版本时返回空列表。
pub async fn versions(client: &Client, project_id: &str) -> Result<Vec<PackVersion>, ModpackError> {
    let versions = modrinth::pack_versions(client, project_id).await?;
    Ok(to_pack_versions(&versions))
}

fn to_pack_versions(versions: &[Version]) -> Vec<PackVersion> {
    let usable: Vec<&Version> = versions
        .iter()
        .filter(|v| is_supported(v) && v.primary_file().is_some())
        .collect();
    let recommended = usable
        .iter()
        .position(|v| v.version_type == "release")
        .unwrap_or(0);
    usable
        .iter()
        .enumerate()
        .map(|(i, v)| PackVersion {
            id: v.id.clone(),
            version_number: v.version_number.clone(),
            game_versions: v.game_versions.clone(),
            loaders: normalize_loaders(&v.loaders),
            release: v.version_type == "release",
            size: v.primary_file().map_or(0.0, |f| f.size as f64),
            recommended: i == recommended,
            published: v.date_published.clone(),
            changelog: v
                .changelog
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            changelog_format: BodyFormat::Markdown,
        })
        .collect()
}

fn normalize_loaders(raw: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for item in raw {
        let label = match item.to_ascii_lowercase().as_str() {
            "neoforge" => "NeoForge",
            "fabric" => "Fabric",
            "forge" => "Forge",
            "quilt" => "Quilt",
            "legacyfabric" | "legacy-fabric" => "Legacy Fabric",
            "babric" => "Babric",
            _ => continue,
        };
        if !out.iter().any(|existing| existing == label) {
            out.push(label.to_owned());
        }
    }
    out
}

fn is_supported(version: &Version) -> bool {
    version.loaders.iter().any(|l| match l.as_str() {
        "forge" => version
            .game_versions
            .iter()
            .any(|mc| LoaderKind::Forge.installable_for(mc)),
        other => PACK_LOADERS.contains(&other),
    })
}

/// 把一个版本的 `.mrpack` 下到 `dir` 里（每次导入一个文件夹，见 [`super::staging_dir`]），返回文件路径。
/// 文件名由启动器自己定，不用网络返回的名字；下载地址必须是 Modrinth 的 CDN，必须有 SHA-1。
pub async fn download<F>(
    client: &Client,
    downloader: &Downloader,
    version_id: &str,
    dir: &Path,
    cancel: &CancellationToken,
    on_progress: F,
) -> Result<PathBuf, ModpackError>
where
    F: Fn(DownloadProgress) + Send + Sync,
{
    let version = modrinth::fetch_version(client, version_id).await?;
    let item = pack_file(&version, dir)?;
    let path = item.path.clone();
    downloader
        .download_all(&[item], cancel, on_progress)
        .await?;
    tracing::info!(version = %version.version_number, file = %path.display(), "整合包已下载");
    Ok(path)
}

fn pack_file(version: &Version, dir: &Path) -> Result<DownloadItem, ModpackError> {
    if !is_supported(version) {
        return Err(ModpackError::UnsupportedLoader(version.loaders.join(" / ")));
    }
    let file = version
        .primary_file()
        .ok_or_else(|| ModpackError::Invalid("这个版本没有文件".to_owned()))?;
    if !file.filename.to_ascii_lowercase().ends_with(".mrpack") {
        return Err(ModpackError::Invalid(format!(
            "{} 不是 Modrinth 整合包文件",
            file.filename
        )));
    }
    if !file.url.starts_with(PACK_HOST) {
        return Err(ModpackError::Rejected(format!(
            "整合包的下载地址不是 Modrinth：{}",
            file.url
        )));
    }
    let sha1 = file
        .hashes
        .get("sha1")
        .map(|h| h.to_ascii_lowercase())
        .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| ModpackError::Invalid("整合包没有有效的 SHA-1，无法校验".to_owned()))?;
    Ok(DownloadItem {
        urls: vec![file.url.clone()],
        // 名字固定：`dir` 是这次导入自己的文件夹，重试时下过的整合包校验通过直接用
        path: dir.join("pack.mrpack"),
        sha1: Some(sha1),
        size: (file.size > 0).then_some(file.size),
    })
}

/// CurseForge 整合包能装的版本（NeoForge，不含服务端包），新的在前。
pub async fn curseforge_versions(
    cf: &CurseForge,
    project_id: u32,
) -> Result<Vec<PackVersion>, ModpackError> {
    let files = cf.mod_files(project_id, None, None).await?;
    Ok(to_curseforge_versions(&files))
}

fn to_curseforge_versions(files: &[curseforge::File]) -> Vec<PackVersion> {
    let usable: Vec<&curseforge::File> = files.iter().filter(|f| !f.is_server_pack).collect();
    let recommended = usable.iter().position(|f| f.is_release()).unwrap_or(0);
    usable
        .iter()
        .enumerate()
        .map(|(i, f)| PackVersion {
            id: f.id.to_string(),
            version_number: if f.display_name.is_empty() {
                f.file_name.clone()
            } else {
                f.display_name.clone()
            },
            // 游戏版本列表里还混着 NeoForge、Client 这类标签，只留数字开头的
            game_versions: f
                .game_versions
                .iter()
                .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
                .cloned()
                .collect(),
            loaders: normalize_loaders(&f.game_versions),
            release: f.is_release(),
            size: f.file_length as f64,
            recommended: i == recommended,
            published: f.file_date.clone(),
            changelog: None,
            changelog_format: BodyFormat::Html,
        })
        .collect()
}

/// 把 CurseForge 整合包的一个版本下到 `dir` 里（每次导入一个文件夹），返回文件路径。
/// 地址必须是 CurseForge 的 CDN，必须有 SHA-1，文件名由启动器自己定。
pub async fn curseforge_download<F>(
    cf: &CurseForge,
    downloader: &Downloader,
    file_id: u32,
    dir: &Path,
    cancel: &CancellationToken,
    on_progress: F,
) -> Result<PathBuf, ModpackError>
where
    F: Fn(DownloadProgress) + Send + Sync,
{
    let file = cf
        .files(&[file_id])
        .await?
        .into_iter()
        .find(|f| f.id == file_id)
        .ok_or_else(|| ModpackError::Invalid("CurseForge 上找不到这个版本".to_owned()))?;
    let item = curseforge_pack_file(&file, dir)?;
    let path = item.path.clone();
    downloader
        .download_all(&[item], cancel, on_progress)
        .await?;
    tracing::info!(version = %file.display_name, file = %path.display(), "CurseForge 整合包已下载");
    Ok(path)
}

fn curseforge_pack_file(file: &curseforge::File, dir: &Path) -> Result<DownloadItem, ModpackError> {
    if !file.file_name.to_ascii_lowercase().ends_with(".zip") {
        return Err(ModpackError::Invalid(format!(
            "{} 不是 CurseForge 整合包文件",
            file.file_name
        )));
    }
    let url = match (file.usable_download_url(), &file.download_url) {
        (Some(url), _) => url,
        (None, Some(other)) => {
            return Err(ModpackError::Rejected(format!(
                "整合包的下载地址不是 CurseForge：{other}"
            )));
        }
        (None, None) => {
            return Err(ModpackError::Invalid(
                "这个整合包的作者不允许第三方启动器下载。请在 CurseForge 网页上下载，再用「新建实例 → 导入整合包文件」导入".to_owned(),
            ));
        }
    };
    let sha1 = file
        .sha1()
        .ok_or_else(|| ModpackError::Invalid("整合包没有有效的 SHA-1，无法校验".to_owned()))?;
    Ok(DownloadItem {
        urls: vec![url.to_owned()],
        path: dir.join("pack.zip"),
        sha1: Some(sha1),
        size: (file.file_length > 0).then_some(file.file_length),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::content::modrinth::VersionFile;

    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

    fn version(id: &str, kind: &str, loader: &str, url: &str) -> Version {
        Version {
            id: id.to_owned(),
            project_id: "p".to_owned(),
            name: String::new(),
            version_number: id.to_owned(),
            version_type: kind.to_owned(),
            date_published: None,
            downloads: None,
            changelog: None,
            loaders: vec![loader.to_owned()],
            game_versions: vec!["1.21.1".to_owned()],
            files: vec![VersionFile {
                url: url.to_owned(),
                filename: format!("{id}.mrpack"),
                primary: true,
                size: 1024,
                hashes: HashMap::from([("sha1".to_owned(), SHA1.to_owned())]),
            }],
            dependencies: Vec::new(),
        }
    }

    const CDN: &str = "https://cdn.modrinth.com/data/p/versions/x/pack.mrpack";

    #[test]
    fn recommends_the_newest_release_among_supported_loaders() {
        let mut unsupported_forge = version("very-old", "release", "forge", CDN);
        unsupported_forge.game_versions = vec!["1.4.7".to_owned()];
        let mut legacy_forge = version("1.12.2-forge", "release", "forge", CDN);
        legacy_forge.game_versions = vec!["1.12.2".to_owned()];
        let list = to_pack_versions(&[
            version("3-beta", "beta", "neoforge", CDN),
            version("unsupported", "release", "liteloader", CDN),
            unsupported_forge,
            legacy_forge,
            version("2-fabric", "release", "fabric", CDN),
            version("2-forge", "release", "forge", CDN),
            version("2-quilt", "release", "quilt", CDN),
            version("2", "release", "neoforge", CDN),
            version("1", "release", "neoforge", CDN),
        ]);
        let ids: Vec<&str> = list.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "3-beta",
                "1.12.2-forge",
                "2-fabric",
                "2-forge",
                "2-quilt",
                "2",
                "1"
            ]
        );
        let recommended: Vec<_> = list.iter().filter(|v| v.recommended).collect();
        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].id, "1.12.2-forge");
        assert!(!list[0].release);

        let only_beta = to_pack_versions(&[version("1-beta", "beta", "neoforge", CDN)]);
        assert!(only_beta[0].recommended, "没有正式版就推荐最新的");
    }

    fn cf_file(id: u32, release_type: u32, url: Option<&str>, server: bool) -> curseforge::File {
        let json = serde_json::json!({
            "id": id, "modId": 1, "displayName": format!("Pack {id}"), "fileName": format!("Pack-{id}.zip"),
            "releaseType": release_type, "fileLength": 2048, "downloadUrl": url,
            "gameVersions": ["1.21.1", "NeoForge"], "isServerPack": server,
            "hashes": [{"value": SHA1, "algo": 1}],
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn curseforge_versions_hide_server_packs_and_tags() {
        let cdn = Some("https://edge.forgecdn.net/files/1/2/Pack.zip");
        let list = to_curseforge_versions(&[
            cf_file(5, 2, cdn, false),
            cf_file(4, 1, cdn, true),
            cf_file(3, 1, cdn, false),
        ]);
        let ids: Vec<&str> = list.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["5", "3"], "服务端包不列出");
        assert!(list[1].recommended && !list[0].recommended);
        assert_eq!(list[0].game_versions, ["1.21.1"], "去掉 NeoForge 这类标签");
    }

    #[test]
    fn curseforge_pack_file_is_checked_before_download() {
        let dir = Path::new("tmp");
        let ok = curseforge_pack_file(
            &cf_file(
                1,
                1,
                Some("https://edge.forgecdn.net/files/1/2/Pack.zip"),
                false,
            ),
            dir,
        )
        .unwrap();
        assert_eq!(ok.sha1.as_deref(), Some(SHA1));
        assert!(ok.path.starts_with(dir));
        let blocked = curseforge_pack_file(&cf_file(1, 1, None, false), dir).unwrap_err();
        assert!(blocked.to_string().contains("导入整合包文件"), "{blocked}");
        assert!(matches!(
            curseforge_pack_file(
                &cf_file(1, 1, Some("https://evil.com/Pack.zip"), false),
                dir
            ),
            Err(ModpackError::Rejected(_))
        ));
    }

    #[test]
    fn pack_file_is_checked_before_download() {
        let dir = Path::new("tmp");
        let ok = pack_file(&version("1", "release", "neoforge", CDN), dir).unwrap();
        assert_eq!(ok.sha1.as_deref(), Some(SHA1));
        assert!(
            ok.path.starts_with(dir),
            "文件放在指定目录里，名字是自己起的"
        );

        let elsewhere = version(
            "1",
            "release",
            "neoforge",
            "https://evil.example/pack.mrpack",
        );
        assert!(matches!(
            pack_file(&elsewhere, dir),
            Err(ModpackError::Rejected(_))
        ));
        let fabric = version("1", "release", "fabric", CDN);
        assert!(pack_file(&fabric, dir).is_ok());
        let mut legacy_forge = version("1", "release", "forge", CDN);
        legacy_forge.game_versions = vec!["1.12.2".to_owned()];
        assert!(pack_file(&legacy_forge, dir).is_ok());

        let mut too_old_forge = version("1", "release", "forge", CDN);
        too_old_forge.game_versions = vec!["1.4.7".to_owned()];
        assert!(matches!(
            pack_file(&too_old_forge, dir),
            Err(ModpackError::UnsupportedLoader(_))
        ));
        let mut no_hash = version("1", "release", "neoforge", CDN);
        no_hash.files[0].hashes.clear();
        assert!(matches!(
            pack_file(&no_hash, dir),
            Err(ModpackError::Invalid(_))
        ));
    }
}
