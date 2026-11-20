//! 由新建、导入、复制和启动共用的加载器列表与安装入口。

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{InstallError, LoaderContext, LoaderProgress, neoforge, processors, profiles};
use crate::config::DownloadSource;
use crate::instance::{LoaderKind, LoaderRef};
use crate::net::{candidate_urls, get_bytes, get_json};
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
    source: DownloadSource,
    mc: &str,
    kind: LoaderKind,
) -> Result<Vec<LoaderVersion>, InstallError> {
    if !version_token(mc) || !kind.installable_for(mc) {
        return Ok(Vec::new());
    }
    match kind {
        LoaderKind::NeoForge => Ok(neoforge::fetch_versions(client, source)
            .await?
            .into_iter()
            .filter(|v| v.game_version == mc)
            .map(|v| LoaderVersion {
                version: v.version,
                stable: v.stable,
            })
            .collect()),
        LoaderKind::Forge => {
            let bytes = get_bytes(
                client,
                &candidate_urls(&format!("{FORGE_MAVEN}/maven-metadata.xml"), source),
            )
            .await?;
            Ok(forge_versions(&String::from_utf8_lossy(&bytes), mc))
        }
        LoaderKind::Fabric | LoaderKind::Quilt => {
            let Some(base) = meta_base(kind) else {
                return Ok(Vec::new());
            };
            let url = format!("{base}/{mc}");
            let entries: Vec<MetaEntry> = get_json(client, &candidate_urls(&url, source)).await?;
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

fn forge_versions(xml: &str, mc: &str) -> Vec<LoaderVersion> {
    let prefix = format!("{mc}-");
    sorted_versions(xml.split("<version>").skip(1).filter_map(|chunk| {
        let (version, _) = chunk.split_once("</version>")?;
        let version = version.trim().strip_prefix(&prefix)?;
        Some(LoaderVersion {
            version: version.to_owned(),
            stable: !version.contains('-'),
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
        LoaderKind::Fabric | LoaderKind::Quilt => profiles::install(ctx, mc, loader).await,
        LoaderKind::NeoForge => {
            neoforge::install(ctx, &loader.version, verify_receipt, on_progress).await
        }
        LoaderKind::Forge => {
            let version = loader
                .version
                .strip_prefix(&format!("{mc}-"))
                .unwrap_or(&loader.version);
            let artifact = format!("{mc}-{version}");
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
    fn forge_catalog_matches_the_exact_minecraft_version_and_sorts_numerically() {
        let xml = "<version>1.21-52.0.1</version><version>1.21.1-52.1.9</version><version>1.21.1-52.1.20</version><version>1.21.1-52.1.20</version>";
        let versions = forge_versions(xml, "1.21.1");
        assert_eq!(
            versions
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["52.1.20", "52.1.9"]
        );
    }

    #[test]
    fn rejects_legacy_forge_and_versions_that_are_not_url_segments() {
        let mut loader = LoaderRef {
            kind: LoaderKind::Forge,
            version: "14.23.5.2860".into(),
        };
        assert!(validate("1.12.2", &loader).is_err());
        loader.version = "../52.1.20".into();
        assert!(validate("1.21.1", &loader).is_err());
    }
}
