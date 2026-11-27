//! 拖进窗口的文件：按**里面的内容**认是什么，不看扩展名。
//!
//! - 整合包：有 `modrinth.index.json`（`.mrpack`）或 CurseForge 的 `manifest.json`
//! - 模组：jar 里有 `neoforge.mods.toml` / `mods.toml` / `fabric.mod.json` / `quilt.mod.json` / `mcmod.info`；
//!   没有这些、但给加载器注册了服务或者套着 `META-INF/jarjar/` 的也是（Sinytra Connector 这类服务型模组）
//! - 光影包：根上有 `shaders/`
//! - 资源包：根上有 `pack.mcmeta`，有 `assets/`（只有 `data/` 的是数据包）
//! - 地图：压缩包或文件夹里有 `level.dat`
//! - 皮肤：64×64 或 64×32 的 PNG（设给当前账号）
//! - 别的都不装，说清楚为什么（加载器安装器、一包模组、整个游戏目录、图片……）
//!
//! 拖进来的路径是用户自己的文件，只读不改；装的时候复制一份。

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::bundle::looks_like_schematic;
use super::identify::{first_mods_table, manifest_value, read_local_meta};
use crate::fs::safe_file_name;

/// 压缩包最多看这么多个条目（再多不是正常的模组、资源包；整合包的配置文件也到不了）
const MAX_ENTRIES: usize = 200_000;
/// 读压缩包里的说明文件最多读这么多
const MAX_META_BYTES: u64 = 1024 * 1024;

/// 拖进来的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum DropKind {
    Modpack,
    Mod,
    ResourcePack,
    Shader,
    DataPack,
    World,
    /// 投影（Litematica 等模组用的建筑文件），放进实例的 `schematics/`
    Schematic,
    /// 一包东西（一堆模组、光影、地图塞在一个 zip 里）：拆开后每一样单独认，见 [`super::bundle`]
    Bundle,
    /// 皮肤图片：不装进实例，设给账号
    Skin,
    /// 认不出、或者认出来了但不该装（原因见 [`DropProblem`]）
    Unknown,
}

/// 为什么不装。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum DropProblem {
    /// NeoForge / Forge / Fabric 的安装器：新建实例时直接选加载器就行
    Installer,
    /// 一包东西里又套着一包：先解压外面那层
    NestedBundle,
    /// 一包东西太大（拆出来超过 4 GB）
    TooLarge,
    /// 整个游戏目录（`.minecraft`），或者别的启动器（MultiMC、Prism、HMCL、PCL）的实例文件夹
    GameDir,
    /// 普通文件夹
    Folder,
    /// 图片
    Image,
    /// 压缩包，但认不出是什么
    UnknownArchive,
    /// 不是压缩包也不是模组
    NotGameFile,
    /// 读不了（坏了、被别的程序占着）
    Unreadable,
}

/// 认出来的一个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub path: PathBuf,
    /// 文件名（给人看）
    pub name: String,
    pub size: u64,
    pub kind: DropKind,
    pub problem: Option<DropProblem>,
    /// 模组 ID（`mods.toml` 的 `modId`、`fabric.mod.json` 的 `id`），用来认「同一个模组的另一个版本」
    pub mod_id: Option<String>,
    /// jar 里写的加载器（`neoforge`、`forge`、`fabric`、`quilt`）
    pub loaders: Vec<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub icon: Option<Vec<u8>>,
    /// 从哪一包东西里拆出来的（那个压缩包的文件名）
    pub from: Option<String>,
}

impl Dropped {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: 0,
            kind: DropKind::Unknown,
            problem: None,
            mod_id: None,
            loaders: Vec::new(),
            title: None,
            version: None,
            icon: None,
            from: None,
        }
    }

    fn unknown(mut self, problem: DropProblem) -> Self {
        self.kind = DropKind::Unknown;
        self.problem = Some(problem);
        self
    }
}

/// 认一个拖进来的文件或文件夹。只读，读文件比较多，在后台线程里调用。
pub fn inspect(path: &Path) -> Dropped {
    let found = Dropped::new(path);
    let Ok(meta) = std::fs::metadata(path) else {
        return found.unknown(DropProblem::Unreadable);
    };
    if meta.is_dir() {
        return inspect_dir(found);
    }
    let found = Dropped {
        size: meta.len(),
        ..found
    };
    let Ok(file) = File::open(path) else {
        return found.unknown(DropProblem::Unreadable);
    };
    match zip::ZipArchive::new(file) {
        Ok(archive) => inspect_archive(found, archive),
        Err(_) if looks_like_schematic(path) => Dropped {
            kind: DropKind::Schematic,
            ..found
        },
        Err(_) if looks_like_skin(path) => Dropped {
            kind: DropKind::Skin,
            ..found
        },
        Err(_) if looks_like_image(path) => found.unknown(DropProblem::Image),
        Err(_) => found.unknown(DropProblem::NotGameFile),
    }
}

fn inspect_dir(mut found: Dropped) -> Dropped {
    let dir = found.path.clone();
    if dir.join("level.dat").is_file() {
        found.kind = DropKind::World;
        found.title = Some(found.name.clone());
        return found;
    }
    // 游戏目录；MultiMC / Prism 的实例（instance.cfg、mmc-pack.json）；
    // HMCL / PCL 放启动器的文件夹（旁边有 .minecraft）
    if dir.join("versions").is_dir()
        || dir.join("launcher_profiles.json").is_file()
        || dir.join("instance.cfg").is_file()
        || dir.join("mmc-pack.json").is_file()
        || dir.join(".minecraft").is_dir()
    {
        return found.unknown(DropProblem::GameDir);
    }
    found.unknown(DropProblem::Folder)
}

/// 皮肤：64×64 或 64×32 的 PNG（先看大小，皮肤只有几 KB）
fn looks_like_skin(path: &Path) -> bool {
    let small =
        std::fs::metadata(path).is_ok_and(|m| m.len() <= crate::auth::skin::MAX_SKIN_BYTES as u64);
    small
        && std::fs::read(path)
            .ok()
            .is_some_and(|bytes| crate::auth::skin::check(&bytes).is_ok())
}

/// 看文件头认图片（png、jpg、gif、webp、bmp），给「认不出」一个更具体的说法
fn looks_like_image(path: &Path) -> bool {
    let mut head = [0u8; 12];
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(n) = file.read(&mut head) else {
        return false;
    };
    let head = &head[..n];
    head.starts_with(b"\x89PNG")
        || head.starts_with(&[0xFF, 0xD8, 0xFF])
        || head.starts_with(b"GIF8")
        || head.starts_with(b"BM")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP".as_slice()))
}

fn read_entry(archive: &mut zip::ZipArchive<File>, name: &str) -> Option<String> {
    let entry = archive.by_name(name).ok()?;
    if entry.size() > MAX_META_BYTES {
        return None;
    }
    let mut text = String::new();
    entry.take(MAX_META_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

fn inspect_archive(mut found: Dropped, mut archive: zip::ZipArchive<File>) -> Dropped {
    if archive.len() > MAX_ENTRIES {
        return found.unknown(DropProblem::UnknownArchive);
    }
    // 文件名不是合法 UTF-8 的条目跳过（不可能是我们要认的那几个文件）
    let names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|n| n.replace('\\', "/"))
        .collect();
    let set: HashSet<&str> = names.iter().map(String::as_str).collect();
    let has = |name: &str| set.contains(name);
    let under = |prefix: &str| names.iter().any(|n| n.starts_with(prefix));

    // 整合包（mrpack 里也可能带模组的 jar、存档，所以最先认）
    if has("modrinth.index.json") {
        found.kind = DropKind::Modpack;
        return found;
    }
    if has("manifest.json")
        && read_entry(&mut archive, "manifest.json").is_some_and(|text| {
            serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|json| {
                json["manifestType"] == "minecraftModpack" || json.get("minecraft").is_some()
            })
        })
    {
        found.kind = DropKind::Modpack;
        return found;
    }
    if crate::modpack::multimc::looks_like_multimc_entries(&names) {
        found.kind = DropKind::Modpack;
        return found;
    }
    // 加载器的安装器（里面有 install_profile.json）：不是模组
    if has("install_profile.json") {
        return found.unknown(DropProblem::Installer);
    }

    // 模组
    let mut loaders = Vec::new();
    let mut mod_id = None;
    if let Some(text) = read_entry(&mut archive, "META-INF/neoforge.mods.toml") {
        loaders.push("neoforge".to_owned());
        mod_id = first_mods_table(&text).get("modId").cloned();
    }
    if let Some(text) = read_entry(&mut archive, "META-INF/mods.toml") {
        // NeoForge 1.20.2–1.20.4 也用 mods.toml，靠依赖里的 neoforge 认出来
        let neo = text.contains("modId=\"neoforge\"") || text.contains("modId = \"neoforge\"");
        let loader = if neo { "neoforge" } else { "forge" };
        if !loaders.iter().any(|l| l == loader) {
            loaders.push(loader.to_owned());
        }
        mod_id = mod_id.or_else(|| first_mods_table(&text).get("modId").cloned());
    }
    if let Some(text) = read_entry(&mut archive, "fabric.mod.json") {
        loaders.push("fabric".to_owned());
        mod_id = mod_id.or_else(|| {
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|j| j["id"].as_str().map(str::to_owned))
        });
    }
    if let Some(text) = read_entry(&mut archive, "quilt.mod.json") {
        loaders.push("quilt".to_owned());
        mod_id = mod_id.or_else(|| {
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|j| j["quilt_loader"]["id"].as_str().map(str::to_owned))
        });
    }
    if loaders.is_empty() && has("mcmod.info") {
        loaders.push("forge".to_owned());
    }
    // 没有说明文件、但是给加载器用的 jar：服务型模组（Sinytra Connector 这类，本体套在
    // META-INF/jarjar/ 里）、语言支持库。加载器看它注册的服务；看不出来就不填（不拦）
    if loaders.is_empty() {
        let service = |prefix: &str| under(&format!("META-INF/services/{prefix}"));
        if service("net.neoforged.") {
            loaders.push("neoforge".to_owned());
        } else if service("net.minecraftforge.") {
            loaders.push("forge".to_owned());
        }
        let manifest = read_entry(&mut archive, "META-INF/MANIFEST.MF");
        let service_mod = !loaders.is_empty()
            || has("META-INF/jarjar/metadata.json")
            || manifest
                .as_deref()
                .is_some_and(|m| manifest_value(m, "FMLModType").is_some());
        if service_mod {
            let meta = read_local_meta(&found.path);
            found.kind = DropKind::Mod;
            found.loaders = loaders;
            // Sinytra Connector 本身：记下 ID，装了它的实例能用 Fabric 模组（见 `mod_fit`）
            found.mod_id = read_entry(&mut archive, "META-INF/jarjar/metadata.json")
                .filter(|text| text.contains("org.sinytra") && text.contains("connector"))
                .map(|_| FABRIC_BRIDGE.to_owned());
            found.version = meta.version.or_else(|| {
                manifest
                    .as_deref()
                    .and_then(|m| manifest_value(m, "Implementation-Version"))
            });
            found.icon = meta.icon;
            return found;
        }
    }
    if !loaders.is_empty() {
        let meta = read_local_meta(&found.path);
        found.kind = DropKind::Mod;
        found.loaders = loaders;
        found.mod_id = mod_id;
        found.title = meta.title;
        found.version = meta.version;
        found.icon = meta.icon;
        return found;
    }

    // 光影包：根上有 shaders/（资源包里的 assets/minecraft/shaders 不算）
    if under("shaders/") {
        found.kind = DropKind::Shader;
        return found;
    }
    // 资源包 / 数据包
    if has("pack.mcmeta") {
        found.kind = if !under("assets/") && under("data/") {
            DropKind::DataPack
        } else {
            DropKind::ResourcePack
        };
        found.icon = read_local_meta(&found.path).icon;
        return found;
    }
    // 地图，或者一包东西（一堆模组、光影、地图塞在一个 zip 里）
    let plan = super::bundle::plan(&names);
    if plan.is_bundle() {
        found.kind = DropKind::Bundle;
        return found;
    }
    if let Some(prefix) = plan.worlds.first() {
        let folder = prefix
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|f| !f.is_empty())
            .map(str::to_owned);
        found.kind = DropKind::World;
        found.title = folder.or_else(|| {
            found
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        });
        return found;
    }
    found.unknown(DropProblem::UnknownArchive)
}

/// 一个模组的 ID 和版本（读 jar 里的说明，读不到返回 `None`）
pub fn mod_identity(path: &Path) -> Option<(String, Option<String>)> {
    let dropped = inspect(path);
    let id = dropped.mod_id?;
    Some((id, dropped.version))
}

/// 实例 `mods` 文件夹里每个模组的 ID → 文件名（启用时的名字）和版本。停用的（`.disabled`）也算。
pub fn installed_mods(mods_dir: &Path) -> BTreeMap<String, (String, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return BTreeMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".jar") || lower.ends_with(".jar.disabled")) {
                return None;
            }
            let (id, version) = mod_identity(&e.path())?;
            let enabled = name.strip_suffix(".disabled").unwrap_or(&name).to_owned();
            Some((id, (enabled, version)))
        })
        .collect()
}

/// 模组和实例对不对得上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ModFit {
    Fits,
    /// 实例是原版，装不了模组
    NoLoader,
    /// 加载器对不上（这个模组要的是 `needs`）
    WrongLoader {
        needs: Vec<String>,
    },
    /// 游戏版本对不上（这个模组支持的最新几个版本）
    WrongGame {
        supports: Vec<String>,
    },
}

/// Sinytra Connector 的模组 ID：NeoForge 实例装了它就能跑 Fabric 模组
pub const FABRIC_BRIDGE: &str = "connector";

/// 看一个模组能不能装进实例。`loaders` / `game_versions` 优先用 Modrinth 上查到的，查不到时
/// `game_versions` 为空（只看加载器）。`bridge`：实例里装了 Sinytra Connector。
pub fn mod_fit(
    instance_game: &str,
    instance_loader: Option<&str>,
    loaders: &[String],
    game_versions: &[String],
    bridge: bool,
) -> ModFit {
    let Some(loader) = instance_loader else {
        return ModFit::NoLoader;
    };
    // NeoForge 1.20.1 是从 Forge 分出来的，能直接用 Forge 1.20.1 的模组
    let forge_ok = loader == "neoforge" && instance_game == "1.20.1";
    let fabric_ok = loader == "neoforge" && bridge;
    let loader_ok = loaders.is_empty()
        || loaders.iter().any(|l| l == loader)
        || (fabric_ok && loaders.iter().any(|l| l == "fabric"))
        || (forge_ok && loaders.iter().any(|l| l == "forge"));
    if !loader_ok {
        return ModFit::WrongLoader {
            needs: loaders.to_vec(),
        };
    }
    if !game_versions.is_empty() && !game_versions.iter().any(|v| v == instance_game) {
        let supports = game_versions
            .iter()
            .rev()
            .filter(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
            .take(3)
            .cloned()
            .collect();
        return ModFit::WrongGame { supports };
    }
    ModFit::Fits
}

/// 把数据包复制进一个世界的 `datapacks/`，返回放进去的文件名（重名时加「 (2)」）。
pub fn install_datapack(source: &Path, world_dir: &Path) -> std::io::Result<String> {
    copy_into(source, &world_dir.join("datapacks"), "datapack.zip")
}

/// 投影复制进实例的 `schematics/`（Litematica 读这里），返回放进去的文件名（重名时加「 (2)」）。
pub fn install_schematic(source: &Path, instance_dir: &Path) -> std::io::Result<String> {
    copy_into(
        source,
        &instance_dir.join("schematics"),
        "schematic.litematic",
    )
}

/// 复制一个文件进 `dir`，名字不和已有的重；先写临时名再改名
fn copy_into(source: &Path, dir: &Path, fallback: &str) -> std::io::Result<String> {
    let raw = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = safe_file_name(&raw)
        .map(str::to_owned)
        .unwrap_or_else(|_| fallback.to_owned());
    std::fs::create_dir_all(dir)?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_owned(), format!(".{ext}")),
        None => (name.clone(), String::new()),
    };
    let final_name = std::iter::once(name.clone())
        .chain((2..).map(|n| format!("{stem} ({n}){ext}")))
        .find(|n| !dir.join(n).exists())
        .unwrap_or(name);
    let tmp = dir.join(format!(".mcl-incoming-{}", uuid::Uuid::new_v4().simple()));
    std::fs::copy(source, &tmp)?;
    if let Err(error) = std::fs::rename(&tmp, dir.join(&final_name)) {
        if let Err(cleanup) = std::fs::remove_file(&tmp) {
            tracing::warn!(%cleanup, "清理没复制完的文件失败");
        }
        return Err(error);
    }
    Ok(final_name)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn zip_file(dir: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let path = dir.join(name);
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for (entry, body) in entries {
            zip.start_file(*entry, SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn recognizes_each_kind_by_what_is_inside() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let cases = [
            (
                zip_file(d, "pack.mrpack", &[("modrinth.index.json", "{}")]),
                DropKind::Modpack,
            ),
            (
                zip_file(
                    d,
                    "cf.zip",
                    &[(
                        "manifest.json",
                        r#"{"manifestType":"minecraftModpack","minecraft":{}}"#,
                    )],
                ),
                DropKind::Modpack,
            ),
            (
                zip_file(
                    d,
                    "sodium.jar",
                    &[(
                        "fabric.mod.json",
                        r#"{"id":"sodium","name":"Sodium","version":"0.6"}"#,
                    )],
                ),
                DropKind::Mod,
            ),
            (
                zip_file(
                    d,
                    "bsl.zip",
                    &[
                        ("shaders/composite.fsh", "x"),
                        ("shaders/lang/zh_cn.lang", "x"),
                    ],
                ),
                DropKind::Shader,
            ),
            (
                zip_file(
                    d,
                    "faithful.zip",
                    &[
                        ("pack.mcmeta", "{}"),
                        ("assets/minecraft/textures/a.png", "x"),
                    ],
                ),
                DropKind::ResourcePack,
            ),
            (
                zip_file(
                    d,
                    "rules.zip",
                    &[("pack.mcmeta", "{}"), ("data/x/recipe/a.json", "{}")],
                ),
                DropKind::DataPack,
            ),
            (
                zip_file(
                    d,
                    "skyblock.zip",
                    &[
                        ("Skyblock/level.dat", "nbt"),
                        ("Skyblock/region/r.0.0.mca", "x"),
                    ],
                ),
                DropKind::World,
            ),
            // 扩展名改了也认得：jar 改成 zip
            (
                zip_file(
                    d,
                    "renamed.zip",
                    &[(
                        "META-INF/neoforge.mods.toml",
                        "[[mods]]\nmodId=\"create\"\ndisplayName=\"Create\"\n",
                    )],
                ),
                DropKind::Mod,
            ),
        ];
        for (path, kind) in cases {
            let found = inspect(&path);
            assert_eq!(found.kind, kind, "{}", found.name);
            assert_eq!(found.problem, None, "{}", found.name);
        }
        let sodium = inspect(&d.join("sodium.jar"));
        assert_eq!(sodium.loaders, ["fabric"]);
        assert_eq!(sodium.mod_id.as_deref(), Some("sodium"));
        assert_eq!(sodium.title.as_deref(), Some("Sodium"));
        assert_eq!(
            inspect(&d.join("skyblock.zip")).title.as_deref(),
            Some("Skyblock")
        );
        assert_eq!(inspect(&d.join("renamed.zip")).loaders, ["neoforge"]);
    }

    #[test]
    fn explains_what_it_will_not_install() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let installer = zip_file(
            d,
            "neoforge-installer.jar",
            &[("install_profile.json", "{}")],
        );
        assert_eq!(inspect(&installer).problem, Some(DropProblem::Installer));
        let mods = zip_file(d, "mods.zip", &[("a.jar", "x"), ("b.jar", "y")]);
        assert_eq!(inspect(&mods).kind, DropKind::Bundle, "一包模组拆开再认");
        // 服务型模组（Sinytra Connector 的样子）：没有 mods.toml，本体套在 META-INF/jarjar/ 里
        let connector = zip_file(
            d,
            "connector-full.jar",
            &[
                (
                    "META-INF/MANIFEST.MF",
                    "Manifest-Version: 1.0\nImplementation-Version: 2.0.0-beta.17+1.21.1\n",
                ),
                (
                    "META-INF/jarjar/metadata.json",
                    r#"{"jars":[{"identifier":{"group":"org.sinytra","artifact":"connector-mod"}}]}"#,
                ),
                ("META-INF/jarjar/connector-mod.jar", "x"),
                (
                    "META-INF/services/net.neoforged.neoforgespi.locating.IModFileCandidateLocator",
                    "org.sinytra.Locator",
                ),
            ],
        );
        let found = inspect(&connector);
        assert_eq!(found.kind, DropKind::Mod);
        assert_eq!(found.loaders, ["neoforge"]);
        assert_eq!(found.version.as_deref(), Some("2.0.0-beta.17+1.21.1"));
        assert_eq!(found.mod_id.as_deref(), Some(FABRIC_BRIDGE));
        let library = zip_file(
            d,
            "kotlin-lang.jar",
            &[("META-INF/MANIFEST.MF", "FMLModType: LANGPROVIDER\n")],
        );
        let found = inspect(&library);
        assert_eq!(found.kind, DropKind::Mod);
        assert!(found.loaders.is_empty());
        let other = zip_file(d, "photos.zip", &[("a.txt", "x")]);
        assert_eq!(inspect(&other).problem, Some(DropProblem::UnknownArchive));
        let png = d.join("skin.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert_eq!(inspect(&png).problem, Some(DropProblem::Image));
        // 真的皮肤（64×64 的 PNG）认成皮肤
        let skin = d.join("real-skin.png");
        std::fs::write(&skin, crate::auth::skin::tests::sample_png(64, 64, true)).unwrap();
        assert_eq!(inspect(&skin).kind, DropKind::Skin);
        let big = d.join("photo.png");
        std::fs::write(&big, crate::auth::skin::tests::sample_png(128, 96, false)).unwrap();
        assert_eq!(inspect(&big).problem, Some(DropProblem::Image));
        let text = d.join("readme.txt");
        std::fs::write(&text, "hello").unwrap();
        assert_eq!(inspect(&text).problem, Some(DropProblem::NotGameFile));
        assert_eq!(
            inspect(&d.join("missing.jar")).problem,
            Some(DropProblem::Unreadable)
        );

        let game = d.join(".minecraft");
        std::fs::create_dir_all(game.join("versions")).unwrap();
        assert_eq!(inspect(&game).problem, Some(DropProblem::GameDir));
        let world = d.join("我的世界");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("level.dat"), "nbt").unwrap();
        let found = inspect(&world);
        assert_eq!(found.kind, DropKind::World);
        assert_eq!(found.title.as_deref(), Some("我的世界"));
        assert_eq!(
            inspect(&d.join(".minecraft/versions")).problem,
            Some(DropProblem::Folder)
        );
        // 别的启动器：Prism 的实例文件夹、PCL 放启动器的文件夹
        let prism = d.join("Prism 实例");
        std::fs::create_dir_all(&prism).unwrap();
        std::fs::write(prism.join("instance.cfg"), "InstanceType=OneSix").unwrap();
        assert_eq!(inspect(&prism).problem, Some(DropProblem::GameDir));
        let pcl = d.join("PCL");
        std::fs::create_dir_all(pcl.join(".minecraft")).unwrap();
        assert_eq!(inspect(&pcl).problem, Some(DropProblem::GameDir));
    }

    /// 造一个 gzip 压缩的 NBT：根是复合标签，里面放这几个空的复合标签
    fn gz_nbt(path: &Path, tags: &[&str]) {
        let mut nbt = vec![10u8, 0, 0];
        for tag in tags {
            nbt.push(10);
            nbt.extend_from_slice(&(tag.len() as u16).to_be_bytes());
            nbt.extend_from_slice(tag.as_bytes());
            nbt.push(0);
        }
        nbt.push(0);
        let mut gz =
            flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap();
    }

    #[test]
    fn schematics_are_recognized_and_copied_into_schematics() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let litematic = d.join("基地.litematic");
        gz_nbt(&litematic, &["Metadata", "Regions"]);
        assert_eq!(inspect(&litematic).kind, DropKind::Schematic);
        let schem = d.join("house.schem");
        gz_nbt(&schem, &["Schematic"]);
        assert_eq!(inspect(&schem).kind, DropKind::Schematic);
        // 别的 gzip NBT（比如 level.dat 单独拖进来）不是投影
        let other = d.join("level.dat");
        gz_nbt(&other, &["Data"]);
        assert_eq!(inspect(&other).problem, Some(DropProblem::NotGameFile));

        let instance = d.join("实例");
        assert_eq!(
            install_schematic(&litematic, &instance).unwrap(),
            "基地.litematic"
        );
        assert_eq!(
            install_schematic(&litematic, &instance).unwrap(),
            "基地 (2).litematic"
        );
        assert!(instance.join("schematics/基地 (2).litematic").is_file());
    }

    #[test]
    fn tells_whether_a_mod_fits_the_instance() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            mod_fit(
                "1.21.1",
                Some("neoforge"),
                &s(&["neoforge"]),
                &s(&["1.21", "1.21.1"]),
                false
            ),
            ModFit::Fits
        );
        assert_eq!(
            mod_fit("1.21.1", None, &s(&["neoforge"]), &[], false),
            ModFit::NoLoader
        );
        assert_eq!(
            mod_fit(
                "1.21.1",
                Some("neoforge"),
                &s(&["fabric", "quilt"]),
                &[],
                false
            ),
            ModFit::WrongLoader {
                needs: s(&["fabric", "quilt"])
            }
        );
        assert_eq!(
            mod_fit(
                "1.21.1",
                Some("neoforge"),
                &s(&["neoforge"]),
                &s(&["1.20.6", "1.21.2", "1.21.3", "24w33a"]),
                false
            ),
            ModFit::WrongGame {
                supports: s(&["1.21.3", "1.21.2", "1.20.6"])
            }
        );
        assert_eq!(
            mod_fit(
                "1.20.1",
                Some("neoforge"),
                &s(&["forge"]),
                &s(&["1.20.1"]),
                false
            ),
            ModFit::Fits,
            "NeoForge 1.20.1 能用 Forge 的模组"
        );
        assert_eq!(
            mod_fit("1.21.1", Some("neoforge"), &[], &[], false),
            ModFit::Fits,
            "什么都查不到时不拦"
        );
        assert_eq!(
            mod_fit("1.21.1", Some("neoforge"), &s(&["fabric"]), &[], true),
            ModFit::Fits,
            "装了 Sinytra Connector 的 NeoForge 实例能用 Fabric 模组"
        );
    }

    #[test]
    fn finds_installed_mods_by_id_and_copies_datapacks() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        zip_file(
            &mods,
            "sodium-0.5.jar",
            &[("fabric.mod.json", r#"{"id":"sodium","version":"0.5"}"#)],
        );
        zip_file(
            &mods,
            "jei.jar.disabled",
            &[(
                "META-INF/neoforge.mods.toml",
                "[[mods]]\nmodId=\"jei\"\nversion=\"19\"\n",
            )],
        );
        std::fs::write(mods.join("notes.txt"), "x").unwrap();
        let found = installed_mods(&mods);
        assert_eq!(
            found.get("sodium"),
            Some(&("sodium-0.5.jar".to_owned(), Some("0.5".to_owned())))
        );
        assert_eq!(
            found.get("jei").map(|f| f.0.as_str()),
            Some("jei.jar"),
            "停用的也算，记启用时的名字"
        );
        assert_eq!(found.len(), 2);

        let world = dir.path().join("saves/w");
        std::fs::create_dir_all(&world).unwrap();
        let pack = zip_file(dir.path(), "rules.zip", &[("pack.mcmeta", "{}")]);
        assert_eq!(install_datapack(&pack, &world).unwrap(), "rules.zip");
        assert_eq!(
            install_datapack(&pack, &world).unwrap(),
            "rules (2).zip",
            "重名不覆盖"
        );
        assert!(world.join("datapacks/rules (2).zip").is_file());
    }
}
