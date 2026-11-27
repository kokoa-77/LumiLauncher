//! 从别家启动器导入实例：Prism / MultiMC、CurseForge、ATLauncher、Modrinth App。
//!
//! 这几家的实例文件夹里没有能直接启动的版本描述（它们启动时自己拼），所以不挂上，而是导入：
//! 把游戏文件（模组、配置、存档……）复制一份进本启动器（别家那边照样能用），
//! 游戏本体和加载器由本启动器自己装。认实例只看各家的实例描述文件：
//! - Prism / MultiMC：`instance.cfg`（名字）+ `mmc-pack.json`（游戏版本、加载器），游戏文件在 `.minecraft/` 或 `minecraft/`
//! - CurseForge：`minecraftinstance.json`
//! - ATLauncher：`instance.json`（`id` 是游戏版本，`launcher.loaderVersion` 是加载器）
//! - Modrinth App：实例信息在它的数据库里（[`super::modrinth_app`]）

use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::Launcher;
use crate::instance::{
    self, CopyError, CopyOptions, CopyProgress, Instance, InstanceConfig, LoaderKind, LoaderRef,
};
use crate::paths::GameDir;

/// 别家启动器的一个实例
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignInstance {
    pub launcher: Launcher,
    pub name: String,
    pub game_version: String,
    pub loader: Option<LoaderRef>,
    /// 实例文件夹
    pub folder: PathBuf,
    /// 游戏文件在哪（Prism 是实例文件夹里的 `.minecraft` / `minecraft`，其他就是实例文件夹）
    pub game_dir: PathBuf,
}

/// 复制时第一层里不要的：别家启动器自己的实例描述（本启动器另外写自己的）
const LAUNCHER_FILES: &[&str] = &[
    "instance.cfg",
    "mmc-pack.json",
    "minecraftinstance.json",
    "instance.json",
    "profile.json",
    ".curseclient",
];

/// 认一个文件夹是不是别家的实例
pub fn inspect(folder: &Path) -> Option<ForeignInstance> {
    if folder.join("mmc-pack.json").is_file() {
        return prism(folder);
    }
    if folder.join("minecraftinstance.json").is_file() {
        return curseforge(folder);
    }
    if folder.join("instance.json").is_file() {
        return atlauncher(folder);
    }
    None
}

/// 用户选的文件夹里的实例：选的就是一个实例；或者是放实例的文件夹；或者是 Prism / ATLauncher 的数据文件夹；
/// 或者选到了实例里面一两层（系统选文件夹的窗口里点中了 `.minecraft`、`mods` 再按「选择文件夹」，选到的是点中的那个）；
/// 或者是 Modrinth App 的实例（它的实例文件夹里没有描述文件，按它数据库里的文件夹对）
pub fn inspect_any(folder: &Path) -> Vec<ForeignInstance> {
    look(folder, true)
}

/// 和 [`inspect_any`] 一样，但不往实例外面找（拖进窗口的文件夹：拖了实例的 `mods` 进来不是要导入整个实例）
pub fn inspect_here(folder: &Path) -> Vec<ForeignInstance> {
    look(folder, false)
}

fn look(folder: &Path, up: bool) -> Vec<ForeignInstance> {
    if let Some(one) = inspect(folder) {
        return vec![one];
    }
    let mut found = children(folder);
    if found.is_empty() {
        for sub in ["instances", "Instances"] {
            found = children(&folder.join(sub));
            if !found.is_empty() {
                break;
            }
        }
    }
    if !found.is_empty() {
        return found;
    }
    if up && let Some(one) = folder.ancestors().skip(1).take(2).find_map(inspect) {
        return vec![one];
    }
    near(folder, super::modrinth_app::instances(), up)
}

/// 这些实例里，选的文件夹是哪一个（`up` 时在它里面一两层也算）；对不上的话，选的是放它们的文件夹（往上一两层）就都要
fn near(folder: &Path, all: Vec<ForeignInstance>, up: bool) -> Vec<ForeignInstance> {
    let depth = if up { 3 } else { 1 };
    let picked: Vec<String> = folder
        .ancestors()
        .take(depth)
        .map(instance::path_key)
        .collect();
    if let Some(one) = all
        .iter()
        .find(|i| picked.contains(&instance::path_key(&i.folder)))
    {
        return vec![one.clone()];
    }
    let key = instance::path_key(folder);
    let mut found: Vec<ForeignInstance> = all
        .into_iter()
        .filter(|i| {
            i.folder
                .ancestors()
                .skip(1)
                .take(2)
                .any(|dir| instance::path_key(dir) == key)
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// 一个文件夹下面的实例（按名字排）
fn children(dir: &Path) -> Vec<ForeignInstance> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<ForeignInstance> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| inspect(&e.path()))
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// 电脑上各家启动器默认位置里的实例：Prism、CurseForge、ATLauncher、Modrinth App
pub fn known_instances() -> Vec<ForeignInstance> {
    let mut found = Vec::new();
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    if let Some(prism) = appdata.as_ref().map(|a| a.join("PrismLauncher")) {
        found.extend(children(&prism_instance_dir(&prism)));
    }
    if let Some(home) = &home {
        for base in [home.clone(), home.join("Documents")] {
            found.extend(children(
                &base.join("curseforge").join("minecraft").join("Instances"),
            ));
        }
    }
    if let Some(atl) = appdata.as_ref().map(|a| a.join("ATLauncher")) {
        found.extend(children(&atl.join("instances")));
    }
    found.extend(super::modrinth_app::instances());
    // 同一个文件夹只列一次（CurseForge 两个默认位置可能是同一个）
    let mut seen = std::collections::HashSet::new();
    found.retain(|i| seen.insert(instance::path_key(&i.folder)));
    found
}

/// Prism 放实例的文件夹：`prismlauncher.cfg` 的 `InstanceDir`（默认 `instances`，可以是绝对路径）
fn prism_instance_dir(data_dir: &Path) -> PathBuf {
    let configured = std::fs::read_to_string(data_dir.join("prismlauncher.cfg"))
        .ok()
        .and_then(|text| ini_value(&text, "InstanceDir"))
        .filter(|v| !v.is_empty());
    match configured.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => data_dir.join(dir),
        None => data_dir.join("instances"),
    }
}

/// Prism 的 ini（`键=值`）里的一个值：去掉引号，还原转义
fn ini_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(v);
        Some(
            v.replace("\\\\", "\u{0}")
                .replace("\\n", " ")
                .replace("\\t", " ")
                .replace("\\\"", "\"")
                .replace('\u{0}', "\\"),
        )
    })
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn prism(folder: &Path) -> Option<ForeignInstance> {
    let pack = read_json(&folder.join("mmc-pack.json"))?;
    let components = pack.get("components")?.as_array()?;
    let version_of = |uid: &str| {
        components
            .iter()
            .find(|c| c.get("uid").and_then(Value::as_str) == Some(uid))
            .and_then(|c| text(c.get("version")).or_else(|| text(c.get("cachedVersion"))))
    };
    let game_version = version_of("net.minecraft")?;
    let has_legacy_intermediary = components.iter().any(|c| {
        c.get("uid")
            .and_then(Value::as_str)
            .is_some_and(|uid| uid.eq_ignore_ascii_case("net.legacyfabric.intermediary"))
    });
    let loader = [
        ("net.neoforged", LoaderKind::NeoForge),
        ("net.minecraftforge", LoaderKind::Forge),
        ("net.fabricmc.fabric-loader", LoaderKind::Fabric),
        ("org.quiltmc.quilt-loader", LoaderKind::Quilt),
    ]
    .into_iter()
    .find_map(|(uid, kind)| {
        version_of(uid).map(|version| {
            let kind = if kind == LoaderKind::Fabric && game_version == "b1.7.3" {
                LoaderKind::Babric
            } else if kind == LoaderKind::Fabric
                && (has_legacy_intermediary
                    || LoaderKind::LegacyFabric.installable_for(&game_version))
            {
                LoaderKind::LegacyFabric
            } else {
                kind
            };
            LoaderRef { kind, version }
        })
    });
    let cfg = std::fs::read_to_string(folder.join("instance.cfg")).unwrap_or_default();
    let name = ini_value(&cfg, "name")
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| folder_name(folder));
    let game_dir = [".minecraft", "minecraft"]
        .iter()
        .map(|sub| folder.join(sub))
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| folder.join(".minecraft"));
    // MultiMC 和 Prism 一个格式：看数据文件夹（实例文件夹的上上层）里是谁的设置文件
    let launcher = if folder
        .parent()
        .and_then(Path::parent)
        .is_some_and(|data| data.join("multimc.cfg").is_file())
    {
        Launcher::MultiMc
    } else {
        Launcher::Prism
    };
    Some(ForeignInstance {
        launcher,
        name,
        game_version,
        loader,
        folder: folder.to_path_buf(),
        game_dir,
    })
}

/// CurseForge 的加载器名：`neoforge-21.1.77`、`forge-47.2.0`、`fabric-0.15.3-1.20.1`、`quilt-0.20.2-1.20.1`
fn curseforge_loader(name: &str, game_version: &str) -> Option<LoaderRef> {
    let (kind, rest) = name.split_once('-')?;
    let kind = match kind.to_ascii_lowercase().as_str() {
        "neoforge" | "neo" => LoaderKind::NeoForge,
        "forge" => LoaderKind::Forge,
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        "legacyfabric" | "legacy-fabric" => LoaderKind::LegacyFabric,
        "babric" => LoaderKind::Babric,
        _ => return None,
    };
    let version = rest
        .strip_suffix(&format!("-{game_version}"))
        .unwrap_or(rest)
        .to_owned();
    (!version.is_empty()).then_some(LoaderRef { kind, version })
}

fn curseforge(folder: &Path) -> Option<ForeignInstance> {
    let json = read_json(&folder.join("minecraftinstance.json"))?;
    let game_version = text(json.get("gameVersion"))?;
    let loader = json
        .get("baseModLoader")
        .and_then(|l| text(l.get("name")))
        .and_then(|name| curseforge_loader(&name, &game_version));
    Some(ForeignInstance {
        launcher: Launcher::CurseForge,
        name: text(json.get("name")).unwrap_or_else(|| folder_name(folder)),
        game_version,
        loader,
        folder: folder.to_path_buf(),
        game_dir: folder.to_path_buf(),
    })
}

fn atlauncher(folder: &Path) -> Option<ForeignInstance> {
    let json = read_json(&folder.join("instance.json"))?;
    // 别的启动器也可能有 instance.json：ATLauncher 的一定有 launcher 这一段
    let launcher = json.get("launcher")?;
    let game_version = text(json.get("id")).or_else(|| text(json.get("minecraftVersion")))?;
    let loader = launcher.get("loaderVersion").and_then(|l| {
        let kind = match text(l.get("type"))?.to_ascii_lowercase().as_str() {
            "neoforge" => LoaderKind::NeoForge,
            "forge" => LoaderKind::Forge,
            "fabric" | "legacyfabric" => LoaderKind::Fabric,
            "quilt" => LoaderKind::Quilt,
            _ => return None,
        };
        Some(LoaderRef {
            kind,
            version: text(l.get("version"))?,
        })
    });
    Some(ForeignInstance {
        launcher: Launcher::AtLauncher,
        name: text(launcher.get("name")).unwrap_or_else(|| folder_name(folder)),
        game_version,
        loader,
        folder: folder.to_path_buf(),
        game_dir: folder.to_path_buf(),
    })
}

/// 导入：把游戏文件复制进 `instances/<ID>/`（先复制到临时文件夹，中途取消不留半个实例），
/// 登记成本启动器的实例。游戏本体和加载器启动前由本启动器自己装（很慢，在后台线程里调用）。
pub fn import(
    game: &GameDir,
    foreign: &ForeignInstance,
    cancel: &CancellationToken,
    on_progress: impl Fn(CopyProgress),
) -> Result<Instance, CopyError> {
    let id = instance::unique_id(game, &foreign.name);
    let options = CopyOptions {
        name: foreign.name.clone(),
        id: id.clone(),
        include_worlds: true,
    };
    let skip: Vec<String> = LAUNCHER_FILES.iter().map(|s| (*s).to_owned()).collect();
    let dest = instance::copy_folder(
        game,
        &foreign.game_dir,
        &options,
        &skip,
        cancel,
        on_progress,
    )?;
    let mut config =
        InstanceConfig::new(&foreign.name, &foreign.game_version, foreign.loader.clone());
    let prism_extras = if matches!(foreign.launcher, Launcher::Prism | Launcher::MultiMc) {
        match crate::modpack::multimc::read_folder_extras(&foreign.folder) {
            Ok(extras) => {
                config.recommended_memory_mb = extras.cfg.recommended_memory_mb;
                config.overrides = extras.cfg.overrides.clone().normalized();
                if let Some(bundle) = &extras.patches
                    && foreign
                        .loader
                        .as_ref()
                        .is_some_and(|l| l.kind == LoaderKind::Forge)
                    && bundle.provides_legacy_forge()
                {
                    // Prism/MultiMC 自带 Forge patches（如 GTNH）时直接以原版为底图加载补丁
                    config.version_id = foreign.game_version.clone();
                }
                Some(extras)
            }
            Err(error) => {
                tracing::warn!(%error, folder = %foreign.folder.display(), "读取 Prism/MultiMC 补丁失败，按普通实例导入");
                None
            }
        }
    } else {
        None
    };
    match instance::register(game, &id, &config) {
        Ok(instance) => {
            if let Some(extras) = prism_extras {
                if let Some(bundle) = &extras.patches
                    && let Err(error) = crate::meta::patches::save_for_instance(&dest, bundle)
                {
                    tracing::warn!(%error, to = %id, "保存 Prism/MultiMC 补丁失败");
                }
                if let Some(notes) = &extras.cfg.notes
                    && let Err(error) = crate::fs::write_atomic(
                        &dest.join(instance::META_DIR).join("notes.md"),
                        notes.as_bytes(),
                    )
                {
                    tracing::warn!(%error, to = %id, "保存 Prism/MultiMC 实例笔记失败");
                }
                let libs_root = dest
                    .join(instance::META_DIR)
                    .join(crate::meta::patches::LOCAL_LIBS_DIR);
                for (src, rel) in extras.local_libraries {
                    if let Ok(target) = crate::fs::safe_join(&libs_root, &rel) {
                        if let Some(parent) = target.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Err(error) = std::fs::copy(&src, &target) {
                            tracing::warn!(%error, src = %src.display(), "复制 Prism/MultiMC 本地补丁库失败");
                        }
                    }
                }
            }
            tracing::info!(
                launcher = foreign.launcher.label(),
                to = %id,
                version = %foreign.game_version,
                loader = ?foreign.loader,
                "导入了别家的实例"
            );
            Ok(instance)
        }
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_dir_all(&dest) {
                tracing::warn!(error = %cleanup, dir = %dest.display(), "清理导入失败的实例失败");
            }
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_prism_instances() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("instances").join("Fabulously");
        write(
            &folder.join("instance.cfg"),
            "[General]\nInstanceType=OneSix\nname=\"我的 \\\"生存\\\"\"\n",
        );
        write(
            &folder.join("mmc-pack.json"),
            r#"{"components":[{"uid":"org.lwjgl3","version":"3.3.3"},
                {"uid":"net.minecraft","version":"1.21.1"},
                {"uid":"net.neoforged","version":"21.1.77"}],"formatVersion":1}"#,
        );
        write(&folder.join(".minecraft").join("mods").join("a.jar"), "x");
        let found = inspect_any(temp.path());
        assert_eq!(found.len(), 1);
        let one = &found[0];
        assert_eq!(one.launcher, Launcher::Prism);
        assert_eq!(one.name, "我的 \"生存\"");
        assert_eq!(one.game_version, "1.21.1");
        assert_eq!(
            one.loader,
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.77".into()
            })
        );
        assert_eq!(one.game_dir, folder.join(".minecraft"));

        // 选到了实例里面的 .minecraft、.minecraft/mods：往上找到实例
        for picked in [
            folder.join(".minecraft"),
            folder.join(".minecraft").join("mods"),
        ] {
            let found = inspect_any(&picked);
            assert_eq!(found.len(), 1, "{}", picked.display());
            assert_eq!(found[0].folder, folder);
            // 拖进来的不往上找
            assert!(inspect_here(&picked).is_empty(), "{}", picked.display());
        }
        assert_eq!(inspect_here(&folder).len(), 1);
    }

    #[test]
    fn modrinth_app_instances_match_by_folder() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("ModrinthApp");
        let base = &data.join("profiles");
        let one = |name: &str| ForeignInstance {
            launcher: Launcher::ModrinthApp,
            name: name.to_owned(),
            game_version: "1.21.1".into(),
            loader: None,
            folder: base.join(name),
            game_dir: base.join(name),
        };
        let all = || vec![one("二号"), one("一号")];
        let names = |found: Vec<ForeignInstance>| -> Vec<String> {
            found.into_iter().map(|i| i.name).collect()
        };
        assert_eq!(names(near(&base.join("一号"), all(), true)), ["一号"]);
        assert_eq!(
            names(near(&base.join("一号").join("mods"), all(), true)),
            ["一号"]
        );
        assert_eq!(names(near(base, all(), true)), ["一号", "二号"]);
        assert_eq!(names(near(&data, all(), true)), ["一号", "二号"]);
        assert!(near(&temp.path().join("别的"), all(), true).is_empty());
        // 不往上找：实例本身、放实例的文件夹认，实例里面的不认
        assert_eq!(names(near(&base.join("一号"), all(), false)), ["一号"]);
        assert!(near(&base.join("一号").join("mods"), all(), false).is_empty());
        assert_eq!(names(near(base, all(), false)), ["一号", "二号"]);
    }

    #[test]
    fn reads_curseforge_and_atlauncher_instances() {
        let temp = tempfile::tempdir().unwrap();
        let cf = temp.path().join("cf");
        write(
            &cf.join("minecraftinstance.json"),
            r#"{"name":"All the Mods","gameVersion":"1.20.1","baseModLoader":{"name":"fabric-0.15.3-1.20.1"}}"#,
        );
        let found = inspect(&cf).unwrap();
        assert_eq!(found.launcher, Launcher::CurseForge);
        assert_eq!(found.name, "All the Mods");
        assert_eq!(
            found.loader,
            Some(LoaderRef {
                kind: LoaderKind::Fabric,
                version: "0.15.3".into()
            })
        );
        assert_eq!(found.game_dir, cf);
        assert_eq!(
            curseforge_loader("neoforge-21.1.77", "1.21.1")
                .unwrap()
                .version,
            "21.1.77"
        );

        let atl = temp.path().join("atl");
        write(
            &atl.join("instance.json"),
            r#"{"id":"1.21.1","launcher":{"name":"ATL 包","loaderVersion":{"type":"NeoForge","version":"21.1.80"}}}"#,
        );
        let found = inspect(&atl).unwrap();
        assert_eq!(found.launcher, Launcher::AtLauncher);
        assert_eq!(found.name, "ATL 包");
        assert_eq!(found.loader.unwrap().kind, LoaderKind::NeoForge);

        // 本启动器自己的实例（instance.json 在 Lumi/ 里）、随便一个 instance.json 都不认
        let other = temp.path().join("other");
        write(&other.join("instance.json"), r#"{"id":"1.21.1"}"#);
        assert!(inspect(&other).is_none());
    }

    #[test]
    fn import_copies_game_files_and_leaves_launcher_files() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path().join("ours"));
        let cf = temp.path().join("cf").join("Pack");
        write(
            &cf.join("minecraftinstance.json"),
            r#"{"name":"Pack","gameVersion":"1.21.1","baseModLoader":{"name":"neoforge-21.1.77"}}"#,
        );
        write(&cf.join("mods").join("a.jar"), "jar");
        write(&cf.join("saves").join("世界").join("level.dat"), "nbt");
        write(&cf.join("logs").join("latest.log"), "log");
        let foreign = inspect(&cf).unwrap();

        let imported = import(&game, &foreign, &CancellationToken::new(), |_| {}).unwrap();
        let dir = imported.game_dir(&game).unwrap();
        assert!(dir.join("mods").join("a.jar").is_file());
        assert!(dir.join("saves").join("世界").join("level.dat").is_file());
        assert!(!dir.join("minecraftinstance.json").exists());
        assert!(!dir.join("logs").exists());
        assert_eq!(imported.version_id(), "neoforge-21.1.77");
        assert!(!imported.is_linked());
        assert!(cf.join("mods").join("a.jar").is_file(), "原来的不动");

        // 再导入一次：换个名字，不覆盖
        let again = import(&game, &foreign, &CancellationToken::new(), |_| {}).unwrap();
        assert_eq!(again.id, "Pack (2)");
    }

    #[test]
    fn import_prism_preserves_patches_jvm_args_local_libs_and_notes() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path().join("ours"));
        let folder = temp.path().join("prism").join("instances").join("GTNH");
        write(
            &folder.join("instance.cfg"),
            "[General]\nname=GTNH\nOverrideMemory=true\nMaxMemAlloc=8192\nOverrideJavaArgs=true\nJvmArgs=-XX:+UseZGC\nnotes=Play with Java 21\n",
        );
        write(
            &folder.join("mmc-pack.json"),
            r#"{"formatVersion":1,"components":[
                {"uid":"org.lwjgl3","version":"3.3.3"},
                {"uid":"net.minecraft","version":"1.7.10"},
                {"uid":"net.minecraftforge","version":"10.13.4.1614"},
                {"uid":"com.gtnewhorizons.lwjgl3ify.forgepatches","version":"1.0"}
            ]}"#,
        );
        write(
            &folder.join("patches").join("net.minecraftforge.json"),
            r#"{"uid":"net.minecraftforge","mainClass":"net.minecraft.launchwrapper.Launch","+tweakers":["cpw.mods.fml.common.launcher.FMLTweaker"],"libraries":[{"name":"net.minecraftforge:forge:1.7.10-10.13.4.1614","url":"https://maven.minecraftforge.net"}]}"#,
        );
        write(
            &folder
                .join("patches")
                .join("com.gtnewhorizons.lwjgl3ify.forgepatches.json"),
            r#"{"uid":"com.gtnewhorizons.lwjgl3ify.forgepatches","compatibleJavaMajors":[17,21],"libraries":[{"name":"com.gtnewhorizons:helper:1.0","MMC-hint":"local","MMC-filename":"helper.jar"}]}"#,
        );
        write(&folder.join("libraries").join("helper.jar"), "local");
        write(
            &folder.join(".minecraft").join("mods").join("gt.jar"),
            "mod",
        );

        let foreign = inspect(&folder).unwrap();
        let imported = import(&game, &foreign, &CancellationToken::new(), |_| {}).unwrap();
        assert_eq!(
            imported.version_id(),
            "1.7.10",
            "补丁自带老 Forge 时以原版为底图"
        );
        assert_eq!(imported.config.recommended_memory_mb, Some(8192));
        assert_eq!(
            imported.config.overrides.memory.unwrap().max_memory_mb,
            8192
        );
        assert_eq!(
            imported.config.overrides.launch.as_ref().unwrap().jvm_args,
            "-XX:+UseZGC"
        );
        let dir = imported.game_dir(&game).unwrap();
        let bundle = crate::meta::patches::load_for_instance(&dir)
            .unwrap()
            .unwrap();
        assert!(bundle.replace_lwjgl2);
        assert_eq!(bundle.preferred_java_major(), Some(21));
        assert!(
            dir.join("Lumi")
                .join("libraries")
                .join("helper.jar")
                .is_file()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("Lumi").join("notes.md")).unwrap(),
            "Play with Java 21"
        );
    }
}
