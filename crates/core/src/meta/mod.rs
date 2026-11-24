//! 版本元数据：版本清单、version json、rules 判定、Maven 坐标、继承合并。
//!
//! 只用 Mojang 和各加载器的官方数据，直连官方地址，不依赖第三方镜像或元数据服务器。

pub mod manifest;
pub mod maven;
pub mod rules;
pub mod version;

pub use manifest::{ManifestEntry, VersionManifest, fetch_manifest, fetch_version_json};
pub use maven::MavenCoord;
pub use rules::{Environment, Rule, RuleAction, rules_allow};
pub use version::{
    ArgValue, Argument, Arguments, Artifact, AssetIndexRef, JavaVersionReq, Library, VersionJson,
    merge, resolve,
};

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error(transparent)]
    Http(#[from] crate::net::HttpError),
    #[error("找不到版本 {0}")]
    NotFound(String),
    #[error("版本号不合法：{0}")]
    InvalidId(String),
    #[error("版本 {id} 的描述文件格式不对")]
    Parse {
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("版本 {id} 的描述文件校验失败：应为 {expected}，实际 {actual}")]
    Checksum {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("版本继承关系出现循环或层数过多：{0}")]
    InheritanceLoop(String),
    #[error("读取版本文件失败：{path}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}
