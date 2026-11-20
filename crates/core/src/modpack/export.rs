//! 导出 Modrinth 整合包（`.mrpack`，D17i）。
//!
//! - 模组、资源包、光影包：在 Modrinth 上按 SHA-1 找到一模一样的文件的，只在清单里写下载地址；
//!   找不到的（CurseForge 独有的、自己做的）直接打包进 `overrides/`。停用的模组也带上，导入后还是停用。
//! - 别的文件（配置、`options.txt`、脚本……）按第一层的名字一项一项给用户勾；日志、缓存、账户数据这类永远不带。
//! - 世界一个一个勾，`session.lock` 不带。
//!
//! 先 [`plan`]（读文件、联网查一次 Modrinth），给界面看 [`ExportPlan::preview`]，再 [`ExportPlan::write`]。
//! 写的时候先写到 `<目标>.part`，写完再改名，中途取消或失败不留半个文件。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio_util::sync::CancellationToken;

use super::ModpackError;
use crate::content::identify::{IdentityCache, file_key};
use crate::content::modrinth;
use crate::content::store::STORE_DIR;
use crate::instance::{Instance, LoaderKind, LoaderRef, META_DIR};
use crate::paths::GameDir;

const INDEX_FILE: &str = super::mrpack::INDEX_FILE;
const OVERRIDES: &str = "overrides";
const DISABLED_SUFFIX: &str = ".disabled";

/// 第一层里永远不带的：日志、缓存、截图、本启动器自己的东西、账户和玩家数据
const NEVER_TOP: &[&str] = &[
    META_DIR,
    "logs",
    "crash-reports",
    "screenshots",
    "debug",
    "downloads",
    "backups",
    ".mixin.out",
    ".fabric",
    ".quilt",
    ".cache",
    ".git",
    "__macosx",
    // Essential 模组的账户数据
    "essential",
    "usercache.json",
    "usernamecache.json",
    "realms_persistence.json",
    "command_history.txt",
    "servers.dat",
    "servers.dat_old",
    // 挂上的别家版本（D19）：别家的游戏本体、启动器自己的设置（没开版本隔离时游戏目录就是整个 .minecraft）
    "versions",
    "libraries",
    "assets",
    "runtime",
    "instances",
    "PCL",
    "BHL",
    "hmclversion.cfg",
    "launcher_profiles.json",
    "launcher_accounts.json",
    "launcher_settings.json",
    "axolotl_config.json",
];
/// 第一层里默认不勾的：地图模组给每个世界、每个服务器存的地图缓存（常常有几百 MB），模组运行时自己生成的缓存
const DEFAULT_OFF_TOP: &[&str] = &[
    "cache",
    "dynamic-data-pack-cache",
    "dynamic-resource-pack-cache",
    "journeymap",
    "xaero",
    "xaerominimap",
    "xaeroworldmap",
    "xaerowaypoints",
    ".bobby",
    "distant_horizons_server_data",
];
/// 单个文件的上限：超过 4 GB 要用 ZIP64，有的启动器读不了
const MAX_FILE_BYTES: u64 = u32::MAX as u64;

/// 一个要打包的文件：在压缩包里叫什么（`overrides/` 后面的部分）、在磁盘上哪里、多大
#[derive(Debug, Clone)]
struct PackedFile {
    rel: String,
    path: PathBuf,
    size: u64,
}

/// 模组 / 资源包 / 光影包里的一个文件
#[derive(Debug, Clone)]
struct ContentFile {
    file: PackedFile,
    disabled: bool,
    sha1: String,
    /// 在 Modrinth 上找到了一模一样的文件
    linked: Option<Linked>,
}

#[derive(Debug, Clone)]
struct Linked {
    url: String,
    sha512: String,
}

/// 第一层的一项（`config`、`options.txt`……），或者解开放的资源包 / 光影包文件夹
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    folder: bool,
    files: Vec<PackedFile>,
}

impl Entry {
    fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Debug, Clone)]
struct WorldEntry {
    folder: String,
    name: String,
    files: Vec<PackedFile>,
}

/// 读好、查好的导出计划。
#[derive(Debug, Clone)]
pub struct ExportPlan {
    name: String,
    game_version: String,
    loader: Option<LoaderRef>,
    content: Vec<ContentFile>,
    /// 解开放的资源包 / 光影包（只能直接打包）
    content_folders: Vec<Entry>,
    entries: Vec<Entry>,
    worlds: Vec<WorldEntry>,
}

/// 给界面看的导出预览。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    /// 默认的整合包名（实例名）
    pub name: String,
    pub game_version: String,
    /// 例如「NeoForge 21.1.265」；原版为空
    pub loader: Option<String>,
    /// 从 Modrinth 下载（清单里只写地址）的文件数
    pub linked: u32,
    /// Modrinth 上没有、要直接打包进去的内容（文件名，按名字排）
    pub bundled: Vec<String>,
    /// 直接打包的内容一共多大
    pub bundled_bytes: f64,
    /// 其中停用的模组数（也会带上，导入后还是停用）
    pub disabled: u32,
    /// 别的文件，按第一层的名字
    pub entries: Vec<ExportEntry>,
    pub worlds: Vec<ExportWorld>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    pub name: String,
    pub folder: bool,
    pub size: f64,
    /// 默认勾不勾（地图缓存这类默认不勾）
    pub default_on: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportWorld {
    pub folder: String,
    pub name: String,
    pub size: f64,
}

/// 用户在导出弹窗里选的。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub name: String,
    /// 整合包自己的版本号，例如 1.0.0
    pub version: String,
    pub summary: Option<String>,
    /// 带上第一层的哪几项（[`ExportEntry::name`]）
    pub entries: Vec<String>,
    /// 带上哪几个世界（存档文件夹名）
    pub worlds: Vec<String>,
}

/// 导出完的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub linked: usize,
    pub bundled: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
}

// ---------- 清单（modrinth.index.json）----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Index<'a> {
    format_version: u32,
    game: &'static str,
    version_id: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    files: Vec<IndexFile<'a>>,
    dependencies: BTreeMap<&'static str, &'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile<'a> {
    path: &'a str,
    hashes: BTreeMap<&'static str, &'a str>,
    downloads: [&'a str; 1],
    file_size: u64,
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> ModpackError + '_ {
    move |source| ModpackError::Io {
        path: path.to_path_buf(),
        source,
    }
}

// ---------- 扫描 ----------

/// 一个文件夹下面所有的文件（不跟随符号链接），压缩包里的名字是 `prefix/相对路径`
fn walk(
    root: &Path,
    prefix: &str,
    skip: &dyn Fn(&str) -> bool,
) -> Result<Vec<PackedFile>, ModpackError> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), prefix.to_owned())];
    while let Some((dir, rel)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(io_error(&dir))?;
        for entry in entries {
            let entry = entry.map_err(io_error(&dir))?;
            let Ok(name) = entry.file_name().into_string() else {
                tracing::warn!(path = %entry.path().display(), "文件名不是有效的文字，不导出");
                continue;
            };
            let child = format!("{rel}/{name}");
            if skip(&child) {
                continue;
            }
            let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
            if file_type.is_symlink() {
                tracing::warn!(path = %entry.path().display(), "符号链接不导出");
            } else if file_type.is_dir() {
                stack.push((entry.path(), child));
            } else {
                let size = entry.metadata().map_err(io_error(&entry.path()))?.len();
                files.push(PackedFile {
                    rel: child,
                    path: entry.path(),
                    size,
                });
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

fn no_skip(_: &str) -> bool {
    false
}

fn is_session_lock(rel: &str) -> bool {
    rel.rsplit('/')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("session.lock"))
}

fn sha1_of(path: &Path) -> Result<String, ModpackError> {
    let mut file = std::fs::File::open(path).map_err(io_error(path))?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(io_error(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// 扫描结果（还没查 Modrinth）
struct Scanned {
    content: Vec<ContentFile>,
    content_folders: Vec<Entry>,
    entries: Vec<Entry>,
    worlds: Vec<WorldEntry>,
}

/// 内容文件夹里的一项算不算内容：模组只认 `.jar`（停用的 `.jar.disabled`），资源包、光影包认 `.zip`
fn content_name(folder: &str, name: &str) -> Option<bool> {
    let lower = name.to_ascii_lowercase();
    let (base, disabled) = match lower.strip_suffix(DISABLED_SUFFIX) {
        Some(base) if folder == "mods" => (base.to_owned(), true),
        _ => (lower, false),
    };
    let wanted = if folder == "mods" { ".jar" } else { ".zip" };
    base.ends_with(wanted).then_some(disabled)
}

fn scan(game: &GameDir, instance: &Instance) -> Result<Scanned, ModpackError> {
    let dir = instance.game_dir(game)?;
    let cache = IdentityCache::load(&game.root().join(STORE_DIR));
    // 版本文件夹里的版本描述、客户端：老实例的（文件夹名），挂上的别家版本的（版本名，D19）
    let version_name = instance
        .config
        .link
        .as_ref()
        .map_or(instance.id.as_str(), |l| l.version.as_str());
    let own_json = format!("{version_name}.json").to_lowercase();
    let own_jar = format!("{version_name}.jar").to_lowercase();
    let own_natives = format!("{version_name}-natives").to_lowercase();

    let mut content = Vec::new();
    let mut content_folders = Vec::new();
    // 资源包、光影包文件夹里不是包的文件（光影的设置 `xxx.zip.txt` 这类）算「别的文件」
    let mut loose: Vec<PackedFile> = Vec::new();
    for folder in ["mods", "resourcepacks", "shaderpacks"] {
        let root = dir.join(folder);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(ModpackError::Io { path: root, source }),
        };
        for entry in entries {
            let entry = entry.map_err(io_error(&root))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
            let rel = format!("{folder}/{name}");
            if file_type.is_symlink() {
                tracing::warn!(path = %entry.path().display(), "符号链接不导出");
                continue;
            }
            if file_type.is_dir() {
                // 模组文件夹里的子文件夹是模组自己的缓存（例如 Connector 的 .connector），不带
                if folder != "mods" {
                    let files = walk(&entry.path(), &rel, &no_skip)?;
                    if !files.is_empty() {
                        content_folders.push(Entry {
                            name: rel,
                            folder: true,
                            files,
                        });
                    }
                }
                continue;
            }
            let metadata = entry.metadata().map_err(io_error(&entry.path()))?;
            let file = PackedFile {
                rel,
                path: entry.path(),
                size: metadata.len(),
            };
            match content_name(folder, &name) {
                Some(disabled) => {
                    let sha1 = match cache.hash_of(&file_key(folder, &name, &metadata)) {
                        Some(hash) => hash.clone(),
                        None => sha1_of(&file.path)?,
                    };
                    content.push(ContentFile {
                        file,
                        disabled,
                        sha1,
                        linked: None,
                    });
                }
                None if folder != "mods" => loose.push(file),
                None => {}
            }
        }
    }

    let mut entries = Vec::new();
    let mut worlds = Vec::new();
    let top = std::fs::read_dir(&dir).map_err(io_error(&dir))?;
    for entry in top {
        let entry = entry.map_err(io_error(&dir))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let lower = name.to_lowercase();
        let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
        let skipped = NEVER_TOP.iter().any(|n| n.eq_ignore_ascii_case(&name))
            || lower.starts_with("natives")
            || lower == own_json
            || lower == own_jar
            || lower == own_natives
            || ["mods", "resourcepacks", "shaderpacks"].contains(&lower.as_str())
            || file_type.is_symlink();
        if skipped {
            continue;
        }
        if lower == "saves" && file_type.is_dir() {
            worlds = scan_worlds(&entry.path())?;
            continue;
        }
        let files = if file_type.is_dir() {
            walk(&entry.path(), &name, &no_skip)?
        } else {
            vec![PackedFile {
                rel: name.clone(),
                path: entry.path(),
                size: entry.metadata().map_err(io_error(&entry.path()))?.len(),
            }]
        };
        if !files.is_empty() {
            entries.push(Entry {
                name,
                folder: file_type.is_dir(),
                files,
            });
        }
    }
    for file in loose {
        entries.push(Entry {
            name: file.rel.clone(),
            folder: false,
            files: vec![file],
        });
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    content.sort_by(|a, b| a.file.rel.cmp(&b.file.rel));
    content_folders.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Scanned {
        content,
        content_folders,
        entries,
        worlds,
    })
}

fn scan_worlds(saves: &Path) -> Result<Vec<WorldEntry>, ModpackError> {
    let instance_dir = saves.parent().unwrap_or(saves);
    let names: BTreeMap<String, String> = crate::saves::list(instance_dir, "")
        .into_iter()
        .map(|w| (w.folder, w.name))
        .collect();
    let mut worlds = Vec::new();
    for entry in std::fs::read_dir(saves).map_err(io_error(saves))? {
        let entry = entry.map_err(io_error(saves))?;
        let Ok(folder) = entry.file_name().into_string() else {
            continue;
        };
        let file_type = entry.file_type().map_err(io_error(&entry.path()))?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let files = walk(&entry.path(), &format!("saves/{folder}"), &is_session_lock)?;
        if files.is_empty() {
            continue;
        }
        worlds.push(WorldEntry {
            name: names
                .get(&folder)
                .cloned()
                .unwrap_or_else(|| folder.clone()),
            folder,
            files,
        });
    }
    worlds.sort_by_key(|w| w.name.to_lowercase());
    Ok(worlds)
}

/// 读实例、联网查一次 Modrinth，得到导出计划。连不上 Modrinth 时报错（不悄悄把所有模组都打包进去）。
pub async fn plan(
    client: &Client,
    game: &GameDir,
    instance: &Instance,
) -> Result<ExportPlan, ModpackError> {
    let scan_game = game.clone();
    let scan_instance = instance.clone();
    let mut scanned = tokio::task::spawn_blocking(move || scan(&scan_game, &scan_instance))
        .await
        .map_err(|e| ModpackError::Invalid(format!("读取实例文件的任务异常结束：{e}")))??;

    let hashes: Vec<String> = scanned.content.iter().map(|c| c.sha1.clone()).collect();
    let found = modrinth::files_by_sha1(client, &hashes).await?;
    for item in &mut scanned.content {
        let Some(file) = found.get(&item.sha1) else {
            continue;
        };
        let sha512 = file.hashes.get("sha512").map(|h| h.to_ascii_lowercase());
        // 大小对不上就当没找到（同一个 SHA-1 不该出现这种情况，保险起见）
        match sha512 {
            Some(sha512) if file.size == item.file.size => {
                item.linked = Some(Linked {
                    url: file.url.clone(),
                    sha512,
                });
            }
            _ => tracing::warn!(file = %item.file.rel, "Modrinth 上的文件信息不全，直接打包"),
        }
    }
    let linked = scanned
        .content
        .iter()
        .filter(|c| c.linked.is_some())
        .count();
    tracing::info!(
        instance = %instance.id,
        content = scanned.content.len(),
        linked,
        entries = scanned.entries.len(),
        worlds = scanned.worlds.len(),
        "导出计划"
    );
    Ok(ExportPlan {
        name: instance.config.name.clone(),
        game_version: instance.config.game_version.clone(),
        loader: instance.config.loader.clone(),
        content: scanned.content,
        content_folders: scanned.content_folders,
        entries: scanned.entries,
        worlds: scanned.worlds,
    })
}

fn display_name(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.strip_suffix(DISABLED_SUFFIX)
        .unwrap_or(name)
        .to_owned()
}

impl ExportPlan {
    pub fn preview(&self) -> ExportPreview {
        let bundled_files: Vec<&ContentFile> =
            self.content.iter().filter(|c| c.linked.is_none()).collect();
        let mut bundled: Vec<String> = bundled_files
            .iter()
            .map(|c| display_name(&c.file.rel))
            .chain(self.content_folders.iter().map(|e| display_name(&e.name)))
            .collect();
        bundled.sort_by_key(|name| name.to_lowercase());
        let bundled_bytes = bundled_files.iter().map(|c| c.file.size).sum::<u64>()
            + self.content_folders.iter().map(Entry::size).sum::<u64>();
        ExportPreview {
            name: self.name.clone(),
            game_version: self.game_version.clone(),
            loader: self
                .loader
                .as_ref()
                .map(|l| format!("{} {}", l.kind.label(), l.version)),
            linked: count(self.content.len() - bundled_files.len()),
            bundled,
            bundled_bytes: bundled_bytes as f64,
            disabled: count(self.content.iter().filter(|c| c.disabled).count()),
            entries: self
                .entries
                .iter()
                .map(|e| ExportEntry {
                    name: e.name.clone(),
                    folder: e.folder,
                    size: e.size() as f64,
                    default_on: !DEFAULT_OFF_TOP
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(&e.name)),
                })
                .collect(),
            worlds: self
                .worlds
                .iter()
                .map(|w| ExportWorld {
                    folder: w.folder.clone(),
                    name: w.name.clone(),
                    size: w.files.iter().map(|f| f.size).sum::<u64>() as f64,
                })
                .collect(),
        }
    }

    /// 写出 `.mrpack`（很慢，在后台线程里调用）。
    pub fn write(
        &self,
        options: &ExportOptions,
        dest: &Path,
        cancel: &CancellationToken,
        on_progress: impl Fn(ExportProgress),
    ) -> Result<ExportSummary, ModpackError> {
        let name = options.name.trim();
        let name = if name.is_empty() {
            self.name.as_str()
        } else {
            name
        };
        let version = options.version.trim();
        let version = if version.is_empty() { "1.0.0" } else { version };
        let summary = options
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let linked: Vec<&ContentFile> =
            self.content.iter().filter(|c| c.linked.is_some()).collect();
        let mut packed: Vec<&PackedFile> = self
            .content
            .iter()
            .filter(|c| c.linked.is_none())
            .map(|c| &c.file)
            .collect();
        packed.extend(self.content_folders.iter().flat_map(|e| &e.files));
        packed.extend(
            self.entries
                .iter()
                .filter(|e| options.entries.contains(&e.name))
                .flat_map(|e| &e.files),
        );
        packed.extend(
            self.worlds
                .iter()
                .filter(|w| options.worlds.contains(&w.folder))
                .flat_map(|w| &w.files),
        );
        if let Some(big) = packed.iter().find(|f| f.size > MAX_FILE_BYTES) {
            return Err(ModpackError::Invalid(format!(
                "{} 超过 4 GB，整合包里放不下",
                big.rel
            )));
        }

        let mut dependencies = BTreeMap::from([("minecraft", self.game_version.as_str())]);
        if let Some(loader) = &self.loader {
            // .mrpack 依赖里的名字（Modrinth 的格式规定）
            let key = match loader.kind {
                LoaderKind::NeoForge => "neoforge",
                LoaderKind::Fabric => "fabric-loader",
                LoaderKind::Forge => "forge",
                LoaderKind::Quilt => "quilt-loader",
            };
            dependencies.insert(key, loader.version.as_str());
        }
        let index = Index {
            format_version: 1,
            game: "minecraft",
            version_id: version,
            name,
            summary,
            files: self
                .content
                .iter()
                .filter_map(|c| {
                    let link = c.linked.as_ref()?;
                    Some(IndexFile {
                        path: &c.file.rel,
                        hashes: BTreeMap::from([
                            ("sha1", c.sha1.as_str()),
                            ("sha512", link.sha512.as_str()),
                        ]),
                        downloads: [link.url.as_str()],
                        file_size: c.file.size,
                    })
                })
                .collect(),
            dependencies,
        };
        let mut index_json = serde_json::to_vec_pretty(&index)
            .map_err(|e| ModpackError::Invalid(format!("清单写不出来：{e}")))?;
        index_json.push(b'\n');

        let total_bytes = packed.iter().map(|f| f.size).sum::<u64>() + index_json.len() as u64;
        let mut progress = ExportProgress {
            done_bytes: 0,
            total_bytes,
        };
        on_progress(progress);

        let part = part_path(dest);
        let result = write_archive(&part, &index_json, &packed, cancel, |n| {
            progress.done_bytes += n;
            on_progress(progress);
        })
        .and_then(|()| std::fs::rename(&part, dest).map_err(io_error(dest)));
        if let Err(error) = result {
            if let Err(cleanup) = std::fs::remove_file(&part)
                && cleanup.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %cleanup, file = %part.display(), "删除导出到一半的文件失败");
            }
            return Err(error);
        }
        let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        tracing::info!(
            file = %dest.display(),
            bytes,
            linked = linked.len(),
            packed = packed.len(),
            "整合包已导出"
        );
        Ok(ExportSummary {
            path: dest.to_path_buf(),
            bytes,
            linked: linked.len(),
            bundled: self.content.len() - linked.len() + self.content_folders.len(),
        })
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// 已经压过的文件（模组、压缩包、图片）只存不压，省时间
fn compressed_already(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let lower = lower.strip_suffix(DISABLED_SUFFIX).unwrap_or(&lower);
    [
        ".jar", ".zip", ".png", ".jpg", ".jpeg", ".ogg", ".mca", ".dat", ".gz",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn write_archive(
    part: &Path,
    index_json: &[u8],
    packed: &[&PackedFile],
    cancel: &CancellationToken,
    mut advance: impl FnMut(u64),
) -> Result<(), ModpackError> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let zip_error = |e: zip::result::ZipError| ModpackError::Io {
        path: part.to_path_buf(),
        source: std::io::Error::other(e),
    };
    let file = std::fs::File::create(part).map_err(io_error(part))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let store = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    zip.start_file(INDEX_FILE, deflate).map_err(zip_error)?;
    zip.write_all(index_json).map_err(io_error(part))?;
    advance(index_json.len() as u64);

    let mut buffer = vec![0u8; 256 * 1024];
    for file in packed {
        if cancel.is_cancelled() {
            return Err(ModpackError::Cancelled);
        }
        let options = if compressed_already(&file.rel) {
            store
        } else {
            deflate
        };
        zip.start_file(format!("{OVERRIDES}/{}", file.rel), options)
            .map_err(zip_error)?;
        let mut source = std::fs::File::open(&file.path).map_err(io_error(&file.path))?;
        loop {
            let n = source.read(&mut buffer).map_err(io_error(&file.path))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buffer[..n]).map_err(io_error(part))?;
            advance(n as u64);
            if cancel.is_cancelled() {
                return Err(ModpackError::Cancelled);
            }
        }
    }
    let writer = zip.finish().map_err(zip_error)?;
    let file = writer.into_inner().map_err(|e| ModpackError::Io {
        path: part.to_path_buf(),
        source: e.into_error(),
    })?;
    file.sync_all().map_err(io_error(part))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{InstanceConfig, create};

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn sample(game: &GameDir) -> Instance {
        let dir = game.instance_dir("Pack").unwrap();
        let config = InstanceConfig::new(
            "我的整合包",
            "1.21.1",
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.265".to_owned(),
            }),
        );
        let instance = create(game, "Pack", &config).unwrap();
        write(&dir.join("mods/on-modrinth.jar"), b"modrinth");
        write(&dir.join("mods/only-here.jar.disabled"), b"local");
        write(&dir.join("mods/.connector/cache.bin"), b"cache");
        write(&dir.join("mods/readme.txt"), b"x");
        write(&dir.join("resourcepacks/Folder Pack/pack.mcmeta"), b"{}");
        write(&dir.join("shaderpacks/shader.zip.txt"), b"setting=1");
        write(&dir.join("config/a.toml"), b"x=1");
        write(&dir.join("options.txt"), b"lang:zh_cn");
        write(&dir.join("journeymap/data/tile.png"), b"tile");
        write(&dir.join("logs/latest.log"), b"log");
        write(&dir.join("servers.dat"), b"servers");
        write(&dir.join("essential/account.json"), b"secret");
        write(&dir.join("saves/World/level.dat"), b"nbt");
        write(&dir.join("saves/World/session.lock"), b"lock");
        write(&dir.join("Lumi/content.json"), b"{}");
        instance
    }

    /// 模拟查过 Modrinth：on-modrinth.jar 找到了
    fn planned(game: &GameDir, instance: &Instance) -> ExportPlan {
        let mut scanned = scan(game, instance).unwrap();
        for item in &mut scanned.content {
            if item.file.rel == "mods/on-modrinth.jar" {
                item.linked = Some(Linked {
                    url: "https://cdn.modrinth.com/data/AAAA/versions/BBBB/on-modrinth.jar"
                        .to_owned(),
                    sha512: "ab".repeat(64),
                });
            }
        }
        ExportPlan {
            name: instance.config.name.clone(),
            game_version: instance.config.game_version.clone(),
            loader: instance.config.loader.clone(),
            content: scanned.content,
            content_folders: scanned.content_folders,
            entries: scanned.entries,
            worlds: scanned.worlds,
        }
    }

    #[test]
    fn preview_lists_what_will_be_exported() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let preview = planned(&game, &instance).preview();
        assert_eq!(preview.name, "我的整合包");
        assert_eq!(preview.loader.as_deref(), Some("NeoForge 21.1.265"));
        assert_eq!(preview.linked, 1);
        assert_eq!(preview.bundled, vec!["Folder Pack", "only-here.jar"]);
        assert_eq!(preview.disabled, 1);
        let names: Vec<&str> = preview.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "config",
                "journeymap",
                "options.txt",
                "shaderpacks/shader.zip.txt"
            ],
            "日志、servers.dat、账户数据、本启动器的文件、版本描述都不列"
        );
        let journeymap = preview
            .entries
            .iter()
            .find(|e| e.name == "journeymap")
            .unwrap();
        assert!(!journeymap.default_on, "地图缓存默认不勾");
        assert_eq!(preview.worlds.len(), 1);
        assert_eq!(preview.worlds[0].folder, "World");
    }

    fn read_back(path: &Path) -> (serde_json::Value, Vec<String>) {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let names: Vec<String> = archive
            .file_names()
            .map(|n| n.unwrap().into_owned())
            .collect();
        let mut index = String::new();
        archive
            .by_name(INDEX_FILE)
            .unwrap()
            .read_to_string(&mut index)
            .unwrap();
        (serde_json::from_str(&index).unwrap(), names)
    }

    #[test]
    fn writes_a_pack_that_our_importer_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let plan = planned(&game, &instance);
        let dest = dir.path().join("out.mrpack");
        let options = ExportOptions {
            name: " 导出的包 ".to_owned(),
            version: String::new(),
            summary: None,
            entries: vec!["config".to_owned(), "options.txt".to_owned()],
            worlds: vec!["World".to_owned()],
        };
        let summary = plan
            .write(&options, &dest, &CancellationToken::new(), |_| {})
            .unwrap();
        assert_eq!(summary.linked, 1);
        assert_eq!(summary.bundled, 2);
        assert!(!part_path(&dest).exists());

        let (index, names) = read_back(&dest);
        assert_eq!(index["name"], "导出的包");
        assert_eq!(index["versionId"], "1.0.0");
        assert_eq!(index["dependencies"]["minecraft"], "1.21.1");
        assert_eq!(index["dependencies"]["neoforge"], "21.1.265");
        assert_eq!(index["files"][0]["path"], "mods/on-modrinth.jar");
        assert_eq!(index["files"][0]["fileSize"], 8);
        assert!(index["files"][0]["hashes"]["sha512"].is_string());
        for expected in [
            "overrides/mods/only-here.jar.disabled",
            "overrides/resourcepacks/Folder Pack/pack.mcmeta",
            "overrides/config/a.toml",
            "overrides/options.txt",
            "overrides/saves/World/level.dat",
        ] {
            assert!(names.iter().any(|n| n == expected), "缺 {expected}");
        }
        for unexpected in [
            "overrides/mods/on-modrinth.jar",
            "overrides/saves/World/session.lock",
            "overrides/journeymap/data/tile.png",
            "overrides/shaderpacks/shader.zip.txt",
            "overrides/essential/account.json",
        ] {
            assert!(
                !names.iter().any(|n| n == unexpected),
                "不该有 {unexpected}"
            );
        }

        // 我们自己的导入能认（下载地址在允许的域名里、没有路径冲突）
        let imported = crate::modpack::mrpack::inspect(&dest).unwrap();
        assert_eq!(imported.preview.file_count, 1);
        assert_eq!(imported.preview.game_version, "1.21.1");
    }

    #[test]
    fn cancelled_export_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        let instance = sample(&game);
        let plan = planned(&game, &instance);
        let dest = dir.path().join("out.mrpack");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let options = ExportOptions {
            name: String::new(),
            version: "2.0".to_owned(),
            summary: None,
            entries: Vec::new(),
            worlds: Vec::new(),
        };
        assert!(matches!(
            plan.write(&options, &dest, &cancel, |_| {}),
            Err(ModpackError::Cancelled)
        ));
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[test]
    fn recognizes_content_files() {
        assert_eq!(content_name("mods", "a.jar"), Some(false));
        assert_eq!(content_name("mods", "a.JAR.disabled"), Some(true));
        assert_eq!(content_name("mods", "a.txt"), None);
        assert_eq!(content_name("shaderpacks", "s.zip"), Some(false));
        assert_eq!(content_name("shaderpacks", "s.zip.txt"), None);
        assert_eq!(content_name("resourcepacks", "r.zip.disabled"), None);
    }
}
