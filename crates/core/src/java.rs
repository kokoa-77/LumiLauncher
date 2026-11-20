//! Java 扫描、选择、自动下载（`runtime`）。
//!
//! 版本号读 Java 目录下的 `release` 文件（不用启动 Java，快）；
//! 32 / 64 位读 `java.exe` 的文件头判断（有的运行时 `release` 里没写架构）。

pub mod prefs;
pub mod runtime;

pub use prefs::{JavaEntry, JavaOverview, JavaPrefs, JavaPrefsStore, overview};
pub use runtime::{JavaContext, JavaError, JavaLocator, JavaRequest, find_or_install};

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha1::{Digest, Sha1};

/// 这个 Java 是从哪来的（决定界面上能不能删、能不能移除）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum JavaOrigin {
    /// 启动器自己下载的（在游戏目录的 `.mcl-java` 里，可以删，需要时会再下）
    Downloaded,
    /// 用户在设置里手动添加的
    Manual,
    /// 官方启动器自带的
    Official,
    /// 电脑上装的（JAVA_HOME、PATH、Program Files、注册表）
    System,
}

impl JavaOrigin {
    /// 日志、命令行里显示的名字
    pub fn label(self) -> &'static str {
        match self {
            JavaOrigin::Downloaded => "启动器下载",
            JavaOrigin::Manual => "手动添加",
            JavaOrigin::Official => "官方启动器",
            JavaOrigin::System => "电脑上装的",
        }
    }
}

/// 一个找到的 Java。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct JavaInstall {
    /// Java 主目录（里面有 `bin/`）
    pub home: PathBuf,
    /// 启动游戏用的程序（Windows 上是 `java.exe`，配合「不弹控制台窗口」标志使用，才能读到日志）
    pub executable: PathBuf,
    /// 完整版本号，例如 `21.0.7`、`1.8.0_381`
    pub version: String,
    /// 主版本号，例如 21、8
    pub major: u32,
    pub is_64bit: bool,
    /// 从哪来的
    pub origin: JavaOrigin,
    /// 谁做的（`release` 文件里的 `IMPLEMENTOR`，例如 Microsoft、Eclipse Adoptium）
    pub vendor: Option<String>,
}

impl JavaInstall {
    /// 界面用的 ID（由主目录算出来，同一个目录每次都一样）
    pub fn id(&self) -> String {
        java_id(&self.home)
    }
}

/// 比较目录用的写法：小写、统一成反斜杠、去掉末尾的斜杠
pub(crate) fn home_key(home: &Path) -> String {
    home.to_string_lossy()
        .to_lowercase()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_owned()
}

/// Java 的 ID：主目录的 SHA-1 前 12 位（界面只认 ID，不碰路径）
pub fn java_id(home: &Path) -> String {
    let digest = Sha1::digest(home_key(home).as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// 扫描这台电脑上的 Java。`downloaded` 是启动器自己下载的目录，`manual` 是用户手动添加的。
pub fn scan(downloaded: &[PathBuf], manual: &[PathBuf]) -> Vec<JavaInstall> {
    let mut candidates: Vec<(PathBuf, JavaOrigin)> = Vec::new();
    for home in downloaded {
        candidates.push((home.clone(), JavaOrigin::Downloaded));
    }
    for home in manual {
        candidates.push((home.clone(), JavaOrigin::Manual));
    }
    collect_official_runtimes(&mut candidates);
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push((PathBuf::from(home), JavaOrigin::System));
    }
    collect_path_entries(&mut candidates);
    collect_program_files(&mut candidates);
    collect_registry(&mut candidates);

    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for (home, origin) in candidates {
        if !seen.insert(home_key(&home)) {
            continue;
        }
        if let Some(install) = inspect(&home, origin) {
            found.push(install);
        }
    }
    found.sort_by(|a, b| {
        b.major
            .cmp(&a.major)
            .then_with(|| compare_versions(&b.version, &a.version))
    });
    found
}

/// 检查一个目录是不是可用的 Java。
pub fn inspect(home: &Path, origin: JavaOrigin) -> Option<JavaInstall> {
    let executable = home
        .join("bin")
        .join(if cfg!(windows) { "java.exe" } else { "java" });
    if !executable.is_file() {
        return None;
    }
    let release = std::fs::read_to_string(home.join("release")).ok()?;
    let version = release_value(&release, "JAVA_VERSION")?;
    let major = major_version(&version)?;
    let is_64bit = executable_is_64bit(&executable).unwrap_or(true);
    let vendor = release_value(&release, "IMPLEMENTOR").filter(|v| !v.is_empty());
    Some(JavaInstall {
        home: home.to_path_buf(),
        executable,
        version,
        major,
        is_64bit,
        origin,
        vendor,
    })
}

/// 用户选了一个 java.exe（或者 Java 的目录）：找到它的主目录。
/// `…/bin/java.exe`、`…/bin/javaw.exe`、`…/bin`、主目录本身都认。
pub fn home_of(picked: &Path) -> PathBuf {
    let is_exe = picked
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| {
            let n = n.to_ascii_lowercase();
            n == "java.exe" || n == "javaw.exe" || n == "java"
        });
    let dir = if is_exe {
        picked.parent().unwrap_or(picked)
    } else {
        picked
    };
    if dir
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
    {
        dir.parent().unwrap_or(dir).to_path_buf()
    } else {
        dir.to_path_buf()
    }
}

/// 按版本要求挑一个 Java：
/// 1. 主版本号一致的优先（64 位优先、小版本新的优先）
/// 2. 要求 17 及以上时，没有一致的就选比它新的里面最旧的那个
/// 3. 要求 16 及以下（老版本、老 Forge）只认一致的版本，新 Java 跑不了
pub fn select(installs: &[JavaInstall], required: u32) -> Option<&JavaInstall> {
    let better = |a: &&JavaInstall, b: &&JavaInstall| {
        a.is_64bit
            .cmp(&b.is_64bit)
            .then_with(|| compare_versions(&a.version, &b.version))
    };
    if let Some(exact) = installs
        .iter()
        .filter(|j| j.major == required)
        .max_by(better)
    {
        return Some(exact);
    }
    if required >= 17 {
        let lowest_newer = installs
            .iter()
            .filter(|j| j.major > required)
            .map(|j| j.major)
            .min()?;
        return installs
            .iter()
            .filter(|j| j.major == lowest_newer)
            .max_by(better);
    }
    None
}

fn release_value(release: &str, key: &str) -> Option<String> {
    release.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix('=')?;
        Some(value.trim().trim_matches('"').to_owned())
    })
}

/// `1.8.0_381` → 8；`21.0.7` → 21；`17` → 17
fn major_version(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// 逐段按数字比较版本号
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |v: &str| -> Vec<u64> {
        v.split(['.', '_', '-', '+'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(a).cmp(&parse(b))
}

/// 读 Windows 可执行文件头里的机器类型：0x014C 是 32 位 x86，其余（x64、ARM64）按 64 位算。
fn executable_is_64bit(path: &Path) -> Option<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf4 = [0u8; 4];
    file.seek(SeekFrom::Start(0x3C)).ok()?;
    file.read_exact(&mut buf4).ok()?;
    let pe_offset = u64::from(u32::from_le_bytes(buf4));
    file.seek(SeekFrom::Start(pe_offset)).ok()?;
    file.read_exact(&mut buf4).ok()?;
    if &buf4 != b"PE\0\0" {
        return None;
    }
    let mut machine = [0u8; 2];
    file.read_exact(&mut machine).ok()?;
    Some(u16::from_le_bytes(machine) != 0x014C)
}

/// 官方启动器（含微软商店版）自带的运行时。目录有两种结构：
/// `runtime/<组件>/bin/` 和 `runtime/<组件>/<平台>/<组件>/bin/`
fn collect_official_runtimes(out: &mut Vec<(PathBuf, JavaOrigin)>) {
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(appdata).join(".minecraft").join("runtime"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(local)
                .join("Packages")
                .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
                .join("LocalCache")
                .join("Local")
                .join("runtime"),
        );
    }
    for root in roots {
        for component in subdirs(&root) {
            out.push((component.clone(), JavaOrigin::Official));
            let name = component
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            for platform in subdirs(&component) {
                out.push((platform.join(&name), JavaOrigin::Official));
            }
        }
    }
}

fn collect_path_entries(out: &mut Vec<(PathBuf, JavaOrigin)>) {
    let Some(path) = std::env::var_os("PATH") else {
        return;
    };
    for dir in std::env::split_paths(&path) {
        if dir
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
            && let Some(home) = dir.parent()
        {
            out.push((home.to_path_buf(), JavaOrigin::System));
        }
    }
}

fn collect_program_files(out: &mut Vec<(PathBuf, JavaOrigin)>) {
    const VENDORS: &[&str] = &[
        "Java",
        "Eclipse Adoptium",
        "Eclipse Foundation",
        "Zulu",
        "Microsoft",
        "BellSoft",
        "Amazon Corretto",
        "Semeru",
        "Semeru Runtimes",
        "Liberica",
    ];
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        let Some(base) = std::env::var_os(var) else {
            continue;
        };
        for vendor in VENDORS {
            for home in subdirs(&PathBuf::from(&base).join(vendor)) {
                out.push((home, JavaOrigin::System));
            }
        }
    }
}

#[cfg(windows)]
fn collect_registry(out: &mut Vec<(PathBuf, JavaOrigin)>) {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for root in ["SOFTWARE", "SOFTWARE\\WOW6432Node"] {
        for kind in [
            "JDK",
            "JRE",
            "Java Development Kit",
            "Java Runtime Environment",
        ] {
            let Ok(key) = hklm.open_subkey(format!("{root}\\JavaSoft\\{kind}")) else {
                continue;
            };
            for version in key.enum_keys().flatten() {
                if let Ok(sub) = key.open_subkey(&version)
                    && let Ok(home) = sub.get_value::<String, _>("JavaHome")
                {
                    out.push((PathBuf::from(home), JavaOrigin::System));
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn collect_registry(_out: &mut Vec<(PathBuf, JavaOrigin)>) {}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn java(version: &str, is_64bit: bool) -> JavaInstall {
        JavaInstall {
            home: PathBuf::from(format!("C:/java/{version}")),
            executable: PathBuf::from("java.exe"),
            version: version.to_owned(),
            major: major_version(version).unwrap(),
            is_64bit,
            origin: JavaOrigin::System,
            vendor: None,
        }
    }

    #[test]
    fn parses_major_versions() {
        assert_eq!(major_version("1.8.0_381"), Some(8));
        assert_eq!(major_version("21.0.7"), Some(21));
        assert_eq!(major_version("17"), Some(17));
        assert_eq!(major_version("25-ea"), Some(25));
        assert_eq!(major_version("garbage"), None);
    }

    #[test]
    fn reads_release_file_values() {
        let release = "JAVA_VERSION=\"21.0.7\"\nMODULES=\"java.base\"\nOS_ARCH=\"amd64\"";
        assert_eq!(
            release_value(release, "JAVA_VERSION").as_deref(),
            Some("21.0.7")
        );
        assert_eq!(release_value(release, "OS_ARCH").as_deref(), Some("amd64"));
        assert_eq!(release_value(release, "MISSING"), None);
    }

    #[test]
    fn prefers_exact_major_then_64bit_then_newest_patch() {
        let installs = [
            java("21.0.3", true),
            java("21.0.7", true),
            java("21.0.9", false),
            java("17.0.12", true),
        ];
        assert_eq!(select(&installs, 21).unwrap().version, "21.0.7");
        assert_eq!(select(&installs, 17).unwrap().version, "17.0.12");
    }

    #[test]
    fn modern_versions_fall_back_to_the_closest_newer_java() {
        let installs = [java("25.0.1", true), java("22.0.2", true)];
        assert_eq!(select(&installs, 21).unwrap().major, 22);
    }

    #[test]
    fn old_versions_never_get_a_newer_java() {
        let installs = [java("21.0.7", true), java("17.0.12", true)];
        assert!(select(&installs, 8).is_none(), "老版本只能用 Java 8");
    }

    #[test]
    fn finds_the_home_of_a_picked_file() {
        let home = PathBuf::from(r"C:\Java\jdk-21");
        assert_eq!(home_of(Path::new(r"C:\Java\jdk-21\bin\java.exe")), home);
        assert_eq!(home_of(Path::new(r"C:\Java\jdk-21\bin\javaw.exe")), home);
        assert_eq!(home_of(Path::new(r"C:\Java\jdk-21\bin")), home);
        assert_eq!(home_of(Path::new(r"C:\Java\jdk-21")), home);
    }

    #[test]
    fn ids_ignore_case_and_slashes() {
        let id = java_id(Path::new(r"C:\Java\JDK-21\"));
        assert_eq!(id.len(), 12);
        assert_eq!(id, java_id(Path::new("c:/java/jdk-21")));
        assert_ne!(id, java_id(Path::new(r"C:\Java\jdk-17")));
    }

    #[test]
    fn reads_the_vendor_from_the_release_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        std::fs::write(bin.join(exe), b"").unwrap();
        std::fs::write(
            dir.path().join("release"),
            "IMPLEMENTOR=\"Microsoft\"\nJAVA_VERSION=\"21.0.7\"\n",
        )
        .unwrap();
        let found = inspect(dir.path(), JavaOrigin::Manual).unwrap();
        assert_eq!(found.major, 21);
        assert_eq!(found.vendor.as_deref(), Some("Microsoft"));
        assert_eq!(found.origin, JavaOrigin::Manual);
    }

    #[test]
    fn compares_versions_numerically() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("21.0.10", "21.0.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.8.0_381", "1.8.0_45"), Ordering::Greater);
    }
}
