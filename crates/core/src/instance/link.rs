//! 挂上的别家版本：别家启动器（官方、PCL、HMCL……）`.minecraft` 里的版本，原地读、不挪文件。
//!
//! 在本启动器的 `instances/<ID>/Lumi/instance.json` 里记一份（名字、分组、单独设置都和自己的实例一样），
//! 多一个 `link` 记着版本在哪：
//! - 版本描述、依赖库、资源从别家的 `.minecraft` 读（缺的照样补进那边，和别家启动器自己补文件一样）
//! - 游戏工作目录按那边的版本隔离设置：开了是 `versions/<版本>/`，没开是 `.minecraft` 本身
//! - 改动历史、启动记录、世界备份这些放在游戏工作目录的 `Lumi/` 里（和 PCL 自己的 `PCL/` 文件夹并排）
//!
//! 移除挂上的实例只删本启动器这边的 `instances/<ID>/`，别家的文件一个不动。
//! 扫描别家的目录、同步成实例在 [`crate::external::root`]。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{CONFIG_FILE, Instance, InstanceError, META_DIR, list, load};
use crate::external::Launcher;
use crate::paths::GameDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// 别家的 `.minecraft`
    pub root: PathBuf,
    /// `versions/` 里的文件夹名
    pub version: String,
    /// 游戏工作目录（见文件开头）
    pub game_dir: PathBuf,
    pub launcher: Launcher,
}

impl Link {
    /// 版本文件夹：`<.minecraft>/versions/<版本>/`
    pub fn version_dir(&self) -> PathBuf {
        self.root.join("versions").join(&self.version)
    }

    /// 开了版本隔离（游戏工作目录是版本文件夹）
    pub fn isolated(&self) -> bool {
        same_path(&self.game_dir, &self.version_dir())
    }
}

/// 两个路径是不是同一个文件夹（Windows 上不分大小写，不管 `/` `\` 和末尾的分隔符）
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// 比较路径用的写法（见 [`same_path`]）
pub fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    } else {
        text.trim_end_matches('/').to_owned()
    }
}

/// 挂在这个 `.minecraft` 上的实例
pub fn linked_to(game: &GameDir, root: &Path) -> Vec<Instance> {
    list(game)
        .into_iter()
        .filter(|i| {
            i.config
                .link
                .as_ref()
                .is_some_and(|l| same_path(&l.root, root))
        })
        .collect()
}

/// 移除挂上的实例：只删本启动器这边的设置（`instances/<ID>/Lumi/instance.json`，再删掉空了的文件夹），
/// 别家的文件不动。不是挂上的实例不删，返回 `false`。
pub fn unlink(game: &GameDir, id: &str) -> Result<bool, InstanceError> {
    let Some(instance) = load(game, id)? else {
        return Ok(false);
    };
    if !instance.is_linked() {
        return Ok(false);
    }
    let dir = game.instance_dir(id)?;
    let meta = dir.join(META_DIR);
    std::fs::remove_file(meta.join(CONFIG_FILE))
        .map_err(|e| InstanceError::Io(format!("删不掉挂上的实例的设置：{e}")))?;
    // 只删空文件夹：里面万一还有别的东西（不该有），留着不动
    for folder in [&meta, &dir] {
        if let Err(error) = std::fs::remove_dir(folder) {
            tracing::debug!(dir = %folder.display(), %error, "文件夹不是空的，留着");
        }
    }
    tracing::info!(instance = %id, "移除了挂上的实例（别家的文件没动）");
    Ok(true)
}

/// 移除挂在这个 `.minecraft` 上的全部实例。返回移除了几个
pub fn unlink_root(game: &GameDir, root: &Path) -> usize {
    linked_to(game, root)
        .iter()
        .filter(|i| match unlink(game, &i.id) {
            Ok(removed) => removed,
            Err(error) => {
                tracing::warn!(instance = %i.id, %error, "移除挂上的实例失败");
                false
            }
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{InstanceConfig, create, register};

    fn link(root: &Path, version: &str, isolated: bool) -> Link {
        let version_dir = root.join("versions").join(version);
        Link {
            root: root.to_path_buf(),
            version: version.to_owned(),
            game_dir: if isolated {
                version_dir
            } else {
                root.to_path_buf()
            },
            launcher: Launcher::Pcl,
        }
    }

    #[test]
    fn paths_compare_like_windows() {
        assert!(same_path(
            Path::new(r"D:\PCL2\.minecraft\"),
            Path::new("d:/pcl2/.minecraft")
        ));
        assert!(!same_path(
            Path::new(r"D:\PCL2\.minecraft"),
            Path::new(r"D:\PCL3\.minecraft")
        ));
    }

    #[test]
    fn linked_instances_use_the_other_folder_and_unlink_leaves_it_alone() {
        let temp = tempfile::tempdir().unwrap();
        let game = GameDir::new(temp.path().join("ours"));
        let root = temp.path().join("PCL2").join(".minecraft");
        let version_dir = root.join("versions").join("1.20.1-Fabric");
        std::fs::create_dir_all(version_dir.join("mods")).unwrap();
        std::fs::write(version_dir.join("mods").join("a.jar"), b"x").unwrap();

        let mut config = InstanceConfig::new("1.20.1-Fabric", "1.20.1", None);
        config.version_id = "1.20.1-Fabric".to_owned();
        config.link = Some(link(&root, "1.20.1-Fabric", true));
        std::fs::create_dir_all(game.instance_dir("1.20.1-Fabric").unwrap()).unwrap();
        let linked = register(&game, "1.20.1-Fabric", &config).unwrap();
        create(
            &game,
            "自己的",
            &InstanceConfig::new("自己的", "1.21.1", None),
        )
        .unwrap();

        assert!(linked.is_linked());
        assert!(linked.config.link.as_ref().unwrap().isolated());
        assert_eq!(linked.game_dir(&game).unwrap(), version_dir);
        assert_eq!(linked.files(&game).root(), root.as_path());
        assert_eq!(linked.version_id(), "1.20.1-Fabric");
        assert_eq!(linked_to(&game, &root).len(), 1);

        // 自己的实例不会被当成挂上的删掉
        assert!(!unlink(&game, "自己的").unwrap());
        assert!(game.instance_dir("自己的").unwrap().exists());

        assert_eq!(unlink_root(&game, &root), 1);
        assert!(!game.instance_dir("1.20.1-Fabric").unwrap().exists());
        assert!(
            version_dir.join("mods").join("a.jar").exists(),
            "别家的文件不动"
        );
    }
}
