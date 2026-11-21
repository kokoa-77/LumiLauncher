//! 新建实例：装好原版（和加载器）、补全文件、登记成实例。建完就能直接玩。

use reqwest::Client;

use super::processors::{LoaderContext, LoaderProgress};
use super::vanilla::{fetch_into, read_local};
use super::{InstallError, InstallPhase, install_files, load_or_fetch_version, loaders};
use crate::config::DownloadSource;
use crate::instance::{self, Instance, InstanceConfig, InstanceError, LoaderRef};
use crate::meta::{Environment, MetaError};
use crate::net::{DownloadError, DownloadProgress};
use crate::paths::GameDir;

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
}

#[derive(Debug, Clone)]
pub enum SetupProgress {
    Loader(LoaderProgress),
    Files(InstallPhase, DownloadProgress),
}

/// 新建实例。`ctx.java` 用来运行加载器的安装处理器；原版实例用不到。
pub async fn create_instance<F>(
    ctx: &LoaderContext<'_>,
    name: &str,
    game_version: &str,
    loader: Option<LoaderRef>,
    on_progress: F,
) -> Result<Instance, SetupError>
where
    F: Fn(SetupProgress) + Send + Sync,
{
    let base_id = install_version(ctx, game_version, loader.as_ref(), on_progress).await?;
    let id = instance::unique_id(ctx.game, name);
    let mut config = InstanceConfig::new(name, game_version, loader);
    config.version_id = base_id;
    Ok(instance::create(ctx.game, &id, &config)?)
}

/// 装好一个游戏版本（和加载器）、补全文件，返回 `versions/` 里要启动的版本 ID。
/// 新建实例、从别家导入实例都用它。
pub async fn install_version<F>(
    ctx: &LoaderContext<'_>,
    game_version: &str,
    loader: Option<&LoaderRef>,
    on_progress: F,
) -> Result<String, SetupError>
where
    F: Fn(SetupProgress) + Send + Sync,
{
    let vanilla =
        load_or_fetch_version(ctx.game, game_version, ctx.client, ctx.meta_source).await?;
    // 实例指向 versions/ 里共用的版本。
    let base_id = match loader {
        Some(loader) => {
            loaders::install(ctx, game_version, loader, |p| {
                on_progress(SetupProgress::Loader(p))
            })
            .await?
        }
        None => vanilla.id.clone(),
    };
    let version = load_or_fetch_version(ctx.game, &base_id, ctx.client, ctx.meta_source).await?;
    install_files(
        version,
        ctx.game,
        &Environment::current(),
        ctx.downloader,
        ctx.source,
        ctx.cancel,
        |phase, p| on_progress(SetupProgress::Files(phase, p)),
    )
    .await?;
    Ok(base_id)
}

/// 原版描述损坏时用同一官方版本的完整描述原子覆盖。
pub async fn repair_base_metadata(
    game: &GameDir,
    version: &str,
    client: &Client,
    source: DownloadSource,
) -> Result<(), InstallError> {
    match read_local(game, version) {
        Ok(desc) if desc.id == version && desc.inherits_from.is_none() => Ok(()),
        Ok(_) | Err(MetaError::NotFound(_) | MetaError::Parse { .. }) => {
            fetch_into(game, version, client, source).await
        }
        Err(error) => Err(error.into()),
    }
}

/// 校验并补全 Lumi 自己管理的版本文件。保留配置所指向的版本 ID。
pub async fn repair_version<F>(
    ctx: &LoaderContext<'_>,
    game_version: &str,
    loader: Option<&LoaderRef>,
    version_id: &str,
    on_progress: F,
) -> Result<(), InstallError>
where
    F: Fn(SetupProgress) + Send + Sync,
{
    if ctx.cancel.is_cancelled() {
        return Err(DownloadError::Cancelled.into());
    }
    repair_base_metadata(ctx.game, game_version, ctx.client, ctx.meta_source).await?;
    if let Some(loader) = loader.filter(|l| l.version_id(game_version) == version_id) {
        loaders::install_checked(ctx, game_version, loader, |p| {
            on_progress(SetupProgress::Loader(p))
        })
        .await?;
    }
    let version = load_or_fetch_version(ctx.game, version_id, ctx.client, ctx.meta_source).await?;
    install_files(
        version,
        ctx.game,
        &Environment::current(),
        ctx.downloader,
        ctx.source,
        ctx.cancel,
        |phase, p| on_progress(SetupProgress::Files(phase, p)),
    )
    .await?;
    Ok(())
}
