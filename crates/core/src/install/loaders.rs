//! 由新建、导入、复制和启动共用的加载器列表与安装入口。

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{InstallError, LoaderContext, LoaderProgress, neoforge, processors, profiles};
use crate::instance::{LoaderKind, LoaderRef};
use crate::net::get_bytes;
use crate::paths::GameDir;

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

#[derive(Deserialize)]
struct MetaEntry {
    loader: MetaVersion,
}

#[derive(Deserialize)]
struct MetaVersion {
    version: String,
    stable: Option<bool>,
}

pub(super) fn meta_base(kind: LoaderKind) -> Option<&'static str> {
    match kind {
        LoaderKind::Fabric => Some("https://meta.fabricmc.net/v2/versions/loader"),
        LoaderKind::Quilt => Some("https://meta.quiltmc.org/v3/versions/loader"),
        LoaderKind::LegacyFabric => Some("https://meta.legacyfabric.net/v2/versions/loader"),
        LoaderKind::Babric => Some("https://meta.babric.glass-launcher.net/v2/versions/loader"),
        _ => None,
    }
}

/// 外部输入用作 URL 片段和版本目录名称。
fn version_token(value: &str) -> bool {
    value.len() <= 96
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
}

pub fn validate(mc: &str, loader: &LoaderRef) -> Result<(), InstallError> {
    let kind = loader.kind;
    if !version_token(mc) || !version_token(&loader.version) || !kind.installable_for(mc) {
        return Err(InstallError::UnsupportedLoader(format!(
            "Minecraft {mc} 的 {} {}",
            kind.label(),
            loader.version
        )));
    }
    if kind == LoaderKind::NeoForge
        && neoforge::game_version_of(&loader.version).as_deref() != Some(mc)
    {
        return Err(InstallError::Installer(format!(
            "NeoForge {} 与 Minecraft {mc} 不兼容",
            loader.version
        )));
    }
    Ok(())
}

/// NeoForge 可以沿用应用已有的完整版本列表缓存，其余加载器按游戏版本查询。
pub async fn fetch_versions(
    client: &Client,
    mc: &str,
    kind: LoaderKind,
) -> Result<Vec<LoaderVersion>, InstallError> {
    if !version_token(mc) || !kind.installable_for(mc) {
        return Ok(Vec::new());
    }
    match kind {
        LoaderKind::NeoForge => Ok(neoforge::fetch_versions(client)
            .await?
            .into_iter()
            .filter(|v| v.game_version == mc)
            .map(|v| LoaderVersion {
                version: v.version,
                stable: v.stable,
            })
            .collect()),
        LoaderKind::Forge => {
            let bytes = get_bytes(client, &[format!("{FORGE_MAVEN}/maven-metadata.xml")]).await?;
            Ok(forge_versions(&String::from_utf8_lossy(&bytes), mc))
        }
        LoaderKind::Fabric | LoaderKind::Quilt | LoaderKind::LegacyFabric | LoaderKind::Babric => {
            let Some(base) = meta_base(kind) else {
                return Ok(Vec::new());
            };
            let url = format!("{base}/{mc}");
            let response =
                client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|source| crate::net::HttpError::Request {
                        url: url.clone(),
                        source,
                    })?;
            let status = response.status();
            // Fabric / Quilt / Legacy Fabric / Babric 官方接口对不支持的旧游戏版本会返回 400 或 404，视为无可用版本而非网络错误
            if matches!(
                status,
                reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::NOT_FOUND
            ) {
                return Ok(Vec::new());
            }
            if !status.is_success() {
                return Err(crate::net::HttpError::Status {
                    url,
                    status: status.as_u16(),
                }
                .into());
            }
            let bytes =
                response
                    .bytes()
                    .await
                    .map_err(|source| crate::net::HttpError::Request {
                        url: url.clone(),
                        source,
                    })?;
            let entries: Vec<MetaEntry> = serde_json::from_slice(&bytes)
                .map_err(|source| crate::net::HttpError::Decode { url, source })?;
            Ok(sorted_versions(entries.into_iter().map(|entry| {
                let v = entry.loader;
                LoaderVersion {
                    stable: v.stable.unwrap_or_else(|| !v.version.contains('-')),
                    version: v.version,
                }
            })))
        }
    }
}

fn sorted_versions(entries: impl IntoIterator<Item = LoaderVersion>) -> Vec<LoaderVersion> {
    let mut versions: Vec<_> = entries
        .into_iter()
        .filter(|v| version_token(&v.version))
        .collect();
    versions.sort_by(|a, b| neoforge::compare_loader_versions(&b.version, &a.version));
    versions.dedup_by(|a, b| a.version == b.version);
    versions
}

/// 剥离老版本 Forge 在 `maven-metadata.xml` 中附带的 MC 分支后缀（如 `10.13.4.1614-1.7.10` → `10.13.4.1614`、`10.12.2.1161-mc172` → `10.12.2.1161`）。
fn strip_forge_branch_suffix<'a>(ver: &'a str, mc: &str) -> &'a str {
    if let Some(stripped) = ver.strip_suffix(&format!("-{mc}")) {
        return stripped;
    }
    let compact = format!("-mc{}", mc.replace('.', ""));
    if let Some(stripped) = ver.strip_suffix(&compact) {
        return stripped;
    }
    ver
}

fn is_legacy_forge_mc(mc: &str) -> bool {
    let mut parts = mc.split('.');
    let Some(1) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
        return false;
    };
    parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .is_some_and(|minor| (5..=12).contains(&minor))
}

fn match_forge_artifact_in_xml(xml: &str, mc: &str, short: &str, clean: &str) -> Option<String> {
    let exact_short = format!("{mc}-{short}");
    let exact_clean = format!("{mc}-{clean}");
    let with_mc = format!("{mc}-{clean}-{mc}");
    let with_compact = format!("{mc}-{clean}-mc{}", mc.replace('.', ""));
    let prefix = format!("{mc}-{clean}-");

    let mut fallback_mc = None;
    let mut fallback_compact = None;
    let mut fallback_prefix = None;

    for chunk in xml.split("<version>").skip(1) {
        let Some((token, _)) = chunk.split_once("</version>") else {
            continue;
        };
        let token = token.trim();
        if token == exact_short || token == exact_clean {
            return Some(token.to_owned());
        }
        if token == with_mc && fallback_mc.is_none() {
            fallback_mc = Some(token.to_owned());
        } else if token == with_compact && fallback_compact.is_none() {
            fallback_compact = Some(token.to_owned());
        } else if token.starts_with(&prefix) && fallback_prefix.is_none() {
            fallback_prefix = Some(token.to_owned());
        }
    }
    fallback_mc.or(fallback_compact).or(fallback_prefix)
}

async fn resolve_forge_artifact(client: &Client, mc: &str, raw_version: &str) -> String {
    let short = raw_version
        .strip_prefix(&format!("{mc}-"))
        .unwrap_or(raw_version);
    let clean = strip_forge_branch_suffix(short, mc);
    if !is_legacy_forge_mc(mc) {
        return format!("{mc}-{short}");
    }
    if let Ok(bytes) = get_bytes(client, &[format!("{FORGE_MAVEN}/maven-metadata.xml")]).await
        && let Some(matched) =
            match_forge_artifact_in_xml(&String::from_utf8_lossy(&bytes), mc, short, clean)
    {
        return matched;
    }
    format!("{mc}-{short}")
}

fn forge_versions(xml: &str, mc: &str) -> Vec<LoaderVersion> {
    let prefix = format!("{mc}-");
    sorted_versions(xml.split("<version>").skip(1).filter_map(|chunk| {
        let (version, _) = chunk.split_once("</version>")?;
        let raw = version.trim().strip_prefix(&prefix)?;
        let clean = strip_forge_branch_suffix(raw, mc);
        Some(LoaderVersion {
            version: clean.to_owned(),
            stable: !clean.contains('-'),
        })
    }))
}

pub fn is_installed(game: &GameDir, mc: &str, loader: &LoaderRef) -> bool {
    if validate(mc, loader).is_err() {
        return false;
    }
    let id = loader.version_id(mc);
    if loader.kind.uses_processor() {
        processors::is_installed(game, &id)
    } else {
        profiles::is_installed(game, mc, &id)
    }
}

pub async fn install<F>(
    ctx: &LoaderContext<'_>,
    mc: &str,
    loader: &LoaderRef,
    on_progress: F,
) -> Result<String, InstallError>
where
    F: Fn(LoaderProgress) + Send + Sync,
{
    install_mode(ctx, mc, loader, false, on_progress).await
}

pub(super) async fn install_checked<F>(
    ctx: &LoaderContext<'_>,
    mc: &str,
    loader: &LoaderRef,
    on_progress: F,
) -> Result<String, InstallError>
where
    F: Fn(LoaderProgress) + Send + Sync,
{
    install_mode(ctx, mc, loader, true, on_progress).await
}

async fn install_mode<F>(
    ctx: &LoaderContext<'_>,
    mc: &str,
    loader: &LoaderRef,
    verify_receipt: bool,
    on_progress: F,
) -> Result<String, InstallError>
where
    F: Fn(LoaderProgress) + Send + Sync,
{
    validate(mc, loader)?;
    match loader.kind {
        LoaderKind::Fabric | LoaderKind::Quilt | LoaderKind::LegacyFabric | LoaderKind::Babric => {
            profiles::install(ctx, mc, loader).await
        }
        LoaderKind::NeoForge => {
            neoforge::install(ctx, &loader.version, verify_receipt, on_progress).await
        }
        LoaderKind::Forge => {
            let artifact = resolve_forge_artifact(ctx.client, mc, &loader.version).await;
            let source = processors::InstallerSource {
                expected_id: loader.version_id(mc),
                accepted_installer_id: None,
                game_version: mc.to_owned(),
                installer_url: format!("{FORGE_MAVEN}/{artifact}/forge-{artifact}-installer.jar"),
            };
            processors::install(ctx, &source, verify_receipt, on_progress).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_catalog_matches_the_exact_minecraft_version_and_strips_branch_suffix() {
        let xml = concat!(
            "<version>1.21-52.0.1</version>",
            "<version>1.21.1-52.1.9</version>",
            "<version>1.21.1-52.1.20</version>",
            "<version>1.21.1-52.1.20</version>",
            "<version>1.7.10-10.13.4.1558-1.7.10</version>",
            "<version>1.7.10-10.13.4.1614-1.7.10</version>",
            "<version>1.7.2-10.12.2.1161-mc172</version>"
        );
        let versions = forge_versions(xml, "1.21.1");
        assert_eq!(
            versions
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["52.1.20", "52.1.9"]
        );
        let old_1710 = forge_versions(xml, "1.7.10");
        assert_eq!(
            old_1710
                .iter()
                .map(|v| (v.version.as_str(), v.stable))
                .collect::<Vec<_>>(),
            [("10.13.4.1614", true), ("10.13.4.1558", true)]
        );
        assert_eq!(
            forge_versions(xml, "1.7.2")
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["10.12.2.1161"]
        );
        assert_eq!(
            match_forge_artifact_in_xml(xml, "1.7.10", "10.13.4.1614", "10.13.4.1614").as_deref(),
            Some("1.7.10-10.13.4.1614-1.7.10")
        );
        assert_eq!(
            match_forge_artifact_in_xml(xml, "1.7.2", "10.12.2.1161", "10.12.2.1161").as_deref(),
            Some("1.7.2-10.12.2.1161-mc172")
        );
    }

    #[test]
    fn validates_legacy_forge_legacy_fabric_babric_and_rejects_invalid_tokens() {
        assert!(
            validate(
                "1.12.2",
                &LoaderRef {
                    kind: LoaderKind::Forge,
                    version: "14.23.5.2860".into(),
                }
            )
            .is_ok()
        );
        assert!(
            validate(
                "1.7.10",
                &LoaderRef {
                    kind: LoaderKind::Forge,
                    version: "10.13.4.1614".into(),
                }
            )
            .is_ok()
        );
        assert!(
            validate(
                "1.8.9",
                &LoaderRef {
                    kind: LoaderKind::LegacyFabric,
                    version: "0.16.10".into(),
                }
            )
            .is_ok()
        );
        assert!(
            validate(
                "b1.7.3",
                &LoaderRef {
                    kind: LoaderKind::Babric,
                    version: "0.16.10".into(),
                }
            )
            .is_ok()
        );

        assert!(
            validate(
                "1.5.1",
                &LoaderRef {
                    kind: LoaderKind::Forge,
                    version: "7.7.2.682".into(),
                }
            )
            .is_err()
        );
        assert!(
            validate(
                "1.21.1",
                &LoaderRef {
                    kind: LoaderKind::Forge,
                    version: "../52.1.20".into(),
                }
            )
            .is_err()
        );
    }
}
