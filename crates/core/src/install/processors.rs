//! Forge 系安装器（NeoForge、新版 Forge）的通用执行流程。
//!
//! 安装器 jar 里有两份描述：`install_profile.json`（要下载的库、要运行的处理器、处理器参数用的数据）
//! 和 `version.json`（装好后的版本描述）。我们不运行官方安装器的界面，而是自己做它做的事：
//!
//! 1. 下载安装器，用 Maven 仓库给的 `.sha1` 校验
//! 2. 下载处理器要用的库（走自己的下载引擎：镜像、续传、校验）
//! 3. 依次运行客户端处理器（参数用数组传，不弹窗口，可取消）
//! 4. 检查处理器产物，写版本描述和「安装回执」
//!
//! 安装回执记下所有必需文件的大小和 SHA-1，启动前只比大小就能判断装没装好，
//! 文件被删或损坏会自动重装。
//!
//! 思路参考 portablemc（Apache-2.0）的 forge 模块，代码是重新写的。

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use super::InstallError;
use super::vanilla::{load_or_fetch_version, run_blocking};
use crate::config::DownloadSource;
use crate::fs::{read_json, safe_join, write_atomic, write_json_atomic};
use crate::meta::{Library, MavenCoord, VersionJson};
use crate::net::{
    DownloadError, DownloadItem, DownloadProgress, Downloader, candidate_urls, get_bytes, sha1_file,
};
use crate::paths::GameDir;
use crate::process::hidden_command;

/// 回执文件名（放在 `versions/<版本>/` 里）
const RECEIPT_FILE: &str = "launcher-install.json";
const RECEIPT_FORMAT: u32 = 1;
/// 处理器失败时，错误详情里保留的输出行数
const OUTPUT_TAIL_LINES: usize = 40;

// 多个实例可能同时依赖同一版本。安装器和共享库产物只让一个任务写。
type InstallLocks = HashMap<PathBuf, Weak<AsyncMutex<()>>>;
static INSTALL_LOCKS: LazyLock<Mutex<InstallLocks>> = LazyLock::new(Mutex::default);

fn install_lock(path: PathBuf) -> Arc<AsyncMutex<()>> {
    let mut locks = INSTALL_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    let slot = locks.entry(path).or_default();
    if let Some(lock) = slot.upgrade() {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    *slot = Arc::downgrade(&lock);
    lock
}

/// 安装加载器需要的环境。
pub struct LoaderContext<'a> {
    pub game: &'a GameDir,
    pub client: &'a Client,
    pub downloader: &'a Downloader,
    /// 文件（依赖库、安装器、客户端）从哪下（设置「Minecraft 文件、加载器与 Java」）
    pub source: DownloadSource,
    /// 版本清单、版本描述从哪拿（设置「Minecraft 元数据」，D18b）
    pub meta_source: DownloadSource,
    /// 运行处理器用的 Java
    pub java: &'a Path,
    /// 临时工作目录，装完删除。要放在空间够的盘上，不要用系统临时目录
    pub work_dir: &'a Path,
    pub cancel: &'a CancellationToken,
}

/// 安装进度。
#[derive(Debug, Clone)]
pub enum LoaderProgress {
    /// 下载原版客户端（处理器要用）
    GameJar(DownloadProgress),
    /// 下载安装器
    Installer(DownloadProgress),
    /// 下载处理器要用的库
    Libraries(DownloadProgress),
    /// 正在运行第 `index` 个处理器（从 1 开始）
    Processor {
        index: usize,
        total: usize,
        task: String,
    },
}

/// 要装哪个安装器。
pub struct InstallerSource {
    /// 装好后的版本 ID，必须和安装器里 `version.json` 的 `id` 一致（除非匹配 `accepted_installer_id`）
    pub expected_id: String,
    /// 旧版 1.20.1 NeoForge 等安装器内部 `version.json` 的 `id` 与预期不同时（如 `1.20.1-forge-47.1.106`），
    /// 允许接受并改写为 `expected_id`，避免和原版 Forge 目录冲突。
    pub accepted_installer_id: Option<String>,
    pub game_version: String,
    /// 安装器的官方地址（会按设置换镜像）
    pub installer_url: String,
}

#[derive(Debug, Deserialize)]
struct InstallProfile {
    minecraft: String,
    /// 安装器里 `version.json` 的位置
    json: String,
    /// 老安装器把主体 jar 打包在自己里面，这里是它的坐标
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    libraries: Vec<Library>,
    #[serde(default)]
    processors: Vec<Processor>,
    #[serde(default)]
    data: HashMap<String, DataEntry>,
}

#[derive(Debug, Deserialize)]
struct Processor {
    jar: String,
    #[serde(default)]
    sides: Option<Vec<String>>,
    #[serde(default)]
    classpath: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    /// 产物文件 → 期望的 SHA-1（两边都可能含 `{占位符}`）
    #[serde(default)]
    outputs: HashMap<String, String>,
}

impl Processor {
    fn runs_on_client(&self) -> bool {
        self.sides
            .as_ref()
            .is_none_or(|sides| sides.iter().any(|s| s == "client"))
    }

    /// `--task XXX` 里的任务名，没有就用 jar 的名字（显示和日志用）
    fn task_name(&self) -> String {
        self.args
            .iter()
            .position(|a| a == "--task")
            .and_then(|i| self.args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| {
                MavenCoord::parse(&self.jar).map_or_else(|| self.jar.clone(), |c| c.artifact)
            })
    }

    fn arg_after(&self, flag: &str) -> Option<&str> {
        self.args
            .iter()
            .position(|a| a == flag)
            .and_then(|i| self.args.get(i + 1))
            .map(String::as_str)
    }
}

#[derive(Debug, Deserialize)]
struct DataEntry {
    client: String,
}

/// 处理器参数用的数据表：名字 → 值
type DataTable = HashMap<String, DataValue>;
/// 要从安装器里解压的条目：条目名 → 目标位置
type Extract = (String, PathBuf);

/// 处理器参数里 `{名字}` 对应的值
#[derive(Debug, Clone, PartialEq, Eq)]
enum DataValue {
    /// 库目录里的文件（来自 `[坐标]`）
    Library(PathBuf),
    /// 从安装器里解压出来的文件，或内置的路径
    File(PathBuf),
    /// `'字面量'`
    Literal(String),
}

impl DataValue {
    fn as_arg(&self) -> String {
        match self {
            DataValue::Library(path) | DataValue::File(path) => path.to_string_lossy().into_owned(),
            DataValue::Literal(text) => text.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallReceipt {
    format: u32,
    installer_sha1: String,
    /// 必需文件（路径相对游戏目录，用 `/` 分隔）
    files: Vec<ReceiptFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReceiptFile {
    path: String,
    size: u64,
    sha1: String,
}

/// 安装（已经装好就直接返回版本 ID）。
pub async fn install<F>(
    ctx: &LoaderContext<'_>,
    src: &InstallerSource,
    verify_receipt: bool,
    on_progress: F,
) -> Result<String, InstallError>
where
    F: Fn(LoaderProgress) + Send + Sync,
{
    let id = src.expected_id.clone();
    let lock = install_lock(ctx.game.version_dir(&id)?);
    let _turn = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => return Err(DownloadError::Cancelled.into()),
        guard = lock.lock() => guard,
    };
    let installed = if verify_receipt {
        is_intact(ctx.game, &id).await
    } else {
        is_installed(ctx.game, &id)
    };
    if installed {
        tracing::info!(version = %id, "加载器已安装，跳过");
        return Ok(id);
    }
    let work_dir = safe_join(ctx.work_dir, &id)?;
    let local_ctx = LoaderContext {
        work_dir: &work_dir,
        ..*ctx
    };
    let ctx = &local_ctx;
    tracing::info!(version = %id, "开始安装加载器");
    tokio::fs::create_dir_all(ctx.work_dir)
        .await
        .map_err(|source| io_error(ctx.work_dir, source))?;

    // 1. 原版：版本描述 + 客户端 jar（处理器要拿它打补丁）
    let vanilla =
        load_or_fetch_version(ctx.game, &src.game_version, ctx.client, ctx.meta_source).await?;
    let client_jar = ensure_client_jar(ctx, &vanilla, &on_progress).await?;

    // 2. 安装器
    let installer = ctx.work_dir.join("installer.jar");
    let installer_sha1 = fetch_maven_sha1(ctx.client, &src.installer_url, ctx.source).await?;
    let item = DownloadItem {
        urls: candidate_urls(&src.installer_url, ctx.source),
        path: installer.clone(),
        sha1: Some(installer_sha1.clone()),
        size: None,
    };
    ctx.downloader
        .download_all(&[item], ctx.cancel, |p| {
            on_progress(LoaderProgress::Installer(p))
        })
        .await?;

    let (profile, version_raw) = {
        let installer = installer.clone();
        run_blocking(move || read_installer(&installer)).await?
    };
    if profile.minecraft != src.game_version {
        return Err(InstallError::Installer(format!(
            "安装器对应的游戏版本是 {}，不是 {}",
            profile.minecraft, src.game_version
        )));
    }
    let version_raw =
        normalize_version_json_id(&id, src.accepted_installer_id.as_deref(), version_raw)?;

    // 3. 处理器要用的库：有地址的下载，没地址的从安装器里解压
    let mut receipt_files: Vec<PathBuf> = Vec::new();
    let mut downloads: Vec<DownloadItem> = Vec::new();
    let mut extracts: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = BTreeSet::new();
    for lib in &profile.libraries {
        let artifact = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref());
        let relative = artifact
            .and_then(|a| a.path.clone())
            .or_else(|| lib.coord().map(|c| c.path()))
            .ok_or_else(|| InstallError::Installer(format!("无法识别的库：{}", lib.name)))?;
        let path = ctx.game.library(&relative)?;
        if !seen.insert(path.clone()) {
            continue;
        }
        receipt_files.push(path.clone());
        match artifact.filter(|a| !a.url.is_empty()) {
            Some(a) => downloads.push(DownloadItem {
                urls: candidate_urls(&a.url, ctx.source),
                path,
                sha1: a.sha1.clone(),
                size: a.size,
            }),
            None => extracts.push((format!("maven/{relative}"), path)),
        }
    }
    if let Some(coord) = profile.path.as_deref().and_then(MavenCoord::parse) {
        let path = ctx.game.library(&coord.path())?;
        if seen.insert(path.clone()) {
            receipt_files.push(path.clone());
            extracts.push((format!("maven/{}", coord.path()), path));
        }
    }

    // 4. 处理器参数用的数据
    let (data, data_extracts) = resolve_data(&profile, ctx, &client_jar, &installer, src)?;
    extracts.extend(data_extracts);
    {
        let installer = installer.clone();
        run_blocking(move || extract_entries(&installer, &extracts)).await?;
    }
    ctx.downloader
        .download_all(&downloads, ctx.cancel, |p| {
            on_progress(LoaderProgress::Libraries(p))
        })
        .await?;

    // 5. 依次运行客户端处理器
    let processors: Vec<&Processor> = profile
        .processors
        .iter()
        .filter(|p| p.runs_on_client())
        .collect();
    let total = processors.len();
    for (index, processor) in processors.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Err(InstallError::Download(DownloadError::Cancelled));
        }
        let task = processor.task_name();
        on_progress(LoaderProgress::Processor {
            index: index + 1,
            total,
            task: task.clone(),
        });
        if task == "DOWNLOAD_MOJMAPS" && !processor.args.iter().any(|a| a == "--sanitize") {
            // 仅原样下载时可以用已校验的 Mojang 文件替代。Forge 的 --sanitize 会转换格式并校验产物。
            download_mojmaps(ctx, &vanilla, processor, &data).await?;
            continue;
        }
        run_processor(ctx, processor, &task, &data).await?;
    }

    // 6. 处理器产物都要在（客户端处理器参数里引用到的库文件）
    for path in referenced_library_outputs(&processors, &data) {
        if !path.is_file() {
            return Err(InstallError::Installer(format!(
                "处理器运行完了，但没有生成 {}",
                path.display()
            )));
        }
        if !receipt_files.contains(&path) {
            receipt_files.push(path);
        }
    }

    // 7. 写版本描述和回执，清理临时文件
    write_atomic(&ctx.game.version_json(&id)?, &version_raw)?;
    let receipt = build_receipt(ctx.game, installer_sha1, &receipt_files).await?;
    write_json_atomic(&receipt_path(ctx.game, &id)?, &receipt)?;
    if let Err(error) = tokio::fs::remove_dir_all(ctx.work_dir).await {
        tracing::warn!(%error, dir = %ctx.work_dir.display(), "清理临时目录失败");
    }
    tracing::info!(version = %id, files = receipt.files.len(), "加载器安装完成");
    Ok(id)
}

/// 版本描述在、回执在、回执里每个文件都在且大小一致，就算装好了。
pub fn is_installed(game: &GameDir, id: &str) -> bool {
    let Ok(json) = game.version_json(id) else {
        return false;
    };
    if !json.is_file() {
        return false;
    }
    let receipt: InstallReceipt = match receipt_path(game, id).map(|p| read_json(&p)) {
        Ok(Ok(Some(receipt))) => receipt,
        Ok(Ok(None)) => return false,
        Ok(Err(error)) => {
            tracing::warn!(version = id, %error, "安装回执损坏，将重新安装");
            return false;
        }
        Err(_) => return false,
    };
    if receipt.format != RECEIPT_FORMAT {
        return false;
    }
    receipt.files.iter().all(|file| {
        let ok = safe_join(game.root(), &file.path)
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .is_some_and(|m| m.len() == file.size);
        if !ok {
            tracing::info!(version = id, file = %file.path, "加载器文件缺失或大小不对，将重新安装");
        }
        ok
    })
}

async fn is_intact(game: &GameDir, id: &str) -> bool {
    if !is_installed(game, id) || super::vanilla::read_local(game, id).is_err() {
        return false;
    }
    let Ok(path) = receipt_path(game, id) else {
        return false;
    };
    let Ok(Some(receipt)) = read_json::<InstallReceipt>(&path) else {
        return false;
    };
    for file in receipt.files {
        let Ok(path) = safe_join(game.root(), &file.path) else {
            return false;
        };
        if !sha1_file(&path)
            .await
            .is_ok_and(|hash| hash.eq_ignore_ascii_case(&file.sha1))
        {
            return false;
        }
    }
    true
}

fn receipt_path(game: &GameDir, id: &str) -> Result<PathBuf, InstallError> {
    Ok(game.version_dir(id)?.join(RECEIPT_FILE))
}

async fn build_receipt(
    game: &GameDir,
    installer_sha1: String,
    files: &[PathBuf],
) -> Result<InstallReceipt, InstallError> {
    let mut entries = Vec::with_capacity(files.len());
    for path in files {
        let size = tokio::fs::metadata(path)
            .await
            .map_err(|source| io_error(path, source))?
            .len();
        let sha1 = sha1_file(path)
            .await
            .map_err(|source| io_error(path, source))?;
        let relative = path
            .strip_prefix(game.root())
            .map_err(|_| InstallError::Installer(format!("{} 不在游戏目录里", path.display())))?;
        entries.push(ReceiptFile {
            path: relative.to_string_lossy().replace('\\', "/"),
            size,
            sha1,
        });
    }
    Ok(InstallReceipt {
        format: RECEIPT_FORMAT,
        installer_sha1,
        files: entries,
    })
}

async fn ensure_client_jar<F>(
    ctx: &LoaderContext<'_>,
    vanilla: &VersionJson,
    on_progress: &F,
) -> Result<PathBuf, InstallError>
where
    F: Fn(LoaderProgress) + Send + Sync,
{
    let jar_id = vanilla.jar.clone().unwrap_or_else(|| vanilla.id.clone());
    let path = ctx.game.version_jar(&jar_id)?;
    match vanilla.downloads.as_ref().and_then(|d| d.client.as_ref()) {
        Some(client) => {
            let item = DownloadItem {
                urls: candidate_urls(&client.url, ctx.source),
                path: path.clone(),
                sha1: client.sha1.clone(),
                size: client.size,
            };
            ctx.downloader
                .download_all(&[item], ctx.cancel, |p| {
                    on_progress(LoaderProgress::GameJar(p))
                })
                .await?;
        }
        None if path.is_file() => {}
        None => return Err(InstallError::NoClientDownload(vanilla.id.clone())),
    }
    Ok(path)
}

/// Maven 仓库每个文件旁边都有 `.sha1`，用它校验安装器。
pub(super) async fn fetch_maven_sha1(
    client: &Client,
    url: &str,
    source: DownloadSource,
) -> Result<String, InstallError> {
    let bytes = get_bytes(client, &candidate_urls(&format!("{url}.sha1"), source)).await?;
    let text = String::from_utf8_lossy(&bytes);
    let hash = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hash)
    } else {
        Err(InstallError::Installer(format!(
            "{url}.sha1 的内容不是有效的 SHA-1"
        )))
    }
}

/// 读出 `install_profile.json` 和 `version.json`（原始字节，原样保存）。
fn read_installer(installer: &Path) -> Result<(InstallProfile, Vec<u8>), InstallError> {
    let mut archive = open_zip(installer)?;
    let profile_raw = read_entry(&mut archive, installer, "install_profile.json")?;
    let profile: InstallProfile = serde_json::from_slice(&profile_raw).map_err(|e| {
        InstallError::Installer(format!(
            "安装器格式不认识（可能是 1.12.2 及更早的老安装器，暂不支持）：{e}"
        ))
    })?;
    let json_entry = profile.json.trim_start_matches('/').to_owned();
    let version_raw = read_entry(&mut archive, installer, &json_entry)?;
    Ok((profile, version_raw))
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, InstallError> {
    let file = std::fs::File::open(path).map_err(|source| io_error(path, source))?;
    zip::ZipArchive::new(file)
        .map_err(|e| InstallError::Installer(format!("{} 不是有效的压缩包：{e}", path.display())))
}

fn read_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    zip_path: &Path,
    name: &str,
) -> Result<Vec<u8>, InstallError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| InstallError::Installer(format!("安装器里缺少 {name}")))?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(zip_path, source))?;
    Ok(bytes)
}

/// 把安装器里的若干条目解压到指定位置（先写临时文件再改名，中途失败不会留下半个文件）。
fn extract_entries(installer: &Path, entries: &[(String, PathBuf)]) -> Result<(), InstallError> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut archive = open_zip(installer)?;
    for (name, dest) in entries {
        let bytes = read_entry(&mut archive, installer, name)?;
        write_atomic(dest, &bytes)?;
    }
    Ok(())
}

/// 处理器参数用的数据：安装器给的 + 内置的。返回数据表和需要从安装器解压的文件。
fn resolve_data(
    profile: &InstallProfile,
    ctx: &LoaderContext<'_>,
    client_jar: &Path,
    installer: &Path,
    src: &InstallerSource,
) -> Result<(DataTable, Vec<Extract>), InstallError> {
    let mut data = HashMap::new();
    let mut extracts = Vec::new();
    for (key, entry) in &profile.data {
        let value = entry.client.as_str();
        let resolved = if let Some(coord) = bracketed(value) {
            DataValue::Library(library_path(ctx.game, coord)?)
        } else if let Some(literal) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            DataValue::Literal(literal.to_owned())
        } else {
            // 安装器里的文件，例如 `/data/client.lzma`
            let entry_name = value.trim_start_matches('/');
            let dest = safe_join(&ctx.work_dir.join("data"), entry_name)?;
            extracts.push((entry_name.to_owned(), dest.clone()));
            DataValue::File(dest)
        };
        data.insert(key.clone(), resolved);
    }
    let builtins = [
        ("SIDE", DataValue::Literal("client".to_owned())),
        ("MINECRAFT_JAR", DataValue::File(client_jar.to_path_buf())),
        (
            "MINECRAFT_VERSION",
            DataValue::Literal(src.game_version.clone()),
        ),
        ("INSTALLER", DataValue::File(installer.to_path_buf())),
        ("LIBRARY_DIR", DataValue::File(ctx.game.libraries())),
        ("ROOT", DataValue::File(ctx.game.root().to_path_buf())),
    ];
    for (key, value) in builtins {
        data.insert(key.to_owned(), value);
    }
    Ok((data, extracts))
}

fn bracketed(value: &str) -> Option<&str> {
    value.strip_prefix('[').and_then(|v| v.strip_suffix(']'))
}

fn library_path(game: &GameDir, coordinate: &str) -> Result<PathBuf, InstallError> {
    let coord = MavenCoord::parse(coordinate)
        .ok_or_else(|| InstallError::Installer(format!("无法识别的库坐标：{coordinate}")))?;
    Ok(game.library(&coord.path())?)
}

/// 展开处理器参数。规则和官方安装器一致：
/// - 整个参数是 `[坐标]` → 库文件路径
/// - `{名字}` → 数据表里的值
/// - `'文字'` → 原样的文字
/// - `\` 转义下一个字符
fn expand_arg(
    arg: &str,
    data: &HashMap<String, DataValue>,
    game: &GameDir,
) -> Result<String, InstallError> {
    if let Some(coord) = bracketed(arg) {
        return Ok(library_path(game, coord)?.to_string_lossy().into_owned());
    }
    let invalid = || InstallError::Installer(format!("处理器参数格式不对：{arg}"));
    let mut out = String::with_capacity(arg.len());
    let mut chars = arg.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next().ok_or_else(invalid)?),
            '{' => {
                let key: String = chars.by_ref().take_while(|&c| c != '}').collect();
                let value = data.get(&key).ok_or_else(|| {
                    InstallError::Installer(format!("处理器参数引用了未知数据 {{{key}}}"))
                })?;
                out.push_str(&value.as_arg());
            }
            '\'' => out.extend(chars.by_ref().take_while(|&c| c != '\'')),
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// 客户端处理器参数里引用到的 `[坐标]` 数据（处理器产物都在这里面）。
fn referenced_library_outputs(
    processors: &[&Processor],
    data: &HashMap<String, DataValue>,
) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for processor in processors {
        for arg in &processor.args {
            let mut rest = arg.as_str();
            while let Some(start) = rest.find('{') {
                let after = &rest[start + 1..];
                let Some(end) = after.find('}') else { break };
                if let Some(DataValue::Library(path)) = data.get(&after[..end]) {
                    paths.insert(path.clone());
                }
                rest = &after[end + 1..];
            }
        }
    }
    paths.into_iter().collect()
}

async fn download_mojmaps(
    ctx: &LoaderContext<'_>,
    vanilla: &VersionJson,
    processor: &Processor,
    data: &HashMap<String, DataValue>,
) -> Result<(), InstallError> {
    let output = processor
        .arg_after("--output")
        .ok_or_else(|| InstallError::Installer("映射表处理器缺少 --output 参数".to_owned()))?;
    let path = PathBuf::from(expand_arg(output, data, ctx.game)?);
    let mappings = vanilla
        .downloads
        .as_ref()
        .and_then(|d| d.client_mappings.as_ref())
        .ok_or_else(|| InstallError::Installer(format!("{} 没有提供映射表下载信息", vanilla.id)))?;
    let item = DownloadItem {
        urls: candidate_urls(&mappings.url, ctx.source),
        path,
        sha1: mappings.sha1.clone(),
        size: mappings.size,
    };
    ctx.downloader
        .download_all(&[item], ctx.cancel, |_| {})
        .await?;
    Ok(())
}

async fn run_processor(
    ctx: &LoaderContext<'_>,
    processor: &Processor,
    task: &str,
    data: &HashMap<String, DataValue>,
) -> Result<(), InstallError> {
    let jar = library_path(ctx.game, &processor.jar)?;
    let mut classpath = vec![jar.clone()];
    for dependency in &processor.classpath {
        classpath.push(library_path(ctx.game, dependency)?);
    }
    if let Some(missing) = classpath.iter().find(|p| !p.is_file()) {
        return Err(InstallError::Installer(format!(
            "处理器 {task} 缺少文件 {}",
            missing.display()
        )));
    }
    let main_class = {
        let jar = jar.clone();
        run_blocking(move || main_class_of(&jar)).await?
    };
    let classpath = std::env::join_paths(&classpath)
        .map_err(|e| InstallError::Installer(format!("处理器类路径无效：{e}")))?;
    let args = processor
        .args
        .iter()
        .map(|a| expand_arg(a, data, ctx.game))
        .collect::<Result<Vec<_>, _>>()?;

    tracing::info!(task, main_class, "运行安装处理器");
    let mut command = hidden_command(ctx.java);
    command
        .arg("-cp")
        .arg(&classpath)
        .arg(&main_class)
        .args(&args)
        .current_dir(ctx.work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 取消时丢弃这个 future，子进程会被一起结束
        .kill_on_drop(true);

    let output = tokio::select! {
        output = command.output() => output.map_err(|source| io_error(ctx.java, source))?,
        () = ctx.cancel.cancelled() => return Err(InstallError::Download(DownloadError::Cancelled)),
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for line in text.lines() {
        tracing::debug!(task, "{line}");
    }
    if !output.status.success() {
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].join("\n");
        return Err(InstallError::Processor {
            task: task.to_owned(),
            code: output.status.code(),
            output: tail,
        });
    }

    for (file, expected) in &processor.outputs {
        let path = PathBuf::from(expand_arg(file, data, ctx.game)?);
        let expected = expand_arg(expected, data, ctx.game)?.to_ascii_lowercase();
        let actual = sha1_file(&path)
            .await
            .map_err(|source| io_error(&path, source))?;
        if actual != expected {
            return Err(InstallError::Installer(format!(
                "处理器 {task} 生成的 {} 校验不通过",
                path.display()
            )));
        }
    }
    Ok(())
}

/// 从 jar 的 `META-INF/MANIFEST.MF` 读主类。清单的长行会折行（下一行以一个空格开头），要先拼回去。
fn main_class_of(jar: &Path) -> Result<String, InstallError> {
    let mut archive = open_zip(jar)?;
    let manifest = read_entry(&mut archive, jar, "META-INF/MANIFEST.MF")?;
    parse_main_class(&String::from_utf8_lossy(&manifest))
        .ok_or_else(|| InstallError::Installer(format!("{} 没有写主类", jar.display())))
}

fn parse_main_class(manifest: &str) -> Option<String> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in manifest.lines() {
        match (line.strip_prefix(' '), unfolded.last_mut()) {
            (Some(continuation), Some(last)) => last.push_str(continuation),
            _ => unfolded.push(line.to_owned()),
        }
    }
    unfolded.iter().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "Main-Class").then(|| value.trim().to_owned())
    })
}

fn io_error(path: &Path, source: io::Error) -> InstallError {
    InstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 校验安装器里 `version.json` 的 `id`；当它匹配 `accepted_installer_id`（例如旧版 1.20.1 NeoForge
/// 的 `1.20.1-forge-47.1.106`）时，把 JSON 里的 `id` 改写为 `expected_id` 避免与原版 Forge 冲突。
fn normalize_version_json_id(
    expected_id: &str,
    accepted_installer_id: Option<&str>,
    version_raw: Vec<u8>,
) -> Result<Vec<u8>, InstallError> {
    let version = VersionJson::parse(expected_id, &version_raw)?;
    if version.id == expected_id {
        return Ok(version_raw);
    }
    if accepted_installer_id == Some(version.id.as_str()) {
        let mut value: serde_json::Value = serde_json::from_slice(&version_raw)
            .map_err(|e| InstallError::Installer(format!("解析版本描述失败：{e}")))?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "id".to_owned(),
                serde_json::Value::String(expected_id.to_owned()),
            );
        }
        let rewritten = serde_json::to_vec_pretty(&value)
            .map_err(|e| InstallError::Installer(format!("重写版本描述失败：{e}")))?;
        let checked = VersionJson::parse(expected_id, &rewritten)?;
        if checked.id == expected_id {
            return Ok(rewritten);
        }
    }
    Err(InstallError::Installer(format!(
        "安装器里的版本名是 {}，和预期的 {expected_id} 不一致",
        version.id
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data(game: &GameDir) -> HashMap<String, DataValue> {
        HashMap::from([
            (
                "PATCHED".to_owned(),
                DataValue::Library(
                    library_path(game, "net.neoforged:neoforge:21.1.265:client").unwrap(),
                ),
            ),
            (
                "MCP_VERSION".to_owned(),
                DataValue::Literal("1.21.1-20240808.144430".to_owned()),
            ),
            ("SIDE".to_owned(), DataValue::Literal("client".to_owned())),
        ])
    }

    #[test]
    fn expands_processor_arguments_like_the_official_installer() {
        let game = GameDir::new("D:/mc");
        let data = sample_data(&game);
        let patched = expand_arg("{PATCHED}", &data, &game).unwrap();
        assert!(
            patched.replace('\\', "/").ends_with(
                "libraries/net/neoforged/neoforge/21.1.265/neoforge-21.1.265-client.jar"
            )
        );
        assert_eq!(
            expand_arg("--side={SIDE}", &data, &game).unwrap(),
            "--side=client"
        );
        assert_eq!(
            expand_arg("'{literal}'", &data, &game).unwrap(),
            "{literal}"
        );
        assert_eq!(expand_arg("a\\{b", &data, &game).unwrap(), "a{b");
        let zip = expand_arg(
            "[net.neoforged:neoform:1.21.1-20240808.144430@zip]",
            &data,
            &game,
        )
        .unwrap();
        assert!(
            zip.replace('\\', "/")
                .ends_with("neoform-1.21.1-20240808.144430.zip")
        );
        assert!(expand_arg("{MISSING}", &data, &game).is_err());
        assert!(expand_arg("trailing\\", &data, &game).is_err());
    }

    #[test]
    fn hostile_coordinates_cannot_escape_the_library_directory() {
        let game = GameDir::new("D:/mc");
        assert!(library_path(&game, "..:..:..").is_err());
        assert!(library_path(&game, "a:b").is_err());
    }

    #[test]
    fn reads_folded_manifest_lines() {
        let manifest = "Manifest-Version: 1.0\r\nMain-Class: net.neoforged.installertools.Insta\r\n llerTools\r\nCreated-By: x\r\n";
        assert_eq!(
            parse_main_class(manifest).as_deref(),
            Some("net.neoforged.installertools.InstallerTools")
        );
        assert_eq!(parse_main_class("Manifest-Version: 1.0\n"), None);
    }

    #[test]
    fn finds_outputs_referenced_by_client_processors() {
        let game = GameDir::new("D:/mc");
        let data = sample_data(&game);
        let processor = Processor {
            jar: "x:y:1".to_owned(),
            sides: None,
            classpath: Vec::new(),
            args: vec![
                "--output".to_owned(),
                "{PATCHED}".to_owned(),
                "{SIDE}".to_owned(),
            ],
            outputs: HashMap::new(),
        };
        let outputs = referenced_library_outputs(&[&processor], &data);
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].ends_with("neoforge-21.1.265-client.jar"));
    }

    #[test]
    fn processor_sides_and_task_names() {
        let p = |sides: Option<Vec<&str>>, args: Vec<&str>| Processor {
            jar: "net.neoforged.installertools:jarsplitter:2.1.2".to_owned(),
            sides: sides.map(|s| s.into_iter().map(str::to_owned).collect()),
            classpath: Vec::new(),
            args: args.into_iter().map(str::to_owned).collect(),
            outputs: HashMap::new(),
        };
        assert!(p(None, vec![]).runs_on_client());
        assert!(p(Some(vec!["client"]), vec![]).runs_on_client());
        assert!(!p(Some(vec!["server"]), vec![]).runs_on_client());
        assert_eq!(p(None, vec!["--task", "MCP_DATA"]).task_name(), "MCP_DATA");
        assert_eq!(p(None, vec!["--input", "x"]).task_name(), "jarsplitter");
    }

    #[test]
    fn missing_receipt_means_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        assert!(!is_installed(&game, "neoforge-21.1.265"));
        std::fs::create_dir_all(game.version_dir("neoforge-21.1.265").unwrap()).unwrap();
        std::fs::write(game.version_json("neoforge-21.1.265").unwrap(), b"{}").unwrap();
        assert!(
            !is_installed(&game, "neoforge-21.1.265"),
            "有版本描述但没有回执"
        );
    }

    #[test]
    fn receipt_detects_missing_or_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let id = "neoforge-21.1.265";
        std::fs::create_dir_all(game.version_dir(id).unwrap()).unwrap();
        std::fs::write(game.version_json(id).unwrap(), b"{}").unwrap();
        let lib = game.library("a/b/c.jar").unwrap();
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::write(&lib, b"12345").unwrap();
        let receipt = InstallReceipt {
            format: RECEIPT_FORMAT,
            installer_sha1: String::new(),
            files: vec![ReceiptFile {
                path: "libraries/a/b/c.jar".to_owned(),
                size: 5,
                sha1: String::new(),
            }],
        };
        write_json_atomic(&receipt_path(&game, id).unwrap(), &receipt).unwrap();
        assert!(is_installed(&game, id));
        std::fs::write(&lib, b"123").unwrap();
        assert!(!is_installed(&game, id), "大小变了");
        std::fs::remove_file(&lib).unwrap();
        assert!(!is_installed(&game, id), "文件没了");
    }

    #[test]
    fn normalizes_legacy_neoforge_1201_version_id() {
        let raw = br#"{"id":"1.20.1-forge-47.1.106","inheritsFrom":"1.20.1","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher","libraries":[]}"#.to_vec();
        let rewritten =
            normalize_version_json_id("neoforge-47.1.106", Some("1.20.1-forge-47.1.106"), raw)
                .unwrap();
        let parsed = VersionJson::parse("neoforge-47.1.106", &rewritten).unwrap();
        assert_eq!(parsed.id, "neoforge-47.1.106");

        let mismatched = br#"{"id":"1.20.1-forge-47.1.99","inheritsFrom":"1.20.1","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher","libraries":[]}"#.to_vec();
        assert!(
            normalize_version_json_id(
                "neoforge-47.1.106",
                Some("1.20.1-forge-47.1.106"),
                mismatched
            )
            .is_err()
        );
    }
}
