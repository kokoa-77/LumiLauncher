//! version json 的数据模型和继承合并。
//!
//! 加载器版本（Fabric、NeoForge……）的 json 用 `inheritsFrom` 指向原版，只写增量。
//! 合并规则：
//! - 主类、资源索引、客户端下载、Java 要求、日志配置、旧格式参数：子版本有就用子版本的
//! - 新格式参数（`arguments.game` / `arguments.jvm`）：父在前、子在后拼接
//! - 依赖库：子在前、父在后。子版本声明相同依赖且覆盖相同规则时替换父版本。
//!   同一层的不同系统版本和旧式本地库条目需要保留。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::MetaError;
use super::maven::MavenCoord;
use super::rules::Rule;

/// 继承链最多几层（原版 ← 加载器 ← 整合包自定义，正常不超过 3 层）
const MAX_INHERITANCE_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    /// release / snapshot / old_beta / old_alpha
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_class: Option<String>,
    /// 旧格式参数（1.12.2 及以前），整个一串
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minecraft_arguments: Option<String>,
    /// 新格式参数（1.13 起）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<VersionDownloads>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_version: Option<JavaVersionReq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<Logging>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_time: Option<String>,
    /// 有些加载器 json 用它指定客户端 jar 来自哪个版本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

/// 参数可以是普通字符串，也可以是「满足规则才加」的一个或多个值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional { rules: Vec<Rule>, value: ArgValue },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgValue {
    One(String),
    Many(Vec<String>),
}

impl ArgValue {
    pub fn values(&self) -> &[String] {
        match self {
            ArgValue::One(value) => std::slice::from_ref(value),
            ArgValue::Many(values) => values,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Library {
    /// Maven 坐标
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<LibraryDownloads>,
    /// 没有 `downloads` 时（Fabric 等）给的 Maven 仓库地址
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<Rule>>,
    /// 旧版本的本地库写法：系统名 → classifier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natives: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<Extract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// 实例本地库路径（例如 MultiMC / Prism `MMC-hint: "local"` 补丁库），不写进 Mojang version json
    #[serde(skip)]
    pub local_path: Option<PathBuf>,
    /// 仅下载到 `libraries/` 而不加入主 classpath（MultiMC / Prism 补丁的 `mavenFiles`）
    #[serde(skip)]
    pub download_only: bool,
}

impl Library {
    pub fn coord(&self) -> Option<MavenCoord> {
        MavenCoord::parse(&self.name)
    }

    /// 合并时判断是否同一个库；坐标解析不了就用原始名字。
    pub(crate) fn identity(&self) -> String {
        self.coord()
            .map_or_else(|| self.name.clone(), |c| c.identity())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Artifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<BTreeMap<String, Artifact>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// NeoForge 等由安装器生成的库这里是空字符串
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VersionDownloads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<Artifact>,
    /// 官方混淆映射表（Forge / NeoForge 安装时要用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_mappings: Option<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersionReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub major_version: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Logging {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

impl VersionJson {
    pub fn parse(id: &str, bytes: &[u8]) -> Result<Self, MetaError> {
        serde_json::from_slice(bytes).map_err(|source| MetaError::Parse {
            id: id.to_owned(),
            source,
        })
    }

    /// 这个版本要求的 Java 主版本号。没写的老版本按 Java 8 处理。
    pub fn required_java(&self) -> u32 {
        self.java_version.as_ref().map_or(8, |j| j.major_version)
    }
}

/// 把子版本叠到父版本上（规则见文件开头）。
pub fn merge(child: VersionJson, parent: VersionJson) -> VersionJson {
    let arguments = match (parent.arguments, child.arguments) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only),
        (Some(mut base), Some(extra)) => {
            base.game.extend(extra.game);
            base.jvm.extend(extra.jvm);
            Some(base)
        }
    };

    let mut overrides = HashMap::<_, Vec<_>>::new();
    for library in &child.libraries {
        overrides
            .entry(library.identity())
            .or_default()
            .push(library);
    }
    // 旧版 Mojang 描述在同一层提供多种系统规则，以及单独的本地库条目。
    let inherited = parent.libraries.into_iter().filter(|library| {
        !overrides
            .get(&library.identity())
            .is_some_and(|candidates| {
                candidates.iter().any(|candidate| {
                    candidate.natives == library.natives
                        && (candidate.rules.as_ref().is_none_or(Vec::is_empty)
                            || candidate.rules == library.rules)
                })
            })
    });
    let inherited: Vec<_> = inherited.collect();
    let mut libraries = child.libraries;
    libraries.extend(inherited);

    VersionJson {
        id: child.id,
        inherits_from: None,
        kind: child.kind.or(parent.kind),
        main_class: child.main_class.or(parent.main_class),
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        arguments,
        libraries,
        asset_index: child.asset_index.or(parent.asset_index),
        assets: child.assets.or(parent.assets),
        downloads: child.downloads.or(parent.downloads),
        java_version: child.java_version.or(parent.java_version),
        logging: child.logging.or(parent.logging),
        release_time: child.release_time.or(parent.release_time),
        jar: child.jar.or(parent.jar),
    }
}

/// 沿着 `inheritsFrom` 一路读到原版，再从上往下合并成完整的版本描述。
/// `load` 负责按 ID 读取单个版本的 json（从磁盘或网络）。
pub fn resolve(
    id: &str,
    mut load: impl FnMut(&str) -> Result<VersionJson, MetaError>,
) -> Result<VersionJson, MetaError> {
    let mut chain = vec![load(id)?];
    while let Some(parent_id) = chain.last().and_then(|v| v.inherits_from.clone()) {
        if chain.len() >= MAX_INHERITANCE_DEPTH || chain.iter().any(|v| v.id == parent_id) {
            let path: Vec<&str> = chain.iter().map(|v| v.id.as_str()).collect();
            return Err(MetaError::InheritanceLoop(format!(
                "{} → {parent_id}",
                path.join(" → ")
            )));
        }
        chain.push(load(&parent_id)?);
    }

    let mut merged = match chain.pop() {
        Some(root) => root,
        None => return Err(MetaError::NotFound(id.to_owned())),
    };
    // 客户端 jar 默认来自最底层的原版（加载器版本没有自己的客户端 jar）
    if merged.jar.is_none() {
        merged.jar = Some(merged.id.clone());
    }
    while let Some(child) = chain.pop() {
        merged = merge(child, merged);
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str) -> Library {
        Library {
            name: name.to_owned(),
            ..Library::default()
        }
    }

    #[test]
    fn merge_keeps_platform_variants_and_legacy_natives_inherited_from_the_parent() {
        let describe = |name: &str, rules: Option<&str>, natives| {
            let mut library = lib(name);
            library.rules = rules.map(|rules| serde_json::from_str(rules).unwrap());
            library.natives = natives;
            library
        };
        let mac = r#"[{"action":"allow","os":{"name":"osx"}}]"#;
        let others = r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}]"#;
        let mac_lwjgl = describe("org.lwjgl:lwjgl:3.2.1", Some(mac), None);
        let windows_lwjgl = describe("org.lwjgl:lwjgl:3.2.2", Some(others), None);
        let windows_natives = describe(
            "org.lwjgl:lwjgl:3.2.2",
            Some(others),
            Some(BTreeMap::from([(
                "windows".to_owned(),
                "natives-windows".to_owned(),
            )])),
        );
        let parent = VersionJson {
            libraries: vec![
                mac_lwjgl.clone(),
                windows_lwjgl.clone(),
                windows_natives.clone(),
            ],
            ..VersionJson::default()
        };
        let child = VersionJson::default();
        assert_eq!(merge(child, parent.clone()).libraries, parent.libraries);

        let override_main = lib("org.lwjgl:lwjgl:3.3.3");
        let child = VersionJson {
            libraries: vec![override_main.clone()],
            ..VersionJson::default()
        };
        let merged = merge(child, parent);
        assert_eq!(merged.libraries, vec![override_main, windows_natives]);
        let env = crate::meta::Environment {
            os_name: "windows".to_owned(),
            os_arch: "x86_64".to_owned(),
            os_version: "10.0".to_owned(),
            features: BTreeMap::new(),
        };
        let files =
            crate::install::plan_libraries(&merged, &crate::paths::GameDir::new("D:/mc"), &env)
                .unwrap();
        let main: Vec<_> = files.iter().filter(|file| file.on_classpath).collect();
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].name, "org.lwjgl:lwjgl:3.3.3");
        assert!(files.iter().any(|file| file.extract_natives.is_some()));
    }

    fn vanilla() -> VersionJson {
        VersionJson {
            id: "1.21.1".into(),
            kind: Some("release".into()),
            main_class: Some("net.minecraft.client.main.Main".into()),
            arguments: Some(Arguments {
                game: vec![Argument::Plain("--username".into())],
                jvm: vec![Argument::Plain("-cp".into())],
            }),
            libraries: vec![
                lib("org.ow2.asm:asm:9.6"),
                lib("org.lwjgl:lwjgl:3.3.3"),
                lib("org.lwjgl:lwjgl:3.3.3:natives-windows"),
            ],
            asset_index: Some(AssetIndexRef {
                id: "17".into(),
                ..AssetIndexRef::default()
            }),
            java_version: Some(JavaVersionReq {
                component: None,
                major_version: 21,
            }),
            ..VersionJson::default()
        }
    }

    fn loader() -> VersionJson {
        VersionJson {
            id: "1.21.1-neoforge-21.1.77".into(),
            inherits_from: Some("1.21.1".into()),
            main_class: Some("cpw.mods.bootstraplauncher.BootstrapLauncher".into()),
            arguments: Some(Arguments {
                game: vec![Argument::Plain("--fml.neoForgeVersion".into())],
                jvm: vec![Argument::Plain("-DlibraryDirectory=x".into())],
            }),
            libraries: vec![
                lib("org.ow2.asm:asm:9.7.1"),
                lib("net.neoforged:neoforge:21.1.77"),
            ],
            ..VersionJson::default()
        }
    }

    #[test]
    fn child_overrides_scalars_and_inherits_the_rest() {
        let merged = merge(loader(), vanilla());
        assert_eq!(merged.id, "1.21.1-neoforge-21.1.77");
        assert_eq!(merged.inherits_from, None);
        assert_eq!(
            merged.main_class.as_deref(),
            Some("cpw.mods.bootstraplauncher.BootstrapLauncher")
        );
        assert_eq!(merged.asset_index.unwrap().id, "17");
        assert_eq!(merged.java_version.unwrap().major_version, 21);
        assert_eq!(merged.kind.as_deref(), Some("release"));
    }

    #[test]
    fn arguments_are_concatenated_parent_first() {
        let args = merge(loader(), vanilla()).arguments.unwrap();
        assert_eq!(
            args.game,
            [
                Argument::Plain("--username".into()),
                Argument::Plain("--fml.neoForgeVersion".into())
            ]
        );
        assert_eq!(args.jvm.len(), 2);
    }

    #[test]
    fn duplicate_libraries_keep_the_childs_version() {
        let libs: Vec<String> = merge(loader(), vanilla())
            .libraries
            .into_iter()
            .map(|l| l.name)
            .collect();
        assert_eq!(
            libs,
            [
                "org.ow2.asm:asm:9.7.1",
                "net.neoforged:neoforge:21.1.77",
                "org.lwjgl:lwjgl:3.3.3",
                "org.lwjgl:lwjgl:3.3.3:natives-windows",
            ]
        );
    }

    #[test]
    fn resolve_follows_the_chain() {
        let merged = resolve("1.21.1-neoforge-21.1.77", |id| match id {
            "1.21.1-neoforge-21.1.77" => Ok(loader()),
            "1.21.1" => Ok(vanilla()),
            other => Err(MetaError::NotFound(other.into())),
        })
        .unwrap();
        assert_eq!(merged.libraries.len(), 4);
        assert_eq!(merged.jar.as_deref(), Some("1.21.1"), "客户端 jar 来自原版");

        let plain = resolve("1.21.1", |_| Ok(vanilla())).unwrap();
        assert_eq!(plain.jar.as_deref(), Some("1.21.1"));
    }

    #[test]
    fn resolve_detects_loops_and_missing_parents() {
        let looping = resolve("a", |id| {
            Ok(VersionJson {
                id: id.into(),
                inherits_from: Some(if id == "a" { "b" } else { "a" }.into()),
                ..VersionJson::default()
            })
        });
        assert!(matches!(looping, Err(MetaError::InheritanceLoop(_))));

        let missing = resolve("1.21.1-neoforge-21.1.77", |id| match id {
            "1.21.1-neoforge-21.1.77" => Ok(loader()),
            other => Err(MetaError::NotFound(other.into())),
        });
        assert!(matches!(missing, Err(MetaError::NotFound(id)) if id == "1.21.1"));
    }

    #[test]
    fn parses_conditional_arguments() {
        let json = br#"{
            "id": "x",
            "arguments": {
                "game": ["--demo", {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"}],
                "jvm": [{"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]}]
            }
        }"#;
        let v = VersionJson::parse("x", json).unwrap();
        let args = v.arguments.unwrap();
        assert!(
            matches!(&args.game[1], Argument::Conditional { value: ArgValue::One(s), .. } if s == "--demo")
        );
        assert!(
            matches!(&args.jvm[0], Argument::Conditional { value: ArgValue::Many(v), .. } if v.len() == 1)
        );
    }

    #[test]
    fn old_versions_default_to_java_8() {
        assert_eq!(VersionJson::default().required_java(), 8);
        assert_eq!(vanilla().required_java(), 21);
    }
}
