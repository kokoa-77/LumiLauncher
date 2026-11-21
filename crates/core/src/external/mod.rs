//! 别家启动器。
//!
//! - [`root`]：挂上别家的 `.minecraft`（官方、PCL、HMCL……），里面的版本原地读、不挪文件
//! - [`foreign`]：从 Prism / MultiMC、CurseForge、ATLauncher、Modrinth App 导入实例（复制一份游戏文件）
//!
//! 只读认版本、认隔离要用的几个设置项；别家设置文件里的账号、令牌一律不碰。

pub mod foreign;
pub mod modrinth_app;
pub mod root;

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 是哪家启动器的东西
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Launcher {
    /// Minecraft 官方启动器
    Official,
    Pcl,
    Hmcl,
    Prism,
    /// MultiMC（和 Prism 同一种实例格式）
    MultiMc,
    CurseForge,
    AtLauncher,
    ModrinthApp,
    /// 认不出是谁的 `.minecraft`
    Other,
}

impl Launcher {
    /// 显示用的名字（专有名词，界面直接用）
    pub fn label(self) -> &'static str {
        match self {
            Launcher::Official => "官方启动器",
            Launcher::Pcl => "PCL",
            Launcher::Hmcl => "HMCL",
            Launcher::Prism => "Prism",
            Launcher::MultiMc => "MultiMC",
            Launcher::CurseForge => "CurseForge",
            Launcher::AtLauncher => "ATLauncher",
            Launcher::ModrinthApp => "Modrinth App",
            Launcher::Other => "其他启动器",
        }
    }
}

/// 读「一行一个 `键:值`」的设置文件（PCL 的 `Setup.ini`、PCL 社区版的 `config.v1.yml` 都是这样），
/// 只留 `wanted` 里的键（别的键可能是账号信息，不读进内存）。文件不在时返回空表
pub(crate) fn read_flat_config(path: &Path, wanted: &[&str]) -> HashMap<String, String> {
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut values = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_start_matches('\u{feff}');
        if wanted.contains(&key) {
            let value = value.trim().trim_matches(['"', '\'']);
            values.insert(key.to_owned(), value.to_owned());
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_config_keeps_only_wanted_keys() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Setup.ini");
        std::fs::write(
            &path,
            "\u{feff}VersionArgumentIndieV2:True\r\nLoginMsJson:{\"token\":\"x\"}\r\nLaunchArgumentIndieV2: 1\n",
        )
        .unwrap();
        let values = read_flat_config(&path, &["VersionArgumentIndieV2", "LaunchArgumentIndieV2"]);
        assert_eq!(values.len(), 2);
        assert_eq!(values["VersionArgumentIndieV2"], "True");
        assert_eq!(values["LaunchArgumentIndieV2"], "1");
        assert!(read_flat_config(&temp.path().join("nope.ini"), &["a"]).is_empty());
    }
}
