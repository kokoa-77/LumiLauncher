//! Mojang 版本清单（`version_manifest_v2.json`）。

use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::MetaError;
use super::version::VersionJson;
use crate::net::{get_bytes, get_json, to_hex};

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub id: String,
    /// release / snapshot / old_beta / old_alpha（用字符串，Mojang 以后加新类型也不会解析失败）
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub release_time: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum GameVersionCategory {
    Release,
    Snapshot,
    AprilFools,
    Ancient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct GameVersionEntry {
    pub id: String,
    pub category: GameVersionCategory,
    pub release_time: String,
    pub is_latest_release: bool,
    pub is_latest_snapshot: bool,
}

/// 识别 Mojang 愚人节特别版本（已知历史清单 + 3 月 31 日/4 月 1 日非常规命名启发式兜底）。
pub fn is_april_fools(id: &str, release_time: &str) -> bool {
    let lower = id.trim().to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "15w14a"
            | "1.rv-pre1"
            | "3d shareware v1.34"
            | "20w14infinite"
            | "20w14∞"
            | "22w13oneblockatatime"
            | "23w13a_or_b"
            | "24w14potato"
            | "25w14craftmine"
            | "26w14a"
    ) {
        return true;
    }
    if [
        "april",
        "shareware",
        "potato",
        "craftmine",
        "oneblockatatime",
        "infinite",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
    {
        return true;
    }
    let is_april_window = release_time.contains("-03-31T") || release_time.contains("-04-01T");
    if is_april_window && !is_standard_release_or_snapshot(&lower) {
        return true;
    }
    false
}

fn is_standard_release_or_snapshot(lower: &str) -> bool {
    let is_dotted_digits = |s: &str| {
        !s.is_empty()
            && s.split('.')
                .all(|seg| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()))
    };
    let is_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());

    // 常规正式版（如 1.21.4、26.1.1）
    if is_dotted_digits(lower) {
        return true;
    }
    // 常规周快照（如 24w14a、25w13b：NNwNN<单字母>）
    let bytes = lower.as_bytes();
    if bytes.len() == 6
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'w'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_lowercase()
    {
        return true;
    }
    // 常规 pre / rc / snapshot 后缀：支持 `<base>-pre1`、`<base>-rc-1`、`<base>-snapshot-1` 等形式
    if let Some((base, suffix)) = lower.split_once('-')
        && is_dotted_digits(base)
    {
        if suffix.starts_with("pre") || suffix.starts_with("rc") || is_digits(suffix) {
            return true;
        }
        if let Some(num) = suffix.strip_prefix("snapshot-")
            && is_digits(num)
        {
            return true;
        }
    }
    false
}

impl ManifestEntry {
    pub fn is_release(&self) -> bool {
        self.kind == "release"
    }

    pub fn category(&self) -> GameVersionCategory {
        if is_april_fools(&self.id, self.release_time.as_deref().unwrap_or_default()) {
            GameVersionCategory::AprilFools
        } else if self.kind == "release" {
            GameVersionCategory::Release
        } else if matches!(self.kind.as_str(), "old_beta" | "old_alpha") {
            GameVersionCategory::Ancient
        } else {
            GameVersionCategory::Snapshot
        }
    }
}

impl VersionManifest {
    pub fn find(&self, id: &str) -> Option<&ManifestEntry> {
        self.versions.iter().find(|v| v.id == id)
    }

    pub fn version_entries(&self) -> Vec<GameVersionEntry> {
        self.versions
            .iter()
            .map(|entry| GameVersionEntry {
                id: entry.id.clone(),
                category: entry.category(),
                release_time: entry.release_time.clone().unwrap_or_default(),
                is_latest_release: entry.id == self.latest.release,
                is_latest_snapshot: entry.id == self.latest.snapshot,
            })
            .collect()
    }
}

/// 从官方源下载版本清单。
pub async fn fetch_manifest(client: &Client) -> Result<VersionManifest, MetaError> {
    Ok(get_json(client, &[MANIFEST_URL.to_owned()]).await?)
}

/// 下载某个版本的 json，校验 SHA-1 后解析。返回解析结果和原始字节（用于原样保存到磁盘）。
pub async fn fetch_version_json(
    client: &Client,
    entry: &ManifestEntry,
) -> Result<(VersionJson, Vec<u8>), MetaError> {
    let bytes = get_bytes(client, std::slice::from_ref(&entry.url)).await?;
    if let Some(expected) = &entry.sha1 {
        let actual = to_hex(&Sha1::digest(&bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(MetaError::Checksum {
                id: entry.id.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    let version = VersionJson::parse(&entry.id, &bytes)?;
    Ok((version, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_and_finds_versions() {
        let json = r#"{
            "latest": {"release": "1.21.1", "snapshot": "24w33a"},
            "versions": [
                {"id": "24w33a", "type": "snapshot", "url": "https://x/24w33a.json", "time": "t", "releaseTime": "t", "sha1": "aa", "complianceLevel": 1},
                {"id": "1.21.1", "type": "release", "url": "https://x/1.21.1.json", "time": "t", "releaseTime": "t", "sha1": "bb", "complianceLevel": 1},
                {"id": "b1.7.3", "type": "old_beta", "url": "https://x/b1.7.3.json", "time": "t", "releaseTime": "t"}
            ]
        }"#;
        let manifest: VersionManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.latest.release, "1.21.1");
        assert!(manifest.find("1.21.1").unwrap().is_release());
        assert!(!manifest.find("24w33a").unwrap().is_release());
        assert_eq!(manifest.find("b1.7.3").unwrap().sha1, None);
        assert!(manifest.find("9.9.9").is_none());
    }

    #[test]
    fn classifies_full_version_matrix_into_four_categories() {
        let json = r#"{
            "latest": {"release": "26.3", "snapshot": "26.4-pre-1"},
            "versions": [
                {"id": "26.4-pre-1", "type": "snapshot", "url": "https://x/26.4-pre-1.json", "releaseTime": "2026-04-02T12:00:00+00:00"},
                {"id": "26w14a", "type": "snapshot", "url": "https://x/26w14a.json", "releaseTime": "2026-04-01T12:13:30+00:00"},
                {"id": "26.1.1-rc-1", "type": "snapshot", "url": "https://x/26.1.1-rc-1.json", "releaseTime": "2026-03-31T10:07:21+00:00"},
                {"id": "25w14craftmine", "type": "snapshot", "url": "https://x/25w14craftmine.json", "releaseTime": "2025-04-01T12:00:00+00:00"},
                {"id": "24w14potato", "type": "snapshot", "url": "https://x/24w14potato.json", "releaseTime": "2024-04-01T12:00:00+00:00"},
                {"id": "26.3", "type": "release", "url": "https://x/26.3.json", "releaseTime": "2026-06-10T12:00:00+00:00"},
                {"id": "20w14infinite", "type": "snapshot", "url": "https://x/20w14infinite.json", "releaseTime": "2020-04-01T12:00:00+00:00"},
                {"id": "3D Shareware v1.34", "type": "snapshot", "url": "https://x/3d.json", "releaseTime": "2019-04-01T12:00:00+00:00"},
                {"id": "1.RV-Pre1", "type": "snapshot", "url": "https://x/rv.json", "releaseTime": "2016-04-01T12:00:00+00:00"},
                {"id": "15w14a", "type": "snapshot", "url": "https://x/15w14a.json", "releaseTime": "2015-04-01T12:00:00+00:00"},
                {"id": "b1.7.3", "type": "old_beta", "url": "https://x/b1.7.3.json", "releaseTime": "2011-07-08T12:00:00+00:00"},
                {"id": "rd-132211", "type": "old_alpha", "url": "https://x/rd.json", "releaseTime": "2009-05-13T20:11:00+00:00"}
            ]
        }"#;
        let manifest: VersionManifest = serde_json::from_str(json).unwrap();
        let entries = manifest.version_entries();
        let cat = |id: &str| entries.iter().find(|e| e.id == id).unwrap().category;
        assert_eq!(cat("26.3"), GameVersionCategory::Release);
        assert_eq!(cat("26.4-pre-1"), GameVersionCategory::Snapshot);
        assert_eq!(cat("26.1.1-rc-1"), GameVersionCategory::Snapshot);
        assert_eq!(cat("26w14a"), GameVersionCategory::AprilFools);
        assert_eq!(cat("25w14craftmine"), GameVersionCategory::AprilFools);
        assert_eq!(cat("24w14potato"), GameVersionCategory::AprilFools);
        assert_eq!(cat("20w14infinite"), GameVersionCategory::AprilFools);
        assert_eq!(cat("3D Shareware v1.34"), GameVersionCategory::AprilFools);
        assert_eq!(cat("1.RV-Pre1"), GameVersionCategory::AprilFools);
        assert_eq!(cat("15w14a"), GameVersionCategory::AprilFools);
        assert_eq!(cat("b1.7.3"), GameVersionCategory::Ancient);
        assert_eq!(cat("rd-132211"), GameVersionCategory::Ancient);
        assert!(entries[0].is_latest_snapshot);
        assert!(entries[5].is_latest_release);
        assert!(!is_april_fools("24w14a", "2024-04-01T12:00:00+00:00"));
        assert!(!is_april_fools("26.1.1-rc-1", "2026-03-31T10:07:21+00:00"));
        assert!(is_april_fools("27w14cheese", "2027-04-01T12:00:00+00:00"));
    }
}
