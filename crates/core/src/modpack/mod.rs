//! 整合包：本地导入（Modrinth `.mrpack`、CurseForge `.zip`）、从 Modrinth 下载、导入前检查缺什么、
//! 把实例导出成 `.mrpack`（D17i；以后还有更新）。
//!
//! 两种格式各自读取、检查，得到同一种 [`PackPlan`]，再走同一条导入流程（[`plan::import`]）。

mod check;
pub mod curseforge;
pub mod export;
pub mod mrpack;
pub mod plan;
pub mod ready;
pub mod remote;

use std::path::{Path, PathBuf};

pub use plan::{
    ImportContext, ImportProgress, PackFormat, PackOrigin, PackPlan, PackPreview, import,
};

/// 导入临时目录的前缀（`<临时根目录>/import-<接着装的键>`）
pub const STAGING_PREFIX: &str = "import-";

/// 一次导入的临时目录（D18 能接着装）：同一个 `resume_key` 总是同一个文件夹。
/// 键只留字母数字和 `-`，别的字符换成 `_`，不会拼出越界的路径。
pub fn staging_dir(temp_root: &Path, resume_key: &str) -> PathBuf {
    let key: String = resume_key
        .chars()
        .take(80)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    temp_root.join(format!("{STAGING_PREFIX}{key}"))
}

/// 本地整合包文件的接着装的键：路径 + 大小 + 修改时间（文件换了就是新的键，不会用上旧的半成品）
pub fn file_resume_key(pack: &Path) -> String {
    use sha1::{Digest, Sha1};
    let meta = std::fs::metadata(pack).ok();
    let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let mut hasher = Sha1::new();
    hasher.update(pack.to_string_lossy().as_bytes());
    hasher.update(size.to_le_bytes());
    hasher.update(modified.to_le_bytes());
    let hex = crate::net::to_hex(&hasher.finalize());
    format!("file-{}", &hex[..16])
}

/// 删掉一次导入的临时目录（成功后、或者不要接着装了）
pub async fn discard_staging(staging: &Path) {
    match tokio::fs::remove_dir_all(staging).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, dir = %staging.display(), "清理导入临时目录失败");
        }
    }
}

/// 看压缩包里有什么来判断整合包格式（会读压缩包，在后台线程里调用）。
pub fn detect(pack_path: &Path) -> Result<PackFormat, ModpackError> {
    let archive = check::open_archive(pack_path)?;
    let has = |name: &str| archive.index_for_name(name).is_some();
    if has(mrpack::INDEX_FILE) {
        Ok(PackFormat::Modrinth)
    } else if has(curseforge::MANIFEST_FILE) {
        Ok(PackFormat::CurseForge)
    } else {
        Err(ModpackError::Invalid(
            "认不出这个整合包：里面既没有 modrinth.index.json，也没有 manifest.json".to_owned(),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModpackError {
    /// 包本身有问题（格式不对、缺东西）
    #[error("整合包有问题：{0}")]
    Invalid(String),
    /// 出于安全拒绝导入（路径越界、符号链接、陌生下载域名……）
    #[error("出于安全考虑拒绝导入：{0}")]
    Rejected(String),
    #[error("暂不支持 {0} 整合包（目前支持 NeoForge 和原版，Fabric、Forge、Quilt 以后支持）")]
    UnsupportedLoader(String),
    /// 联网查 Modrinth 失败
    #[error(transparent)]
    Remote(#[from] crate::content::ContentError),
    #[error(transparent)]
    Install(#[from] crate::install::InstallError),
    #[error(transparent)]
    Download(#[from] crate::net::DownloadError),
    #[error(transparent)]
    Instance(#[from] crate::instance::InstanceError),
    #[error(transparent)]
    UnsafePath(#[from] crate::fs::UnsafePath),
    #[error("保存文件失败")]
    Write(#[from] crate::fs::AtomicWriteError),
    #[error("已取消")]
    Cancelled,
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_dir_is_stable_and_stays_inside() {
        let root = Path::new("tmp");
        assert_eq!(
            staging_dir(root, "mr-AbC123"),
            root.join("import-mr-AbC123")
        );
        assert_eq!(
            staging_dir(root, "../../evil/x"),
            root.join("import-______evil_x"),
            "斜杠、点都换掉，拼不出上级目录"
        );
    }

    #[test]
    fn local_pack_key_changes_when_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("a.mrpack");
        std::fs::write(&pack, b"one").unwrap();
        let first = file_resume_key(&pack);
        assert_eq!(first, file_resume_key(&pack), "同一个文件同一个键");
        assert!(first.starts_with("file-"));
        std::fs::write(&pack, b"two, longer").unwrap();
        assert_ne!(
            first,
            file_resume_key(&pack),
            "文件换了，不接着用旧的半成品"
        );
    }
}
