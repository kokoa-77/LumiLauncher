//! Fabric / Quilt 从官方接口拿客户端版本描述，并把缺少的库校验值保存到本地。

use futures_util::{StreamExt, TryStreamExt, stream};
use serde_json::Value;

use super::{InstallError, LoaderContext, loaders::meta_base, processors::fetch_maven_sha1};
use crate::fs::write_json_atomic;
use crate::instance::LoaderRef;
use crate::meta::{Environment, Library, VersionJson};
use crate::net::{DownloadError, get_json};
use crate::paths::GameDir;

fn bad_profile(reason: impl Into<String>) -> InstallError {
    InstallError::Installer(reason.into())
}

fn checked_profile(bytes: &[u8], mc: &str, id: &str) -> Result<VersionJson, InstallError> {
    let profile = VersionJson::parse(id, bytes)?;
    if profile.id != id
        || profile.inherits_from.as_deref() != Some(mc)
        || profile.main_class.is_none()
    {
        return Err(bad_profile(format!(
            "版本描述与 Minecraft {mc} 的加载器 {id} 不匹配"
        )));
    }
    if profile.libraries.is_empty() {
        return Err(bad_profile("加载器版本描述没有库文件"));
    }
    Ok(profile)
}

fn is_sha1(value: Option<&str>) -> bool {
    value.is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn cached_profile(game: &GameDir, mc: &str, id: &str) -> Option<VersionJson> {
    let raw = std::fs::read(game.version_json(id).ok()?).ok()?;
    let profile = checked_profile(&raw, mc, id).ok()?;
    profile
        .libraries
        .iter()
        .all(|library| {
            let hash = match library.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
                Some(artifact) => artifact.sha1.as_deref(),
                None => library.sha1.as_deref(),
            };
            is_sha1(hash)
        })
        .then_some(profile)
}

pub(super) fn is_installed(game: &GameDir, mc: &str, id: &str) -> bool {
    cached_profile(game, mc, id)
        .and_then(|v| super::plan_libraries(&v, game, &Environment::current()).ok())
        .is_some_and(|libs| {
            libs.into_iter().all(|lib| {
                let Ok(info) = std::fs::metadata(&lib.path) else {
                    return false;
                };
                info.is_file()
                    && lib
                        .download
                        .and_then(|d| d.size)
                        .is_none_or(|size| size == info.len())
            })
        })
}

fn library_url(game: &GameDir, library: &Library) -> Result<String, InstallError> {
    let coordinate = library
        .coord()
        .ok_or_else(|| bad_profile(format!("库坐标格式无效：{}", library.name)))?;
    let artifact = library.downloads.as_ref().and_then(|d| d.artifact.as_ref());
    let relative = artifact
        .and_then(|a| a.path.clone())
        .unwrap_or_else(|| coordinate.path());
    game.library(&relative)?;
    let base = library
        .url
        .as_deref()
        .unwrap_or("https://libraries.minecraft.net");
    let url = artifact.map_or_else(
        || format!("{}/{relative}", base.trim_end_matches('/')),
        |a| a.url.clone(),
    );
    let parsed = reqwest::Url::parse(&url).map_err(|_| bad_profile("加载器库的下载地址无效"))?;
    let official = matches!(
        parsed.host_str(),
        Some("maven.fabricmc.net" | "maven.quiltmc.org" | "libraries.minecraft.net")
    );
    if parsed.scheme() != "https"
        || !official
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(bad_profile(format!(
            "库 {} 的下载地址不在加载器的官方仓库",
            library.name
        )));
    }
    Ok(url)
}

pub(super) async fn install(
    ctx: &LoaderContext<'_>,
    mc: &str,
    loader: &LoaderRef,
) -> Result<String, InstallError> {
    let id = loader.version_id(mc);
    if cached_profile(ctx.game, mc, &id).is_some() {
        return Ok(id);
    }
    let base = meta_base(loader.kind).ok_or_else(|| bad_profile("没有这个加载器的版本描述接口"))?;
    let url = format!("{base}/{mc}/{}/profile/json", loader.version);
    let mut json: Value = get_json(ctx.client, std::slice::from_ref(&url)).await?;
    let raw = serde_json::to_vec(&json).map_err(|err| bad_profile(err.to_string()))?;
    let profile = checked_profile(&raw, mc, &id)?;
    let updates: Vec<(usize, String, Option<u64>, bool)> =
        stream::iter(profile.libraries.into_iter().enumerate())
            .map(|(index, library)| async move {
                let artifact = library.downloads.as_ref().and_then(|d| d.artifact.as_ref());
                let url = library_url(ctx.game, &library)?;
                let expected = artifact.map_or(library.sha1.as_deref(), |a| a.sha1.as_deref());
                let declared_size = artifact.map_or(library.size, |a| a.size);
                let hash = match expected.filter(|s| is_sha1(Some(s))) {
                    Some(hash) => hash.to_ascii_lowercase(),
                    None => fetch_maven_sha1(ctx.client, &url).await?,
                };
                let size = if declared_size.is_some() {
                    declared_size
                } else {
                    ctx.client
                        .head(&url)
                        .send()
                        .await
                        .ok()
                        .filter(|response| response.status().is_success())
                        .and_then(|response| {
                            response
                                .headers()
                                .get(reqwest::header::CONTENT_LENGTH)?
                                .to_str()
                                .ok()?
                                .parse::<u64>()
                                .ok()
                        })
                        .filter(|size| *size > 0)
                };
                Ok::<_, InstallError>((index, hash, size, artifact.is_some()))
            })
            .buffer_unordered(6)
            .try_collect()
            .await?;
    if ctx.cancel.is_cancelled() {
        return Err(DownloadError::Cancelled.into());
    }
    for (index, hash, size, artifact) in updates {
        let lib = &mut json["libraries"][index];
        let dest = if artifact {
            &mut lib["downloads"]["artifact"]
        } else {
            lib
        };
        dest["sha1"] = hash.into();
        if let Some(size) = size {
            dest["size"] = size.into();
        }
    }
    write_json_atomic(&ctx.game.version_json(&id)?, &json)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_matching_profile_identity_and_parent() {
        let json = br#"{"id":"fabric-loader-0.16.10-1.21.1","inheritsFrom":"1.21.1","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient","libraries":[{"name":"net.fabricmc:fabric-loader:0.16.10","url":"https://maven.fabricmc.net/"}]}"#;
        let id = "fabric-loader-0.16.10-1.21.1";
        assert!(checked_profile(json, "1.21.1", id).is_ok());
        assert!(checked_profile(json, "1.21", id).is_err());
    }
}
