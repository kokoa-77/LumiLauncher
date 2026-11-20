//! 启动前极速本地预检（D25d）：拉起 JVM 前只读本地元数据，拦截高确定性必崩问题。
//!
//! 检查四类硬伤：
//! 1. 手动指定的 Java 大版本低于版本要求（`JavaTooOld`）
//! 2. 加载器明显错配（例如纯 Fabric 模组放进 NeoForge / Forge 实例，装了 Sinytra Connector 时跳过）
//! 3. `mods/` 内存在重复的 `modId`（常见于手动拖入新版但未删旧版）
//! 4. 缺失必需前置模组（会识别嵌套 JiJ 子模组与内置运行时 ID，尽量避免误报）

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

use serde::Serialize;

use crate::instance::{Instance, LoaderKind};
use crate::meta::MetaError;
use crate::meta::version::{self, VersionJson};
use crate::paths::GameDir;

/// 单个元信息文件读取上限（防止恶意巨型条目拖慢预检）
const MAX_META_BYTES: u64 = 2 * 1024 * 1024;
/// 单次预检最多扫多少个 JAR（避免异常目录拖慢启动）
const MAX_MOD_JARS: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum PreflightIssueKind {
    JavaTooOld,
    WrongLoader,
    DuplicateModId,
    MissingDependency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PreflightIssue {
    pub kind: PreflightIssueKind,
    /// 出问题的模组名称（Java 检查时为空）
    pub mod_name: Option<String>,
    /// 缺失的前置 ID、重复的 modId、所需 Java 版本或模组声明的加载器
    pub target: String,
    /// 补充说明（例如涉及的两个文件名、当前 Java 版本）
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub issues: Vec<PreflightIssue>,
    pub checked_mods: u32,
    pub elapsed_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModEco {
    Fabric,
    Quilt,
    NeoForge,
    Forge,
}

impl ModEco {
    fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::NeoForge => "NeoForge",
            Self::Forge => "Forge",
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedMod {
    file_name: String,
    mod_ids: Vec<String>,
    display_name: String,
    ecos: Vec<ModEco>,
    provides: Vec<String>,
    required_deps: Vec<String>,
    embedded_ids: Vec<String>,
}

/// 对一个实例做本地快速启动预检。
/// `selected_java_major`：仅当用户明确固定了某个 Java（实例指定或该大版本默认）且已测得其主版本号时传入。
pub fn check(
    game: &GameDir,
    instance: &Instance,
    selected_java_major: Option<u32>,
) -> PreflightReport {
    let started = Instant::now();
    let mut issues = Vec::new();

    // 1. Java 主版本预检（仅在本地版本 JSON 可读且用户显式指定了更低版本 Java 时提示）
    if let Some(actual_major) = selected_java_major
        && let Some(required_major) =
            local_required_java(&instance.files(game), &instance.version_id())
        && actual_major < required_major
    {
        issues.push(PreflightIssue {
            kind: PreflightIssueKind::JavaTooOld,
            mod_name: None,
            target: format!("Java {required_major}"),
            detail: format!("Java {actual_major}"),
        });
    }

    // 2. 扫描实例 `mods/` 中已启用的 .jar
    let loader_kind = instance.config.loader.as_ref().map(|l| l.kind);
    let mods_dir = instance
        .game_dir(game)
        .map(|d| d.join("mods"))
        .unwrap_or_default();
    let (checked_mods, mod_issues) = check_mods_dir(&mods_dir, loader_kind);
    issues.extend(mod_issues);

    let elapsed_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    PreflightReport {
        issues,
        checked_mods,
        elapsed_ms,
    }
}

fn local_required_java(files: &GameDir, version_id: &str) -> Option<u32> {
    let merged = version::resolve(version_id, |id| {
        let path = files
            .version_json(id)
            .map_err(|_| MetaError::InvalidId(id.to_owned()))?;
        let bytes = std::fs::read(&path).map_err(|_| MetaError::InvalidId(id.to_owned()))?;
        VersionJson::parse(id, &bytes)
    })
    .ok()?;
    Some(merged.required_java())
}

/// 纯目录级检查（方便单元测试直接构造假 `mods/` 目录验证）
pub fn check_mods_dir(
    mods_dir: &Path,
    instance_loader: Option<LoaderKind>,
) -> (u32, Vec<PreflightIssue>) {
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return (0, Vec::new());
    };
    let mut jars: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
        .collect();
    jars.sort();
    jars.truncate(MAX_MOD_JARS);

    let mut parsed = Vec::with_capacity(jars.len());
    for jar in &jars {
        if let Some(info) = inspect_jar(jar) {
            parsed.push(info);
        }
    }
    let checked = u32::try_from(jars.len()).unwrap_or(u32::MAX);
    let issues = evaluate_mods(&parsed, instance_loader);
    (checked, issues)
}

fn evaluate_mods(mods: &[ParsedMod], instance_loader: Option<LoaderKind>) -> Vec<PreflightIssue> {
    let mut issues = Vec::new();
    let mut seen_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_present: HashSet<String> = HashSet::new();

    for m in mods {
        for id in &m.mod_ids {
            let key = id.to_ascii_lowercase();
            seen_ids
                .entry(key.clone())
                .or_default()
                .push(m.file_name.clone());
            all_present.insert(key);
        }
        for p in &m.provides {
            all_present.insert(p.to_ascii_lowercase());
        }
        for emb in &m.embedded_ids {
            all_present.insert(emb.to_ascii_lowercase());
        }
    }

    // Fabric API 在不同模组里可能写作 `fabric` 或 `fabric-api`
    if all_present.contains("fabric-api") || all_present.contains("fabric") {
        all_present.insert("fabric".to_owned());
        all_present.insert("fabric-api".to_owned());
    }
    if all_present.contains("quilted_fabric_api") {
        all_present.insert("fabric".to_owned());
        all_present.insert("fabric-api".to_owned());
    }

    let has_connector = all_present.contains("connector")
        || all_present.contains("connectormod")
        || all_present.contains("forgified_fabric_api");

    // 1. 重复 modId
    let mut duplicate_keys: Vec<_> = seen_ids
        .iter()
        .filter(|(id, files)| files.len() > 1 && !is_ignorable_dup_id(id))
        .collect();
    duplicate_keys.sort_by(|a, b| a.0.cmp(b.0));
    for (mod_id, files) in duplicate_keys {
        issues.push(PreflightIssue {
            kind: PreflightIssueKind::DuplicateModId,
            mod_name: Some(mod_id.clone()),
            target: mod_id.clone(),
            detail: files.join(" / "),
        });
    }

    // 2. 加载器错配（仅当实例有明确加载器，且未装 Sinytra Connector 兼容层）
    if let Some(loader) = instance_loader
        && !has_connector
    {
        for m in mods {
            if !m.ecos.is_empty() && !ecos_compatible(&m.ecos, loader) {
                let declared = m
                    .ecos
                    .iter()
                    .map(|e| e.label())
                    .collect::<Vec<_>>()
                    .join(" / ");
                issues.push(PreflightIssue {
                    kind: PreflightIssueKind::WrongLoader,
                    mod_name: Some(m.display_name.clone()),
                    target: declared,
                    detail: format!("{} ({})", m.file_name, loader.label()),
                });
            }
        }
    }

    // 3. 缺失必需前置
    let mut reported_missing: HashSet<(String, String)> = HashSet::new();
    for m in mods {
        for dep in &m.required_deps {
            let key = dep.to_ascii_lowercase();
            if is_builtin_dep(&key) || all_present.contains(&key) {
                continue;
            }
            if matches_fabric_submodule(&key, &all_present) {
                continue;
            }
            let dedup = (m.display_name.clone(), key.clone());
            if reported_missing.insert(dedup) {
                issues.push(PreflightIssue {
                    kind: PreflightIssueKind::MissingDependency,
                    mod_name: Some(m.display_name.clone()),
                    target: dep.clone(),
                    detail: m.file_name.clone(),
                });
            }
        }
    }

    issues
}

fn ecos_compatible(ecos: &[ModEco], loader: LoaderKind) -> bool {
    match loader {
        LoaderKind::Fabric => ecos.contains(&ModEco::Fabric),
        // Quilt 原生兼容大部分 Fabric 模组
        LoaderKind::Quilt => ecos.contains(&ModEco::Quilt) || ecos.contains(&ModEco::Fabric),
        LoaderKind::NeoForge => ecos.contains(&ModEco::NeoForge) || ecos.contains(&ModEco::Forge),
        LoaderKind::Forge => ecos.contains(&ModEco::Forge),
    }
}

fn is_ignorable_dup_id(id: &str) -> bool {
    matches!(
        id,
        "examplemod" | "minecraft" | "java" | "forge" | "neoforge"
    )
}

fn is_builtin_dep(id: &str) -> bool {
    matches!(
        id,
        "minecraft"
            | "java"
            | "forge"
            | "neoforge"
            | "fabricloader"
            | "fabric-loader"
            | "quilt_loader"
            | "fmlcore"
            | "lowcodeloader"
            | "mclanguage"
            | "javafml"
            | "mixinextras"
            | "optifine"
    )
}

/// Fabric API 打包了所有 `fabric-*` 子模块（如 `fabric-resource-loader-v0`）
fn matches_fabric_submodule(dep: &str, present: &HashSet<String>) -> bool {
    (dep.starts_with("fabric-") || dep.starts_with("quilted_fabric_"))
        && (present.contains("fabric-api") || present.contains("fabric"))
}

fn inspect_jar(path: &Path) -> Option<ParsedMod> {
    let file_name = path.file_name()?.to_str()?.to_owned();
    let handle = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(handle).ok()?;

    let mut read_entry = |name: &str| -> Option<Vec<u8>> {
        let mut entry = archive.by_name(name).ok()?;
        if entry.size() > MAX_META_BYTES {
            return None;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    };

    let fabric_raw = read_entry("fabric.mod.json");
    let quilt_raw = read_entry("quilt.mod.json");
    let neo_raw = read_entry("META-INF/neoforge.mods.toml");
    let forge_raw = read_entry("META-INF/mods.toml");
    let jarjar_raw = read_entry("META-INF/jarjar/metadata.json");

    if fabric_raw.is_none() && quilt_raw.is_none() && neo_raw.is_none() && forge_raw.is_none() {
        return None;
    }

    let mut ecos = Vec::new();
    let mut mod_ids = Vec::new();
    let mut display_name = None;
    let mut provides = Vec::new();
    let mut required_deps = Vec::new();
    let mut embedded_ids = Vec::new();

    if let Some(bytes) = fabric_raw
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        ecos.push(ModEco::Fabric);
        if let Some(id) = json["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            mod_ids.push(id.to_owned());
        }
        if display_name.is_none() {
            display_name = json["name"]
                .as_str()
                .or_else(|| json["id"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
        }
        if let Some(arr) = json["provides"].as_array() {
            for item in arr {
                if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    provides.push(s.to_owned());
                }
            }
        }
        if let Some(deps) = json["depends"].as_object() {
            for dep_id in deps.keys() {
                let dep_id = dep_id.trim();
                if !dep_id.is_empty() {
                    required_deps.push(dep_id.to_owned());
                }
            }
        }
        // Fabric JiJ 嵌套模组：读取嵌套 jar 的 fabric.mod.json 中的 id
        if let Some(jars) = json["jars"].as_array() {
            for item in jars {
                if let Some(nested) = item["file"].as_str() {
                    if let Some(id) = nested_jar_mod_id(&mut archive, nested) {
                        embedded_ids.push(id);
                    }
                    if let Some(stem) = jar_file_stem_id(nested) {
                        embedded_ids.push(stem);
                    }
                }
            }
        }
    }

    if let Some(bytes) = quilt_raw
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        ecos.push(ModEco::Quilt);
        if let Some(id) = json["quilt_loader"]["id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !mod_ids
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(id))
        {
            mod_ids.push(id.to_owned());
        }
        if display_name.is_none() {
            display_name = json["quilt_loader"]["metadata"]["name"]
                .as_str()
                .or_else(|| json["quilt_loader"]["id"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
        }
    }

    if let Some(bytes) = neo_raw {
        let text = String::from_utf8_lossy(&bytes);
        ecos.push(ModEco::NeoForge);
        parse_mods_toml(
            &text,
            &mut mod_ids,
            &mut display_name,
            &mut required_deps,
            true,
        );
    }

    if let Some(bytes) = forge_raw {
        let text = String::from_utf8_lossy(&bytes);
        ecos.push(ModEco::Forge);
        parse_mods_toml(
            &text,
            &mut mod_ids,
            &mut display_name,
            &mut required_deps,
            false,
        );
    }

    // NeoForge / Forge Jar-in-Jar 嵌套库
    if let Some(bytes) = jarjar_raw
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(jars) = json["jars"].as_array()
    {
        for item in jars {
            if let Some(artifact) = item["identifier"]["artifact"].as_str() {
                embedded_ids.push(artifact.to_owned());
            }
            if let Some(nested) = item["path"].as_str()
                && let Some(id) = nested_jar_mod_id(&mut archive, nested)
            {
                embedded_ids.push(id);
            }
        }
    }

    let display_name = display_name
        .or_else(|| mod_ids.first().cloned())
        .unwrap_or_else(|| file_name.clone());

    Some(ParsedMod {
        file_name,
        mod_ids,
        display_name,
        ecos,
        provides,
        required_deps,
        embedded_ids,
    })
}

fn nested_jar_mod_id(archive: &mut zip::ZipArchive<File>, nested_path: &str) -> Option<String> {
    let mut entry = archive.by_name(nested_path).ok()?;
    if entry.size() > 16 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).ok()?;
    let mut inner = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    if let Ok(mut f) = inner.by_name("fabric.mod.json") {
        let mut s = Vec::new();
        if f.read_to_end(&mut s).is_ok()
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&s)
            && let Some(id) = v["id"].as_str()
        {
            return Some(id.to_owned());
        }
    }
    for toml in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Ok(mut f) = inner.by_name(toml) {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                let mut ids = Vec::new();
                let mut name = None;
                let mut deps = Vec::new();
                parse_mods_toml(
                    &s,
                    &mut ids,
                    &mut name,
                    &mut deps,
                    toml.contains("neoforge"),
                );
                if let Some(id) = ids.into_iter().next() {
                    return Some(id);
                }
            }
        }
    }
    None
}

fn jar_file_stem_id(path: &str) -> Option<String> {
    let name = Path::new(path).file_stem()?.to_str()?;
    let prefix = name.split(['-', '_']).next()?.trim();
    (!prefix.is_empty()).then(|| prefix.to_owned())
}

/// 轻量解析 `mods.toml` / `neoforge.mods.toml`：提取 `[[mods]]` 与必需的 `[[dependencies.*]]`
fn parse_mods_toml(
    text: &str,
    mod_ids: &mut Vec<String>,
    display_name: &mut Option<String>,
    required_deps: &mut Vec<String>,
    is_neoforge: bool,
) {
    #[derive(PartialEq, Eq)]
    enum Section {
        None,
        Mod,
        Dep,
    }
    let mut section = Section::None;
    let mut dep_id: Option<String> = None;
    let mut dep_required: Option<bool> = None;
    let mut dep_side: Option<String> = None;

    let flush_dep = |dep_id: &mut Option<String>,
                     dep_required: &mut Option<bool>,
                     dep_side: &mut Option<String>,
                     out: &mut Vec<String>| {
        let is_client_relevant = dep_side
            .as_deref()
            .is_none_or(|s| !s.eq_ignore_ascii_case("SERVER"));
        if dep_required.unwrap_or(false)
            && is_client_relevant
            && let Some(id) = dep_id.take()
            && !id.is_empty()
            && !out
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&id))
        {
            out.push(id);
        }
        *dep_id = None;
        *dep_required = None;
        *dep_side = None;
    };

    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if section == Section::Dep {
                flush_dep(&mut dep_id, &mut dep_required, &mut dep_side, required_deps);
            }
            if line == "[[mods]]" {
                section = Section::Mod;
            } else if line.starts_with("[[dependencies.") && line.ends_with("]]") {
                section = Section::Dep;
            } else {
                section = Section::None;
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = unquote_toml(v.trim());
        match section {
            Section::Mod => {
                if key == "modId"
                    && !val.is_empty()
                    && !mod_ids
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(&val))
                {
                    mod_ids.push(val);
                } else if key == "displayName" && display_name.is_none() && !val.is_empty() {
                    *display_name = Some(val);
                }
            }
            Section::Dep => match key {
                "modId" => dep_id = Some(val),
                "mandatory" => dep_required = Some(val.eq_ignore_ascii_case("true")),
                "type" if is_neoforge => {
                    dep_required = Some(val.eq_ignore_ascii_case("required"));
                }
                "side" => dep_side = Some(val),
                _ => {}
            },
            Section::None => {}
        }
    }
    if section == Section::Dep {
        flush_dep(&mut dep_id, &mut dep_required, &mut dep_side, required_deps);
    }
}

fn unquote_toml(raw: &str) -> String {
    let s = raw.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return inner.to_owned();
    }
    if let Some(inner) = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return inner.to_owned();
    }
    s.to_owned()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn write_jar(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        for (name, bytes) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn detects_missing_dep_duplicate_and_wrong_loader() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();

        // NeoForge 模组缺少前置 balm
        write_jar(
            &mods.join("waystones.jar"),
            &[(
                "META-INF/neoforge.mods.toml",
                br#"
[[mods]]
modId="waystones"
displayName="Waystones"
[[dependencies.waystones]]
modId="balm"
type="required"
side="BOTH"
[[dependencies.waystones]]
modId="neoforge"
type="required"
"#,
            )],
        );
        // 重复的 waystones
        write_jar(
            &mods.join("waystones-old.jar"),
            &[(
                "META-INF/neoforge.mods.toml",
                br#"
[[mods]]
modId="waystones"
displayName="Waystones"
"#,
            )],
        );
        // 混进来的纯 Fabric 模组
        write_jar(
            &mods.join("sodium-fabric.jar"),
            &[(
                "fabric.mod.json",
                br#"{"schemaVersion":1,"id":"sodium","name":"Sodium","depends":{"fabricloader":">=0.15"}}"#,
            )],
        );
        // 停用的模组不参与检查
        write_jar(
            &mods.join("disabled.jar.disabled"),
            &[(
                "fabric.mod.json",
                br#"{"schemaVersion":1,"id":"waystones","name":"Disabled"}"#,
            )],
        );

        let (checked, issues) = check_mods_dir(mods, Some(LoaderKind::NeoForge));
        assert_eq!(checked, 3);
        assert!(
            issues
                .iter()
                .any(|i| i.kind == PreflightIssueKind::DuplicateModId && i.target == "waystones"),
            "应检出重复 modId：{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == PreflightIssueKind::MissingDependency && i.target == "balm"),
            "应检出缺失前置 balm：{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == PreflightIssueKind::WrongLoader && i.target == "Fabric"),
            "应检出加载器错配：{issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.target == "neoforge"),
            "内置运行时 neoforge 不应误报"
        );
    }

    #[test]
    fn clean_fabric_setup_with_fabric_api_has_no_issues() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();

        write_jar(
            &mods.join("fabric-api.jar"),
            &[(
                "fabric.mod.json",
                br#"{"schemaVersion":1,"id":"fabric-api","name":"Fabric API"}"#,
            )],
        );
        write_jar(
            &mods.join("cool-mod.jar"),
            &[(
                "fabric.mod.json",
                br#"{"schemaVersion":1,"id":"coolmod","name":"Cool Mod","depends":{"fabric":"*","fabric-resource-loader-v0":"*","minecraft":"~1.21"}}"#,
            )],
        );

        let (checked, issues) = check_mods_dir(mods, Some(LoaderKind::Fabric));
        assert_eq!(checked, 2);
        assert!(issues.is_empty(), "正常 Fabric 组合不应误报：{issues:?}");
    }

    #[test]
    fn sinytra_connector_skips_wrong_loader_warning() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();

        write_jar(
            &mods.join("connector.jar"),
            &[(
                "META-INF/neoforge.mods.toml",
                br#"[[mods]]
modId="connector"
displayName="Sinytra Connector"
"#,
            )],
        );
        write_jar(
            &mods.join("fabric-only.jar"),
            &[(
                "fabric.mod.json",
                br#"{"schemaVersion":1,"id":"fabonly","name":"FabOnly"}"#,
            )],
        );

        let (_, issues) = check_mods_dir(mods, Some(LoaderKind::NeoForge));
        assert!(
            !issues
                .iter()
                .any(|i| i.kind == PreflightIssueKind::WrongLoader),
            "装了 Sinytra Connector 不应误报加载器错配：{issues:?}"
        );
    }

    #[test]
    fn detects_java_too_old_against_version_json() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path().to_path_buf());
        let ver_dir = game.version_dir("1.21.1").unwrap();
        std::fs::create_dir_all(&ver_dir).unwrap();
        std::fs::write(
            ver_dir.join("1.21.1.json"),
            r#"{"id":"1.21.1","type":"release","mainClass":"net.minecraft.client.main.Main","javaVersion":{"component":"java-runtime-delta","majorVersion":21},"libraries":[]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(game.instance_dir("test").unwrap()).unwrap();
        let config = crate::instance::InstanceConfig::new("test", "1.21.1", None);
        let instance = crate::instance::register(&game, "test", &config).unwrap();

        let bad = check(&game, &instance, Some(17));
        assert!(
            bad.issues
                .iter()
                .any(|i| { i.kind == PreflightIssueKind::JavaTooOld && i.target == "Java 21" })
        );
        let ok = check(&game, &instance, Some(21));
        assert!(ok.issues.is_empty());
    }
}
