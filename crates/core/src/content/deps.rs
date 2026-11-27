//! 模组离线反向依赖图与孤儿前置识别引擎（V1.1-B2）。
//!
//! 结合两路信号零联网构建实例模组依赖图：
//! 1. **离线解析 `.jar` 清单**：支持 Fabric（`fabric.mod.json` 含 `provides` 与嵌套 `jars` 子模块）、
//!    Quilt（`quilt.mod.json`）、NeoForge（`META-INF/neoforge.mods.toml`）与 Forge（`META-INF/mods.toml`），
//!    结果按 `file_key` 缓存在 `.mcl-store/mod-deps.json`，二次扫描毫秒级完成；
//! 2. **启动器安装记录（`Lumi/content.json`）**：利用 `auto_dependency` 标记与安装时记录的 `dep_projects`，
//!    精准区分「自动拉入的前置库」与「玩家主动安装的模组」。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::identify::{IdentityCache, file_key};
use super::{ContentError, ContentKind, InstanceContent, display_name, record_key};
use crate::fs::{read_json, write_json_atomic};

const DEPS_CACHE_FILE: &str = "mod-deps.json";
/// 解析规则升级时递增，自动失效旧缓存并重扫 JAR。
const DEPS_CACHE_VERSION: u32 = 2;
const MAX_CACHE_ENTRIES: usize = 2000;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_NESTED_JAR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NESTED_JARS: usize = 80;

/// 依赖关系引用的一个本地模组。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DepRef {
    pub file_name: String,
    pub title: String,
    pub enabled: bool,
}

/// 单个模组在实例中的正/反向依赖信息。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ModDepInfo {
    /// 哪些本地模组依赖它
    pub required_by: Vec<DepRef>,
    /// 它依赖哪些本地模组
    pub requires: Vec<DepRef>,
    /// 是否由启动器作为依赖自动安装
    pub auto_dependency: bool,
}

/// 整个实例的模组反向依赖图（键为启用态文件名 `file_name`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ModDepsGraph {
    pub items: BTreeMap<String, ModDepInfo>,
}

/// 即将对模组执行的变更类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ImpactAction {
    Disable,
    Remove,
}

/// 停用或卸载某个模组时，仍被其他已启用模组依赖的告警项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DependentWarning {
    pub target_file: String,
    pub target_title: String,
    pub dependents: Vec<DepRef>,
}

/// 卸载目标模组后失去全部依赖方的孤儿前置候选。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct OrphanDep {
    pub file_name: String,
    pub title: String,
    pub version: Option<String>,
    pub size: f64,
    /// 为 true 表示当初由启动器作为前置自动安装，弹窗默认勾选清理
    pub auto_dependency: bool,
    /// 原本依赖它的被删模组名列表（便于向用户说明为什么它变成孤儿）
    pub freed_by: Vec<String>,
}

/// 停用或卸载模组前的依赖影响分析报告。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ModImpactReport {
    pub dependent_warnings: Vec<DependentWarning>,
    pub orphans: Vec<OrphanDep>,
}

/// 单个 `.jar` 解析出的规范化模组 ID、必选依赖 ID 与官方库声明（全小写、去重、已滤除内置运行时）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JarDepMeta {
    pub provided_ids: Vec<String>,
    pub required_ids: Vec<String>,
    /// 模组清单明确声明为库（Fabric `modmenu.badges: ["library"]` 或 Forge/NeoForge `FMLModType: LIBRARY/GAMELIBRARY`）
    #[serde(default)]
    pub is_library: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DepsCache {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, JarDepMeta>,
}

impl Default for DepsCache {
    fn default() -> Self {
        Self {
            version: DEPS_CACHE_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl DepsCache {
    fn load(store_root: &Path) -> Self {
        match read_json::<Self>(&store_root.join(DEPS_CACHE_FILE)) {
            Ok(Some(cache)) if cache.version == DEPS_CACHE_VERSION => cache,
            Ok(Some(_)) => Self::default(),
            Ok(None) => Self::default(),
            Err(error) => {
                tracing::warn!(%error, "读取模组依赖缓存失败，重新扫描");
                Self::default()
            }
        }
    }

    fn save(&mut self, store_root: &Path) {
        self.version = DEPS_CACHE_VERSION;
        if self.entries.len() > MAX_CACHE_ENTRIES {
            let excess = self.entries.len() - MAX_CACHE_ENTRIES;
            let keys: Vec<String> = self.entries.keys().take(excess).cloned().collect();
            for k in keys {
                self.entries.remove(&k);
            }
        }
        if let Err(error) = write_json_atomic(&store_root.join(DEPS_CACHE_FILE), self) {
            tracing::warn!(%error, "保存模组依赖缓存失败");
        }
    }
}

fn normalize_mod_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() || trimmed == "*" || is_builtin_mod_id(&trimmed) {
        return None;
    }
    Some(trimmed)
}

/// 过滤 Minecraft / 加载器 / Java 运行时内置 ID，避免把它们当成外部模组依赖。
fn is_builtin_mod_id(id: &str) -> bool {
    matches!(
        id,
        "minecraft"
            | "java"
            | "forge"
            | "neoforge"
            | "fml"
            | "javafml"
            | "lowcodefml"
            | "fabricloader"
            | "fabric-loader"
            | "quilt_loader"
            | "quilt_base"
            | "mixinextras"
    )
}

fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Option<Vec<u8>> {
    let entry = archive.by_name(name).ok()?;
    if entry.size() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    // 对实际解压流再加一层上限，防止恶意或损坏 ZIP 伪造 entry.size() 耗尽内存
    entry
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > limit {
        return None;
    }
    Some(bytes)
}

/// 离线解析单个 `.jar` 文件提供的模组 ID、必选依赖 ID 与官方库标记。
/// 读不了（损坏或非 ZIP）返回空元数据，不报错。
pub(crate) fn read_jar_deps(file: &Path) -> JarDepMeta {
    let Ok(handle) = std::fs::File::open(file) else {
        return JarDepMeta::default();
    };
    let Ok(mut archive) = zip::ZipArchive::new(handle) else {
        return JarDepMeta::default();
    };

    let mut provided = BTreeSet::new();
    let mut required = BTreeSet::new();
    let mut is_library = false;

    // 0. JAR Manifest (`META-INF/MANIFEST.MF`)：Forge / NeoForge 库声明 `FMLModType: LIBRARY / GAMELIBRARY`
    if let Some(bytes) = read_zip_entry(&mut archive, "META-INF/MANIFEST.MF", MAX_MANIFEST_BYTES) {
        let text = String::from_utf8_lossy(&bytes);
        if manifest_declares_library(&text) {
            is_library = true;
        }
    }

    // 1. NeoForge / Forge (`META-INF/neoforge.mods.toml`, `META-INF/mods.toml`) + Jar-in-Jar
    for (toml_path, is_neoforge) in [
        ("META-INF/neoforge.mods.toml", true),
        ("META-INF/mods.toml", false),
    ] {
        if let Some(bytes) = read_zip_entry(&mut archive, toml_path, MAX_MANIFEST_BYTES) {
            let text = String::from_utf8_lossy(&bytes);
            parse_mods_toml(&text, is_neoforge, &mut provided, &mut required);
            break;
        }
    }
    if let Some(bytes) = read_zip_entry(
        &mut archive,
        "META-INF/jarjar/metadata.json",
        MAX_MANIFEST_BYTES,
    ) && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(jars) = json["jars"].as_array()
    {
        for entry in jars.iter().take(MAX_NESTED_JARS) {
            let Some(path) = entry["path"].as_str() else {
                continue;
            };
            let clean = path.trim_start_matches('/');
            if let Some(jar_bytes) = read_zip_entry(&mut archive, clean, MAX_NESTED_JAR_BYTES)
                && let Ok(mut inner) = zip::ZipArchive::new(std::io::Cursor::new(jar_bytes))
            {
                collect_nested_forge_provides(&mut inner, &mut provided);
            }
        }
    }

    // 2. Fabric (`fabric.mod.json` + 嵌套 `META-INF/jars/*.jar` 子模块 ID，例如 Fabric API)
    if let Some(bytes) = read_zip_entry(&mut archive, "fabric.mod.json", MAX_MANIFEST_BYTES)
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        let (nested_paths, fabric_lib) = parse_fabric_json(&json, &mut provided, &mut required);
        if fabric_lib {
            is_library = true;
        }
        for nested in nested_paths.into_iter().take(MAX_NESTED_JARS) {
            let clean = nested.trim_start_matches('/');
            if let Some(jar_bytes) = read_zip_entry(&mut archive, clean, MAX_NESTED_JAR_BYTES)
                && let Ok(mut inner) = zip::ZipArchive::new(std::io::Cursor::new(jar_bytes))
                && let Some(inner_json_bytes) =
                    read_zip_entry(&mut inner, "fabric.mod.json", MAX_MANIFEST_BYTES)
                && let Ok(inner_json) =
                    serde_json::from_slice::<serde_json::Value>(&inner_json_bytes)
            {
                collect_fabric_provides(&inner_json, &mut provided);
            }
        }
    }

    // 3. Quilt (`quilt.mod.json` + 嵌套 `jars`)
    if let Some(bytes) = read_zip_entry(&mut archive, "quilt.mod.json", MAX_MANIFEST_BYTES)
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        let (nested_paths, quilt_lib) = parse_quilt_json(&json, &mut provided, &mut required);
        if quilt_lib {
            is_library = true;
        }
        for nested in nested_paths.into_iter().take(MAX_NESTED_JARS) {
            let clean = nested.trim_start_matches('/');
            if let Some(jar_bytes) = read_zip_entry(&mut archive, clean, MAX_NESTED_JAR_BYTES)
                && let Ok(mut inner) = zip::ZipArchive::new(std::io::Cursor::new(jar_bytes))
            {
                if let Some(inner_bytes) =
                    read_zip_entry(&mut inner, "quilt.mod.json", MAX_MANIFEST_BYTES)
                    && let Ok(inner_json) =
                        serde_json::from_slice::<serde_json::Value>(&inner_bytes)
                {
                    collect_quilt_provides(&inner_json, &mut provided);
                } else if let Some(inner_bytes) =
                    read_zip_entry(&mut inner, "fabric.mod.json", MAX_MANIFEST_BYTES)
                    && let Ok(inner_json) =
                        serde_json::from_slice::<serde_json::Value>(&inner_bytes)
                {
                    collect_fabric_provides(&inner_json, &mut provided);
                }
            }
        }
    }

    // 移除自依赖（例如同一个多模块 JAR 内部声明的相互依赖）
    for own in &provided {
        required.remove(own);
    }

    JarDepMeta {
        provided_ids: provided.into_iter().collect(),
        required_ids: required.into_iter().collect(),
        is_library,
    }
}

fn manifest_declares_library(text: &str) -> bool {
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("FMLModType") {
            let kind = val.trim().to_ascii_uppercase();
            if kind == "LIBRARY" || kind == "GAMELIBRARY" {
                return true;
            }
        }
    }
    false
}

fn collect_nested_forge_provides<R: Read + std::io::Seek>(
    inner: &mut zip::ZipArchive<R>,
    provided: &mut BTreeSet<String>,
) {
    let mut ignored_req = BTreeSet::new();
    for (toml_path, is_neoforge) in [
        ("META-INF/neoforge.mods.toml", true),
        ("META-INF/mods.toml", false),
    ] {
        if let Some(bytes) = read_zip_entry(inner, toml_path, MAX_MANIFEST_BYTES) {
            let text = String::from_utf8_lossy(&bytes);
            parse_mods_toml(&text, is_neoforge, provided, &mut ignored_req);
            break;
        }
    }
}

fn badges_has_library(badges: &serde_json::Value) -> bool {
    badges.as_array().is_some_and(|list| {
        list.iter()
            .filter_map(|v| v.as_str())
            .any(|b| b.trim().eq_ignore_ascii_case("library"))
    })
}

fn collect_fabric_provides(json: &serde_json::Value, provided: &mut BTreeSet<String>) {
    if let Some(id) = json["id"].as_str().and_then(normalize_mod_id) {
        provided.insert(id);
    }
    if let Some(list) = json["provides"].as_array() {
        for item in list {
            if let Some(id) = item.as_str().and_then(normalize_mod_id) {
                provided.insert(id);
            }
        }
    }
}

fn parse_fabric_json(
    json: &serde_json::Value,
    provided: &mut BTreeSet<String>,
    required: &mut BTreeSet<String>,
) -> (Vec<String>, bool) {
    collect_fabric_provides(json, provided);
    let is_library = badges_has_library(&json["custom"]["modmenu"]["badges"]);
    if let Some(deps) = json["depends"].as_object() {
        for key in deps.keys() {
            if let Some(id) = normalize_mod_id(key) {
                required.insert(id);
            }
        }
    }
    let mut nested = Vec::new();
    if let Some(jars) = json["jars"].as_array() {
        for entry in jars {
            if let Some(file) = entry["file"].as_str() {
                nested.push(file.to_owned());
            }
        }
    }
    (nested, is_library)
}

fn collect_quilt_provides(json: &serde_json::Value, provided: &mut BTreeSet<String>) {
    let loader = &json["quilt_loader"];
    if let Some(id) = loader["id"].as_str().and_then(normalize_mod_id) {
        provided.insert(id);
    }
    if let Some(provides) = loader["provides"].as_array() {
        for entry in provides {
            let raw = entry.as_str().or_else(|| entry["id"].as_str());
            if let Some(id) = raw.and_then(normalize_mod_id) {
                provided.insert(id);
            }
        }
    }
}

fn parse_quilt_json(
    json: &serde_json::Value,
    provided: &mut BTreeSet<String>,
    required: &mut BTreeSet<String>,
) -> (Vec<String>, bool) {
    collect_quilt_provides(json, provided);
    let loader = &json["quilt_loader"];
    let is_library = badges_has_library(&json["modmenu"]["badges"])
        || badges_has_library(&loader["metadata"]["modmenu"]["badges"]);
    if let Some(depends) = loader["depends"].as_array() {
        for entry in depends {
            if let Some(id) = entry.as_str().and_then(normalize_mod_id) {
                required.insert(id);
                continue;
            }
            if entry["optional"].as_bool() == Some(true) {
                continue;
            }
            if let Some(id) = entry["id"].as_str().and_then(normalize_mod_id) {
                required.insert(id);
            }
        }
    }
    let mut nested = Vec::new();
    if let Some(jars) = loader["jars"].as_array() {
        for entry in jars {
            if let Some(path) = entry.as_str() {
                nested.push(path.to_owned());
            }
        }
    }
    (nested, is_library)
}

/// 解析 `neoforge.mods.toml` / `mods.toml` 中的全部 `[[mods]]` 与 `[[dependencies.*]]` 表。
fn parse_mods_toml(
    text: &str,
    is_neoforge: bool,
    provided: &mut BTreeSet<String>,
    required: &mut BTreeSet<String>,
) {
    enum Section {
        None,
        Mod,
        Dep {
            mod_id: Option<String>,
            mandatory: Option<bool>,
            dep_type: Option<String>,
            side: Option<String>,
        },
    }

    let flush_dep = |sec: &Section, required: &mut BTreeSet<String>| {
        if let Section::Dep {
            mod_id: Some(raw_id),
            mandatory,
            dep_type,
            side,
        } = sec
        {
            // 仅服务端依赖在客户端启动器中跳过
            if side
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("server"))
            {
                return;
            }
            let is_required = if let Some(kind) = dep_type.as_deref() {
                kind.eq_ignore_ascii_case("required")
            } else if let Some(flag) = mandatory {
                *flag
            } else {
                // NeoForge 20.5+ 规范：省略 type 与 mandatory 时默认为 required
                is_neoforge
            };
            if is_required && let Some(id) = normalize_mod_id(raw_id) {
                required.insert(id);
            }
        }
    };

    let mut section = Section::None;
    for raw_line in text.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            flush_dep(&section, required);
            let compact = line.replace(' ', "");
            if compact == "[[mods]]" {
                section = Section::Mod;
            } else if compact.starts_with("[[dependencies.") && compact.ends_with("]]") {
                section = Section::Dep {
                    mod_id: None,
                    mandatory: None,
                    dep_type: None,
                    side: None,
                };
            } else {
                section = Section::None;
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        match &mut section {
            Section::Mod => {
                if key == "modId"
                    && let Some(id) = unquote_toml(val).and_then(normalize_mod_id)
                {
                    provided.insert(id);
                }
            }
            Section::Dep {
                mod_id,
                mandatory,
                dep_type,
                side,
            } => match key {
                "modId" => {
                    if let Some(s) = unquote_toml(val) {
                        *mod_id = Some(s.to_owned());
                    }
                }
                "mandatory" => {
                    if val.eq_ignore_ascii_case("true") {
                        *mandatory = Some(true);
                    } else if val.eq_ignore_ascii_case("false") {
                        *mandatory = Some(false);
                    }
                }
                "type" => {
                    if let Some(s) = unquote_toml(val) {
                        *dep_type = Some(s.to_owned());
                    }
                }
                "side" => {
                    if let Some(s) = unquote_toml(val) {
                        *side = Some(s.to_owned());
                    }
                }
                _ => {}
            },
            Section::None => {}
        }
    }
    flush_dep(&section, required);
}

/// 去掉行尾 `# 注释`（忽略引号内的 `#`）。
fn strip_toml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_double => escaped = true,
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn unquote_toml(value: &str) -> Option<&str> {
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    if value.starts_with("'''") || value.starts_with("\"\"\"") {
        return None;
    }
    let rest = &value[1..];
    let end = rest.find(quote)?;
    Some(&rest[..end])
}

struct ModNode {
    file_name: String,
    title: String,
    version: Option<String>,
    size: u64,
    enabled: bool,
    locked: bool,
    project_id: Option<String>,
    auto_dependency: bool,
    dep_projects: Vec<String>,
    jar_meta: JarDepMeta,
}

struct ResolvedGraph {
    nodes: Vec<ModNode>,
    /// `by_file[lowercase(file_name)] -> index`
    by_file: HashMap<String, usize>,
    /// `requires[i]` = 模组 `i` 直接依赖的本地模组下标集合
    requires: Vec<BTreeSet<usize>>,
    /// `required_by[i]` = 直接依赖模组 `i` 的本地模组下标集合
    required_by: Vec<BTreeSet<usize>>,
}

impl InstanceContent {
    /// 扫描实例 `mods/` 构建离线反向依赖图（带持久化缓存，毫秒级返回）。
    fn build_mod_graph(&self) -> Result<ResolvedGraph, ContentError> {
        let records = self.load_records();
        let locks = self.load_locks();
        let id_cache = IdentityCache::load(self.store.root());
        let mut deps_cache = DepsCache::load(self.store.root());
        let mut cache_dirty = false;

        let scanned = self.scan(ContentKind::Mod)?;
        let mut nodes = Vec::with_capacity(scanned.len());

        for s in scanned {
            if !s.metadata.is_file() {
                continue;
            }
            let rkey = record_key(ContentKind::Mod, &s.file_name);
            let record = records.get(&rkey);
            let id_fkey = file_key(ContentKind::Mod.folder(), &s.disk_name, &s.metadata);
            let dep_fkey = file_key(ContentKind::Mod.folder(), &s.file_name, &s.metadata);
            let sha1 = id_cache.hash_of(&id_fkey);
            let identity = sha1.and_then(|h| id_cache.identity(h));

            let jar_meta = if let Some(cached) = deps_cache.entries.get(&dep_fkey) {
                cached.clone()
            } else {
                let parsed = read_jar_deps(&self.folder(ContentKind::Mod).join(&s.disk_name));
                deps_cache.entries.insert(dep_fkey, parsed.clone());
                cache_dirty = true;
                parsed
            };

            let title = record
                .map(|r| r.title.clone())
                .or_else(|| identity.map(|i| i.title.clone()))
                .unwrap_or_else(|| display_name(&s.file_name));
            let version = record
                .map(|r| r.version_number.clone())
                .or_else(|| identity.and_then(|i| i.version.clone()));
            let project_id = record
                .map(|r| r.project_id.clone())
                .or_else(|| identity.and_then(|i| i.project_id.clone()));

            nodes.push(ModNode {
                file_name: s.file_name,
                title,
                version,
                size: s.metadata.len(),
                enabled: !s.disabled,
                locked: locks.contains(&rkey),
                project_id,
                auto_dependency: record.is_some_and(|r| r.auto_dependency),
                dep_projects: record.map_or_else(Vec::new, |r| r.dep_projects.clone()),
                jar_meta,
            });
        }

        if cache_dirty {
            deps_cache.save(self.store.root());
        }

        let mut by_file = HashMap::with_capacity(nodes.len());
        let mut by_mod_id: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_project_id: HashMap<String, Vec<usize>> = HashMap::new();

        for (idx, node) in nodes.iter().enumerate() {
            by_file.insert(node.file_name.to_ascii_lowercase(), idx);
            for mid in &node.jar_meta.provided_ids {
                by_mod_id.entry(mid.clone()).or_default().push(idx);
            }
            if let Some(pid) = &node.project_id {
                by_project_id
                    .entry(pid.to_ascii_lowercase())
                    .or_default()
                    .push(idx);
            }
        }

        let mut requires = vec![BTreeSet::new(); nodes.len()];
        let mut required_by = vec![BTreeSet::new(); nodes.len()];

        for (idx, node) in nodes.iter().enumerate() {
            // 信号 1：JAR 清单声明的必选 modId
            for req_id in &node.jar_meta.required_ids {
                if let Some(providers) = by_mod_id.get(req_id) {
                    for &dep_idx in providers {
                        if dep_idx != idx {
                            requires[idx].insert(dep_idx);
                            required_by[dep_idx].insert(idx);
                        }
                    }
                }
            }
            // 信号 2：通过启动器安装时记录的依赖项目 ID
            for dep_pid in &node.dep_projects {
                if let Some(providers) = by_project_id.get(&dep_pid.to_ascii_lowercase()) {
                    for &dep_idx in providers {
                        if dep_idx != idx {
                            requires[idx].insert(dep_idx);
                            required_by[dep_idx].insert(idx);
                        }
                    }
                }
            }
        }

        Ok(ResolvedGraph {
            nodes,
            by_file,
            requires,
            required_by,
        })
    }

    /// 返回实例全部模组的反向依赖图（供内容列表展示「被 XX 依赖」与「孤儿前置」标记）。
    pub fn mod_deps(&self) -> Result<ModDepsGraph, ContentError> {
        let graph = self.build_mod_graph()?;
        let mut items = BTreeMap::new();
        for (idx, node) in graph.nodes.iter().enumerate() {
            let mut req_by: Vec<DepRef> = graph.required_by[idx]
                .iter()
                .map(|&i| DepRef {
                    file_name: graph.nodes[i].file_name.clone(),
                    title: graph.nodes[i].title.clone(),
                    enabled: graph.nodes[i].enabled,
                })
                .collect();
            req_by.sort_by_key(|d| (!d.enabled, d.title.to_lowercase()));

            let mut reqs: Vec<DepRef> = graph.requires[idx]
                .iter()
                .map(|&i| DepRef {
                    file_name: graph.nodes[i].file_name.clone(),
                    title: graph.nodes[i].title.clone(),
                    enabled: graph.nodes[i].enabled,
                })
                .collect();
            reqs.sort_by_key(|d| d.title.to_lowercase());

            items.insert(
                node.file_name.clone(),
                ModDepInfo {
                    required_by: req_by,
                    requires: reqs,
                    auto_dependency: node.auto_dependency,
                },
            );
        }
        Ok(ModDepsGraph { items })
    }

    /// 分析停用或卸载指定模组集合的影响：
    /// 1. 仍被其他已启用模组依赖时产出 `dependent_warnings`；
    /// 2. 卸载时采用不动点传播找出失去全部依赖方的孤儿前置 `orphans`（锁定版本的模组绝不判为孤儿）。
    pub fn analyze_mod_impact(
        &self,
        file_names: &[String],
        action: ImpactAction,
    ) -> Result<ModImpactReport, ContentError> {
        let graph = self.build_mod_graph()?;
        let target_indices: BTreeSet<usize> = file_names
            .iter()
            .filter_map(|name| graph.by_file.get(&name.to_ascii_lowercase()).copied())
            .collect();
        if target_indices.is_empty() {
            return Ok(ModImpactReport::default());
        }

        // 1. 被依赖预警：检查是否仍有处于启用状态、且不在本次目标集合里的模组依赖它
        let mut dependent_warnings = Vec::new();
        for &idx in &target_indices {
            let mut dependents: Vec<DepRef> = graph.required_by[idx]
                .iter()
                .filter(|&&dep_of| !target_indices.contains(&dep_of) && graph.nodes[dep_of].enabled)
                .map(|&dep_of| DepRef {
                    file_name: graph.nodes[dep_of].file_name.clone(),
                    title: graph.nodes[dep_of].title.clone(),
                    enabled: graph.nodes[dep_of].enabled,
                })
                .collect();
            if !dependents.is_empty() {
                dependents.sort_by_key(|d| d.title.to_lowercase());
                dependent_warnings.push(DependentWarning {
                    target_file: graph.nodes[idx].file_name.clone(),
                    target_title: graph.nodes[idx].title.clone(),
                    dependents,
                });
            }
        }
        dependent_warnings.sort_by_key(|w| w.target_title.to_lowercase());

        // 2. 孤儿前置识别（仅卸载时计算，支持 A → B → C 传递孤儿）
        let mut orphans = Vec::new();
        if action == ImpactAction::Remove {
            let mut gone = target_indices.clone();
            let mut orphan_indices = BTreeSet::new();
            loop {
                let mut added_any = false;
                for idx in 0..graph.nodes.len() {
                    let candidate = &graph.nodes[idx];
                    if gone.contains(&idx) || candidate.locked {
                        continue;
                    }
                    // 仅对「启动器自动拉入的前置」或「清单明确声明为库」的模组做孤儿判定，
                    // 避免卸载附属模组（如 Iris / Create 附属）时误把主模组（Sodium / Create）判成孤儿。
                    if !candidate.auto_dependency && !candidate.jar_meta.is_library {
                        continue;
                    }
                    let deps = &graph.required_by[idx];
                    if deps.is_empty() {
                        continue;
                    }
                    // 至少被一个将要移除的模组依赖，且没有任何留存模组再依赖它
                    let lost_all = deps.iter().all(|d| gone.contains(d));
                    if lost_all {
                        gone.insert(idx);
                        orphan_indices.insert(idx);
                        added_any = true;
                    }
                }
                if !added_any {
                    break;
                }
            }

            for idx in orphan_indices {
                let node = &graph.nodes[idx];
                let mut freed_by: Vec<String> = graph.required_by[idx]
                    .iter()
                    .map(|&i| graph.nodes[i].title.clone())
                    .collect();
                freed_by.sort_by_key(|t| t.to_lowercase());
                freed_by.dedup();
                orphans.push(OrphanDep {
                    file_name: node.file_name.clone(),
                    title: node.title.clone(),
                    version: node.version.clone(),
                    size: node.size as f64,
                    auto_dependency: node.auto_dependency,
                    freed_by,
                });
            }
            // 自动前置排在前面，方便一眼确认
            orphans.sort_by_key(|o| (!o.auto_dependency, o.title.to_lowercase()));
        }

        Ok(ModImpactReport {
            dependent_warnings,
            orphans,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::content::store::STORE_DIR;
    use crate::content::{ContentSource, Record};

    fn make_jar(path: &Path, entries: &[(&str, &[u8])]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn make_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn parses_fabric_neoforge_forge_and_quilt_manifests() {
        let dir = tempfile::tempdir().unwrap();

        // 1. Fabric + 嵌套子模块（模拟 Fabric API）
        let nested_sub = make_zip_bytes(&[(
            "fabric.mod.json",
            br#"{"schemaVersion":1,"id":"fabric-resource-loader-v0","version":"1.0"}"#,
        )]);
        let fabric_api = dir.path().join("fabric-api.jar");
        make_jar(
            &fabric_api,
            &[
                (
                    "fabric.mod.json",
                    br#"{"id":"fabric-api","provides":["fabric"],"jars":[{"file":"META-INF/jars/sub.jar"}]}"#,
                ),
                ("META-INF/jars/sub.jar", &nested_sub),
            ],
        );
        let meta = read_jar_deps(&fabric_api);
        assert!(meta.provided_ids.contains(&"fabric-api".to_owned()));
        assert!(meta.provided_ids.contains(&"fabric".to_owned()));
        assert!(
            meta.provided_ids
                .contains(&"fabric-resource-loader-v0".to_owned())
        );

        // 2. NeoForge mods.toml（区分 required 与 optional，过滤 minecraft/neoforge）
        let nf_mod = dir.path().join("waystones-nf.jar");
        make_jar(
            &nf_mod,
            &[(
                "META-INF/neoforge.mods.toml",
                br#"
[[mods]]
modId="waystones"

[[dependencies.waystones]]
modId="neoforge"
type="required"

[[dependencies.waystones]]
modId="balm"
# NeoForge defaults to required when type is omitted

[[dependencies.waystones]]
modId="jei"
type="optional"
"#,
            )],
        );
        let nf_meta = read_jar_deps(&nf_mod);
        assert_eq!(nf_meta.provided_ids, vec!["waystones"]);
        assert_eq!(nf_meta.required_ids, vec!["balm"]);

        // 3. Forge mods.toml（mandatory=true vs false）
        let forge_mod = dir.path().join("forge-mod.jar");
        make_jar(
            &forge_mod,
            &[(
                "META-INF/mods.toml",
                br#"
[[mods]]
modId="create"

[[dependencies.create]]
modId="forge"
mandatory=true

[[dependencies.create]]
modId="flywheel"
mandatory=true

[[dependencies.create]]
modId="jei"
mandatory=false
"#,
            )],
        );
        let forge_meta = read_jar_deps(&forge_mod);
        assert_eq!(forge_meta.provided_ids, vec!["create"]);
        assert_eq!(forge_meta.required_ids, vec!["flywheel"]);

        // 4. Quilt quilt.mod.json
        let quilt_mod = dir.path().join("quilt-mod.jar");
        make_jar(
            &quilt_mod,
            &[(
                "quilt.mod.json",
                br#"{"quilt_loader":{"id":"qmod","provides":[{"id":"qmod_alias"}],"depends":["qsl",{"id":"opt_lib","optional":true},{"id":"minecraft"}]}}"#,
            )],
        );
        let quilt_meta = read_jar_deps(&quilt_mod);
        assert!(quilt_meta.provided_ids.contains(&"qmod".to_owned()));
        assert!(quilt_meta.provided_ids.contains(&"qmod_alias".to_owned()));
        assert_eq!(quilt_meta.required_ids, vec!["qsl"]);
    }

    #[test]
    fn reverse_deps_orphan_detection_and_shared_dependency_protection() {
        let dir = tempfile::tempdir().unwrap();
        let content = InstanceContent::new(dir.path(), dir.path().join(STORE_DIR));
        let mods = dir.path().join("mods");

        // Waystones → Balm → CoreLib（传递依赖）
        make_jar(
            &mods.join("corelib.jar"),
            &[("fabric.mod.json", br#"{"id":"corelib","version":"1.0"}"#)],
        );
        make_jar(
            &mods.join("balm.jar"),
            &[(
                "fabric.mod.json",
                br#"{"id":"balm","depends":{"corelib":"*","minecraft":"*"}}"#,
            )],
        );
        make_jar(
            &mods.join("waystones.jar"),
            &[(
                "fabric.mod.json",
                br#"{"id":"waystones","depends":{"balm":">=21.0"}}"#,
            )],
        );

        // 写入启动器记录，标记 balm 和 corelib 为自动前置
        let mut records = BTreeMap::new();
        records.insert(
            record_key(ContentKind::Mod, "waystones.jar"),
            Record {
                project_id: "waystones-id".to_owned(),
                version_id: "v1".to_owned(),
                title: "Waystones".to_owned(),
                version_number: "21.1.0".to_owned(),
                icon_url: None,
                source: ContentSource::Modrinth,
                auto_dependency: false,
                dep_projects: vec!["balm-id".to_owned()],
            },
        );
        records.insert(
            record_key(ContentKind::Mod, "balm.jar"),
            Record {
                project_id: "balm-id".to_owned(),
                version_id: "v1".to_owned(),
                title: "Balm".to_owned(),
                version_number: "21.0.9".to_owned(),
                icon_url: None,
                source: ContentSource::Modrinth,
                auto_dependency: true,
                dep_projects: vec!["corelib-id".to_owned()],
            },
        );
        records.insert(
            record_key(ContentKind::Mod, "corelib.jar"),
            Record {
                project_id: "corelib-id".to_owned(),
                version_id: "v1".to_owned(),
                title: "CoreLib".to_owned(),
                version_number: "1.0.0".to_owned(),
                icon_url: None,
                source: ContentSource::Modrinth,
                auto_dependency: true,
                dep_projects: Vec::new(),
            },
        );
        std::fs::create_dir_all(dir.path().join("Lumi")).unwrap();
        write_json_atomic(&dir.path().join("Lumi/content.json"), &records).unwrap();

        // 1. 反向依赖图：Balm 应显示被 Waystones 依赖
        let graph = content.mod_deps().unwrap();
        let balm_info = graph.items.get("balm.jar").unwrap();
        assert!(balm_info.auto_dependency);
        assert_eq!(balm_info.required_by.len(), 1);
        assert_eq!(balm_info.required_by[0].title, "Waystones");

        // 2. 尝试停用/卸载 Balm：应告警正被已启用的 Waystones 依赖
        let disable_balm = content
            .analyze_mod_impact(&["balm.jar".to_owned()], ImpactAction::Disable)
            .unwrap();
        assert_eq!(disable_balm.dependent_warnings.len(), 1);
        assert_eq!(
            disable_balm.dependent_warnings[0].dependents[0].title,
            "Waystones"
        );
        assert!(disable_balm.orphans.is_empty(), "停用不触发孤儿卸载清理");

        // 3. 卸载 Waystones：Balm 与传递前置 CoreLib 均失去全部依赖方，应被识别为孤儿
        let remove_ws = content
            .analyze_mod_impact(&["waystones.jar".to_owned()], ImpactAction::Remove)
            .unwrap();
        assert!(remove_ws.dependent_warnings.is_empty());
        let orphan_files: BTreeSet<&str> = remove_ws
            .orphans
            .iter()
            .map(|o| o.file_name.as_str())
            .collect();
        assert!(orphan_files.contains("balm.jar"));
        assert!(orphan_files.contains("corelib.jar"));

        // 4. 锁定 corelib.jar 版本后，不再把它判为孤儿
        content
            .set_locked(ContentKind::Mod, "corelib.jar", true)
            .unwrap();
        let remove_ws_locked = content
            .analyze_mod_impact(&["waystones.jar".to_owned()], ImpactAction::Remove)
            .unwrap();
        assert!(
            !remove_ws_locked
                .orphans
                .iter()
                .any(|o| o.file_name == "corelib.jar")
        );

        // 5. 再加入第二个也依赖 Balm 的模组（Crafting Tweaks）→ 卸载 Waystones 时绝不把 Balm 判为孤儿！
        make_jar(
            &mods.join("craftingtweaks.jar"),
            &[(
                "fabric.mod.json",
                br#"{"id":"craftingtweaks","depends":{"balm":">=21.0"}}"#,
            )],
        );
        let remove_ws_shared = content
            .analyze_mod_impact(&["waystones.jar".to_owned()], ImpactAction::Remove)
            .unwrap();
        assert!(
            remove_ws_shared.orphans.is_empty(),
            "存在第二个依赖 Balm 的模组时，绝不能把 Balm 判为孤儿"
        );
    }

    #[test]
    fn jarjar_server_side_library_detection_and_no_false_orphan_on_main_mod() {
        let dir = tempfile::tempdir().unwrap();
        let content = InstanceContent::new(dir.path(), dir.path().join(STORE_DIR));
        let mods = dir.path().join("mods");

        // 1. NeoForge 主模组 Create 内嵌 flywheel（Jar-in-Jar），且声明一条仅服务端依赖
        let flywheel_inner = make_zip_bytes(&[(
            "META-INF/neoforge.mods.toml",
            br#"
[[mods]]
modId="flywheel"
"#,
        )]);
        make_jar(
            &mods.join("create.jar"),
            &[
                (
                    "META-INF/neoforge.mods.toml",
                    br#"
[[mods]]
modId="create"

[[dependencies.create]]
modId="server_only_helper"
type="required"
side="SERVER"
"#,
                ),
                (
                    "META-INF/jarjar/metadata.json",
                    br#"{"jars":[{"path":"META-INF/jarjar/flywheel.jar"}]}"#,
                ),
                ("META-INF/jarjar/flywheel.jar", &flywheel_inner),
            ],
        );

        // 2. 官方声明的库（Fabric modmenu.badges: ["library"] + Forge FMLModType: GAMELIBRARY）
        make_jar(
            &mods.join("cloth-config.jar"),
            &[(
                "fabric.mod.json",
                br#"{"id":"cloth-config","custom":{"modmenu":{"badges":["library"]}}}"#,
            )],
        );
        make_jar(
            &mods.join("fml-lib.jar"),
            &[
                (
                    "META-INF/MANIFEST.MF",
                    b"Manifest-Version: 1.0\r\nFMLModType: GAMELIBRARY\r\n",
                ),
                ("META-INF/mods.toml", b"[[mods]]\nmodId=\"fmllib\"\n"),
            ],
        );

        // 3. 附属模组依赖 Create（通过内嵌 flywheel）与官方库 cloth-config、fmllib
        make_jar(
            &mods.join("create-addon.jar"),
            &[(
                "META-INF/neoforge.mods.toml",
                br#"
[[mods]]
modId="create_addon"

[[dependencies.create_addon]]
modId="flywheel"
type="required"

[[dependencies.create_addon]]
modId="cloth-config"
type="required"

[[dependencies.create_addon]]
modId="fmllib"
type="required"
"#,
            )],
        );

        let create_meta = read_jar_deps(&mods.join("create.jar"));
        assert!(create_meta.provided_ids.contains(&"flywheel".to_owned()));
        assert!(
            !create_meta
                .required_ids
                .contains(&"server_only_helper".to_owned()),
            "side=SERVER 依赖不应算入客户端必选依赖"
        );
        assert!(!create_meta.is_library);
        assert!(read_jar_deps(&mods.join("cloth-config.jar")).is_library);
        assert!(read_jar_deps(&mods.join("fml-lib.jar")).is_library);

        // 写入旧版本缓存文件（version=0），验证启动时自动失效并重扫
        let store_dir = dir.path().join(STORE_DIR);
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(
            store_dir.join(DEPS_CACHE_FILE),
            r#"{"version":0,"entries":{}}"#,
        )
        .unwrap();

        // 卸载 create-addon.jar：
        // - create.jar 既不是 auto_dependency 也不是 library → 绝不误判为孤儿！
        // - cloth-config.jar 与 fml-lib.jar 明确声明为库 → 应列为可选清理的孤儿前置（auto_dependency=false）
        let impact = content
            .analyze_mod_impact(&["create-addon.jar".to_owned()], ImpactAction::Remove)
            .unwrap();
        let orphan_files: BTreeSet<&str> = impact
            .orphans
            .iter()
            .map(|o| o.file_name.as_str())
            .collect();
        assert!(
            !orphan_files.contains("create.jar"),
            "卸载附属模组时不能把主内容模组 Create 误判为孤儿前置"
        );
        assert!(orphan_files.contains("cloth-config.jar"));
        assert!(orphan_files.contains("fml-lib.jar"));
        assert!(impact.orphans.iter().all(|o| !o.auto_dependency));
    }
}
