//! 原版文件补全：依赖库、本地库、资源文件、日志配置、客户端 jar。
//!
//! 已经装好的文件会被跳过，所以「安装」和「启动前补全」是同一个函数：
//! 安装时用完整校验，启动前用快速校验（只比大小），断网时也能启动装好的版本。

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::InstallError;
use crate::config::DownloadSource;
use crate::fs::{safe_join, write_atomic};
use crate::meta::{
    Environment, Library, MetaError, VersionJson, fetch_manifest, fetch_version_json, resolve,
    rules_allow,
};
use crate::net::{DownloadItem, DownloadProgress, Downloader, candidate_urls};
use crate::paths::GameDir;

const LIBRARIES_BASE: &str = "https://libraries.minecraft.net";
const RESOURCES_BASE: &str = "https://resources.download.minecraft.net";
/// 从清单补版本描述文件的最大次数（原版 ← 加载器 ← 自定义，够用了）
const MAX_FETCH_ROUNDS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    /// 下载资源索引
    AssetIndex,
    /// 下载依赖库、客户端、资源文件、日志配置
    Files,
    /// 解压旧版本的本地库
    Natives,
}

/// 一个依赖库文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFile {
    pub name: String,
    pub path: PathBuf,
    /// 没有下载地址的库（由加载器安装器生成）为 `None`，只检查是否存在
    pub download: Option<DownloadItem>,
    /// 是否放进 classpath（旧版本的本地库压缩包不放）
    pub on_classpath: bool,
    /// 旧版本的本地库：需要解压到 natives 目录，值是要排除的路径前缀
    pub extract_natives: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: BTreeMap<String, AssetObject>,
    /// 1.6 以前：资源要按原始文件名复制到 `assets/virtual/<索引>`
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    /// 更早的版本：资源要复制到游戏目录的 `resources/`
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

/// 补全完成后，启动需要的全部信息。
#[derive(Debug, Clone)]
pub struct InstalledVersion {
    /// 合并后的完整版本描述
    pub version: VersionJson,
    /// 依赖库 + 客户端 jar，按顺序
    pub classpath: Vec<PathBuf>,
    pub client_jar: PathBuf,
    pub natives_dir: PathBuf,
    pub asset_index_id: Option<String>,
    /// 旧版本的资源目录（`${game_assets}`），新版本为 `None`
    pub legacy_assets_dir: Option<PathBuf>,
    pub log_config: Option<PathBuf>,
}

/// 读取并合并版本描述。本地有就只读本地（断网也能用）；缺哪个原版就从官方清单补哪个。
pub async fn load_or_fetch_version(
    game: &GameDir,
    id: &str,
    client: &Client,
    source: DownloadSource,
) -> Result<VersionJson, InstallError> {
    for _ in 0..MAX_FETCH_ROUNDS {
        match resolve(id, |vid| read_local(game, vid)) {
            Ok(version) => return Ok(version),
            Err(MetaError::NotFound(missing)) => {
                fetch_into(game, &missing, client, source).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(MetaError::NotFound(id.to_owned()).into())
}

pub(super) fn read_local(game: &GameDir, id: &str) -> Result<VersionJson, MetaError> {
    let path = game
        .version_json(id)
        .map_err(|_| MetaError::InvalidId(id.to_owned()))?;
    match std::fs::read(&path) {
        Ok(bytes) => VersionJson::parse(id, &bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(MetaError::NotFound(id.to_owned())),
        Err(source) => Err(MetaError::Io { path, source }),
    }
}

pub(super) async fn fetch_into(
    game: &GameDir,
    id: &str,
    client: &Client,
    source: DownloadSource,
) -> Result<(), InstallError> {
    let manifest = fetch_manifest(client, source).await?;
    let entry = manifest
        .find(id)
        .ok_or_else(|| MetaError::NotFound(id.to_owned()))?;
    let (_, raw) = fetch_version_json(client, entry, source).await?;
    write_atomic(&game.version_json(id)?, &raw)?;
    tracing::info!(version = id, "已从官方清单获取版本描述文件");
    Ok(())
}

/// 算出当前系统需要哪些依赖库文件（rules 已过滤）。
pub fn plan_libraries(
    version: &VersionJson,
    game: &GameDir,
    env: &Environment,
    source: DownloadSource,
) -> Result<Vec<LibraryFile>, InstallError> {
    let mut files = Vec::new();
    let mut main_libraries = HashSet::new();
    let mut native_libraries = HashSet::new();
    for lib in version
        .libraries
        .iter()
        .filter(|l| rules_allow(l.rules.as_deref(), env))
    {
        // 旧版本写法：natives 把系统名映射到一个 classifier
        if let Some(template) = lib.natives.as_ref().and_then(|n| n.get(&env.os_name)) {
            let bits = if env.os_arch == "x86" { "32" } else { "64" };
            let classifier = template.replace("${arch}", bits);
            if let Some(file) = natives_file(lib, &classifier, game, source)?
                && native_libraries.insert((lib.identity(), classifier))
            {
                files.push(file);
            }
            let natives_only = lib.downloads.as_ref().is_some_and(|d| d.artifact.is_none());
            if natives_only {
                continue;
            }
        }
        // 系统规则已经生效，主库使用第一个匹配项（子版本优先）。
        if let Some(file) = main_file(lib, game, source)?
            && main_libraries.insert(lib.identity())
        {
            files.push(file);
        }
    }
    Ok(files)
}

fn main_file(
    lib: &Library,
    game: &GameDir,
    source: DownloadSource,
) -> Result<Option<LibraryFile>, InstallError> {
    if let Some(artifact) = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
        let Some(relative) = artifact
            .path
            .clone()
            .or_else(|| lib.coord().map(|c| c.path()))
        else {
            tracing::warn!(library = %lib.name, "库没有路径也无法解析坐标，跳过");
            return Ok(None);
        };
        let path = game.library(&relative)?;
        let download = (!artifact.url.is_empty()).then(|| DownloadItem {
            urls: candidate_urls(&artifact.url, source),
            path: path.clone(),
            sha1: artifact.sha1.clone(),
            size: artifact.size,
        });
        return Ok(Some(LibraryFile {
            name: lib.name.clone(),
            path,
            download,
            on_classpath: true,
            extract_natives: None,
        }));
    }

    // 没有 downloads（Fabric 等）：按 Maven 规则拼出地址
    let Some(coord) = lib.coord() else {
        tracing::warn!(library = %lib.name, "无法解析库坐标，跳过");
        return Ok(None);
    };
    let relative = coord.path();
    let path = game.library(&relative)?;
    let base = lib.url.as_deref().unwrap_or(LIBRARIES_BASE);
    let url = format!("{}/{relative}", base.trim_end_matches('/'));
    Ok(Some(LibraryFile {
        name: lib.name.clone(),
        path: path.clone(),
        download: Some(DownloadItem {
            urls: candidate_urls(&url, source),
            path,
            sha1: lib.sha1.clone(),
            size: lib.size,
        }),
        on_classpath: true,
        extract_natives: None,
    }))
}

fn natives_file(
    lib: &Library,
    classifier: &str,
    game: &GameDir,
    source: DownloadSource,
) -> Result<Option<LibraryFile>, InstallError> {
    let exclude = lib
        .extract
        .as_ref()
        .map(|e| e.exclude.clone())
        .unwrap_or_default();
    let classified = lib
        .downloads
        .as_ref()
        .and_then(|d| d.classifiers.as_ref())
        .and_then(|c| c.get(classifier));

    let (relative, url, sha1, size) = match classified {
        Some(artifact) => {
            let relative = match &artifact.path {
                Some(path) => path.clone(),
                None => match lib.coord() {
                    Some(mut coord) => {
                        coord.classifier = Some(classifier.to_owned());
                        coord.path()
                    }
                    None => return Ok(None),
                },
            };
            (
                relative,
                artifact.url.clone(),
                artifact.sha1.clone(),
                artifact.size,
            )
        }
        None => {
            let Some(mut coord) = lib.coord() else {
                return Ok(None);
            };
            coord.classifier = Some(classifier.to_owned());
            let relative = coord.path();
            let base = lib.url.as_deref().unwrap_or(LIBRARIES_BASE);
            let url = format!("{}/{relative}", base.trim_end_matches('/'));
            (relative, url, None, None)
        }
    };
    let path = game.library(&relative)?;
    Ok(Some(LibraryFile {
        name: format!("{}:{classifier}", lib.name),
        path: path.clone(),
        download: (!url.is_empty()).then(|| DownloadItem {
            urls: candidate_urls(&url, source),
            path,
            sha1,
            size,
        }),
        on_classpath: false,
        extract_natives: Some(exclude),
    }))
}

/// 补全一个版本的全部文件。`version` 必须是已经合并过继承链的完整描述。
pub async fn install_files<F>(
    version: VersionJson,
    game: &GameDir,
    env: &Environment,
    downloader: &Downloader,
    source: DownloadSource,
    cancel: &CancellationToken,
    on_progress: F,
) -> Result<InstalledVersion, InstallError>
where
    F: Fn(InstallPhase, DownloadProgress) + Send + Sync,
{
    let libraries = plan_libraries(&version, game, env, source)?;
    let jar_id = version.jar.clone().unwrap_or_else(|| version.id.clone());
    let client_jar = game.version_jar(&jar_id)?;

    let mut items: Vec<DownloadItem> = libraries
        .iter()
        .filter_map(|l| l.download.clone())
        .collect();

    match version.downloads.as_ref().and_then(|d| d.client.as_ref()) {
        Some(client) => items.push(DownloadItem {
            urls: candidate_urls(&client.url, source),
            path: client_jar.clone(),
            sha1: client.sha1.clone(),
            size: client.size,
        }),
        None if client_jar.exists() => {}
        None => return Err(InstallError::NoClientDownload(version.id.clone())),
    }

    let log_config = match version.logging.as_ref().and_then(|l| l.client.as_ref()) {
        Some(logging) => {
            let path = game.log_config(&logging.file.id)?;
            items.push(DownloadItem {
                urls: candidate_urls(&logging.file.url, source),
                path: path.clone(),
                sha1: logging.file.sha1.clone(),
                size: logging.file.size,
            });
            Some(path)
        }
        None => None,
    };

    let mut index = None;
    if let Some(index_ref) = &version.asset_index {
        let index_path = game.asset_index(&index_ref.id)?;
        let index_item = DownloadItem {
            urls: candidate_urls(&index_ref.url, source),
            path: index_path.clone(),
            sha1: index_ref.sha1.clone(),
            size: index_ref.size,
        };
        downloader
            .download_all(std::slice::from_ref(&index_item), cancel, |p| {
                on_progress(InstallPhase::AssetIndex, p);
            })
            .await?;
        let bytes = tokio::fs::read(&index_path)
            .await
            .map_err(|source| InstallError::Io {
                path: index_path.clone(),
                source,
            })?;
        let parsed: AssetIndex =
            serde_json::from_slice(&bytes).map_err(|source| InstallError::AssetIndex {
                id: index_ref.id.clone(),
                source,
            })?;

        let mut seen = HashSet::new();
        for object in parsed.objects.values() {
            if !seen.insert(object.hash.as_str()) {
                continue; // 不同文件名可能指向同一个资源
            }
            // 先校验哈希格式（40 位十六进制），再用它拼地址
            let path = game.asset_object(&object.hash)?;
            let url = format!("{RESOURCES_BASE}/{}/{}", &object.hash[..2], object.hash);
            items.push(DownloadItem {
                urls: candidate_urls(&url, source),
                path,
                sha1: Some(object.hash.clone()),
                size: Some(object.size),
            });
        }
        index = Some((index_ref.id.clone(), parsed));
    }

    let mut seen_paths = HashSet::new();
    items.retain(|item| seen_paths.insert(item.path.clone()));
    downloader
        .download_all(&items, cancel, |p| on_progress(InstallPhase::Files, p))
        .await?;

    for lib in &libraries {
        if lib.download.is_none() && !lib.path.exists() {
            return Err(InstallError::MissingLibrary(lib.name.clone()));
        }
    }

    let natives_dir = game.natives_dir(&version.id, &env.os_name, &env.os_arch)?;
    tokio::fs::create_dir_all(&natives_dir)
        .await
        .map_err(|source| InstallError::Io {
            path: natives_dir.clone(),
            source,
        })?;
    let to_extract: Vec<(PathBuf, Vec<String>)> = libraries
        .iter()
        .filter_map(|l| l.extract_natives.clone().map(|ex| (l.path.clone(), ex)))
        .collect();
    if !to_extract.is_empty() {
        on_progress(InstallPhase::Natives, DownloadProgress::default());
        let dest = natives_dir.clone();
        run_blocking(move || {
            for (jar, exclude) in &to_extract {
                extract_natives(jar, &dest, exclude).map_err(|source| InstallError::Extract {
                    path: jar.clone(),
                    source,
                })?;
            }
            Ok(())
        })
        .await?;
    }

    let mut legacy_assets_dir = None;
    if let Some((id, parsed)) = index {
        let target = if parsed.map_to_resources {
            Some(game.root().join("resources"))
        } else if parsed.is_virtual {
            Some(safe_join(&game.assets().join("virtual"), &id)?)
        } else {
            None
        };
        if let Some(dir) = target {
            let objects_root = game.clone();
            let dest = dir.clone();
            run_blocking(move || copy_legacy_assets(&objects_root, &parsed, &dest)).await?;
            legacy_assets_dir = Some(dir);
        }
    }

    let mut classpath: Vec<PathBuf> = Vec::new();
    for path in libraries
        .iter()
        .filter(|l| l.on_classpath)
        .map(|l| &l.path)
        .chain([&client_jar])
    {
        if !classpath.contains(path) {
            classpath.push(path.clone());
        }
    }

    Ok(InstalledVersion {
        asset_index_id: version.asset_index.as_ref().map(|a| a.id.clone()),
        version,
        classpath,
        client_jar,
        natives_dir,
        legacy_assets_dir,
        log_config,
    })
}

pub(super) async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, InstallError> + Send + 'static,
) -> Result<T, InstallError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| InstallError::Io {
            path: PathBuf::new(),
            source: io::Error::other(e),
        })?
}

/// 把旧版本的本地库压缩包解压到 natives 目录（跳过 META-INF 和排除项，路径经过安全校验）。
fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> io::Result<()> {
    let file = std::fs::File::open(jar)?;
    let mut archive = zip::ZipArchive::new(file).map_err(io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        if entry.is_dir() {
            continue;
        }
        // 文件名不是合法 UTF-8 的条目不可能是我们要的本地库，跳过
        let name = match entry.name() {
            Ok(name) => name.into_owned(),
            Err(error) => {
                tracing::warn!(%error, jar = %jar.display(), "本地库压缩包里有无法识别的文件名，已跳过");
                continue;
            }
        };
        if name.starts_with("META-INF/") || exclude.iter().any(|e| name.starts_with(e.as_str())) {
            continue;
        }
        let Ok(target) = safe_join(dest, &name) else {
            tracing::warn!(entry = %name, "本地库压缩包里有不安全的路径，已跳过");
            continue;
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// 旧版本按原始文件名复制资源（已存在的跳过）。
fn copy_legacy_assets(game: &GameDir, index: &AssetIndex, dest: &Path) -> Result<(), InstallError> {
    for (name, object) in &index.objects {
        let target = safe_join(dest, name)?;
        if target.exists() {
            continue;
        }
        let source = game.asset_object(&object.hash)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| InstallError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::copy(&source, &target).map_err(|e| InstallError::Io {
            path: target.clone(),
            source: e,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::version::{Artifact, Extract, LibraryDownloads};

    fn windows() -> Environment {
        Environment {
            os_name: "windows".into(),
            os_arch: "x86_64".into(),
            os_version: "10.0".into(),
            features: BTreeMap::new(),
        }
    }

    #[test]
    fn old_style_natives_are_extracted_not_put_on_classpath() {
        let lwjgl_platform = Library {
            name: "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209".into(),
            natives: Some(BTreeMap::from([("windows".into(), "natives-windows".into())])),
            extract: Some(Extract { exclude: vec!["META-INF/".into()] }),
            downloads: Some(LibraryDownloads {
                artifact: None,
                classifiers: Some(BTreeMap::from([(
                    "natives-windows".into(),
                    Artifact {
                        path: Some("org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar".into()),
                        url: "https://libraries.minecraft.net/x.jar".into(),
                        sha1: Some("aa".into()),
                        size: Some(1),
                    },
                )])),
            }),
            ..Library::default()
        };
        let version = VersionJson {
            libraries: vec![lwjgl_platform],
            ..VersionJson::default()
        };
        let files = plan_libraries(
            &version,
            &GameDir::new("D:/mc"),
            &windows(),
            DownloadSource::Official,
        )
        .unwrap();

        assert_eq!(files.len(), 1, "只有本地库、没有主 jar");
        assert!(!files[0].on_classpath);
        assert_eq!(
            files[0].extract_natives.as_deref(),
            Some(&["META-INF/".to_owned()][..])
        );
    }

    #[test]
    fn maven_style_libraries_get_a_url_from_the_repository() {
        let fabric = Library {
            name: "net.fabricmc:fabric-loader:0.16.9".into(),
            url: Some("https://maven.fabricmc.net/".into()),
            sha1: Some("bb".into()),
            ..Library::default()
        };
        let version = VersionJson {
            libraries: vec![fabric],
            ..VersionJson::default()
        };
        let files = plan_libraries(
            &version,
            &GameDir::new("D:/mc"),
            &windows(),
            DownloadSource::Official,
        )
        .unwrap();

        let item = files[0].download.as_ref().unwrap();
        assert_eq!(
            item.urls[0],
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert!(files[0].on_classpath);
    }

    #[test]
    fn libraries_without_url_must_already_exist() {
        let generated = Library {
            name: "net.neoforged:neoforge:21.1.77:client".into(),
            downloads: Some(LibraryDownloads {
                artifact: Some(Artifact {
                    path: Some("net/neoforged/neoforge/21.1.77/neoforge-21.1.77-client.jar".into()),
                    url: String::new(),
                    sha1: None,
                    size: None,
                }),
                classifiers: None,
            }),
            ..Library::default()
        };
        let version = VersionJson {
            libraries: vec![generated],
            ..VersionJson::default()
        };
        let files = plan_libraries(
            &version,
            &GameDir::new("D:/mc"),
            &windows(),
            DownloadSource::Official,
        )
        .unwrap();
        assert!(
            files[0].download.is_none(),
            "没有地址就不下载，由安装器生成"
        );
    }

    #[test]
    fn hostile_library_paths_are_rejected() {
        let evil = Library {
            name: "a:b:1".into(),
            downloads: Some(LibraryDownloads {
                artifact: Some(Artifact {
                    path: Some("../../Windows/evil.dll".into()),
                    url: "https://x/y".into(),
                    sha1: None,
                    size: None,
                }),
                classifiers: None,
            }),
            ..Library::default()
        };
        let version = VersionJson {
            libraries: vec![evil],
            ..VersionJson::default()
        };
        let result = plan_libraries(
            &version,
            &GameDir::new("D:/mc"),
            &windows(),
            DownloadSource::Official,
        );
        assert!(matches!(result, Err(InstallError::UnsafePath(_))));
    }

    #[test]
    fn real_1_21_1_plan_matches_the_official_layout() {
        let version =
            VersionJson::parse("1.21.1", include_bytes!("../../tests/fixtures/1.21.1.json"))
                .unwrap();
        let files = plan_libraries(
            &version,
            &GameDir::new("D:/mc"),
            &windows(),
            DownloadSource::Official,
        )
        .unwrap();
        assert_eq!(files.len(), 70, "和 D2 实测的「当前系统需要 70 个」一致");
        assert!(
            files
                .iter()
                .all(|f| f.on_classpath && f.extract_natives.is_none()),
            "1.19 以后的本地库也放 classpath，由 LWJGL 自己解压"
        );
        assert!(files.iter().all(|f| f.download.is_some()));
    }
}
