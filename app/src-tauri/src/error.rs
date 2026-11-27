//! 所有命令统一的错误格式：错误码 + 给用户看的中文提示 + 给排查用的详情（已脱敏）。

use std::error::Error as StdError;
use std::fmt::Display;

use launcher_core::config::SettingsError;
use launcher_core::redact::redact;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    /// 稳定的错误码，格式 `<域>.<原因>`，前端可以按它给出「下一步」按钮
    pub code: String,
    /// 给用户看的中文人话
    pub message: String,
    /// 技术细节（错误链），已脱敏，可复制给开发者
    pub detail: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Display) -> Self {
        self.detail = Some(redact(&detail.to_string()).into_owned());
        self
    }
}

/// 把错误和它的所有 `source` 拼成一行，方便排查。
fn error_chain(error: &dyn StdError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(" ← ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

impl From<SettingsError> for AppError {
    fn from(error: SettingsError) -> Self {
        tracing::error!(error = %error_chain(&error), "设置读写失败");
        AppError::new(
            "settings.save_failed",
            "保存设置失败，请检查磁盘空间是否足够",
        )
        .with_detail(error_chain(&error))
    }
}

/// 核心库的错误：消息本身已经是中文人话，直接给用户看；错误链放进详情。
fn failure(code: &str, error: &dyn StdError) -> AppError {
    let chain = error_chain(error);
    tracing::error!(code, error = %chain, "命令失败");
    AppError::new(code, error.to_string()).with_detail(chain)
}

impl From<launcher_core::install::InstallError> for AppError {
    fn from(error: launcher_core::install::InstallError) -> Self {
        use launcher_core::install::InstallError;
        use launcher_core::net::DownloadError;
        match &error {
            InstallError::Download(DownloadError::Cancelled) => {
                AppError::new("job.cancelled", "已取消")
            }
            InstallError::Download(DownloadError::Failed(failures)) => {
                download_failed(failures, &error)
            }
            InstallError::Meta(
                launcher_core::meta::MetaError::Parse { .. }
                | launcher_core::meta::MetaError::Checksum { .. },
            )
            | InstallError::NoClientDownload(_)
            | InstallError::MissingLibrary(_)
            | InstallError::AssetIndex { .. }
            | InstallError::Extract { .. } => failure("install.files_invalid", &error),
            _ => failure("install.failed", &error),
        }
    }
}

impl From<launcher_core::install::setup::SetupError> for AppError {
    fn from(error: launcher_core::install::setup::SetupError) -> Self {
        use launcher_core::install::setup::SetupError;
        match error {
            SetupError::Install(e) => e.into(),
            SetupError::Instance(e) => e.into(),
        }
    }
}

impl From<launcher_core::launch::LaunchError> for AppError {
    fn from(error: launcher_core::launch::LaunchError) -> Self {
        use launcher_core::launch::LaunchError;
        match error {
            LaunchError::Install(e) => e.into(),
            LaunchError::Instance(e) => e.into(),
            LaunchError::Java(e) => e.into(),
            LaunchError::Authorization(error) => {
                use launcher_core::auth::LaunchAuthorizationError;
                let code = match error {
                    LaunchAuthorizationError::Expired => "auth.expired",
                    LaunchAuthorizationError::Revoked => "auth.account_changed",
                    LaunchAuthorizationError::Cancelled => "auth.cancelled",
                };
                AppError::new(code, error.to_string())
            }
            LaunchError::Parameters(e) => AppError::new("launch.auth_parameters", e.to_string()),
            other => failure("launch.failed", &other),
        }
    }
}

impl From<launcher_core::java::JavaError> for AppError {
    fn from(error: launcher_core::java::JavaError) -> Self {
        use launcher_core::java::JavaError;
        use launcher_core::net::DownloadError;
        match &error {
            JavaError::Download(DownloadError::Cancelled) => {
                AppError::new("job.cancelled", "已取消")
            }
            JavaError::Download(DownloadError::Failed(failures)) => AppError::new(
                "java.download_failed",
                format!(
                    "下载 Java 时有 {} 个文件失败，请检查网络后重试（已经下好的不会重下）",
                    failures.len()
                ),
            )
            .with_detail(error_chain(&error)),
            JavaError::NotOffered { .. } => failure("java.missing", &error),
            JavaError::NotDownloaded => failure("java.delete_failed", &error),
            _ => failure("java.download_failed", &error),
        }
    }
}

impl From<launcher_core::instance::InstanceError> for AppError {
    fn from(error: launcher_core::instance::InstanceError) -> Self {
        failure("instance.failed", &error)
    }
}

impl From<launcher_core::instance::CopyError> for AppError {
    fn from(error: launcher_core::instance::CopyError) -> Self {
        use launcher_core::instance::CopyError;
        match &error {
            CopyError::Instance(_) | CopyError::UnsafePath(_) => failure("instance.failed", &error),
            CopyError::NotFound => {
                AppError::new("instance.not_found", "找不到这个实例，可能已经被删除")
            }
            CopyError::Taken(_) => failure("instance.name_taken", &error),
            CopyError::Cancelled => AppError::new("job.cancelled", "已取消"),
            CopyError::Io { .. } => {
                let chain = error_chain(&error);
                tracing::error!(error = %chain, "复制实例失败");
                AppError::new(
                    "instance.copy_failed",
                    "复制失败：可能是磁盘满了，或者有文件正被别的程序用着",
                )
                .with_detail(chain)
            }
            CopyError::Trash(_) => {
                let chain = error_chain(&error);
                tracing::error!(error = %chain, "移到回收站失败");
                AppError::new(
                    "instance.trash_failed",
                    "没能移到回收站：可能有文件正被别的程序用着，关掉它再试",
                )
                .with_detail(chain)
            }
        }
    }
}

/// 有文件下载失败：带着凭据还被拒绝的（CurseForge 的 Key 无效或被撤销）单独说清楚
fn download_failed(failures: &[launcher_core::net::FileFailure], error: &dyn StdError) -> AppError {
    let denied = failures
        .iter()
        .any(|f| f.reason.starts_with(launcher_core::net::DENIED_PREFIX));
    if denied {
        tracing::error!(error = %error_chain(error), "下载被拒绝");
        return AppError::new(
            "curseforge.denied",
            "CurseForge 拒绝了下载：启动器的 API Key 可能无效或被撤销，请更新启动器",
        )
        .with_detail(error_chain(error));
    }
    let mismatches = failures.iter().filter(|file| file.is_integrity_mismatch());
    if let Some(first) = mismatches.clone().next() {
        let count = mismatches.clone().count();
        let name = first.path.file_name().unwrap_or_default().to_string_lossy();
        let message = if count == 1 {
            format!("文件「{name}」未通过完整性校验，请重试下载")
        } else {
            format!("「{name}」等 {count} 个文件未通过完整性校验，请重试下载")
        };
        let detail = mismatches
            .take(5)
            .map(|file| {
                format!(
                    "{}：{}",
                    file.path.file_name().unwrap_or_default().to_string_lossy(),
                    file.reason
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        return AppError::new("download.integrity", message).with_detail(detail);
    }
    AppError::new(
        "download.failed",
        format!("有 {} 个文件下载失败，请检查网络后重试", failures.len()),
    )
    .with_detail(error_chain(error))
}

impl From<launcher_core::content::ContentError> for AppError {
    fn from(error: launcher_core::content::ContentError) -> Self {
        use launcher_core::content::ContentError;
        use launcher_core::net::DownloadError;
        let code = match &error {
            ContentError::Http(_) => "network.failed",
            ContentError::NoCompatibleVersion { .. } => "content.no_version",
            ContentError::NeedsLoader => "content.needs_loader",
            ContentError::NoCurseForgeKey => "curseforge.no_key",
            ContentError::CurseForgeDenied { .. } => "curseforge.denied",
            ContentError::CurseForge(_) => "curseforge.failed",
            ContentError::Reverted { .. } => "content.reverted",
            ContentError::NotReverted { .. } => "content.not_reverted",
            ContentError::Pending { .. } => "content.pending",
            ContentError::Download(DownloadError::Failed(failures)) => {
                return download_failed(failures, &error);
            }
            _ => "content.failed",
        };
        failure(code, &error)
    }
}

impl From<launcher_core::modpack::ModpackError> for AppError {
    fn from(error: launcher_core::modpack::ModpackError) -> Self {
        use launcher_core::modpack::ModpackError;
        use launcher_core::net::DownloadError;
        match error {
            ModpackError::Install(e) => e.into(),
            ModpackError::Instance(e) => e.into(),
            ModpackError::Remote(e) => e.into(),
            ModpackError::Download(DownloadError::Cancelled) | ModpackError::Cancelled => {
                AppError::new("job.cancelled", "已取消")
            }
            ModpackError::Download(DownloadError::Failed(ref failures)) => {
                download_failed(failures, &error)
            }
            ModpackError::Rejected(_) => failure("modpack.rejected", &error),
            ModpackError::UnsupportedLoader(_) => failure("modpack.unsupported", &error),
            other => failure("modpack.failed", &other),
        }
    }
}

impl From<launcher_core::meta::MetaError> for AppError {
    fn from(error: launcher_core::meta::MetaError) -> Self {
        failure("meta.failed", &error)
    }
}

impl From<launcher_core::fs::UnsafePath> for AppError {
    fn from(error: launcher_core::fs::UnsafePath) -> Self {
        failure("path.unsafe", &error)
    }
}

impl From<launcher_core::jobs::JobError> for AppError {
    fn from(error: launcher_core::jobs::JobError) -> Self {
        AppError::new(
            "job.already_running",
            "这件事已经在进行中了，可以在任务中心查看进度",
        )
        .with_detail(error.to_string())
    }
}

impl From<tauri::Error> for AppError {
    fn from(error: tauri::Error) -> Self {
        tracing::error!(error = %error_chain(&error), "窗口或运行时操作失败");
        AppError::new(
            "app.internal",
            "启动器内部出错了，请重试；反复出现请导出日志反馈",
        )
        .with_detail(error_chain(&error))
    }
}

impl From<launcher_core::content::maps::MapError> for AppError {
    fn from(error: launcher_core::content::maps::MapError) -> Self {
        use launcher_core::content::maps::MapError;
        match error {
            MapError::Content(inner) => inner.into(),
            MapError::Manual { .. } => failure("map.manual", &error),
            MapError::NoWorld => failure("map.no_world", &error),
            MapError::TooLarge => failure("map.too_large", &error),
            _ => failure("map.install_failed", &error),
        }
    }
}

impl From<launcher_core::saves::backup::BackupError> for AppError {
    fn from(error: launcher_core::saves::backup::BackupError) -> Self {
        use launcher_core::saves::backup::BackupError;
        match error {
            BackupError::Cancelled => AppError::new("job.cancelled", "已取消"),
            BackupError::NoWorld => failure("world.not_found", &error),
            BackupError::NoBackup => failure("backup.not_found", &error),
            other => failure("backup.failed", &other),
        }
    }
}

impl From<launcher_core::saves::notes::NotesError> for AppError {
    fn from(error: launcher_core::saves::notes::NotesError) -> Self {
        failure("notes.failed", &error)
    }
}

impl From<launcher_core::saves::servers::ServersError> for AppError {
    fn from(error: launcher_core::saves::servers::ServersError) -> Self {
        use launcher_core::saves::servers::ServersError;
        let code = match &error {
            ServersError::BadAddress(_) => "server.bad_address",
            ServersError::Corrupt(_) => "server.list_corrupt",
            _ => "server.list_failed",
        };
        failure(code, &error)
    }
}

impl From<launcher_core::upgrade::UpgradeError> for AppError {
    fn from(error: launcher_core::upgrade::UpgradeError) -> Self {
        use launcher_core::upgrade::UpgradeError;
        match error {
            UpgradeError::LinkedInstance => failure("upgrade.linked", &error),
            UpgradeError::Unchanged => failure("upgrade.unchanged", &error),
            UpgradeError::NotFound => failure("instance.not_found", &error),
            UpgradeError::EmptyCloneName => failure("instance.name_empty", &error),
            UpgradeError::Cancelled => AppError::new("upgrade.cancelled", "已取消"),
            UpgradeError::Instance(e) => e.into(),
            UpgradeError::Copy(e) => e.into(),
            UpgradeError::Install(e) => e.into(),
            UpgradeError::Content(e) => e.into(),
            UpgradeError::UnsafePath(e) => e.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::modpack::ModpackError;
    use launcher_core::net::{DownloadError, FileFailure};

    #[test]
    fn import_hash_mismatch_explains_the_check_failure() {
        let error = ModpackError::Download(DownloadError::Failed(vec![FileFailure::simple(
            "fixture.jar".into(),
            "SHA-1 校验失败：应为 expected，实际 actual".into(),
        )]));
        let error = AppError::from(error);
        assert_eq!(error.code, "download.integrity");
        assert!(error.message.contains("完整性校验"));
        assert!(error.message.contains("fixture.jar"));
        assert!(
            error
                .detail
                .is_some_and(|text| text.contains("fixture.jar"))
        );
    }
}
