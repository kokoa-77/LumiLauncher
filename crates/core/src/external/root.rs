//! 挂上别家的 `.minecraft`（D19，「原地读」的思路学 Axolotl，代码自己写）。
//!
//! 扫 `versions/` 里每个有版本描述的文件夹，认出游戏版本、加载器和游戏工作目录（版本隔离）：
//! - **PCL**：版本自己的设置（`versions/<版本>/PCL/Setup.ini` 的 `VersionArgumentIndieV2`，
//!   或老的 `VersionArgumentIndie`：1 开、2 关、0 跟全局）→ 版本文件夹里已经有模组或存档就算开了 →
//!   PCL 的全局设置（PCL 所在文件夹的 `PCL/Setup.ini` 的 `LaunchArgumentIndieV2`：0 都不隔离、
//!   1 隔离能装模组的、2 隔离非正式版、3 两种都隔离、4 全部隔离，没写按 4）
//! - **HMCL**：版本自己的设置（`versions/<版本>/hmclversion.cfg` 的 `gameDirType`：0 `.minecraft`、
//!   1 版本文件夹、2 自定义的 `gameDir`）；跟着全局或没写时看版本文件夹里有没有游戏内容
//! - **其他**：版本文件夹里有游戏内容就算开了隔离
//!
//! 只读这几个设置项，别家设置文件里的其他东西（账号、令牌）不碰。
//! 同步成实例（[`sync`]）后，实例的设置在本启动器这边，见 [`crate::instance::Link`]。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{Launcher, read_flat_config};
use crate::instance::{self, InstanceConfig, Link, LoaderKind, LoaderRef, path_key};
use crate::meta::MetaError;
use crate::meta::version::{Argument, VersionJson, resolve};
use crate::paths::GameDir;

/// 别家 `.minecraft` 里能启动的一个版本
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundVersion {
    /// `versions/` 里的文件夹名
    pub folder: String,
    pub game_version: String,
    pub loader: Option<LoaderRef>,
    /// 游戏工作目录（版本隔离开了是版本文件夹，没开是 `.minecraft`）
    pub game_dir: PathBuf,
}

/// 扫一个 `.minecraft` 的结果
#[derive(Debug, Clone)]
pub struct ScannedRoot {
    pub root: PathBuf,
    pub launcher: Launcher,
    pub versions: Vec<FoundVersion>,
    /// 有版本描述但读不了的（文件夹名、原因）：同步时不当成「没了」
    pub skipped: Vec<(String, String)>,
}

/// 是不是一个游戏目录（有 `versions/`）
pub fn is_game_root(path: &Path) -> bool {
    path.join("versions").is_dir()
}

/// 用户选的文件夹 → 它的 `.minecraft`：选的就是；里面有一个 `.minecraft`（选了 PCL 所在的文件夹）；
/// 或者选到了 `.minecraft` 里面一两层（系统选文件夹的窗口里点中了 `versions`、某个版本再按「选择文件夹」，选到的是点中的那个）
pub fn game_root_near(picked: &Path) -> Option<PathBuf> {
    game_root_in(picked).or_else(|| {
        picked
            .ancestors()
            .skip(1)
            .take(2)
            .find(|dir| is_game_root(dir))
            .map(Path::to_path_buf)
    })
}

/// 和 [`game_root_near`] 一样，但不往上找（拖进窗口的文件夹：拖了 `mods` 进来不是要挂整个 `.minecraft`）
pub fn game_root_in(folder: &Path) -> Option<PathBuf> {
    if is_game_root(folder) {
        return Some(folder.to_path_buf());
    }
    let inner = folder.join(".minecraft");
    is_game_root(&inner).then_some(inner)
}

/// 这个 `.minecraft` 是哪家启动器的
pub fn detect_launcher(root: &Path) -> Launcher {
    let parent = root.parent();
    let version_dirs = version_folders(root);
    let any_version_has = |name: &str| version_dirs.iter().any(|(_, dir)| dir.join(name).exists());
    if parent.is_some_and(|p| p.join("PCL").is_dir()) || any_version_has("PCL") {
        return Launcher::Pcl;
    }
    if parent.is_some_and(|p| p.join(".hmcl").is_dir() || p.join("hmcl.json").is_file())
        || any_version_has("hmclversion.cfg")
    {
        return Launcher::Hmcl;
    }
    if official_root().is_some_and(|official| path_key(&official) == path_key(root)) {
        return Launcher::Official;
    }
    Launcher::Other
}

/// 官方启动器的 `.minecraft`：`%APPDATA%\.minecraft`
pub fn official_root() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join(".minecraft"))
}

/// `versions/` 里的文件夹（名字, 路径），按名字排
fn version_folders(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root.join("versions")) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// 有几个版本（有版本描述的文件夹）
pub fn count_versions(root: &Path) -> usize {
    version_folders(root)
        .iter()
        .filter(|(name, dir)| dir.join(format!("{name}.json")).is_file())
        .count()
}

/// 扫一个 `.minecraft`：列出能启动的版本。`versions/` 读不了时报错
pub fn scan(root: &Path) -> std::io::Result<ScannedRoot> {
    std::fs::read_dir(root.join("versions"))?;
    let launcher = detect_launcher(root);
    let files = GameDir::new(root);
    let mut versions = Vec::new();
    let mut skipped = Vec::new();
    for (folder, dir) in version_folders(root) {
        let json = dir.join(format!("{folder}.json"));
        if !json.is_file() {
            // 空文件夹、下载到一半的、别的启动器放的杂物
            continue;
        }
        match read_version(&files, &folder) {
            Ok((raw, version)) => {
                let args = plain_game_args(&version);
                let loader = detect_loader(&version, &args);
                let game_version = detect_game_version(&raw, &version, &args, &folder);
                let game_dir = game_dir_for(root, &folder, launcher, &version, loader.as_ref());
                versions.push(FoundVersion {
                    folder,
                    game_version,
                    loader,
                    game_dir,
                });
            }
            Err(reason) => {
                tracing::warn!(root = %root.display(), version = %folder, %reason, "别家的版本读不了，跳过");
                skipped.push((folder, reason));
            }
        }
    }
    Ok(ScannedRoot {
        root: root.to_path_buf(),
        launcher,
        versions,
        skipped,
    })
}

/// 读一个版本：原样的 json（认 PCL、HMCL 自己加的字段）和沿着 `inheritsFrom` 合并后的版本描述。
/// 父版本本地没有时（启动时会从官方补）只用它自己那份
fn read_version(files: &GameDir, folder: &str) -> Result<(serde_json::Value, VersionJson), String> {
    let path = files.version_json(folder).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| format!("读不了版本描述：{e}"))?;
    let raw: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("版本描述格式不对：{e}"))?;
    let own = VersionJson::parse(folder, &bytes).map_err(|e| e.to_string())?;
    let merged = match resolve(folder, |id| {
        if id == folder {
            return Ok(own.clone());
        }
        let path = files
            .version_json(id)
            .map_err(|_| MetaError::InvalidId(id.to_owned()))?;
        match std::fs::read(&path) {
            Ok(bytes) => VersionJson::parse(id, &bytes),
            Err(_) => Err(MetaError::NotFound(id.to_owned())),
        }
    }) {
        Ok(merged) => merged,
        Err(MetaError::NotFound(_)) => own,
        Err(error) => return Err(error.to_string()),
    };
    Ok((raw, merged))
}

/// 游戏参数里的普通字符串（新格式）加上旧格式的整串参数
fn plain_game_args(version: &VersionJson) -> Vec<String> {
    let mut args: Vec<String> = version
        .arguments
        .iter()
        .flat_map(|a| a.game.iter())
        .filter_map(|arg| match arg {
            Argument::Plain(text) => Some(text.clone()),
            Argument::Conditional { .. } => None,
        })
        .collect();
    if let Some(legacy) = &version.minecraft_arguments {
        args.extend(legacy.split_whitespace().map(str::to_owned));
    }
    args
}

/// 参数 `name` 后面跟的值
fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

/// 依赖库里 `group:artifact` 的版本号
fn library_version(version: &VersionJson, group: &str, artifact: &str) -> Option<String> {
    version
        .libraries
        .iter()
        .filter_map(|lib| lib.coord())
        .find(|c| c.group == group && c.artifact == artifact)
        .map(|c| c.version)
}

/// Forge 的库版本号带着游戏版本：`1.12.2-14.23.5.2860`、`1.7.10-10.13.4.1614-1.7.10` → 只留 Forge 自己的
fn strip_game_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('-').collect();
    match parts.as_slice() {
        [game, forge, rest @ ..] if game.contains('.') && !forge.is_empty() => {
            let mut kept = vec![*forge];
            kept.extend(rest.iter().filter(|p| *p != game));
            kept.join("-")
        }
        _ => version.to_owned(),
    }
}

/// 认加载器：先看启动参数（NeoForge、新版 Forge 都写着），再看依赖库
fn detect_loader(version: &VersionJson, args: &[String]) -> Option<LoaderRef> {
    let make = |kind, version: String| Some(LoaderRef { kind, version });
    if let Some(v) = arg_value(args, "--fml.neoForgeVersion")
        .or_else(|| library_version(version, "net.neoforged", "neoforge"))
    {
        return make(LoaderKind::NeoForge, v);
    }
    // 1.20.1 的 NeoForge 还叫 forge：net.neoforged:forge:1.20.1-47.1.106
    if let Some(v) = library_version(version, "net.neoforged", "forge") {
        return make(LoaderKind::NeoForge, strip_game_version(&v));
    }
    if let Some(v) = library_version(version, "org.quiltmc", "quilt-loader") {
        return make(LoaderKind::Quilt, v);
    }
    if let Some(v) = library_version(version, "net.fabricmc", "fabric-loader") {
        return make(LoaderKind::Fabric, v);
    }
    if let Some(v) = arg_value(args, "--fml.forgeVersion") {
        return make(LoaderKind::Forge, v);
    }
    if let Some(v) = library_version(version, "net.minecraftforge", "forge")
        .or_else(|| library_version(version, "net.minecraftforge", "fmlloader"))
    {
        return make(LoaderKind::Forge, strip_game_version(&v));
    }
    None
}

/// 认游戏版本：PCL、HMCL 写的 `clientVersion` → 加载器参数里的 → HMCL 的 `patches` →
/// 继承的原版 → 依赖库里带的 → 文件夹名
fn detect_game_version(
    raw: &serde_json::Value,
    version: &VersionJson,
    args: &[String],
    folder: &str,
) -> String {
    let text = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(v) = text(raw.get("clientVersion")) {
        return v;
    }
    if let Some(v) = arg_value(args, "--fml.mcVersion") {
        return v;
    }
    if let Some(patches) = raw.get("patches").and_then(serde_json::Value::as_array)
        && let Some(game) = patches
            .iter()
            .find(|p| p.get("id").and_then(serde_json::Value::as_str) == Some("game"))
        && let Some(v) = text(game.get("version"))
    {
        return v;
    }
    if let Some(parent) = text(raw.get("inheritsFrom")) {
        // 合并成功时 jar 是最底下的原版；父版本本地没有时就是它直接继承的那个
        return version
            .jar
            .clone()
            .filter(|jar| jar != folder)
            .unwrap_or(parent);
    }
    for (group, artifact) in [
        ("net.fabricmc", "intermediary"),
        ("org.quiltmc", "hashed"),
        ("net.minecraftforge", "forge"),
        ("net.neoforged", "forge"),
    ] {
        if let Some(v) = library_version(version, group, artifact) {
            return v.split('-').next().unwrap_or(&v).to_owned();
        }
    }
    folder.to_owned()
}

/// 版本文件夹里有没有游戏内容（模组、存档……）：有就是开了版本隔离在玩
fn has_content(version_dir: &Path) -> bool {
    let has_file =
        |dir: &Path| std::fs::read_dir(dir).is_ok_and(|mut entries| entries.any(|e| e.is_ok()));
    has_file(&version_dir.join("mods")) || has_file(&version_dir.join("saves"))
}

/// 游戏工作目录（见文件开头的规则）
fn game_dir_for(
    root: &Path,
    folder: &str,
    launcher: Launcher,
    version: &VersionJson,
    loader: Option<&LoaderRef>,
) -> PathBuf {
    let version_dir = root.join("versions").join(folder);
    let isolated = match launcher {
        Launcher::Pcl => pcl_isolated(root, &version_dir, version, loader),
        Launcher::Hmcl => match hmcl_game_dir(root, &version_dir) {
            Some(dir) => return dir,
            None => has_content(&version_dir),
        },
        _ => has_content(&version_dir),
    };
    if isolated {
        version_dir
    } else {
        root.to_path_buf()
    }
}

const PCL_VERSION_KEYS: &[&str] = &["VersionArgumentIndieV2", "VersionArgumentIndie"];
const PCL_GLOBAL_KEYS: &[&str] = &["LaunchArgumentIndieV2"];

/// PCL（含社区版）的设置：`<文件夹>/PCL/Setup.ini`，社区版是 `config.v1.yml`
fn pcl_values(dir: &Path, keys: &[&str]) -> std::collections::HashMap<String, String> {
    let mut values = read_flat_config(&dir.join("PCL").join("Setup.ini"), keys);
    for (key, value) in read_flat_config(&dir.join("PCL").join("config.v1.yml"), keys) {
        values.entry(key).or_insert(value);
    }
    values
}

fn pcl_isolated(
    root: &Path,
    version_dir: &Path,
    version: &VersionJson,
    loader: Option<&LoaderRef>,
) -> bool {
    let own = pcl_values(version_dir, PCL_VERSION_KEYS);
    if let Some(value) = own.get("VersionArgumentIndieV2") {
        return value.eq_ignore_ascii_case("true");
    }
    match own
        .get("VersionArgumentIndie")
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(1) => return true,
        Some(2) => return false,
        _ => {}
    }
    if has_content(version_dir) {
        return true;
    }
    let policy = root
        .parent()
        .map(|launcher_dir| pcl_values(launcher_dir, PCL_GLOBAL_KEYS))
        .and_then(|g| g.get("LaunchArgumentIndieV2")?.parse::<i64>().ok())
        .unwrap_or(4);
    let modded = loader.is_some()
        || version.main_class.as_deref().is_some_and(|main| {
            main != "net.minecraft.client.main.Main" && main != "net.minecraft.client.Minecraft"
        });
    let release = version.kind.as_deref().is_none_or(|kind| kind == "release");
    match policy {
        0 => false,
        1 => modded,
        2 => !release,
        3 => modded || !release,
        _ => true,
    }
}

/// HMCL 版本自己设了游戏目录时返回它；跟着全局或没写时 `None`
fn hmcl_game_dir(root: &Path, version_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(version_dir.join("hmclversion.cfg")).ok()?;
    let config: serde_json::Value = serde_json::from_str(&text).ok()?;
    if config
        .get("usesGlobal")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return None;
    }
    let kind = config.get("gameDirType")?;
    let kind = kind.as_i64().or_else(|| match kind.as_str()? {
        "ROOT_FOLDER" => Some(0),
        "VERSION_FOLDER" => Some(1),
        "CUSTOM" => Some(2),
        _ => None,
    })?;
    match kind {
        0 => Some(root.to_path_buf()),
        1 => Some(version_dir.to_path_buf()),
        2 => {
            let custom = config.get("gameDir")?.as_str()?.trim();
            if custom.is_empty() {
                return None;
            }
            let path = PathBuf::from(custom);
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            path.is_dir().then_some(path)
        }
        _ => None,
    }
}

/// 同步的结果
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// 把扫到的版本同步成挂上的实例：新的版本加上（放进分组 `group`）；已经挂上的更新游戏版本、加载器、
/// 游戏工作目录（名字、分组、单独设置留着）；版本没了的移除（只删本启动器这边的设置）。
/// 读不了的版本不算「没了」。`hidden` 是用户自己从 Lumi 移除过的版本（`versions/` 里的文件夹名）：不再加回来。
pub fn sync(game: &GameDir, scanned: &ScannedRoot, group: &str, hidden: &[String]) -> SyncReport {
    let mut report = SyncReport::default();
    let existing = instance::linked_to(game, &scanned.root);
    for found in &scanned.versions {
        let link = Link {
            root: scanned.root.clone(),
            version: found.folder.clone(),
            game_dir: found.game_dir.clone(),
            launcher: scanned.launcher,
        };
        let current = existing.iter().find(|i| {
            i.config
                .link
                .as_ref()
                .is_some_and(|l| l.version == found.folder)
        });
        match current {
            Some(current) => {
                let c = &current.config;
                let unchanged = c.game_version == found.game_version
                    && c.loader == found.loader
                    && c.version_id == found.folder
                    && c.link.as_ref() == Some(&link);
                if unchanged {
                    continue;
                }
                let result = instance::update(game, &current.id, |c| {
                    c.game_version.clone_from(&found.game_version);
                    c.loader.clone_from(&found.loader);
                    c.version_id.clone_from(&found.folder);
                    c.link = Some(link);
                });
                match result {
                    Ok(_) => report.updated += 1,
                    Err(error) => {
                        tracing::warn!(instance = %current.id, %error, "更新挂上的实例失败");
                    }
                }
            }
            None if hidden.contains(&found.folder) => {}
            None => {
                let id = instance::unique_id(game, &found.folder);
                let mut config =
                    InstanceConfig::new(&found.folder, &found.game_version, found.loader.clone());
                config.version_id.clone_from(&found.folder);
                config.group = instance::clean_group(Some(group));
                config.link = Some(link);
                match instance::create(game, &id, &config) {
                    Ok(_) => report.added += 1,
                    Err(error) => {
                        tracing::warn!(version = %found.folder, %error, "挂上版本失败");
                    }
                }
            }
        }
    }
    let present: HashSet<&str> = scanned
        .versions
        .iter()
        .map(|v| v.folder.as_str())
        .chain(scanned.skipped.iter().map(|(folder, _)| folder.as_str()))
        .collect();
    for gone in existing.iter().filter(|i| {
        i.config
            .link
            .as_ref()
            .is_some_and(|l| !present.contains(l.version.as_str()))
    }) {
        match instance::unlink(game, &gone.id) {
            Ok(true) => report.removed += 1,
            Ok(false) => {}
            Err(error) => tracing::warn!(instance = %gone.id, %error, "移除没了的版本失败"),
        }
    }
    if report != SyncReport::default() {
        tracing::info!(root = %scanned.root.display(), ?report, "同步了挂上的目录");
    }
    report
}

/// 给挂上的目录起个分组名：官方的叫「官方启动器」，其他用 `.minecraft` 外面那个文件夹的名字（`PCL2`）
pub fn group_name(root: &Path, launcher: Launcher) -> String {
    if launcher == Launcher::Official {
        return launcher.label().to_owned();
    }
    let named = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
    };
    let own = named(root);
    let parent = root.parent().and_then(named);
    match own {
        Some(name) if !name.eq_ignore_ascii_case(".minecraft") => name,
        _ => parent.unwrap_or_else(|| launcher.label().to_owned()),
    }
}

/// 找的时候跳过的文件夹（系统的、开发工具的，里面不会有游戏）
const SKIP_DIRS: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "system volume information",
    "recovery",
    "perflogs",
    "msocache",
    "intel",
    "amd",
    "nvidia",
    "node_modules",
];

/// 在电脑上找别家的 `.minecraft`（只看常见的位置，不深挖，几秒内找完）：
/// 官方的 `%APPDATA%\.minecraft`；每个盘的根目录往下两层（C 盘只看一层）；桌面、下载、文档往下两层。
/// 只留 `versions/` 里至少有一个版本的；`exclude` 里的（自己的游戏目录、已经挂上的）、
/// 本启动器自己的游戏目录（有 `.mcl-store`）不要
pub fn discover(exclude: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen: HashSet<String> = exclude.iter().map(|p| path_key(p)).collect();
    let mut found = Vec::new();
    let mut consider = |path: PathBuf| {
        if is_game_root(&path)
            && !path.join(crate::content::store::STORE_DIR).exists()
            && seen.insert(path_key(&path))
            && count_versions(&path) > 0
        {
            found.push(path);
        }
    };
    if let Some(official) = official_root() {
        consider(official);
    }
    let mut bases: Vec<(PathBuf, usize)> = Vec::new();
    for letter in b'C'..=b'Z' {
        let drive = PathBuf::from(format!("{}:\\", letter as char));
        if drive.is_dir() {
            bases.push((drive, if letter == b'C' { 1 } else { 2 }));
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        for name in ["Desktop", "Downloads", "Documents"] {
            bases.push((home.join(name), 2));
        }
    }
    for (base, depth) in bases {
        let mut level = vec![base];
        for _ in 0..=depth {
            let mut next = Vec::new();
            for dir in &level {
                consider(dir.join(".minecraft"));
                next.extend(child_dirs(dir));
            }
            level = next;
        }
    }
    found
}

/// 一个文件夹下面的子文件夹（跳过系统的、隐藏的、`$` 开头的）
fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            !name.starts_with('.') && !name.starts_with('$') && !SKIP_DIRS.contains(&name.as_str())
        })
        .map(|e| e.path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// 一个 PCL 的目录：`<temp>/PCL2/.minecraft` + `<temp>/PCL2/PCL/Setup.ini`
    fn pcl_root(temp: &Path, global_policy: Option<i64>) -> PathBuf {
        let launcher = temp.join("PCL2");
        std::fs::create_dir_all(launcher.join("PCL")).unwrap();
        if let Some(policy) = global_policy {
            write(
                &launcher.join("PCL").join("Setup.ini"),
                &format!("LaunchArgumentIndieV2:{policy}\nLoginMsJson:secret\n"),
            );
        }
        let root = launcher.join(".minecraft");
        std::fs::create_dir_all(root.join("versions")).unwrap();
        root
    }

    fn add_version(root: &Path, folder: &str, json: &str, pcl_setup: Option<&str>) -> PathBuf {
        let dir = root.join("versions").join(folder);
        write(&dir.join(format!("{folder}.json")), json);
        if let Some(setup) = pcl_setup {
            write(&dir.join("PCL").join("Setup.ini"), setup);
        }
        dir
    }

    const NEOFORGE_JSON: &str = r#"{"id":"1.21.1-NeoForge_21.1.217","clientVersion":"1.21.1","type":"release",
        "mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher",
        "arguments":{"game":["--fml.neoForgeVersion","21.1.248","--fml.mcVersion","1.21.1"]},
        "libraries":[{"name":"net.neoforged.fancymodloader:loader:4.0.43"}]}"#;
    const FABRIC_JSON: &str = r#"{"id":"1.20.1-Fabric 0.15.3","clientVersion":"1.20.1","type":"release",
        "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
        "libraries":[{"name":"net.fabricmc:fabric-loader:0.15.3","url":"https://maven.fabricmc.net/"},
                     {"name":"net.fabricmc:intermediary:1.20.1"}]}"#;
    const VANILLA_JSON: &str = r#"{"id":"1.21.1","type":"release","mainClass":"net.minecraft.client.main.Main","libraries":[]}"#;

    #[test]
    fn reads_loader_and_game_version_like_pcl_wrote_them() {
        let temp = tempfile::tempdir().unwrap();
        let root = pcl_root(temp.path(), Some(1));
        add_version(&root, "1.21.1-NeoForge_21.1.217", NEOFORGE_JSON, None);
        add_version(&root, "1.20.1-Fabric 0.15.3", FABRIC_JSON, None);
        add_version(&root, "1.21.1", VANILLA_JSON, None);
        // 空文件夹、没有版本描述的不算
        std::fs::create_dir_all(root.join("versions").join("BHL")).unwrap();
        // 读不了的单独列出来
        add_version(&root, "坏了", "{不是 json", None);

        let scanned = scan(&root).unwrap();
        assert_eq!(scanned.launcher, Launcher::Pcl);
        assert_eq!(scanned.skipped.len(), 1);
        let by = |folder: &str| {
            scanned
                .versions
                .iter()
                .find(|v| v.folder == folder)
                .unwrap()
                .clone()
        };
        let neo = by("1.21.1-NeoForge_21.1.217");
        assert_eq!(neo.game_version, "1.21.1");
        // 文件夹名是 217，PCL 原地升级过，以参数为准
        assert_eq!(
            neo.loader,
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.248".into()
            })
        );
        let fabric = by("1.20.1-Fabric 0.15.3");
        assert_eq!(fabric.loader.as_ref().unwrap().kind, LoaderKind::Fabric);
        assert_eq!(fabric.loader.as_ref().unwrap().version, "0.15.3");
        assert_eq!(fabric.game_version, "1.20.1");
        let vanilla = by("1.21.1");
        assert_eq!(vanilla.loader, None);
        assert_eq!(vanilla.game_version, "1.21.1");

        // 全局设置 1：只隔离能装模组的
        assert_eq!(neo.game_dir, root.join("versions").join(&neo.folder));
        assert_eq!(fabric.game_dir, root.join("versions").join(&fabric.folder));
        assert_eq!(vanilla.game_dir, root);
    }

    #[test]
    fn pcl_isolation_follows_the_version_then_content_then_global() {
        let temp = tempfile::tempdir().unwrap();
        let root = pcl_root(temp.path(), Some(0));
        let on = add_version(
            &root,
            "on",
            VANILLA_JSON,
            Some("VersionArgumentIndieV2:True"),
        );
        add_version(
            &root,
            "off",
            VANILLA_JSON,
            Some("VersionArgumentIndieV2:False"),
        );
        let old_on = add_version(
            &root,
            "old-on",
            VANILLA_JSON,
            Some("VersionArgumentIndie:1"),
        );
        add_version(&root, "follow", FABRIC_JSON, Some("VersionArgumentIndie:0"));
        let played = add_version(
            &root,
            "played",
            VANILLA_JSON,
            Some("VersionArgumentIndie:0"),
        );
        std::fs::create_dir_all(played.join("saves").join("新的世界")).unwrap();

        let scanned = scan(&root).unwrap();
        let dir = |folder: &str| {
            scanned
                .versions
                .iter()
                .find(|v| v.folder == folder)
                .unwrap()
                .game_dir
                .clone()
        };
        assert_eq!(dir("on"), on);
        assert_eq!(dir("off"), root);
        assert_eq!(dir("old-on"), old_on);
        assert_eq!(dir("follow"), root, "全局设置 0：都不隔离");
        assert_eq!(dir("played"), played, "版本文件夹里有存档就是在隔离着玩");
    }

    #[test]
    fn pcl_without_global_settings_isolates_everything() {
        let temp = tempfile::tempdir().unwrap();
        let root = pcl_root(temp.path(), None);
        let dir = add_version(
            &root,
            "1.21.1",
            VANILLA_JSON,
            Some("VersionArgumentIndie:0"),
        );
        assert_eq!(scan(&root).unwrap().versions[0].game_dir, dir);
    }

    #[test]
    fn hmcl_reads_its_own_game_dir_setting() {
        let temp = tempfile::tempdir().unwrap();
        let launcher = temp.path().join("HMCL");
        std::fs::create_dir_all(launcher.join(".hmcl")).unwrap();
        let root = launcher.join(".minecraft");
        let isolated = add_version(&root, "a", VANILLA_JSON, None);
        write(
            &isolated.join("hmclversion.cfg"),
            r#"{"usesGlobal":false,"gameDirType":1}"#,
        );
        let shared = add_version(&root, "b", VANILLA_JSON, None);
        write(
            &shared.join("hmclversion.cfg"),
            r#"{"usesGlobal":false,"gameDirType":0}"#,
        );
        let custom_dir = temp.path().join("自定义");
        std::fs::create_dir_all(&custom_dir).unwrap();
        let custom = add_version(&root, "c", VANILLA_JSON, None);
        write(
            &custom.join("hmclversion.cfg"),
            &serde_json::json!({"usesGlobal": false, "gameDirType": 2, "gameDir": custom_dir})
                .to_string(),
        );
        // HMCL 装的 Fabric：继承原版
        add_version(
            &root,
            "fabric-loader-0.16.10-1.21.1",
            r#"{"id":"fabric-loader-0.16.10-1.21.1","inheritsFrom":"1.21.1",
                "libraries":[{"name":"net.fabricmc:fabric-loader:0.16.10"}]}"#,
            None,
        );

        let scanned = scan(&root).unwrap();
        assert_eq!(scanned.launcher, Launcher::Hmcl);
        let by = |folder: &str| {
            scanned
                .versions
                .iter()
                .find(|v| v.folder == folder)
                .unwrap()
                .clone()
        };
        assert_eq!(by("a").game_dir, isolated);
        assert_eq!(by("b").game_dir, root);
        assert_eq!(by("c").game_dir, custom_dir);
        let fabric = by("fabric-loader-0.16.10-1.21.1");
        assert_eq!(fabric.game_version, "1.21.1", "父版本本地没有也认得出");
        assert_eq!(fabric.loader.unwrap().kind, LoaderKind::Fabric);
        assert_eq!(fabric.game_dir, root, "没写设置、没有内容：不隔离");
    }

    #[test]
    fn old_forge_versions_are_recognised() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".minecraft");
        add_version(
            &root,
            "1.7.10-Forge",
            r#"{"id":"1.7.10-Forge","minecraftArguments":"--username ${auth_player_name} --tweakClass cpw.mods.fml.common.launcher.FMLTweaker",
                "libraries":[{"name":"net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10"}]}"#,
            None,
        );
        let found = &scan(&root).unwrap().versions[0];
        assert_eq!(found.game_version, "1.7.10");
        assert_eq!(
            found.loader,
            Some(LoaderRef {
                kind: LoaderKind::Forge,
                version: "10.13.4.1614".into()
            })
        );
        assert_eq!(strip_game_version("1.12.2-14.23.5.2860"), "14.23.5.2860");
        assert_eq!(strip_game_version("47.2.0"), "47.2.0");
    }

    #[test]
    fn sync_adds_updates_and_removes_linked_instances() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path().join("ours"));
        let root = pcl_root(temp.path(), Some(4));
        let neo = add_version(&root, "1.21.1-NeoForge", NEOFORGE_JSON, None);
        add_version(&root, "1.20.1-Fabric", FABRIC_JSON, None);

        let scanned = scan(&root).unwrap();
        let report = sync(&game, &scanned, &group_name(&root, scanned.launcher), &[]);
        assert_eq!(report.added, 2);
        let linked = instance::linked_to(&game, &root);
        assert_eq!(linked.len(), 2);
        let first = linked
            .iter()
            .find(|i| i.config.name == "1.21.1-NeoForge")
            .unwrap();
        assert_eq!(first.config.group.as_deref(), Some("PCL2"));
        assert_eq!(first.game_dir(&game).unwrap(), neo);
        assert_eq!(first.files(&game).root(), root.as_path());

        // 改了名字、分组，再扫一次不变
        instance::update(&game, &first.id, |c| {
            c.name = "我的整合包".into();
            c.group = Some("常玩".into());
        })
        .unwrap();
        assert_eq!(
            sync(&game, &scan(&root).unwrap(), "PCL2", &[]),
            SyncReport::default()
        );

        // PCL 里关了隔离：跟着改，名字留着
        write(
            &neo.join("PCL").join("Setup.ini"),
            "VersionArgumentIndieV2:False",
        );
        let report = sync(&game, &scan(&root).unwrap(), "PCL2", &[]);
        assert_eq!(report.updated, 1);
        let again = instance::load(&game, &first.id).unwrap().unwrap();
        assert_eq!(again.config.name, "我的整合包");
        assert_eq!(again.config.group.as_deref(), Some("常玩"));
        assert_eq!(again.game_dir(&game).unwrap(), root);

        // PCL 里删了 Fabric 那个版本：移除挂上的实例
        std::fs::remove_dir_all(root.join("versions").join("1.20.1-Fabric")).unwrap();
        let report = sync(&game, &scan(&root).unwrap(), "PCL2", &[]);
        assert_eq!(report.removed, 1);
        assert_eq!(instance::linked_to(&game, &root).len(), 1);
        assert!(neo.exists(), "别家的文件不动");

        // 用户从 Lumi 移除了 NeoForge 那个：再扫不加回来；不在移除列表里了才加回来
        assert!(instance::unlink(&game, &first.id).unwrap());
        let hidden = vec!["1.21.1-NeoForge".to_owned()];
        assert_eq!(
            sync(&game, &scan(&root).unwrap(), "PCL2", &hidden),
            SyncReport::default()
        );
        assert!(instance::linked_to(&game, &root).is_empty());
        let report = sync(&game, &scan(&root).unwrap(), "PCL2", &[]);
        assert_eq!(report.added, 1);
    }

    #[test]
    fn picked_folder_finds_its_game_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = pcl_root(temp.path(), None);
        let version = add_version(&root, "1.21.1", VANILLA_JSON, None);
        for picked in [
            root.clone(),
            temp.path().join("PCL2"),
            root.join("versions"),
            version,
        ] {
            assert_eq!(
                game_root_near(&picked).as_deref(),
                Some(root.as_path()),
                "{}",
                picked.display()
            );
        }
        assert_eq!(game_root_near(&temp.path().join("PCL2").join("PCL")), None);
        // 拖进来的不往上找
        assert_eq!(game_root_in(&root.join("versions")), None);
        assert_eq!(
            game_root_in(&temp.path().join("PCL2")).as_deref(),
            Some(root.as_path())
        );
    }

    #[test]
    fn group_names_come_from_the_launcher_folder() {
        assert_eq!(
            group_name(Path::new(r"D:\PCL2\.minecraft"), Launcher::Pcl),
            "PCL2"
        );
        assert_eq!(
            group_name(Path::new(r"D:\游戏\我的目录"), Launcher::Other),
            "我的目录"
        );
    }
}
