//! Java 自动下载：电脑上没有合适的 Java 时，下载 Mojang 官方运行时（和官方启动器用的是同一份）。
//!
//! 流程：总清单 `all.json` → 按平台和组件名（版本文件里的 `javaVersion.component`，例如 `java-runtime-delta`）
//! 找到这个运行时的文件清单 → 逐个文件下载，每个都校验大小和 SHA-1。
//!
//! 下载的 Java 放在 `<游戏目录>/.mcl-java/<组件名>/`（和游戏放同一个盘，不占系统盘）。
//! 全部文件下完、确认能用之后才写标记文件；没有标记文件的目录不算数，下次接着下。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio_util::sync::CancellationToken;

use super::prefs::JavaPrefs;
use super::{JavaInstall, JavaOrigin, home_key, inspect, scan, select, subdirs};
use crate::fs::{AtomicWriteError, UnsafePath, safe_join, write_json_atomic};
use crate::meta::VersionJson;
use crate::net::{
    DownloadError, DownloadItem, DownloadProgress, Downloader, HttpError, get_bytes, get_json,
    to_hex,
};

const ALL_RUNTIMES_URL: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
/// 游戏目录下放启动器下载的 Java 的文件夹
pub const MANAGED_DIR: &str = ".mcl-java";
/// 标记文件：有它才说明这个 Java 下完整了
const MARKER: &str = ".mcl-runtime.json";

#[derive(Debug, thiserror::Error)]
pub enum JavaError {
    #[error("Mojang 没有提供适用于这台电脑的 Java {major}，请手动安装一个")]
    NotOffered { major: u32 },
    #[error("获取 Java 下载清单失败，请检查网络")]
    Http(#[from] HttpError),
    #[error("Java 下载清单有问题：{0}")]
    Manifest(String),
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("保存文件失败")]
    Write(#[from] AtomicWriteError),
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("下载的 Java {major} 用不了，请重试")]
    Broken { major: u32 },
    #[error("只能删除启动器自己下载的 Java")]
    NotDownloaded,
}

/// 游戏要哪个 Java。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRequest {
    /// 主版本号，例如 21
    pub major: u32,
    /// Mojang 运行时的组件名，例如 `java-runtime-delta`；老版本的版本文件里没有
    pub component: Option<String>,
}

impl JavaRequest {
    /// 以版本文件里的 `javaVersion` 为准（没有这个字段的老版本用 Java 8）
    pub fn of(version: &VersionJson) -> Self {
        Self {
            major: version.required_java(),
            component: version
                .java_version
                .as_ref()
                .and_then(|j| j.component.clone())
                .filter(|c| !c.is_empty()),
        }
    }
}

/// 去哪里找 Java。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaLocator {
    /// 启动器自己下载的 Java 放这里
    pub managed_root: PathBuf,
    /// 要不要用电脑上已有的 Java。关掉就只认启动器下载的（开发时用来模拟「一台没装 Java 的电脑」）
    pub use_system: bool,
    /// 用户在设置里手动添加的 Java（主目录）
    pub manual: Vec<PathBuf>,
    /// 每个大版本默认用哪个（主目录）；没选的大版本自动挑
    pub defaults: BTreeMap<u32, PathBuf>,
}

impl JavaLocator {
    pub fn for_game(game_root: &Path) -> Self {
        Self {
            managed_root: game_root.join(MANAGED_DIR),
            use_system: true,
            manual: Vec::new(),
            defaults: BTreeMap::new(),
        }
    }

    /// 带上用户在设置里的选择（手动添加的、每个大版本的默认）
    pub fn with_prefs(mut self, prefs: &JavaPrefs) -> Self {
        self.manual.clone_from(&prefs.manual);
        self.defaults.clone_from(&prefs.defaults);
        self
    }

    /// 扫描能用的 Java（要读文件和注册表，放到后台线程调用）。
    pub fn scan(&self) -> Vec<JavaInstall> {
        let homes = managed_homes(&self.managed_root);
        if self.use_system {
            scan(&homes, &self.manual)
        } else {
            homes
                .iter()
                .filter_map(|home| inspect(home, JavaOrigin::Downloaded))
                .collect()
        }
    }

    /// 按版本要求挑一个：先按通常的规则定用哪个大版本（[`select`]），
    /// 再看用户有没有给这个大版本选过默认的。选过的那个找不到了（被删了）就自动挑。
    pub fn pick<'a>(&self, installs: &'a [JavaInstall], required: u32) -> Option<&'a JavaInstall> {
        let auto = select(installs, required)?;
        let chosen = self.defaults.get(&auto.major).and_then(|home| {
            let key = home_key(home);
            installs
                .iter()
                .find(|j| j.major == auto.major && home_key(&j.home) == key)
        });
        Some(chosen.unwrap_or(auto))
    }

    /// 删掉一个启动器下载的 Java（需要时会再下载）。先删标记文件，删到一半也不会被当成能用的。
    pub fn delete_downloaded(&self, install: &JavaInstall) -> Result<(), JavaError> {
        let inside = install
            .home
            .parent()
            .is_some_and(|parent| home_key(parent) == home_key(&self.managed_root));
        if install.origin != JavaOrigin::Downloaded || !inside {
            return Err(JavaError::NotDownloaded);
        }
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source: std::io::Error| JavaError::Io { path, source }
        };
        let marker = install.home.join(MARKER);
        match std::fs::remove_file(&marker) {
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
                return Err(io(&marker)(source));
            }
            _ => {}
        }
        std::fs::remove_dir_all(&install.home).map_err(io(&install.home))?;
        tracing::info!(home = %install.home.display(), major = install.major, "已删除下载的 Java");
        Ok(())
    }
}

/// 启动器下载好的 Java 目录（只算下完整的）
fn managed_homes(root: &Path) -> Vec<PathBuf> {
    subdirs(root)
        .into_iter()
        .filter(|home| home.join(MARKER).is_file())
        .collect()
}

pub struct JavaContext<'a> {
    pub locator: &'a JavaLocator,
    pub client: &'a Client,
    pub downloader: &'a Downloader,
    pub cancel: &'a CancellationToken,
}

/// 找一个能用的 Java；电脑上没有就下载。`on_progress` 只在下载时调用。
pub async fn find_or_install<F>(
    ctx: &JavaContext<'_>,
    request: &JavaRequest,
    on_progress: F,
) -> Result<JavaInstall, JavaError>
where
    F: Fn(DownloadProgress) + Send + Sync,
{
    let locator = ctx.locator.clone();
    let installs = match tokio::task::spawn_blocking(move || locator.scan()).await {
        Ok(installs) => installs,
        Err(error) => {
            tracing::warn!(%error, "扫描 Java 的任务异常结束");
            Vec::new()
        }
    };
    if let Some(found) = ctx.locator.pick(&installs, request.major) {
        return Ok(found.clone());
    }
    tracing::info!(
        major = request.major,
        found = installs.len(),
        "没找到合适的 Java，开始下载"
    );
    install(ctx, request, on_progress).await
}

/// 下载并装好一个 Mojang 运行时，返回装好的 Java。已经下过的文件校验通过就跳过。
pub async fn install<F>(
    ctx: &JavaContext<'_>,
    request: &JavaRequest,
    on_progress: F,
) -> Result<JavaInstall, JavaError>
where
    F: Fn(DownloadProgress) + Send + Sync,
{
    let not_offered = || JavaError::NotOffered {
        major: request.major,
    };
    let platform =
        platform_key(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(not_offered)?;
    let mut all: BTreeMap<String, serde_json::Value> =
        get_json(ctx.client, &[ALL_RUNTIMES_URL.to_owned()]).await?;
    let runtimes: BTreeMap<String, Vec<RuntimeEntry>> = all
        .remove(platform)
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| JavaError::Manifest(e.to_string()))?
        .ok_or_else(not_offered)?;
    let (component, entry) = pick_runtime(&runtimes, request).ok_or_else(not_offered)?;
    let manifest = fetch_manifest(ctx, &entry.manifest).await?;

    let home = safe_join(&ctx.locator.managed_root, component)?;
    let marker = home.join(MARKER);
    // 重新下载期间这个目录不算数
    if let Err(source) = std::fs::remove_file(&marker)
        && source.kind() != std::io::ErrorKind::NotFound
    {
        return Err(JavaError::Io {
            path: marker,
            source,
        });
    }

    let mut items = Vec::new();
    let mut executables = Vec::new();
    for (name, file) in &manifest.files {
        // 清单来自网络：文件名只能落在这个 Java 的目录里
        let path = safe_join(&home, name)?;
        match file.kind.as_str() {
            "directory" => std::fs::create_dir_all(&path).map_err(|source| JavaError::Io {
                path: path.clone(),
                source,
            })?,
            "file" => {
                let raw = file
                    .downloads
                    .as_ref()
                    .and_then(|d| d.raw.as_ref())
                    .ok_or_else(|| JavaError::Manifest(format!("{name} 没有下载地址")))?;
                if file.executable {
                    executables.push(path.clone());
                }
                items.push(DownloadItem {
                    urls: vec![raw.url.clone()],
                    path,
                    sha1: Some(raw.sha1.clone()),
                    size: Some(raw.size),
                });
            }
            // 符号链接只在 macOS / Linux 的运行时里有（指向许可文件），不影响运行
            other => tracing::debug!(name, kind = other, "跳过清单里的条目"),
        }
    }
    tracing::info!(
        component,
        version = %entry.version.name,
        files = items.len(),
        "下载 Java 运行时"
    );
    ctx.downloader
        .download_all(&items, ctx.cancel, on_progress)
        .await?;
    mark_executable(&executables);

    let install = inspect(&home, JavaOrigin::Downloaded).ok_or(JavaError::Broken {
        major: request.major,
    })?;
    write_json_atomic(
        &marker,
        &Marker {
            component: component.to_owned(),
            version: entry.version.name.clone(),
            manifest_sha1: entry.manifest.sha1.clone(),
        },
    )?;
    tracing::info!(version = %install.version, home = %home.display(), "Java 已装好");
    Ok(install)
}

/// 从官方地址下载运行时的文件清单并核对它自己的 SHA-1
async fn fetch_manifest(ctx: &JavaContext<'_>, file: &RemoteFile) -> Result<Manifest, JavaError> {
    let bytes = get_bytes(ctx.client, std::slice::from_ref(&file.url)).await?;
    if !to_hex(&Sha1::digest(&bytes)).eq_ignore_ascii_case(&file.sha1) {
        tracing::warn!(url = %file.url, "Java 文件清单校验不对");
        return Err(JavaError::Manifest("文件清单校验不对".to_owned()));
    }
    serde_json::from_slice(&bytes).map_err(|e| JavaError::Manifest(e.to_string()))
}

#[cfg(unix)]
fn mark_executable(paths: &[PathBuf]) {
    use std::os::unix::fs::PermissionsExt;
    for path in paths {
        if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)) {
            tracing::warn!(%error, file = %path.display(), "设置可执行权限失败");
        }
    }
}

#[cfg(not(unix))]
fn mark_executable(_paths: &[PathBuf]) {}

/// Mojang 清单里的平台名
fn platform_key(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("windows", "x86") => Some("windows-x86"),
        ("windows", "aarch64") => Some("windows-arm64"),
        ("macos", "x86_64") => Some("mac-os"),
        ("macos", "aarch64") => Some("mac-os-arm64"),
        ("linux", "x86_64") => Some("linux"),
        ("linux", "x86") => Some("linux-i386"),
        _ => None,
    }
}

/// 挑运行时：版本文件点名的组件优先；没点名（或者这个平台没有）就找主版本号一样的。
fn pick_runtime<'a>(
    runtimes: &'a BTreeMap<String, Vec<RuntimeEntry>>,
    request: &JavaRequest,
) -> Option<(&'a str, &'a RuntimeEntry)> {
    if let Some((name, entries)) = request
        .component
        .as_ref()
        .and_then(|c| runtimes.get_key_value(c))
        && let Some(entry) = entries.first()
    {
        return Some((name.as_str(), entry));
    }
    runtimes
        .iter()
        .filter(|(name, _)| name.starts_with("java-runtime") || name.starts_with("jre"))
        .filter_map(|(name, entries)| Some((name.as_str(), entries.first()?)))
        .filter(|(_, entry)| runtime_major(&entry.version.name) == Some(request.major))
        // 同一个主版本有几份时，不要快照专用的那份
        .min_by_key(|(name, _)| name.contains("snapshot"))
}

/// 运行时版本名的主版本号：`21.0.7` → 21，`8u51-cacert462b08` → 8，`1.8.0_51` → 8
fn runtime_major(name: &str) -> Option<u32> {
    let digits = |text: &str| -> Option<u32> {
        let end = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        text[..end].parse().ok()
    };
    match digits(name)? {
        1 => digits(name.strip_prefix("1.")?),
        major => Some(major),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeEntry {
    manifest: RemoteFile,
    version: RuntimeVersion,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeVersion {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteFile {
    sha1: String,
    size: u64,
    url: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    files: BTreeMap<String, ManifestFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    downloads: Option<FileDownloads>,
}

#[derive(Debug, Deserialize)]
struct FileDownloads {
    #[serde(default)]
    raw: Option<RemoteFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    component: String,
    version: String,
    manifest_sha1: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtimes() -> BTreeMap<String, Vec<RuntimeEntry>> {
        // 2026-11 实际清单里 windows-x64 的样子（只留用得到的字段）
        serde_json::from_str(
            r#"{
              "java-runtime-alpha": [{"manifest": {"sha1": "a", "size": 1, "url": "https://piston-meta.mojang.com/a"}, "version": {"name": "16.0.1.9.1"}}],
              "java-runtime-beta": [{"manifest": {"sha1": "b", "size": 1, "url": "https://piston-meta.mojang.com/b"}, "version": {"name": "17.0.15"}}],
              "java-runtime-delta": [{"availability": {"group": 312, "progress": 100}, "manifest": {"sha1": "d", "size": 1, "url": "https://piston-meta.mojang.com/d"}, "version": {"name": "21.0.7", "released": "2025-05-19T08:34:42+00:00"}}],
              "java-runtime-gamma": [{"manifest": {"sha1": "g", "size": 1, "url": "https://piston-meta.mojang.com/g"}, "version": {"name": "17.0.15"}}],
              "java-runtime-gamma-snapshot": [{"manifest": {"sha1": "s", "size": 1, "url": "https://piston-meta.mojang.com/s"}, "version": {"name": "17.0.15"}}],
              "jre-legacy": [{"manifest": {"sha1": "l", "size": 1, "url": "https://piston-meta.mojang.com/l"}, "version": {"name": "8u51-cacert462b08"}}],
              "minecraft-java-exe": [{"manifest": {"sha1": "e", "size": 1, "url": "https://piston-meta.mojang.com/e"}, "version": {"name": "14"}}],
              "java-runtime-empty": []
            }"#,
        )
        .unwrap()
    }

    fn request(major: u32, component: Option<&str>) -> JavaRequest {
        JavaRequest {
            major,
            component: component.map(str::to_owned),
        }
    }

    #[test]
    fn picks_the_component_named_by_the_version_file() {
        let all = runtimes();
        let (name, entry) = pick_runtime(&all, &request(21, Some("java-runtime-delta"))).unwrap();
        assert_eq!(name, "java-runtime-delta");
        assert_eq!(entry.version.name, "21.0.7");
        assert_eq!(
            pick_runtime(&all, &request(17, Some("java-runtime-gamma")))
                .unwrap()
                .0,
            "java-runtime-gamma"
        );
    }

    #[test]
    fn falls_back_to_the_major_version() {
        let all = runtimes();
        // 老版本的版本文件没写组件名
        assert_eq!(
            pick_runtime(&all, &request(8, None)).unwrap().0,
            "jre-legacy"
        );
        // 点名的组件这个平台没有（或者是空的）：按主版本号找，不要快照专用的
        let (name, _) = pick_runtime(&all, &request(17, Some("java-runtime-unknown"))).unwrap();
        assert!(!name.contains("snapshot"), "{name}");
        assert_eq!(
            pick_runtime(&all, &request(16, Some("java-runtime-empty")))
                .unwrap()
                .0,
            "java-runtime-alpha"
        );
        // 14 只有 `minecraft-java-exe`（不是运行时），30 还没有
        assert!(pick_runtime(&all, &request(14, None)).is_none());
        assert!(pick_runtime(&all, &request(30, None)).is_none());
    }

    #[test]
    fn reads_runtime_version_names() {
        assert_eq!(runtime_major("21.0.7"), Some(21));
        assert_eq!(runtime_major("16.0.1.9.1"), Some(16));
        assert_eq!(runtime_major("8u51-cacert462b08"), Some(8));
        assert_eq!(runtime_major("1.8.0_51"), Some(8));
        assert_eq!(runtime_major("25"), Some(25));
        assert_eq!(runtime_major("ea"), None);
    }

    #[test]
    fn knows_mojang_platform_names() {
        assert_eq!(platform_key("windows", "x86_64"), Some("windows-x64"));
        assert_eq!(platform_key("windows", "aarch64"), Some("windows-arm64"));
        assert_eq!(platform_key("macos", "aarch64"), Some("mac-os-arm64"));
        assert_eq!(platform_key("linux", "x86_64"), Some("linux"));
        assert_eq!(platform_key("freebsd", "x86_64"), None);
    }

    #[test]
    fn parses_a_runtime_manifest() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"files": {
              "bin": {"type": "directory"},
              "bin/java.exe": {"type": "file", "executable": true, "downloads": {
                "lzma": {"sha1": "b4d6", "size": 14407, "url": "https://piston-data.mojang.com/v1/objects/b4d6/java.exe"},
                "raw": {"sha1": "38dc", "size": 39424, "url": "https://piston-data.mojang.com/v1/objects/38dc/java.exe"}}},
              "legal/java.base/LICENSE": {"type": "link", "target": "../../LICENSE"}
            }}"#,
        )
        .unwrap();
        let java = &manifest.files["bin/java.exe"];
        assert!(java.executable);
        let raw = java.downloads.as_ref().unwrap().raw.as_ref().unwrap();
        assert_eq!((raw.sha1.as_str(), raw.size), ("38dc", 39424));
        assert_eq!(manifest.files["bin"].kind, "directory");
        assert_eq!(manifest.files["legal/java.base/LICENSE"].kind, "link");
    }

    fn fake_java(home: &Path, version: &str) {
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        std::fs::write(home.join("bin").join(exe), b"not a real java").unwrap();
        std::fs::write(
            home.join("release"),
            format!("JAVA_VERSION=\"{version}\"\n"),
        )
        .unwrap();
    }

    #[test]
    fn only_complete_downloads_count() {
        let dir = tempfile::tempdir().unwrap();
        let locator = JavaLocator {
            use_system: false,
            ..JavaLocator::for_game(dir.path())
        };
        assert!(locator.scan().is_empty(), "目录还不存在");

        let done = locator.managed_root.join("java-runtime-delta");
        fake_java(&done, "21.0.7");
        std::fs::write(done.join(MARKER), b"{}").unwrap();
        // 下到一半的：文件看起来齐了，但没有标记
        fake_java(&locator.managed_root.join("java-runtime-gamma"), "17.0.15");

        let found = locator.scan();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].major, 21);
        assert_eq!(found[0].origin, JavaOrigin::Downloaded);
    }

    fn install(home: &str, version: &str, origin: JavaOrigin) -> JavaInstall {
        JavaInstall {
            home: PathBuf::from(home),
            executable: PathBuf::from(home).join("bin").join("java.exe"),
            version: version.to_owned(),
            major: version.split('.').next().unwrap().parse().unwrap(),
            is_64bit: true,
            origin,
            vendor: None,
        }
    }

    #[test]
    fn the_users_default_wins_within_its_major_version() {
        let installs = [
            install(r"C:\Java\jdk-21.0.7", "21.0.7", JavaOrigin::System),
            install(r"C:\Java\jdk-21.0.3", "21.0.3", JavaOrigin::Manual),
            install(r"C:\Java\jdk-17", "17.0.12", JavaOrigin::System),
        ];
        let mut locator = JavaLocator::for_game(Path::new("D:/mc"));
        assert_eq!(
            locator.pick(&installs, 21).unwrap().version,
            "21.0.7",
            "没选时挑最新的"
        );

        locator
            .defaults
            .insert(21, PathBuf::from(r"c:\java\JDK-21.0.3\"));
        assert_eq!(locator.pick(&installs, 21).unwrap().version, "21.0.3");
        assert_eq!(
            locator.pick(&installs, 17).unwrap().version,
            "17.0.12",
            "别的大版本不受影响"
        );

        // 选的那个被删了：自动挑
        locator.defaults.insert(21, PathBuf::from(r"C:\gone"));
        assert_eq!(locator.pick(&installs, 21).unwrap().version, "21.0.7");
        // 默认只在同一个大版本里生效：要 Java 8 的老版本不会拿到 Java 21
        locator
            .defaults
            .insert(8, PathBuf::from(r"C:\Java\jdk-21.0.7"));
        assert!(locator.pick(&installs, 8).is_none());
    }

    #[test]
    fn only_downloaded_java_can_be_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let locator = JavaLocator::for_game(dir.path());
        let home = locator.managed_root.join("java-runtime-delta");
        fake_java(&home, "21.0.7");
        std::fs::write(home.join(MARKER), b"{}").unwrap();
        let downloaded = inspect(&home, JavaOrigin::Downloaded).unwrap();

        let mut pretend = downloaded.clone();
        pretend.origin = JavaOrigin::System;
        assert!(matches!(
            locator.delete_downloaded(&pretend),
            Err(JavaError::NotDownloaded)
        ));
        let outside = install(r"C:\Java\jdk-21", "21.0.7", JavaOrigin::Downloaded);
        assert!(
            matches!(
                locator.delete_downloaded(&outside),
                Err(JavaError::NotDownloaded)
            ),
            "不在 .mcl-java 里的不删"
        );

        locator.delete_downloaded(&downloaded).unwrap();
        assert!(!home.exists());
    }

    #[test]
    fn request_follows_the_version_file() {
        let version: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.1","javaVersion":{"component":"java-runtime-delta","majorVersion":21}}"#,
        )
        .unwrap();
        assert_eq!(
            JavaRequest::of(&version),
            request(21, Some("java-runtime-delta"))
        );
        assert_eq!(JavaRequest::of(&VersionJson::default()), request(8, None));
    }
}
