//! 安装：把一个版本需要的全部文件下载齐、校验好。
//!
//! 第 3 天只有原版；加载器（NeoForge、Fabric……）的安装在之后的开发日加进来，
//! 它们最后也都落到 [`vanilla::install_files`]：先合并出完整的版本描述，再补全文件。

pub mod loaders;
pub mod neoforge;
pub mod processors;
mod profiles;
pub mod setup;
pub mod vanilla;

pub use processors::{LoaderContext, LoaderProgress};
pub use vanilla::{
    AssetIndex, InstallPhase, InstalledVersion, LibraryFile, install_files, load_or_fetch_version,
    plan_libraries,
};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Meta(#[from] crate::meta::MetaError),
    #[error(transparent)]
    Http(#[from] crate::net::HttpError),
    #[error("文件路径不安全：{0}")]
    UnsafePath(#[from] crate::fs::UnsafePath),
    #[error("保存文件失败")]
    Write(#[from] crate::fs::AtomicWriteError),
    #[error(transparent)]
    Download(#[from] crate::net::DownloadError),
    #[error("版本 {0} 缺少客户端下载信息")]
    NoClientDownload(String),
    #[error("缺少依赖库 {0}（它没有下载地址，应该由加载器安装器生成）")]
    MissingLibrary(String),
    #[error("暂不支持 {0}")]
    UnsupportedLoader(String),
    #[error("加载器安装器有问题：{0}")]
    Installer(String),
    #[error("安装处理器 {task} 运行失败（退出码 {code:?}）")]
    Processor {
        task: String,
        code: Option<i32>,
        /// 处理器最后几十行输出，放进错误详情方便排查
        output: String,
    },
    #[error("资源索引 {id} 格式不对")]
    AssetIndex {
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("解压本地库失败：{path}")]
    Extract {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("文件操作失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
