//! 安装版的签名更新。前端只能选择已检查到的版本。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Config, Error, Update, UpdaterExt};

use crate::error::AppError;
use crate::events::emit;
use crate::state::AppState;

const RELEASE_FEED: &str =
    "https://github.com/kokoa-77/LumiLauncher/releases/latest/download/latest.json";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum UpdateMode {
    Installed,
    Portable,
    Disabled,
}

pub struct Updates {
    mode: UpdateMode,
    operation: tokio::sync::Mutex<()>,
    pending: Mutex<Option<Update>>,
}

impl Updates {
    pub fn new(app: &tauri::App, portable: bool) -> Self {
        let mode = if portable {
            UpdateMode::Portable
        } else if cfg!(windows)
            && tauri::utils::platform::bundle_type() == Some(tauri::utils::config::BundleType::Nsis)
            && app.config().plugins.0.get("updater").is_some_and(|value| {
                serde_json::from_value::<Config>(value.clone())
                    .is_ok_and(|config| permitted(&config, &app.config().identifier))
            })
        {
            UpdateMode::Installed
        } else {
            UpdateMode::Disabled
        };
        Self {
            mode,
            operation: tokio::sync::Mutex::new(()),
            pending: Mutex::default(),
        }
    }

    fn enabled(&self) -> Result<(), AppError> {
        if self.mode == UpdateMode::Installed {
            Ok(())
        } else {
            Err(AppError::new(
                "updates.unavailable",
                "此版本未启用安装版自动更新",
            ))
        }
    }
}

fn permitted(config: &Config, identifier: &str) -> bool {
    if config.pubkey.trim().is_empty()
        || !config.require_signed_version
        || config.allow_downgrades
        || config.dangerous_accept_invalid_certs
        || config.dangerous_accept_invalid_hostnames
    {
        return false;
    }
    let [endpoint] = config.endpoints.as_slice() else {
        return false;
    };
    if !config.dangerous_insecure_transport_protocol && endpoint.as_str() == RELEASE_FEED {
        return true;
    }
    // 真机验收用单独的调试安装标识，不改变系统证书和正式发布版的传输配置。
    cfg!(debug_assertions)
        && identifier == "com.lumilauncher.devqa"
        && config.dangerous_insecure_transport_protocol
        && endpoint.scheme() == "http"
        && endpoint.host_str() == Some("127.0.0.1")
        && endpoint.port().is_some()
        && endpoint.username().is_empty()
        && endpoint.password().is_none()
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct UpdateOffer {
    version: String,
    notes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct UpdateProgress {
    downloaded: f64,
    total: Option<f64>,
}

#[tauri::command]
#[specta::specta]
pub fn updates_mode(updates: State<'_, Updates>) -> UpdateMode {
    updates.mode
}

#[tauri::command]
#[specta::specta]
pub async fn updates_check(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> Result<Option<UpdateOffer>, AppError> {
    updates.enabled()?;
    let _guard = updates.operation.try_lock().map_err(|_| in_progress())?;
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(check_error)?;
    let Some(mut update) = updater.check().await.map_err(check_error)? else {
        *updates
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        return Ok(None);
    };
    update.timeout = Some(Duration::from_secs(120));
    let offer = UpdateOffer {
        version: update.version.clone(),
        notes: update
            .body
            .as_ref()
            .map(|notes| notes.chars().take(1200).collect()),
    };
    *updates
        .pending
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(update);
    Ok(Some(offer))
}

#[tauri::command]
#[specta::specta]
pub async fn updates_install(
    app: AppHandle,
    updates: State<'_, Updates>,
    state: State<'_, AppState>,
    version: String,
) -> Result<(), AppError> {
    updates.enabled()?;
    let _guard = updates.operation.try_lock().map_err(|_| in_progress())?;
    if state
        .sessions
        .when_idle(|| state.jobs.when_idle(|| ()))
        .flatten()
        .is_none()
    {
        return Err(activity_error());
    }
    let update = updates
        .pending
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .filter(|update| update.version == version)
        .cloned()
        .ok_or_else(|| AppError::new("updates.recheck", "更新信息已变化，请重新检查更新"))?;

    let mut downloaded = 0usize;
    let mut last_emit = None;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded = downloaded.saturating_add(chunk);
                let now = Instant::now();
                if last_emit
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(100))
                    || total.is_some_and(|total| downloaded as u64 >= total)
                {
                    last_emit = Some(now);
                    emit(
                        &app,
                        UpdateProgress {
                            downloaded: downloaded as f64,
                            total: total.map(|size| size as f64),
                        },
                    );
                }
            },
            || {},
        )
        .await
        .map_err(download_error)?;

    state
        .sessions
        .when_idle(|| state.jobs.when_idle(|| update.install(bytes)))
        .flatten()
        .ok_or_else(activity_error)?
        .map_err(|_| AppError::new("updates.install_failed", "无法启动更新安装程序，请重试"))
}

fn in_progress() -> AppError {
    AppError::new("updates.running", "已有更新操作正在进行")
}

fn activity_error() -> AppError {
    AppError::new("updates.app_busy", "请在游戏退出、其他任务完成后再安装更新")
}

fn check_error(_: Error) -> AppError {
    AppError::new("updates.check_failed", "无法检查更新，请稍后重试")
}

fn download_error(error: Error) -> AppError {
    match error {
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::MissingSignedVersion
        | Error::SignedVersionMismatch { .. } => AppError::new(
            "updates.signature_invalid",
            "更新签名或版本验证失败，安装已取消",
        ),
        _ => AppError::new("updates.download_failed", "下载更新失败，请稍后重试"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_production_feed_or_the_isolated_debug_install_is_enabled() {
        let mut config = Config {
            pubkey: "test key".to_owned(),
            require_signed_version: true,
            endpoints: vec![RELEASE_FEED.parse().unwrap()],
            ..Config::default()
        };
        assert!(permitted(&config, "com.lumilauncher.lumi"));
        config.allow_downgrades = true;
        assert!(!permitted(&config, "com.lumilauncher.lumi"));
        config.allow_downgrades = false;
        config.require_signed_version = false;
        assert!(!permitted(&config, "com.lumilauncher.lumi"));
        config.require_signed_version = true;
        config.endpoints = vec!["http://127.0.0.1:1625/latest.json".parse().unwrap()];
        config.dangerous_insecure_transport_protocol = true;
        assert!(!permitted(&config, "com.lumilauncher.lumi"));
        assert_eq!(
            permitted(&config, "com.lumilauncher.devqa"),
            cfg!(debug_assertions)
        );
    }
}
