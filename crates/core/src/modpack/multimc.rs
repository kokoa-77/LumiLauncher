//! MultiMC / Prism Launcher 实例包（`.zip`）导入解析。
//!
//! 支持根目录或单层顶层目录下的：
//! - `mmc-pack.json`（组件清单、游戏版本、加载器、LWJGL 2/3 选择）
//! - `instance.cfg`（名称、内存、JVM 参数、窗口尺寸、实例笔记）
//! - `patches/*.json`（Meta v1 底层补丁，例如 GTNH Java 17–25 的 `org.lwjgl3` + `net.minecraftforge` + `lwjgl3ify`）
//! - `libraries/*`（补丁声明 `MMC-hint: "local"` 时引用的实例本地库）
//! - `.minecraft/` 或 `minecraft/`（覆写游戏文件）

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use super::ModpackError;
use super::check::{Archive, ConflictChecker, Layer, collect_overrides, open_archive};
use super::plan::{PackFormat, PackPlan, PackPreview};
use crate::config::{GcPreset, WindowMode, WindowSettings, clamp_memory_mb};
use crate::fs::safe_join;
use crate::instance::{InstanceOverrides, LaunchOverride, LoaderKind, LoaderRef, MemoryOverride};
use crate::meta::MetaError;
use crate::meta::patches::{InstancePatches, MetaPatch, parse_patch};

pub(super) const PACK_FILE: &str = "mmc-pack.json";
const CONFIG_FILE: &str = "instance.cfg";
const MAX_META_BYTES: u64 = 4 * 1024 * 1024;
const MAX_NOTES_BYTES: usize = 256 * 1024;
const MAX_LOCAL_LIBS_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MmcPack {
    #[serde(default)]
    components: Vec<MmcComponent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MmcComponent {
    uid: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    cached_version: Option<String>,
    #[serde(default)]
    disabled: bool,
}

impl MmcComponent {
    fn resolved_version(&self) -> Option<&str> {
        self.version
            .as_deref()
            .or(self.cached_version.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// 从 `instance.cfg` 提取的实例附加设置。
#[derive(Debug, Clone, Default)]
pub struct ParsedInstanceCfg {
    pub name: Option<String>,
    pub notes: Option<String>,
    pub recommended_memory_mb: Option<u32>,
    pub overrides: InstanceOverrides,
}

/// 判断压缩包是否包含 MultiMC / Prism 实例清单，并返回根前缀（`""` 或 `"<单层目录>/"`）。
pub(super) fn detect_root_prefix(archive: &Archive) -> Option<String> {
    if archive.index_for_name(PACK_FILE).is_some() {
        return Some(String::new());
    }
    let mut single_top: Option<String> = None;
    let mut has_nested_pack = false;
    for raw in archive.file_names() {
        let Ok(name) = raw else { continue };
        let normalized = name.replace('\\', "/");
        let (top, rest) = normalized.split_once('/')?;
        if top.is_empty() {
            return None;
        }
        match &single_top {
            None => single_top = Some(top.to_owned()),
            Some(prev) if prev != top => return None,
            Some(_) => {}
        }
        if rest == PACK_FILE {
            has_nested_pack = true;
        }
    }
    has_nested_pack.then(|| format!("{}/", single_top.unwrap_or_default()))
}

/// 供 `content::dropped` 复用的快速识别（基于已归一化的条目名切片）。
pub fn looks_like_multimc_entries(names: &[String]) -> bool {
    if names.iter().any(|n| n == PACK_FILE) {
        return true;
    }
    let mut single_top: Option<&str> = None;
    let mut has_nested_pack = false;
    for name in names {
        let Some((top, rest)) = name.split_once('/') else {
            return false;
        };
        if top.is_empty() {
            return false;
        }
        match single_top {
            None => single_top = Some(top),
            Some(prev) if prev != top => return false,
            Some(_) => {}
        }
        if rest == PACK_FILE {
            has_nested_pack = true;
        }
    }
    has_nested_pack
}

/// 解析 `instance.cfg` 文本中的实例名称、内存、JVM 参数、窗口尺寸与笔记。
pub fn parse_instance_cfg(text: &str) -> ParsedInstanceCfg {
    let mut values: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        values.insert(k.trim().to_owned(), unescape_ini(v.trim()));
    }

    let get_str = |key: &str| {
        values
            .get(key)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let get_bool = |key: &str| {
        values
            .get(key)
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
    };
    let get_u32 = |key: &str| values.get(key).and_then(|v| v.trim().parse::<u32>().ok());

    let name = get_str("name");
    let notes = values
        .get("notes")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() > MAX_NOTES_BYTES {
                s.chars().take(MAX_NOTES_BYTES / 4).collect()
            } else {
                s
            }
        });

    let max_mem = get_u32("MaxMemAlloc").filter(|mb| (512..=65_536).contains(mb));
    let mut overrides = InstanceOverrides::default();
    if get_bool("OverrideMemory")
        && let Some(mb) = max_mem
    {
        overrides.memory = Some(MemoryOverride {
            auto: false,
            max_memory_mb: clamp_memory_mb(mb),
        });
    }

    if get_bool("OverrideJavaArgs")
        && let Some(jvm_args) = get_str("JvmArgs")
    {
        overrides.launch = Some(LaunchOverride {
            gc_preset: GcPreset::Auto,
            jvm_args,
            game_args: String::new(),
        });
    }

    if get_bool("OverrideWindow") {
        if get_bool("LaunchMaximized") {
            overrides.window = Some(WindowSettings {
                mode: WindowMode::Maximized,
                ..WindowSettings::default()
            });
        } else if let (Some(width), Some(height)) =
            (get_u32("MinecraftWinWidth"), get_u32("MinecraftWinHeight"))
        {
            overrides.window = Some(
                WindowSettings {
                    mode: WindowMode::Custom,
                    width,
                    height,
                }
                .normalized(),
            );
        }
    }

    ParsedInstanceCfg {
        name,
        notes,
        recommended_memory_mb: max_mem,
        overrides: overrides.normalized(),
    }
}

fn unescape_ini(raw: &str) -> String {
    let raw = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(raw);
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 从本机 Prism / MultiMC 实例文件夹中读取补丁、`instance.cfg` 覆盖设置与本地补丁库。
#[derive(Debug, Clone, Default)]
pub struct FolderExtras {
    pub patches: Option<InstancePatches>,
    pub cfg: ParsedInstanceCfg,
    /// `(源文件绝对路径, 目标相对路径，位于 Lumi/libraries/ 下)`
    pub local_libraries: Vec<(std::path::PathBuf, String)>,
}

pub fn read_folder_extras(folder: &Path) -> Result<FolderExtras, MetaError> {
    let cfg = std::fs::read_to_string(folder.join(CONFIG_FILE))
        .ok()
        .map(|text| parse_instance_cfg(&text))
        .unwrap_or_default();

    let pack_bytes = std::fs::read(folder.join(PACK_FILE)).map_err(|source| MetaError::Io {
        path: folder.join(PACK_FILE),
        source,
    })?;
    let pack: MmcPack = serde_json::from_slice(&pack_bytes).map_err(|source| MetaError::Parse {
        id: PACK_FILE.to_owned(),
        source,
    })?;
    let active: Vec<&MmcComponent> = pack.components.iter().filter(|c| !c.disabled).collect();
    let replace_lwjgl2 = should_replace_lwjgl2(&active);

    let mut raw_patches: HashMap<String, MetaPatch> = HashMap::new();
    let patches_dir = folder.join("patches");
    if patches_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&patches_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file()
                || path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_none_or(|e| !e.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|source| MetaError::Io {
                path: path.clone(),
                source,
            })?;
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let patch = parse_patch(&file_name, &bytes)?;
            raw_patches.insert(patch.uid.to_ascii_lowercase(), patch);
        }
    }

    let ordered = order_patches(&active, raw_patches);
    let bundle = InstancePatches::new(ordered, replace_lwjgl2);

    let mut local_libraries = Vec::new();
    let libs_dir = folder.join("libraries");
    if libs_dir.is_dir() && !bundle.is_empty() {
        collect_folder_local_libs(&libs_dir, &libs_dir, &mut local_libraries);
    }

    Ok(FolderExtras {
        patches: (!bundle.is_empty()).then_some(bundle),
        cfg,
        local_libraries,
    })
}

fn collect_folder_local_libs(root: &Path, dir: &Path, out: &mut Vec<(std::path::PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_folder_local_libs(root, &path, out);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if safe_join(Path::new("libraries"), &rel_str).is_ok() {
                out.push((path, rel_str));
            }
        }
    }
}

/// 读取并检查 MultiMC / Prism `.zip` 整合包，产出统一 [`PackPlan`]。
pub fn inspect(pack_path: &Path) -> Result<PackPlan, ModpackError> {
    let mut archive = open_archive(pack_path)?;
    let prefix = detect_root_prefix(&archive).ok_or_else(|| {
        ModpackError::Invalid("缺少 mmc-pack.json，不是 MultiMC / Prism 整合包".to_owned())
    })?;

    let pack_entry = format!("{prefix}{PACK_FILE}");
    let pack_bytes = read_bounded(&mut archive, &pack_entry)?;
    let pack: MmcPack = serde_json::from_slice(&pack_bytes)
        .map_err(|e| ModpackError::Invalid(format!("{PACK_FILE} 格式不对：{e}")))?;

    let active: Vec<&MmcComponent> = pack.components.iter().filter(|c| !c.disabled).collect();
    let game_version = active
        .iter()
        .find(|c| c.uid.eq_ignore_ascii_case("net.minecraft"))
        .and_then(|c| c.resolved_version())
        .map(str::to_owned)
        .ok_or_else(|| ModpackError::Invalid("mmc-pack.json 缺少 Minecraft 版本号".to_owned()))?;
    let loader = detect_loader(&active, &game_version);
    let replace_lwjgl2 = should_replace_lwjgl2(&active);

    // 解析 instance.cfg（可选，缺了就回退到压缩包文件名）
    let cfg_entry = format!("{prefix}{CONFIG_FILE}");
    let cfg = if archive.index_for_name(&cfg_entry).is_some() {
        let bytes = read_bounded(&mut archive, &cfg_entry)?;
        parse_instance_cfg(&String::from_utf8_lossy(&bytes))
    } else {
        ParsedInstanceCfg::default()
    };

    // 解析 patches/*.json（全部做严格安全校验）
    let patch_prefix = format!("{prefix}patches/");
    let patch_entries: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|name| {
            name.starts_with(&patch_prefix)
                && !name.ends_with('/')
                && name.to_ascii_lowercase().ends_with(".json")
        })
        .map(std::borrow::Cow::into_owned)
        .collect();

    let mut raw_patches: HashMap<String, MetaPatch> = HashMap::new();
    for entry_name in patch_entries {
        let rel = entry_name
            .strip_prefix(&patch_prefix)
            .unwrap_or(&entry_name);
        safe_join(Path::new("patches"), rel)
            .map_err(|reason| ModpackError::Rejected(format!("补丁路径不安全（{reason}）")))?;
        let bytes = read_bounded(&mut archive, &entry_name)?;
        let patch = parse_patch(rel, &bytes).map_err(|err| match err {
            MetaError::InvalidPatch(msg) => ModpackError::Rejected(msg),
            other => ModpackError::Invalid(other.to_string()),
        })?;
        raw_patches.insert(patch.uid.to_ascii_lowercase(), patch);
    }

    let ordered_patches = order_patches(&active, raw_patches);
    let bundle = InstancePatches::new(ordered_patches, replace_lwjgl2);
    let patches_opt = (!bundle.is_empty()).then_some(bundle);

    // 判断加载器是由补丁自己提供，还是走标准加载器安装器
    let skip_loader_install = match (&loader, &patches_opt) {
        (Some(loader_ref), Some(bundle)) => loader_provided_by_patches(loader_ref, bundle),
        _ => false,
    };
    if let Some(loader_ref) = &loader
        && !skip_loader_install
        && !loader_ref.kind.installable_for(&game_version)
    {
        return Err(ModpackError::UnsupportedLoader(format!(
            "Minecraft {game_version} 的 {} {}（包内未自带对应 patches 补丁）",
            loader_ref.kind.label(),
            loader_ref.version
        )));
    }

    // 收集包内本地补丁库（libraries/）
    let libs_prefix = format!("{prefix}libraries/");
    let local_libraries = collect_local_libraries(&mut archive, &libs_prefix)?;

    // 收集游戏覆写文件（.minecraft/ 或 minecraft/）
    let dot_mc = format!("{prefix}.minecraft/");
    let plain_mc = format!("{prefix}minecraft/");
    let mut conflicts = ConflictChecker::default();
    let overrides = collect_overrides(
        &mut archive,
        &[
            (dot_mc.as_str(), Layer::Overrides),
            (plain_mc.as_str(), Layer::ClientOverrides),
        ],
        &mut conflicts,
    )?;

    let fallback_name = pack_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "MultiMC Instance".to_owned());
    let name = cfg.name.unwrap_or(fallback_name);

    let patch_count = patches_opt
        .as_ref()
        .map_or(0, |b| u32::try_from(b.patches.len()).unwrap_or(u32::MAX));
    let patch_names = patches_opt
        .as_ref()
        .map(InstancePatches::display_names)
        .unwrap_or_default();
    let java_majors = patches_opt
        .as_ref()
        .map(InstancePatches::compatible_java_majors)
        .unwrap_or_default();

    let preview = PackPreview {
        format: PackFormat::MultiMc,
        name,
        version: game_version.clone(),
        summary: cfg
            .notes
            .as_deref()
            .and_then(|n| n.lines().find(|l| !l.trim().is_empty()))
            .map(|l| l.trim().chars().take(120).collect()),
        game_version,
        loader,
        file_count: 0,
        download_bytes: 0.0,
        override_count: u32::try_from(overrides.len()).unwrap_or(u32::MAX),
        optional_files: Vec::new(),
        manual_files: Vec::new(),
        from_modrinth: 0,
        recommended_memory_mb: cfg.recommended_memory_mb,
        patch_count,
        patch_names,
        java_majors,
    };

    Ok(PackPlan {
        preview,
        origin: None,
        pack_path: pack_path.to_path_buf(),
        downloads: Vec::new(),
        overrides,
        skip_loader_install,
        patches: patches_opt,
        local_libraries,
        overrides_config: cfg.overrides,
        notes: cfg.notes,
    })
}

fn read_bounded(archive: &mut Archive, name: &str) -> Result<Vec<u8>, ModpackError> {
    let entry = archive
        .by_name(name)
        .map_err(|_| ModpackError::Invalid(format!("缺少 {name}")))?;
    if entry.is_symlink() {
        return Err(ModpackError::Rejected(format!("包含符号链接：{name}")));
    }
    if entry.size() > MAX_META_BYTES {
        return Err(ModpackError::Rejected(format!("文件过大：{name}")));
    }
    let mut bytes = Vec::new();
    entry
        .take(MAX_META_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ModpackError::Invalid(format!("{name} 读取失败：{e}")))?;
    if bytes.len() as u64 > MAX_META_BYTES {
        return Err(ModpackError::Rejected(format!("文件过大：{name}")));
    }
    Ok(bytes)
}

fn collect_local_libraries(
    archive: &mut Archive,
    prefix: &str,
) -> Result<Vec<(String, String)>, ModpackError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut total_bytes = 0u64;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| ModpackError::Invalid(format!("压缩包损坏：{e}")))?;
        let name = entry
            .name()
            .map_err(|_| ModpackError::Invalid("压缩包里有无法识别的文件名".to_owned()))?
            .into_owned();
        let Some(relative) = name.strip_prefix(prefix) else {
            continue;
        };
        if entry.is_dir() || relative.is_empty() {
            continue;
        }
        if entry.is_symlink() {
            return Err(ModpackError::Rejected(format!("包含符号链接：{name}")));
        }
        safe_join(Path::new("libraries"), relative)
            .map_err(|reason| ModpackError::Rejected(format!("本地库路径不安全（{reason}）")))?;
        if !seen.insert(relative.to_ascii_lowercase()) {
            return Err(ModpackError::Rejected(format!(
                "本地库同名冲突：{relative}"
            )));
        }
        total_bytes += entry.size();
        if total_bytes > MAX_LOCAL_LIBS_BYTES {
            return Err(ModpackError::Rejected(
                "本地补丁库总大小超过上限".to_owned(),
            ));
        }
        let relative = relative.to_owned();
        out.push((name, relative));
    }
    Ok(out)
}

fn detect_loader(active: &[&MmcComponent], game_version: &str) -> Option<LoaderRef> {
    const MAP: &[(&str, LoaderKind)] = &[
        ("net.neoforged", LoaderKind::NeoForge),
        ("net.minecraftforge", LoaderKind::Forge),
        ("net.fabricmc.fabric-loader", LoaderKind::Fabric),
        ("org.quiltmc.quilt-loader", LoaderKind::Quilt),
    ];
    let has_legacy_intermediary = active
        .iter()
        .any(|c| c.uid.eq_ignore_ascii_case("net.legacyfabric.intermediary"));
    for &(uid, kind) in MAP {
        if let Some(version) = active
            .iter()
            .find(|c| c.uid.eq_ignore_ascii_case(uid))
            .and_then(|c| c.resolved_version())
        {
            let kind = if kind == LoaderKind::Fabric && game_version == "b1.7.3" {
                LoaderKind::Babric
            } else if kind == LoaderKind::Fabric
                && (has_legacy_intermediary
                    || LoaderKind::LegacyFabric.installable_for(game_version))
            {
                LoaderKind::LegacyFabric
            } else {
                kind
            };
            return Some(LoaderRef {
                kind,
                version: version.to_owned(),
            });
        }
    }
    None
}

fn should_replace_lwjgl2(active: &[&MmcComponent]) -> bool {
    let has_lwjgl3 = active
        .iter()
        .any(|c| c.uid.eq_ignore_ascii_case("org.lwjgl3"));
    let has_lwjgl2 = active
        .iter()
        .any(|c| c.uid.eq_ignore_ascii_case("org.lwjgl"));
    has_lwjgl3 && !has_lwjgl2
}

fn loader_provided_by_patches(loader: &LoaderRef, bundle: &InstancePatches) -> bool {
    match loader.kind {
        LoaderKind::Forge => bundle.provides_legacy_forge(),
        LoaderKind::NeoForge => bundle.provides_component("net.neoforged"),
        LoaderKind::Fabric | LoaderKind::LegacyFabric | LoaderKind::Babric => {
            bundle.provides_component("net.fabricmc.fabric-loader")
        }
        LoaderKind::Quilt => bundle.provides_component("org.quiltmc.quilt-loader"),
    }
}

fn order_patches(active: &[&MmcComponent], mut raw: HashMap<String, MetaPatch>) -> Vec<MetaPatch> {
    let mut ordered = Vec::new();
    for component in active {
        if let Some(patch) = raw.remove(&component.uid.to_ascii_lowercase()) {
            ordered.push(patch);
        }
    }
    let mut rest: Vec<(String, MetaPatch)> = raw.into_iter().collect();
    rest.sort_by(|(a, _), (b, _)| a.cmp(b));
    ordered.extend(rest.into_iter().map(|(_, p)| p));
    ordered
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn inspects_nested_gtnh_style_prism_zip() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("GTNH-2.7.zip");
        write_zip(
            &pack,
            &[
                (
                    "GT_New_Horizons/mmc-pack.json",
                    br#"{
                        "formatVersion": 1,
                        "components": [
                            { "uid": "org.lwjgl3", "version": "3.3.3" },
                            { "uid": "net.minecraft", "version": "1.7.10" },
                            { "uid": "net.minecraftforge", "version": "10.13.4.1614" },
                            { "uid": "com.gtnewhorizons.lwjgl3ify.forgepatches", "version": "1.0" }
                        ]
                    }"#,
                ),
                (
                    "GT_New_Horizons/instance.cfg",
                    b"[General]\nname=\"GT New Horizons\"\nOverrideMemory=true\nMaxMemAlloc=6144\nOverrideJavaArgs=true\nJvmArgs=\"-XX:+UseZGC -Dfml.readTimeout=180\"\nnotes=\"Line1\\nLine2\"\n",
                ),
                (
                    "GT_New_Horizons/patches/org.lwjgl3.json",
                    br#"{"uid":"org.lwjgl3","name":"LWJGL 3","libraries":[{"name":"org.lwjgl:lwjgl:3.3.3","url":"https://libraries.minecraft.net"}]}"#,
                ),
                (
                    "GT_New_Horizons/patches/net.minecraftforge.json",
                    br#"{"uid":"net.minecraftforge","name":"Forge","mainClass":"net.minecraft.launchwrapper.Launch","+tweakers":["cpw.mods.fml.common.launcher.FMLTweaker"],"libraries":[{"name":"net.minecraftforge:forge:1.7.10-10.13.4.1614","url":"https://maven.minecraftforge.net"}]}"#,
                ),
                (
                    "GT_New_Horizons/patches/com.gtnewhorizons.lwjgl3ify.forgepatches.json",
                    br#"{"uid":"com.gtnewhorizons.lwjgl3ify.forgepatches","name":"lwjgl3ify","compatibleJavaMajors":[17,21],"+jvmArgs":["--add-opens","java.base/java.lang=ALL-UNNAMED"],"libraries":[{"name":"com.gtnewhorizons:helper:1.0","MMC-hint":"local","MMC-filename":"helper.jar"}]}"#,
                ),
                ("GT_New_Horizons/libraries/helper.jar", b"localjar"),
                ("GT_New_Horizons/.minecraft/mods/gt.jar", b"mod"),
                ("GT_New_Horizons/.minecraft/config/gt.cfg", b"cfg"),
            ],
        );

        assert_eq!(crate::modpack::detect(&pack).unwrap(), PackFormat::MultiMc);
        let plan = inspect(&pack).unwrap();
        assert_eq!(plan.preview.format, PackFormat::MultiMc);
        assert_eq!(plan.preview.name, "GT New Horizons");
        assert_eq!(plan.preview.game_version, "1.7.10");
        assert_eq!(
            plan.preview.loader,
            Some(LoaderRef {
                kind: LoaderKind::Forge,
                version: "10.13.4.1614".to_owned()
            })
        );
        assert!(plan.skip_loader_install, "1.7.10 Forge 由包内补丁提供");
        assert_eq!(plan.preview.patch_count, 3);
        assert_eq!(plan.preview.java_majors, vec![17, 21]);
        assert_eq!(plan.preview.recommended_memory_mb, Some(6144));
        assert_eq!(plan.preview.override_count, 2);
        assert_eq!(plan.local_libraries.len(), 1);
        assert_eq!(plan.overrides_config.memory.unwrap().max_memory_mb, 6144);
        assert_eq!(
            plan.overrides_config.launch.unwrap().jvm_args,
            "-XX:+UseZGC -Dfml.readTimeout=180"
        );
        assert_eq!(plan.notes.as_deref(), Some("Line1\nLine2"));
    }

    #[test]
    fn rejects_insecure_patch_and_path_traversal_in_multimc_zip() {
        let dir = tempfile::tempdir().unwrap();
        let bad_url = dir.path().join("bad-url.zip");
        write_zip(
            &bad_url,
            &[
                (
                    "mmc-pack.json",
                    br#"{"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
                ),
                (
                    "patches/evil.json",
                    br#"{"uid":"evil","libraries":[{"name":"a:b:1","url":"http://insecure.example"}]}"#,
                ),
            ],
        );
        assert!(matches!(inspect(&bad_url), Err(ModpackError::Rejected(_))));

        let bad_path = dir.path().join("bad-path.zip");
        write_zip(
            &bad_path,
            &[
                (
                    "mmc-pack.json",
                    br#"{"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
                ),
                (".minecraft/../../outside.txt", b"escape"),
            ],
        );
        assert!(matches!(inspect(&bad_path), Err(ModpackError::Rejected(_))));
    }
}
