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

impl ManifestEntry {
    pub fn is_release(&self) -> bool {
        self.kind == "release"
    }
}

impl VersionManifest {
    pub fn find(&self, id: &str) -> Option<&ManifestEntry> {
        self.versions.iter().find(|v| v.id == id)
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
}
