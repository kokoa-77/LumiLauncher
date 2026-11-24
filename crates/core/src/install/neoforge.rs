//! NeoForge：版本列表、版本号对应关系、安装入口。
//!
//! 安装本身走 [`super::processors`]（Forge 系安装器的通用执行流程），这里只负责
//! 「哪个版本、从哪下、装成什么名字」。

use std::cmp::Ordering;
use std::path::Path;

use reqwest::Client;
use serde::Serialize;

use super::InstallError;
use super::processors::{self, InstallerSource, LoaderContext, LoaderProgress};
use crate::net::get_bytes;

const MAVEN: &str = "https://maven.neoforged.net/releases";
const ARTIFACT_PATH: &str = "net/neoforged/neoforge";
/// Minecraft 1.20.1 早期 NeoForge 47.1.x 发布在旧坐标 `net.neoforged:forge` 下
const LEGACY_ARTIFACT_PATH: &str = "net/neoforged/forge";
const LEGACY_1201_PREFIX: &str = "1.20.1-";

/// 一个 NeoForge 版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct NeoForgeVersion {
    /// NeoForge 版本号，例如 `21.1.265`
    pub version: String,
    /// 对应的 Minecraft 版本，例如 `1.21.1`
    pub game_version: String,
    /// 正式版（不是 beta / alpha）
    pub stable: bool,
}

impl NeoForgeVersion {
    /// 装好后的版本 ID（版本文件夹名），和官方安装器一致
    pub fn version_id(&self) -> String {
        version_id(&self.version)
    }
}

/// 识别 1.20.1 早期 NeoForge 47.1.x 版本号（支持 `47.1.106` 与 `1.20.1-47.1.106`），返回去掉 `1.20.1-` 后的短版本号。
fn legacy_1201_short(loader_version: &str) -> Option<&str> {
    if loader_version.contains('+') {
        return None;
    }
    let short = loader_version
        .strip_prefix(LEGACY_1201_PREFIX)
        .unwrap_or(loader_version);
    let core = short.split('-').next()?;
    let parts: Vec<u32> = core
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    matches!(parts.as_slice(), [47, 1, _]).then_some(short)
}

pub fn version_id(loader_version: &str) -> String {
    let normalized = legacy_1201_short(loader_version).unwrap_or(loader_version);
    format!("neoforge-{normalized}")
}

/// 从 NeoForge 版本号推出 Minecraft 版本号：
/// - 1.20.1 早期分叉：`47.1.106` / `1.20.1-47.1.106` → `1.20.1`
/// - 2026 年以前：`21.1.265` → `1.21.1`，`21.0.167` → `1.21`
/// - 年份版本号：`26.3.0.102` → `26.3`，`26.3.1.5` → `26.3.1`
///
/// 快照版用的 `+snapshot-N` 版本和愚人节版本认不出来，返回 `None`（不支持）。
pub fn game_version_of(loader_version: &str) -> Option<String> {
    if legacy_1201_short(loader_version).is_some() {
        return Some("1.20.1".to_owned());
    }
    if loader_version.contains('+') {
        return None;
    }
    let core = loader_version.split('-').next()?;
    let parts: Vec<u32> = core
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [year, drop, hotfix, _build] if *year >= 26 => Some(if *hotfix == 0 {
            format!("{year}.{drop}")
        } else {
            format!("{year}.{drop}.{hotfix}")
        }),
        [minor, 0, _build] if *minor < 26 => Some(format!("1.{minor}")),
        [minor, patch, _build] if *minor < 26 => Some(format!("1.{minor}.{patch}")),
        _ => None,
    }
}

/// 拉取全部 NeoForge 版本（新的在前），并合并 1.20.1 旧坐标 `net/neoforged/forge` 的 47.1.x 版本。
pub async fn fetch_versions(client: &Client) -> Result<Vec<NeoForgeVersion>, InstallError> {
    let main_url = format!("{MAVEN}/{ARTIFACT_PATH}/maven-metadata.xml");
    let legacy_url = format!("{MAVEN}/{LEGACY_ARTIFACT_PATH}/maven-metadata.xml");
    let (main, legacy) = tokio::join!(
        get_bytes(client, std::slice::from_ref(&main_url)),
        get_bytes(client, std::slice::from_ref(&legacy_url)),
    );
    let mut versions = parse_metadata(&String::from_utf8_lossy(&main?));
    match legacy {
        Ok(bytes) => {
            versions.extend(parse_legacy_metadata(&String::from_utf8_lossy(&bytes)));
            versions.sort_by(|a, b| compare_loader_versions(&b.version, &a.version));
            versions.dedup_by(|a, b| a.version == b.version);
        }
        Err(error) => {
            tracing::warn!(%error, "读取 1.20.1 旧 NeoForge 版本列表失败，保留其余版本");
        }
    }
    Ok(versions)
}

/// 解析 1.20.1 旧坐标 `net/neoforged/forge/maven-metadata.xml` 里的 47.1.x 版本。
pub fn parse_legacy_metadata(xml: &str) -> Vec<NeoForgeVersion> {
    let mut versions: Vec<NeoForgeVersion> = xml
        .split("<version>")
        .skip(1)
        .filter_map(|chunk| chunk.split_once("</version>").map(|(v, _)| v.trim()))
        .filter_map(|raw| {
            let short = legacy_1201_short(raw)?;
            Some(NeoForgeVersion {
                game_version: "1.20.1".to_owned(),
                stable: !short.contains('-'),
                version: short.to_owned(),
            })
        })
        .collect();
    versions.sort_by(|a, b| compare_loader_versions(&b.version, &a.version));
    versions.dedup_by(|a, b| a.version == b.version);
    versions
}

/// 解析 Maven 的 `maven-metadata.xml`。格式固定，只取 `<version>` 标签，不引入 XML 库。
pub fn parse_metadata(xml: &str) -> Vec<NeoForgeVersion> {
    let mut versions: Vec<NeoForgeVersion> = xml
        .split("<version>")
        .skip(1)
        .filter_map(|chunk| chunk.split_once("</version>").map(|(v, _)| v.trim()))
        .filter_map(|version| {
            Some(NeoForgeVersion {
                game_version: game_version_of(version)?,
                stable: !version.contains('-'),
                version: version.to_owned(),
            })
        })
        .collect();
    versions.sort_by(|a, b| compare_loader_versions(&b.version, &a.version));
    versions.dedup_by(|a, b| a.version == b.version);
    versions
}

/// 某个 Minecraft 版本可用的最新 NeoForge（优先正式版）。
pub fn latest_for<'a>(
    versions: &'a [NeoForgeVersion],
    game_version: &str,
) -> Option<&'a NeoForgeVersion> {
    let mut candidates = versions.iter().filter(|v| v.game_version == game_version);
    let first = candidates.clone().next();
    candidates.find(|v| v.stable).or(first)
}

/// 按数字比较版本号；`-beta` 之类的后缀排在同号正式版之前。
pub(super) fn compare_loader_versions(a: &str, b: &str) -> Ordering {
    let split = |v: &str| {
        let (core, suffix) = v.split_once('-').unwrap_or((v, ""));
        let numbers: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        (numbers, suffix.to_owned())
    };
    let (a_num, a_suffix) = split(a);
    let (b_num, b_suffix) = split(b);
    a_num
        .cmp(&b_num)
        .then_with(|| match (a_suffix.is_empty(), b_suffix.is_empty()) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => a_suffix.cmp(&b_suffix),
        })
}

/// 安装一个 NeoForge 版本，返回版本 ID（例如 `neoforge-21.1.265`）。
/// 已经装好的直接返回（只检查文件在不在、大小对不对，很快）。
pub async fn install<F>(
    ctx: &LoaderContext<'_>,
    loader_version: &str,
    verify_receipt: bool,
    on_progress: F,
) -> Result<String, InstallError>
where
    F: Fn(LoaderProgress) + Send + Sync,
{
    let game_version = game_version_of(loader_version)
        .ok_or_else(|| InstallError::UnsupportedLoader(format!("NeoForge {loader_version}")))?;
    let source = match legacy_1201_short(loader_version) {
        Some(short) => InstallerSource {
            expected_id: version_id(short),
            accepted_installer_id: Some(format!("1.20.1-forge-{short}")),
            game_version,
            installer_url: format!(
                "{MAVEN}/{LEGACY_ARTIFACT_PATH}/{LEGACY_1201_PREFIX}{short}/forge-{LEGACY_1201_PREFIX}{short}-installer.jar"
            ),
        },
        None => InstallerSource {
            expected_id: version_id(loader_version),
            accepted_installer_id: None,
            game_version,
            installer_url: format!(
                "{MAVEN}/{ARTIFACT_PATH}/{loader_version}/neoforge-{loader_version}-installer.jar"
            ),
        },
    };
    processors::install(ctx, &source, verify_receipt, on_progress).await
}

/// 版本 ID 是不是 NeoForge（启动前用来决定要不要检查安装状态）。
pub fn loader_version_of(version_id: &str) -> Option<&str> {
    version_id.strip_prefix("neoforge-")
}

/// 工作目录：`<工作根目录>/neoforge-<版本>`
pub fn work_dir(base: &Path, loader_version: &str) -> std::path::PathBuf {
    base.join(version_id(loader_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_loader_versions_to_game_versions() {
        assert_eq!(game_version_of("21.1.265").as_deref(), Some("1.21.1"));
        assert_eq!(game_version_of("21.0.167").as_deref(), Some("1.21"));
        assert_eq!(game_version_of("20.2.3-beta").as_deref(), Some("1.20.2"));
        assert_eq!(game_version_of("21.10.64").as_deref(), Some("1.21.10"));
        assert_eq!(game_version_of("26.3.0.102").as_deref(), Some("26.3"));
        assert_eq!(game_version_of("26.3.1.5").as_deref(), Some("26.3.1"));
        assert_eq!(game_version_of("47.1.106").as_deref(), Some("1.20.1"));
        assert_eq!(
            game_version_of("1.20.1-47.1.106").as_deref(),
            Some("1.20.1")
        );
        assert_eq!(game_version_of("26.1.0.0-alpha.1+snapshot-1"), None);
        assert_eq!(game_version_of("0.25w14craftmine.3"), None);
    }

    #[test]
    fn parses_legacy_1201_metadata_and_normalizes_id() {
        let xml = "<metadata><versioning><versions>\
            <version>1.20.1-47.1.5</version><version>1.20.1-47.1.106</version>\
            <version>1.20.1-47.1.70-beta</version><version>1.20.1-47.0.1</version>\
            </versions></versioning></metadata>";
        let versions = parse_legacy_metadata(xml);
        let names: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(names, ["47.1.106", "47.1.70-beta", "47.1.5"]);
        assert_eq!(versions[0].game_version, "1.20.1");
        assert!(versions[0].stable);
        assert!(!versions[1].stable);
        assert_eq!(version_id("1.20.1-47.1.106"), "neoforge-47.1.106");
        assert_eq!(
            latest_for(&versions, "1.20.1").map(|v| v.version.as_str()),
            Some("47.1.106")
        );
    }

    #[test]
    fn parses_and_sorts_metadata() {
        let xml = "<metadata><versioning><versions>\
            <version>21.1.9</version><version>21.1.10</version><version>21.1.0-beta</version>\
            <version>21.0.167</version><version>26.1.0.0-alpha.1+snapshot-1</version>\
            <version>21.1.1</version></versions></versioning></metadata>";
        let versions = parse_metadata(xml);
        let names: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(
            names,
            ["21.1.10", "21.1.9", "21.1.1", "21.1.0-beta", "21.0.167"]
        );
        assert!(!versions[3].stable);
        assert_eq!(versions[0].version_id(), "neoforge-21.1.10");
    }

    #[test]
    fn latest_prefers_stable_builds() {
        let versions = parse_metadata(
            "<version>21.1.2-beta</version><version>21.1.1</version><version>21.0.5</version>",
        );
        assert_eq!(
            latest_for(&versions, "1.21.1").map(|v| v.version.as_str()),
            Some("21.1.1")
        );
        assert_eq!(
            latest_for(&versions, "1.21").map(|v| v.version.as_str()),
            Some("21.0.5")
        );
        assert_eq!(latest_for(&versions, "1.20.1"), None);
    }

    #[test]
    fn recognises_neoforge_version_ids() {
        assert_eq!(loader_version_of("neoforge-21.1.265"), Some("21.1.265"));
        assert_eq!(loader_version_of("1.21.1"), None);
    }
}
