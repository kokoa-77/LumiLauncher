//! 导入前看看还缺什么：游戏本体、加载器、Java。
//!
//! 都只是告诉用户：加载器、游戏本体、Java 缺了，导入时都会自动装（Java 也会自动下载）。

use reqwest::Client;
use serde::Serialize;

use super::plan::PackPreview;
use crate::install::{load_or_fetch_version, loaders};
use crate::instance::LoaderRef;
use crate::java::{self, JavaInstall};
use crate::paths::GameDir;

/// 导入前的准备情况。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PackReadiness {
    /// 游戏本体装过了（不用再下）
    pub game_installed: bool,
    /// 加载器装过了；原版整合包恒为 `true`
    pub loader_installed: bool,
    /// 需要的 Java 主版本号；没联网、又没装过这个游戏版本时不知道
    pub java_required: Option<u32>,
    /// 找到的合适 Java 的版本号；没找到为 `None`（导入时会自动下载）
    pub java_found: Option<String>,
}

/// 检查一个已经读过的整合包的准备情况。`javas` 是扫描到的 Java
/// （[`java::scan`]，调用方放到后台线程去扫）。
pub async fn readiness(
    game: &GameDir,
    client: &Client,
    preview: &PackPreview,
    javas: &[JavaInstall],
) -> PackReadiness {
    for_game(
        game,
        client,
        &preview.game_version,
        preview.loader.as_ref(),
        javas,
    )
    .await
}

/// 只知道游戏版本（和加载器）时的准备情况：在线安装时，整合包还没下载就能先告诉用户缺不缺 Java。
/// 不知道加载器具体版本时传 `None`（`loader_installed` 会是 `true`，界面不要当真）。
pub async fn for_game(
    game: &GameDir,
    client: &Client,
    mc: &str,
    loader: Option<&LoaderRef>,
    javas: &[JavaInstall],
) -> PackReadiness {
    // 先看游戏本体在不在（下面查 Java 时可能会补一份版本描述文件，所以要先看）
    let game_installed = game.version_json(mc).is_ok_and(|p| p.is_file())
        && game.version_jar(mc).is_ok_and(|p| p.is_file());
    let loader_installed = loader.is_none_or(|loader| loaders::is_installed(game, mc, loader));
    let java_required = match load_or_fetch_version(game, mc, client).await {
        Ok(version) => Some(version.required_java()),
        Err(error) => {
            tracing::warn!(%error, version = mc, "查不到这个游戏版本要哪个 Java（可能断网）");
            None
        }
    };
    let java_found = java_required
        .and_then(|required| java::select(javas, required))
        .map(|j| j.version.clone());
    PackReadiness {
        game_installed,
        loader_installed,
        java_required,
        java_found,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::instance::LoaderKind;

    use super::*;

    fn preview(loader: Option<LoaderRef>) -> PackPreview {
        PackPreview {
            format: super::super::PackFormat::Modrinth,
            name: "Pack".to_owned(),
            version: "1".to_owned(),
            summary: None,
            game_version: "1.21.1".to_owned(),
            loader,
            file_count: 0,
            download_bytes: 0.0,
            override_count: 0,
            optional_files: Vec::new(),
            manual_files: Vec::new(),
            from_modrinth: 0,
            recommended_memory_mb: None,
        }
    }

    fn java(major: u32) -> JavaInstall {
        JavaInstall {
            home: PathBuf::from("java"),
            executable: PathBuf::from("java/bin/java.exe"),
            version: format!("{major}.0.5"),
            major,
            is_64bit: true,
            origin: crate::java::JavaOrigin::System,
            vendor: None,
        }
    }

    #[tokio::test]
    async fn reports_what_is_missing_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let game = GameDir::new(dir.path());
        // 只有版本描述、没有客户端 jar：游戏本体算没装好；版本描述够查 Java
        let json = game.version_json("1.21.1").unwrap();
        std::fs::create_dir_all(json.parent().unwrap()).unwrap();
        std::fs::write(
            &json,
            br#"{"id":"1.21.1","javaVersion":{"component":"java-runtime-delta","majorVersion":21}}"#,
        )
        .unwrap();
        let client = Client::new();
        let neoforge = Some(LoaderRef {
            kind: LoaderKind::NeoForge,
            version: "21.1.265".to_owned(),
        });

        let ready = readiness(&game, &client, &preview(neoforge), &[java(17), java(21)]).await;
        assert!(!ready.game_installed);
        assert!(!ready.loader_installed);
        assert_eq!(ready.java_required, Some(21));
        assert_eq!(ready.java_found.as_deref(), Some("21.0.5"));

        let no_java = readiness(&game, &client, &preview(None), &[java(17)]).await;
        assert!(no_java.loader_installed, "原版整合包不需要加载器");
        assert_eq!(
            no_java.java_found, None,
            "缺 Java 只是告诉用户，导入时会自动下载"
        );

        std::fs::write(game.version_jar("1.21.1").unwrap(), b"jar").unwrap();
        let installed = readiness(&game, &client, &preview(None), &[java(21)]).await;
        assert!(installed.game_installed);
    }
}
