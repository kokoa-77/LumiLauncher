//! MultiMC / Prism Meta v1 组件补丁引擎（`patches/*.json`）。
//!
//! 负责解析、安全校验、持久化以及把实例级补丁叠加到基础 [`VersionJson`] 上：
//! - 覆写 `mainClass`、`minecraftArguments`
//! - 追加 `+tweakers`（自动拼成 `--tweakClass <cls>` 且去重）与 `+jvmArgs`
//! - 合并 `libraries` / `mavenFiles` / `mainJar`（支持按 Maven 坐标替换原版库、支持 `org.lwjgl3` 替换 1.7.10 LWJGL 2、支持 `MMC-hint: "local"` 本地库）
//! - 处理 `compatibleJavaMajors`（如 GTNH 1.7.10 补丁声明兼容 Java 17–25，覆盖原版默认的 Java 8）

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::MetaError;
use super::rules::Rule;
use super::version::{
    Argument, Arguments, Extract, JavaVersionReq, Library, LibraryDownloads, VersionJson, merge,
};
use crate::fs::{AtomicWriteError, safe_join, write_json_atomic};

/// 实例元信息目录名（与 `instance::META_DIR` 一致；放在 `meta` 层不反向依赖上层 `instance`）
const INSTANCE_META_DIR: &str = "Lumi";
/// 持久化在 `instances/<id>/Lumi/patches.json`
pub const PATCHES_FILE: &str = "patches.json";
/// 实例本地补丁库目录 `instances/<id>/Lumi/libraries/`
pub const LOCAL_LIBS_DIR: &str = "libraries";
const PATCHES_FORMAT: u32 = 1;
/// 单个实例最多允许的补丁数
const MAX_PATCHES: usize = 64;

/// 持久化在实例目录内的全部 MultiMC / Prism 补丁。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstancePatches {
    pub format: u32,
    /// 为 true 时从基础版本中剔除旧版 LWJGL 2（`org.lwjgl.lwjgl:*`），避免与 LWJGL 3 冲突
    #[serde(default)]
    pub replace_lwjgl2: bool,
    /// 按 `mmc-pack.json` 组件顺序排列的补丁列表
    #[serde(default)]
    pub patches: Vec<MetaPatch>,
}

impl InstancePatches {
    pub fn new(patches: Vec<MetaPatch>, replace_lwjgl2: bool) -> Self {
        Self {
            format: PATCHES_FORMAT,
            replace_lwjgl2,
            patches,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patches.is_empty() && !self.replace_lwjgl2
    }

    /// 供预览卡片展示的补丁可读名列表
    pub fn display_names(&self) -> Vec<String> {
        self.patches
            .iter()
            .map(|p| {
                p.name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&p.uid)
                    .to_owned()
            })
            .collect()
    }

    /// 汇总所有补丁声明的兼容 Java 主版本列表（多个补丁都声明时取交集，无交集取最后一个）。
    pub fn compatible_java_majors(&self) -> Vec<u32> {
        let mut combined: Option<Vec<u32>> = None;
        for patch in &self.patches {
            let majors: Vec<u32> = patch
                .compatible_java_majors
                .iter()
                .copied()
                .filter(|m| (8..=99).contains(m))
                .collect();
            if majors.is_empty() {
                continue;
            }
            combined = Some(match combined {
                None => majors,
                Some(prev) => {
                    let inter: Vec<u32> = prev.into_iter().filter(|m| majors.contains(m)).collect();
                    if inter.is_empty() { majors } else { inter }
                }
            });
        }
        let mut out = combined.unwrap_or_default();
        let mut seen = Vec::new();
        for major in out.drain(..) {
            if !seen.contains(&major) {
                seen.push(major);
            }
        }
        seen
    }

    /// 根据补丁声明的 `compatibleJavaMajors` 挑出一个默认目标 Java 主版本。
    /// 优先 21（官方运行时可直下且现代 GC 最佳），其次 17，其次 8，再其次列表首项。
    pub fn preferred_java_major(&self) -> Option<u32> {
        preferred_java_major(&self.compatible_java_majors())
    }

    /// 补丁包是否已经自带了指定加载器 UID（如 `net.minecraftforge`）的完整定义。
    pub fn provides_component(&self, uid: &str) -> bool {
        self.patches.iter().any(|patch| {
            patch.uid.eq_ignore_ascii_case(uid)
                && (patch.main_class.is_some()
                    || !patch.libraries.is_empty()
                    || !patch.tweakers.is_empty())
        })
    }

    /// 是否自带了能绕过标准 Forge 安装器的补丁链（如 1.7.10 GTNH 的 `net.minecraftforge` + `lwjgl3ify`）。
    pub fn provides_legacy_forge(&self) -> bool {
        self.provides_component("net.minecraftforge")
            || self.patches.iter().any(|patch| {
                patch.uid.to_ascii_lowercase().contains("forgepatches")
                    && (!patch.libraries.is_empty() || !patch.maven_files.is_empty())
            })
    }
}

/// 单个 MultiMC / Prism Meta v1 补丁（对应 `patches/<uid>.json`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetaPatch {
    pub uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(
        rename = "formatVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub format_version: Option<u32>,
    #[serde(rename = "mainClass", default, skip_serializing_if = "Option::is_none")]
    pub main_class: Option<String>,
    #[serde(
        rename = "minecraftArguments",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub minecraft_arguments: Option<String>,
    #[serde(
        rename = "+tweakers",
        alias = "tweakers",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tweakers: Vec<String>,
    #[serde(
        rename = "+jvmArgs",
        alias = "jvmArgs",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub jvm_args: Vec<String>,
    #[serde(
        rename = "+traits",
        alias = "traits",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub traits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<PatchLibrary>,
    #[serde(rename = "mavenFiles", default, skip_serializing_if = "Vec::is_empty")]
    pub maven_files: Vec<PatchLibrary>,
    #[serde(rename = "mainJar", default, skip_serializing_if = "Option::is_none")]
    pub main_jar: Option<PatchLibrary>,
    #[serde(
        rename = "compatibleJavaMajors",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub compatible_java_majors: Vec<u32>,
}

/// 补丁中的单个库条目（兼容 Mojang `Library` 及 MultiMC `MMC-hint` 本地库字段）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PatchLibrary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<Rule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natives: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<Extract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(rename = "MMC-hint", default, skip_serializing_if = "Option::is_none")]
    pub mmc_hint: Option<String>,
    #[serde(
        rename = "MMC-filename",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mmc_filename: Option<String>,
}

impl PatchLibrary {
    pub fn is_local(&self) -> bool {
        self.mmc_hint
            .as_deref()
            .is_some_and(|hint| hint.eq_ignore_ascii_case("local"))
    }

    /// 转为运行期 [`Library`]；本地库映射到 `instances/<id>/Lumi/libraries/<filename>`。
    fn into_library(
        self,
        local_libs_root: &Path,
        download_only: bool,
    ) -> Result<Library, MetaError> {
        let local_path = if self.is_local() {
            let file_name = self.mmc_filename.as_deref().ok_or_else(|| {
                MetaError::InvalidPatch(format!("本地库 {} 缺少 MMC-filename", self.name))
            })?;
            let path = safe_join(local_libs_root, file_name).map_err(|reason| {
                MetaError::InvalidPatch(format!("本地库路径不安全（{file_name}：{reason}）"))
            })?;
            Some(path)
        } else {
            None
        };
        Ok(Library {
            name: self.name,
            downloads: self.downloads,
            url: self.url,
            rules: self.rules,
            natives: self.natives,
            extract: self.extract,
            sha1: self.sha1,
            size: self.size,
            local_path,
            download_only,
        })
    }
}

/// 从兼容 Java 主版本列表中挑选推荐的默认主版本。
pub fn preferred_java_major(majors: &[u32]) -> Option<u32> {
    for preferred in [21, 17, 8] {
        if majors.contains(&preferred) {
            return Some(preferred);
        }
    }
    majors.iter().copied().max()
}

/// 解析单个 `patches/*.json` 字节流并立即做安全校验。
pub fn parse_patch(name: &str, bytes: &[u8]) -> Result<MetaPatch, MetaError> {
    let patch: MetaPatch = serde_json::from_slice(bytes).map_err(|source| MetaError::Parse {
        id: name.to_owned(),
        source,
    })?;
    validate_patch(&patch)?;
    Ok(patch)
}

/// 安全校验补丁内容：所有远程 URL 仅允许 `https://`，所有路径严格防穿越。
pub fn validate_patch(patch: &MetaPatch) -> Result<(), MetaError> {
    if patch.uid.trim().is_empty()
        || !patch
            .uid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(MetaError::InvalidPatch(format!(
            "补丁 uid 不合法：{}",
            patch.uid
        )));
    }
    if let Some(main_class) = &patch.main_class {
        let valid = !main_class.is_empty()
            && main_class.split('.').all(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
            });
        if !valid {
            return Err(MetaError::InvalidPatch(format!(
                "补丁 {} 的 mainClass 不合法：{main_class}",
                patch.uid
            )));
        }
    }
    for tweaker in &patch.tweakers {
        let valid = !tweaker.is_empty()
            && tweaker
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'$'));
        if !valid {
            return Err(MetaError::InvalidPatch(format!(
                "补丁 {} 的 tweaker 类名不合法：{tweaker}",
                patch.uid
            )));
        }
    }
    for lib in patch
        .libraries
        .iter()
        .chain(patch.maven_files.iter())
        .chain(patch.main_jar.iter())
    {
        validate_library(&patch.uid, lib)?;
    }
    Ok(())
}

pub fn validate_bundle(bundle: &InstancePatches) -> Result<(), MetaError> {
    if bundle.patches.len() > MAX_PATCHES {
        return Err(MetaError::InvalidPatch(format!(
            "补丁数量过多（{} 项，上限 {MAX_PATCHES}）",
            bundle.patches.len()
        )));
    }
    for patch in &bundle.patches {
        validate_patch(patch)?;
    }
    Ok(())
}

fn validate_https_url(uid: &str, url: &str) -> Result<(), MetaError> {
    if url.is_empty() {
        return Ok(());
    }
    if !url.starts_with("https://") {
        return Err(MetaError::InvalidPatch(format!(
            "补丁 {uid} 包含非 HTTPS 下载地址：{url}"
        )));
    }
    Ok(())
}

fn validate_library(uid: &str, lib: &PatchLibrary) -> Result<(), MetaError> {
    if lib.name.trim().is_empty() {
        return Err(MetaError::InvalidPatch(format!(
            "补丁 {uid} 包含空名称的库条目"
        )));
    }
    if let Some(url) = &lib.url {
        validate_https_url(uid, url)?;
    }
    if let Some(downloads) = &lib.downloads {
        if let Some(artifact) = &downloads.artifact {
            validate_https_url(uid, &artifact.url)?;
            if let Some(path) = &artifact.path {
                safe_join(Path::new("libraries"), path).map_err(|reason| {
                    MetaError::InvalidPatch(format!(
                        "补丁 {uid} 的库路径不安全（{path}：{reason}）"
                    ))
                })?;
            }
        }
        if let Some(classifiers) = &downloads.classifiers {
            for artifact in classifiers.values() {
                validate_https_url(uid, &artifact.url)?;
                if let Some(path) = &artifact.path {
                    safe_join(Path::new("libraries"), path).map_err(|reason| {
                        MetaError::InvalidPatch(format!(
                            "补丁 {uid} 的本地库路径不安全（{path}：{reason}）"
                        ))
                    })?;
                }
            }
        }
    }
    if lib.is_local() {
        let file = lib.mmc_filename.as_deref().ok_or_else(|| {
            MetaError::InvalidPatch(format!(
                "补丁 {uid} 的本地库 {} 缺少 MMC-filename",
                lib.name
            ))
        })?;
        safe_join(Path::new("libraries"), file).map_err(|reason| {
            MetaError::InvalidPatch(format!("补丁 {uid} 的本地库路径不安全（{file}：{reason}）"))
        })?;
    }
    Ok(())
}

/// 读取实例的 `Lumi/patches.json`；不存在返回 `Ok(None)`。
pub fn load_for_instance(instance_dir: &Path) -> Result<Option<InstancePatches>, MetaError> {
    let path = instance_dir.join(INSTANCE_META_DIR).join(PATCHES_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(MetaError::Io { path, source }),
    };
    let bundle: InstancePatches =
        serde_json::from_slice(&bytes).map_err(|source| MetaError::Parse {
            id: PATCHES_FILE.to_owned(),
            source,
        })?;
    validate_bundle(&bundle)?;
    Ok(Some(bundle))
}

/// 原子保存实例的 `Lumi/patches.json`。
pub fn save_for_instance(
    instance_dir: &Path,
    bundle: &InstancePatches,
) -> Result<(), AtomicWriteError> {
    let path = instance_dir.join(INSTANCE_META_DIR).join(PATCHES_FILE);
    write_json_atomic(&path, bundle)
}

/// 把实例补丁链叠加到基础 [`VersionJson`] 上。
///
/// 处理规则与 Prism / MultiMC 保持一致：
/// 1. 若启用了 LWJGL 3 替换（`replace_lwjgl2` 或含 `org.lwjgl3` 补丁），移除原版 `org.lwjgl.lwjgl:*`；
/// 2. 逐补丁合成 `mainClass`、`minecraftArguments`、`+tweakers`、`+jvmArgs` 与库列表；
/// 3. 旧版 `arguments` 为空但补丁追加了 `+jvmArgs` 时，先补齐旧版默认 JVM 参数（`-Djava.library.path` 与 `-cp ${classpath}`），避免丢失类路径；
/// 4. 若补丁声明了 `compatibleJavaMajors`，覆盖基础版本的 `java_version`。
pub fn apply(
    mut base: VersionJson,
    bundle: &InstancePatches,
    instance_dir: &Path,
) -> Result<VersionJson, MetaError> {
    validate_bundle(bundle)?;
    if bundle.is_empty() {
        return Ok(base);
    }

    let replace_lwjgl2 = bundle.replace_lwjgl2
        || bundle
            .patches
            .iter()
            .any(|patch| patch.uid.eq_ignore_ascii_case("org.lwjgl3"));
    if replace_lwjgl2 {
        base.libraries.retain(|lib| !is_lwjgl2_library(&lib.name));
    }

    let local_libs_root = instance_dir.join(INSTANCE_META_DIR).join(LOCAL_LIBS_DIR);
    let mut all_tweakers: Vec<String> = Vec::new();
    let mut all_jvm_args: Vec<String> = Vec::new();

    for patch in &bundle.patches {
        let mut patch_libs = Vec::new();
        for lib in &patch.libraries {
            patch_libs.push(lib.clone().into_library(&local_libs_root, false)?);
        }
        for lib in &patch.maven_files {
            patch_libs.push(lib.clone().into_library(&local_libs_root, true)?);
        }
        if let Some(main_jar) = &patch.main_jar {
            patch_libs.push(main_jar.clone().into_library(&local_libs_root, false)?);
        }

        let overlay = VersionJson {
            id: base.id.clone(),
            main_class: patch.main_class.clone(),
            minecraft_arguments: patch.minecraft_arguments.clone(),
            libraries: patch_libs,
            ..VersionJson::default()
        };
        base = merge(overlay, base);

        for tweaker in &patch.tweakers {
            if !all_tweakers.contains(tweaker) {
                all_tweakers.push(tweaker.clone());
            }
        }
        all_jvm_args.extend(patch.jvm_args.iter().cloned());
    }

    if !all_tweakers.is_empty() {
        append_tweakers(&mut base, &all_tweakers);
    }

    if !all_jvm_args.is_empty() {
        let arguments = base.arguments.get_or_insert_with(Arguments::default);
        if arguments.jvm.is_empty() {
            arguments.jvm.extend([
                Argument::Plain("-Djava.library.path=${natives_directory}".to_owned()),
                Argument::Plain("-cp".to_owned()),
                Argument::Plain("${classpath}".to_owned()),
            ]);
        }
        arguments
            .jvm
            .extend(all_jvm_args.into_iter().map(Argument::Plain));
    }

    if let Some(major) = bundle.preferred_java_major() {
        base.java_version = Some(JavaVersionReq {
            component: None,
            major_version: major,
        });
    }

    Ok(base)
}

fn is_lwjgl2_library(coord: &str) -> bool {
    coord.starts_with("org.lwjgl.lwjgl:")
}

fn append_tweakers(version: &mut VersionJson, tweakers: &[String]) {
    if let Some(arguments) = version.arguments.as_mut().filter(|a| !a.game.is_empty()) {
        let existing: Vec<String> = arguments
            .game
            .iter()
            .filter_map(|arg| match arg {
                Argument::Plain(s) => Some(s.clone()),
                Argument::Conditional { .. } => None,
            })
            .collect();
        for tweaker in tweakers {
            if existing
                .windows(2)
                .any(|w| w[0] == "--tweakClass" && w[1] == *tweaker)
            {
                continue;
            }
            arguments
                .game
                .push(Argument::Plain("--tweakClass".to_owned()));
            arguments.game.push(Argument::Plain(tweaker.clone()));
        }
        return;
    }

    let legacy = version.minecraft_arguments.get_or_insert_with(String::new);
    let tokens: Vec<String> = legacy.split_whitespace().map(str::to_owned).collect();
    for tweaker in tweakers {
        if tokens
            .windows(2)
            .any(|w| w[0] == "--tweakClass" && w[1] == *tweaker)
        {
            continue;
        }
        if !legacy.is_empty() && !legacy.ends_with(' ') {
            legacy.push(' ');
        }
        legacy.push_str("--tweakClass ");
        legacy.push_str(tweaker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vanilla_1_7_10() -> VersionJson {
        VersionJson {
            id: "1.7.10".to_owned(),
            main_class: Some("net.minecraft.client.main.Main".to_owned()),
            minecraft_arguments: Some(
                "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userProperties ${user_properties} --userType ${user_type}".to_owned(),
            ),
            libraries: vec![
                Library {
                    name: "org.lwjgl.lwjgl:lwjgl:2.9.1".to_owned(),
                    ..Library::default()
                },
                Library {
                    name: "org.lwjgl.lwjgl:lwjgl_util:2.9.1".to_owned(),
                    ..Library::default()
                },
                Library {
                    name: "org.ow2.asm:asm-all:4.1".to_owned(),
                    ..Library::default()
                },
            ],
            ..VersionJson::default()
        }
    }

    #[test]
    fn applies_gtnh_style_patches_with_lwjgl3_tweakers_and_java21() {
        let dir = tempfile::tempdir().unwrap();
        let lwjgl3 = parse_patch(
            "org.lwjgl3.json",
            br#"{
                "uid": "org.lwjgl3",
                "name": "LWJGL 3",
                "version": "3.3.3",
                "libraries": [
                    { "name": "org.lwjgl:lwjgl:3.3.3", "url": "https://libraries.minecraft.net" }
                ]
            }"#,
        )
        .unwrap();
        let forge = parse_patch(
            "net.minecraftforge.json",
            br#"{
                "uid": "net.minecraftforge",
                "name": "Forge",
                "version": "10.13.4.1614",
                "mainClass": "net.minecraft.launchwrapper.Launch",
                "+tweakers": ["cpw.mods.fml.common.launcher.FMLTweaker"],
                "libraries": [
                    { "name": "org.ow2.asm:asm-all:5.0.3", "url": "https://maven.minecraftforge.net" },
                    { "name": "net.minecraftforge:forge:1.7.10-10.13.4.1614", "url": "https://maven.minecraftforge.net" }
                ]
            }"#,
        )
        .unwrap();
        let lwjgl3ify = parse_patch(
            "com.gtnewhorizons.lwjgl3ify.forgepatches.json",
            br#"{
                "uid": "com.gtnewhorizons.lwjgl3ify.forgepatches",
                "name": "lwjgl3ify forgePatches",
                "compatibleJavaMajors": [17, 21, 25],
                "+jvmArgs": [
                    "--add-opens",
                    "java.base/java.lang=ALL-UNNAMED",
                    "-Dfml.ignoreInvalidMinecraftCertificates=true"
                ],
                "libraries": [
                    {
                        "name": "com.gtnewhorizons:local-helper:1.0",
                        "MMC-hint": "local",
                        "MMC-filename": "local-helper.jar"
                    }
                ],
                "mavenFiles": [
                    {
                        "name": "com.gtnewhorizons:rfb:1.0",
                        "url": "https://nexus.gtnewhorizons.com/repository/public"
                    }
                ]
            }"#,
        )
        .unwrap();

        let bundle = InstancePatches::new(vec![lwjgl3, forge, lwjgl3ify], true);
        assert!(bundle.provides_legacy_forge());
        assert_eq!(bundle.compatible_java_majors(), vec![17, 21, 25]);
        assert_eq!(bundle.preferred_java_major(), Some(21));

        let merged = apply(vanilla_1_7_10(), &bundle, dir.path()).unwrap();
        assert_eq!(
            merged.main_class.as_deref(),
            Some("net.minecraft.launchwrapper.Launch")
        );
        assert_eq!(merged.required_java(), 21);

        // LWJGL 2 已移除，LWJGL 3 已加入，asm-all 升级为 5.0.3
        let names: Vec<&str> = merged.libraries.iter().map(|l| l.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.starts_with("org.lwjgl.lwjgl:")));
        assert!(names.contains(&"org.lwjgl:lwjgl:3.3.3"));
        assert!(names.contains(&"org.ow2.asm:asm-all:5.0.3"));
        assert!(!names.contains(&"org.ow2.asm:asm-all:4.1"));

        // 本地库正确解析到 Lumi/libraries/，mavenFiles 标记为 download_only
        let local = merged
            .libraries
            .iter()
            .find(|l| l.name == "com.gtnewhorizons:local-helper:1.0")
            .unwrap();
        assert_eq!(
            local.local_path.as_deref(),
            Some(
                dir.path()
                    .join("Lumi")
                    .join("libraries")
                    .join("local-helper.jar")
                    .as_path()
            )
        );
        assert!(!local.download_only);
        let maven_only = merged
            .libraries
            .iter()
            .find(|l| l.name == "com.gtnewhorizons:rfb:1.0")
            .unwrap();
        assert!(maven_only.download_only);

        // 旧版追加 +jvmArgs 时保留了默认 -cp ${classpath}
        let jvm = &merged.arguments.as_ref().unwrap().jvm;
        assert!(jvm.contains(&Argument::Plain("-cp".to_owned())));
        assert!(jvm.contains(&Argument::Plain("${classpath}".to_owned())));
        assert!(jvm.contains(&Argument::Plain("--add-opens".to_owned())));
        assert!(jvm.contains(&Argument::Plain(
            "-Dfml.ignoreInvalidMinecraftCertificates=true".to_owned()
        )));

        // 旧版 minecraftArguments 正确追加了 --tweakClass 且不重复
        let mc_args = merged.minecraft_arguments.as_deref().unwrap();
        assert!(mc_args.contains("--tweakClass cpw.mods.fml.common.launcher.FMLTweaker"));

        // 持久化读写闭环
        save_for_instance(dir.path(), &bundle).unwrap();
        let loaded = load_for_instance(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, bundle);
    }

    #[test]
    fn rejects_insecure_patch_urls_and_path_traversal() {
        let http_patch = br#"{
            "uid": "evil.http",
            "libraries": [{ "name": "a:b:1.0", "url": "http://example.com/repo" }]
        }"#;
        assert!(matches!(
            parse_patch("evil.json", http_patch),
            Err(MetaError::InvalidPatch(_))
        ));

        let traversal_patch = br#"{
            "uid": "evil.local",
            "libraries": [{
                "name": "a:b:1.0",
                "MMC-hint": "local",
                "MMC-filename": "../../escape.jar"
            }]
        }"#;
        assert!(matches!(
            parse_patch("evil.json", traversal_patch),
            Err(MetaError::InvalidPatch(_))
        ));

        let bad_main = br#"{
            "uid": "evil.main",
            "mainClass": "net.minecraft.Main; rm -rf /"
        }"#;
        assert!(matches!(
            parse_patch("evil.json", bad_main),
            Err(MetaError::InvalidPatch(_))
        ));
    }
}
