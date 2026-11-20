//! 定名（2026-11-15「微光 / Lumi」）以前，应用 ID 是代号 `com.unnamed.mclauncher`：
//! 设置、下载历史、窗口位置在 `%APPDATA%\<ID>`，日志和界面自己存的偏好（WebView2 的数据）在
//! `%LOCALAPPDATA%\<ID>`。第一次用新 ID 打开时整个文件夹改名过去（同一个盘里改名，瞬间完成）。
//!
//! 必须在窗口创建之前做：窗口一建好，WebView2 就占住新位置了，所以在 `run()` 里、`builder.run` 之前调用；
//! 那时日志还没准备好，结果先存着，日志准备好以后再记（[`log`]）。

use std::path::{Path, PathBuf};

/// 定名前的应用 ID
const OLD_IDENTIFIER: &str = "com.unnamed.mclauncher";
/// 两个放应用数据的地方（Windows）。别的系统没有这两个环境变量，什么也不做
const BASES: [&str; 2] = ["APPDATA", "LOCALAPPDATA"];

/// 一个文件夹搬的结果
#[derive(Debug)]
pub enum Moved {
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// 新旧两个都在（新的已经用过了）：不合并，旧的原样留着
    BothExist {
        old: PathBuf,
    },
    Failed {
        from: PathBuf,
        error: std::io::Error,
    },
}

/// 把旧 ID 下的应用数据搬到 `identifier` 下。没有旧数据时返回空
pub fn migrate(identifier: &str) -> Vec<Moved> {
    BASES
        .iter()
        .filter_map(|var| std::env::var_os(var).map(PathBuf::from))
        .filter_map(|base| migrate_in(&base, identifier))
        .collect()
}

fn migrate_in(base: &Path, identifier: &str) -> Option<Moved> {
    if identifier == OLD_IDENTIFIER {
        return None;
    }
    let old = base.join(OLD_IDENTIFIER);
    if !old.is_dir() {
        return None;
    }
    let new = base.join(identifier);
    if new.exists() {
        return Some(Moved::BothExist { old });
    }
    Some(match std::fs::rename(&old, &new) {
        Ok(()) => Moved::Renamed { from: old, to: new },
        Err(error) => Moved::Failed { from: old, error },
    })
}

/// 日志准备好以后记下搬的结果
pub fn log(report: &[Moved]) {
    for moved in report {
        match moved {
            Moved::Renamed { from, to } => {
                tracing::info!(from = %from.display(), to = %to.display(), "定名后搬了应用数据");
            }
            Moved::BothExist { old } => {
                tracing::warn!(old = %old.display(), "定名前的应用数据还在，新位置也已经有数据：没有合并，旧的原样留着");
            }
            Moved::Failed { from, error } => {
                tracing::warn!(from = %from.display(), %error, "定名前的应用数据没搬成（可能旧版还开着），这次用新位置");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_folder_is_renamed_once() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join(OLD_IDENTIFIER);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), "{}").unwrap();

        let moved = migrate_in(base.path(), "com.lumilauncher.lumi");
        assert!(matches!(moved, Some(Moved::Renamed { .. })));
        let new = base.path().join("com.lumilauncher.lumi");
        assert!(new.join("settings.json").is_file());
        assert!(!old.exists());
        assert!(
            migrate_in(base.path(), "com.lumilauncher.lumi").is_none(),
            "搬过的不再搬"
        );
    }

    #[test]
    fn never_merges_into_a_used_folder() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join(OLD_IDENTIFIER);
        let new = base.path().join("com.lumilauncher.lumi");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("settings.json"), "新").unwrap();

        assert!(matches!(
            migrate_in(base.path(), "com.lumilauncher.lumi"),
            Some(Moved::BothExist { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(new.join("settings.json")).unwrap(),
            "新"
        );
        assert!(old.is_dir());
    }
}
