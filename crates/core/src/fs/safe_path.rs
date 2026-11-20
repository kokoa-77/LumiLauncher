//! 安全路径拼接：所有来自外部的路径（整合包里的文件名、压缩包条目、网络返回的文件名）
//! 都必须经过这里，防止写到目标目录之外（路径穿越）或写出 Windows 不认的文件名。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnsafePath {
    #[error("路径为空")]
    Empty,
    #[error("不允许绝对路径、盘符或网络路径：{0}")]
    Absolute(String),
    #[error("不允许用 .. 跳出目录：{0}")]
    ParentDir(String),
    #[error("文件名不合法：{0}")]
    InvalidName(String),
}

/// Windows 保留的设备名，不区分大小写，带扩展名也不行（如 `con.txt`）。
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const FORBIDDEN_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// 校验单个文件名（不含任何路径分隔符）。
pub fn safe_file_name(name: &str) -> Result<&str, UnsafePath> {
    let invalid = || UnsafePath::InvalidName(name.to_owned());
    if name.is_empty() || name == "." || name == ".." {
        return Err(invalid());
    }
    if name
        .chars()
        .any(|c| FORBIDDEN_CHARS.contains(&c) || c.is_control())
    {
        return Err(invalid());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(invalid());
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(invalid());
    }
    Ok(name)
}

/// 把不可信的相对路径安全地拼到 `base` 下面。
///
/// `/` 和 `\` 都当作分隔符；空段和 `.` 段被忽略；任何绝对路径、盘符、`..`、非法文件名都拒绝。
pub fn safe_join(base: &Path, untrusted: &str) -> Result<PathBuf, UnsafePath> {
    if untrusted.starts_with('/') || untrusted.starts_with('\\') || untrusted.contains(':') {
        return Err(UnsafePath::Absolute(untrusted.to_owned()));
    }

    let mut result = base.to_path_buf();
    let mut pushed = 0usize;
    for segment in untrusted.split(['/', '\\']) {
        match segment {
            "" | "." => continue,
            ".." => return Err(UnsafePath::ParentDir(untrusted.to_owned())),
            name => {
                safe_file_name(name).map_err(|_| UnsafePath::InvalidName(untrusted.to_owned()))?;
                result.push(name);
                pushed += 1;
            }
        }
    }

    if pushed == 0 {
        return Err(UnsafePath::Empty);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PathBuf {
        PathBuf::from("game")
    }

    #[test]
    fn accepts_normal_relative_paths_with_either_separator() {
        assert_eq!(
            safe_join(&base(), "mods/sodium.jar").unwrap(),
            base().join("mods").join("sodium.jar")
        );
        assert_eq!(
            safe_join(&base(), r"config\iris\settings.json").unwrap(),
            base().join("config").join("iris").join("settings.json")
        );
        assert_eq!(
            safe_join(&base(), "./overrides//options.txt").unwrap(),
            base().join("overrides").join("options.txt")
        );
        assert_eq!(
            safe_join(&base(), "资源包/中文.zip").unwrap(),
            base().join("资源包").join("中文.zip")
        );
    }

    #[test]
    fn rejects_escaping_the_base_directory() {
        for bad in [
            "../evil.jar",
            "mods/../../evil.jar",
            r"mods\..\..\evil.jar",
            "..",
        ] {
            assert!(
                matches!(safe_join(&base(), bad), Err(UnsafePath::ParentDir(_))),
                "应该拒绝：{bad}"
            );
        }
    }

    #[test]
    fn rejects_absolute_drive_and_network_paths() {
        for bad in [
            "/etc/passwd",
            r"\Windows\System32",
            r"C:\Windows",
            "C:evil",
            r"\\server\share\x",
            "mods/a.jar:stream",
        ] {
            assert!(
                matches!(safe_join(&base(), bad), Err(UnsafePath::Absolute(_))),
                "应该拒绝：{bad}"
            );
        }
    }

    #[test]
    fn rejects_empty_paths() {
        for bad in ["", "/", "./.", "//"] {
            assert!(safe_join(&base(), bad).is_err(), "应该拒绝：{bad:?}");
        }
    }

    #[test]
    fn rejects_windows_unfriendly_names() {
        for bad in [
            "con",
            "CON.txt",
            "mods/nul.jar",
            "lpt1",
            "trailing.",
            "trailing ",
            "a|b",
            "q?.jar",
        ] {
            assert!(safe_join(&base(), bad).is_err(), "应该拒绝：{bad}");
        }
        assert!(safe_file_name("console.jar").is_ok());
        assert!(safe_file_name("com10.txt").is_ok());
    }
}
