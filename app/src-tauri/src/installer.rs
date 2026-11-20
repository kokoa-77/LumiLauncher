//! Windows 安装器的游戏目录交接、只读查找和资源管理器入口。

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use launcher_core::config::SettingsStore;
use launcher_core::external::{foreign, root};

pub const SCAN_ARG: &str = "--lumi-setup-scan";
pub const OPEN_PACK_ARG: &str = "--open-mrpack";
const GAME_LOCATION: &str = ".lumi-setup-game-dir";

pub fn scan() -> i32 {
    let excluded = std::env::args_os().nth(2).map(PathBuf::from);
    let versions: usize = root::discover(excluded.as_slice())
        .iter()
        .map(|path| root::count_versions(path))
        .sum();
    let count = versions + foreign::known_instances().len();
    i32::from(write!(io::stdout(), "{count}").is_err())
}

pub fn pack_argument(flag: &OsStr, path: &OsStr) -> Option<PathBuf> {
    if flag != OPEN_PACK_ARG {
        return None;
    }
    let path = PathBuf::from(path);
    (path.is_absolute()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mrpack"))
        && path.is_file())
    .then_some(path)
}

fn parse_game_location(bytes: &[u8]) -> Option<(PathBuf, bool)> {
    let bytes = bytes.strip_prefix(&[0xff, 0xfe])?;
    let (chunks, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let wide: Vec<_> = chunks.iter().copied().map(u16::from_le_bytes).collect();
    let value = String::from_utf16(&wide).ok()?;
    let (mode, path) = value.split_once('\n')?;
    let explicit = match mode.trim() {
        "default" => false,
        "selected" => true,
        _ => return None,
    };
    let path = path.trim();
    if path.contains(['\n', '\r', '\0']) {
        return None;
    }
    let path = PathBuf::from(path);
    path.is_absolute().then_some((path, explicit))
}

pub fn apply_game_location(settings: &SettingsStore, data_dir: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let marker = exe.with_file_name(GAME_LOCATION);
    let Ok(bytes) = std::fs::read(&marker) else {
        return;
    };
    if let Some((path, explicit)) = parse_game_location(&bytes) {
        let mut next = settings.get();
        if explicit || (next.game.directory.is_none() && !data_dir.join("minecraft").is_dir()) {
            next.game.directory = Some(path.to_string_lossy().into_owned());
            if let Err(error) = settings.set(next) {
                tracing::warn!(%error, "无法保存安装时选择的游戏目录");
                return;
            }
        }
    }
    if let Err(error) = std::fs::remove_file(marker) {
        tracing::warn!(%error, "无法清除安装器游戏目录标记");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_opens_only_absolute_mrpack_files() {
        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("测试 pack.MRPACK");
        std::fs::write(&pack, []).unwrap();
        assert_eq!(
            pack_argument(OPEN_PACK_ARG.as_ref(), pack.as_os_str()),
            Some(pack)
        );
        assert!(pack_argument(OPEN_PACK_ARG.as_ref(), "relative.mrpack".as_ref()).is_none());
        assert!(pack_argument(OPEN_PACK_ARG.as_ref(), temp.path().as_os_str()).is_none());
    }

    #[test]
    fn game_path_marker_keeps_unicode_and_explicit_selection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("游戏目录");
        let text = format!("\u{feff}selected\r\n{}", path.display());
        let bytes: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse_game_location(&bytes), Some((path, true)));
        assert!(parse_game_location(b"default\nrelative").is_none());
    }
}
