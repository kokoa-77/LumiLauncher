//! 游戏目录的结构：共用的游戏本体按标准 `.minecraft` 布局放（`versions/`、`libraries/`、`assets/`），
//! 实例放在 `instances/<实例名>/`（D18a，和 Prism、Modrinth App、CurseForge 一样，见 [`crate::instance`]）。
//!
//! 版本号、库路径、资源哈希都来自外部 json，拼成磁盘路径前在这里统一校验，
//! 保证不会写到游戏目录外面。

use std::path::{Path, PathBuf};

use crate::fs::{UnsafePath, safe_file_name, safe_join};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDir {
    root: PathBuf,
}

impl GameDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// 放实例的文件夹（D18a）
    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }

    /// `instances/<实例 ID>/`：实例文件夹，也就是这个实例的游戏目录。ID 必须是单个合法的文件名。
    pub fn instance_dir(&self, id: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self.instances().join(safe_file_name(id)?))
    }

    /// `versions/<版本号>/`。版本号必须是单个合法的文件名。
    pub fn version_dir(&self, id: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self.versions().join(safe_file_name(id)?))
    }

    pub fn version_json(&self, id: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self.version_dir(id)?.join(format!("{id}.json")))
    }

    pub fn version_jar(&self, id: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self.version_dir(id)?.join(format!("{id}.jar")))
    }

    /// 解压本地库（.dll）的目录：`versions/<版本号>/natives-<系统>-<架构>`
    pub fn natives_dir(&self, id: &str, os: &str, arch: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self.version_dir(id)?.join(format!("natives-{os}-{arch}")))
    }

    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    /// 库文件路径。`relative` 来自 json，必须经过安全拼接。
    pub fn library(&self, relative: &str) -> Result<PathBuf, UnsafePath> {
        safe_join(&self.libraries(), relative)
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn asset_index(&self, id: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self
            .assets()
            .join("indexes")
            .join(format!("{}.json", safe_file_name(id)?)))
    }

    /// 资源文件：`assets/objects/<哈希前两位>/<哈希>`。哈希必须是 40 位十六进制。
    pub fn asset_object(&self, hash: &str) -> Result<PathBuf, UnsafePath> {
        if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(UnsafePath::InvalidName(hash.to_owned()));
        }
        Ok(self.assets().join("objects").join(&hash[..2]).join(hash))
    }

    pub fn log_config(&self, id: &str) -> Result<PathBuf, UnsafePath> {
        Ok(self.assets().join("log_configs").join(safe_file_name(id)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_standard_layout() {
        let game = GameDir::new("D:/mc");
        assert_eq!(
            game.version_json("1.21.1").unwrap(),
            PathBuf::from("D:/mc/versions/1.21.1/1.21.1.json")
        );
        assert_eq!(
            game.version_jar("1.21.1").unwrap(),
            PathBuf::from("D:/mc/versions/1.21.1/1.21.1.jar")
        );
        assert_eq!(
            game.instance_dir("我的生存").unwrap(),
            PathBuf::from("D:/mc/instances/我的生存")
        );
        assert_eq!(
            game.asset_object("0123456789abcdef0123456789abcdef01234567")
                .unwrap(),
            PathBuf::from("D:/mc/assets/objects/01/0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            game.library("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar")
                .unwrap(),
            PathBuf::from("D:/mc/libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar")
        );
    }

    #[test]
    fn rejects_hostile_names() {
        let game = GameDir::new("D:/mc");
        assert!(game.version_dir("../escape").is_err());
        assert!(game.version_dir("a/b").is_err());
        assert!(game.instance_dir("../escape").is_err());
        assert!(game.library("../../Windows/evil.jar").is_err());
        assert!(game.asset_object("../../../../etc").is_err());
        assert!(
            game.asset_object("zz23456789abcdef0123456789abcdef01234567")
                .is_err()
        );
        assert!(game.asset_index("../x").is_err());
    }
}
