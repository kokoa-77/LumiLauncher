//! Modrinth 整合包（`.mrpack`）：读取并检查，得到导入计划（导入流程见 [`super::plan`]）。
//!
//! 格式：压缩包里有 `modrinth.index.json`（游戏版本、加载器、要下载的文件）和
//! `overrides/`、`client-overrides/`（直接放进游戏目录的配置等文件）。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::ModpackError;
use super::check::{
    ConflictChecker, Layer, check_relative, collect_overrides, open_archive, read_json_entry,
};
use super::plan::{PackFormat, PackPlan, PackPreview, PlannedFile};
use crate::instance::{LoaderKind, LoaderRef};
use crate::net::url_host;

pub(super) const INDEX_FILE: &str = "modrinth.index.json";
const OVERRIDES: &str = "overrides/";
const CLIENT_OVERRIDES: &str = "client-overrides/";
/// Modrinth 规范允许的下载域名（平台上传时也按这个检查）
const ALLOWED_HOSTS: &[&str] = &[
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    format_version: u32,
    game: String,
    version_id: String,
    name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    files: Vec<IndexFile>,
    dependencies: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile {
    path: String,
    #[serde(default)]
    hashes: HashMap<String, String>,
    #[serde(default)]
    env: Option<FileEnv>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FileEnv {
    #[serde(default)]
    client: Option<String>,
}

/// 读取并检查整合包（会读压缩包，在后台线程里调用）。任何一项检查不过都返回错误。
pub fn inspect(pack_path: &Path) -> Result<PackPlan, ModpackError> {
    let mut archive = open_archive(pack_path)?;
    let index: Index = read_json_entry(&mut archive, INDEX_FILE, " Modrinth 整合包")?;
    if index.format_version != 1 || index.game != "minecraft" {
        return Err(ModpackError::Invalid(format!(
            "不支持的整合包格式（formatVersion {}，game {}）",
            index.format_version, index.game
        )));
    }
    let game_version = index
        .dependencies
        .get("minecraft")
        .cloned()
        .ok_or_else(|| ModpackError::Invalid("整合包没写游戏版本".to_owned()))?;
    let loader = loader_of(&index.dependencies)?;

    let mut conflicts = ConflictChecker::default();
    let mut downloads = Vec::new();
    let mut optional_files = Vec::new();
    let mut download_bytes = 0u64;
    for file in &index.files {
        let client_env = file.env.as_ref().and_then(|e| e.client.as_deref());
        if client_env == Some("unsupported") {
            continue;
        }
        if client_env == Some("optional") {
            optional_files.push(file.path.clone());
        }
        check_relative(&file.path)?;
        conflicts.add(&file.path, Layer::Files)?;
        let sha1 = file
            .hashes
            .get("sha1")
            .map(|h| h.to_ascii_lowercase())
            .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| {
                ModpackError::Invalid(format!("{} 没有有效的 SHA-1，无法校验", file.path))
            })?;
        if file.downloads.is_empty() {
            return Err(ModpackError::Invalid(format!("{} 没有下载地址", file.path)));
        }
        for url in &file.downloads {
            check_download_url(url)?;
        }
        download_bytes += file.file_size.unwrap_or(0);
        downloads.push(PlannedFile {
            path: file.path.clone(),
            urls: file.downloads.clone(),
            sha1,
            size: file.file_size,
        });
    }

    let overrides = collect_overrides(
        &mut archive,
        &[
            (OVERRIDES, Layer::Overrides),
            (CLIENT_OVERRIDES, Layer::ClientOverrides),
        ],
        &mut conflicts,
    )?;

    let preview = PackPreview {
        format: PackFormat::Modrinth,
        name: index.name.clone(),
        version: index.version_id.clone(),
        summary: index.summary.clone(),
        game_version,
        loader,
        file_count: u32::try_from(downloads.len()).unwrap_or(u32::MAX),
        download_bytes: download_bytes as f64,
        override_count: u32::try_from(overrides.len()).unwrap_or(u32::MAX),
        optional_files,
        manual_files: Vec::new(),
        from_modrinth: 0,
        recommended_memory_mb: None,
    };
    Ok(PackPlan {
        preview,
        origin: None,
        pack_path: pack_path.to_path_buf(),
        downloads,
        overrides,
    })
}

fn loader_of(dependencies: &HashMap<String, String>) -> Result<Option<LoaderRef>, ModpackError> {
    let mut loaders = dependencies
        .iter()
        .filter(|(k, _)| k.as_str() != "minecraft");
    let first = loaders.next();
    if loaders.next().is_some() {
        return Err(ModpackError::Invalid(
            "整合包同时要求了多个加载器".to_owned(),
        ));
    }
    let Some((kind, version)) = first else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "neoforge" => LoaderKind::NeoForge,
        "fabric-loader" => LoaderKind::Fabric,
        "quilt-loader" => LoaderKind::Quilt,
        "forge" => LoaderKind::Forge,
        other => {
            return Err(ModpackError::UnsupportedLoader(format!(
                "{other} {version}"
            )));
        }
    };
    if !kind.installable_for(dependencies.get("minecraft").map_or("", String::as_str)) {
        return Err(ModpackError::UnsupportedLoader(format!(
            "{} {version}",
            kind.label()
        )));
    }
    Ok(Some(LoaderRef {
        kind,
        version: version.clone(),
    }))
}

fn check_download_url(url: &str) -> Result<(), ModpackError> {
    match url_host(url) {
        Some(host) if ALLOWED_HOSTS.contains(&host.as_str()) => Ok(()),
        _ => Err(ModpackError::Rejected(format!(
            "下载地址不在 Modrinth 允许的域名里：{url}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

    fn index(files: &str, deps: &str) -> String {
        format!(
            r#"{{"formatVersion":1,"game":"minecraft","versionId":"1.0","name":"Test Pack",
               "files":[{files}],"dependencies":{{{deps}}}}}"#
        )
    }

    fn file(path: &str, url: &str) -> String {
        format!(
            r#"{{"path":"{path}","hashes":{{"sha1":"{SHA1}"}},"env":{{"client":"required","server":"required"}},
               "downloads":["{url}"],"fileSize":10}}"#
        )
    }

    /// 造一个整合包：`entries` 是（条目名, 内容），`symlink` 为真时把最后一个条目标成符号链接
    fn pack(index_json: &str, entries: &[(&str, &str)], symlink: bool) -> tempfile::NamedTempFile {
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(out.reopen().unwrap());
        let options = SimpleFileOptions::default();
        zip.start_file(INDEX_FILE, options).unwrap();
        zip.write_all(index_json.as_bytes()).unwrap();
        for (i, (name, body)) in entries.iter().enumerate() {
            if symlink && i == entries.len() - 1 {
                zip.add_symlink(*name, *body, options).unwrap();
            } else {
                zip.start_file(*name, options).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
        }
        zip.finish().unwrap();
        out
    }

    const NEOFORGE: &str = r#""minecraft":"1.21.1","neoforge":"21.1.248""#;

    #[test]
    fn accepts_a_normal_pack() {
        let files = [
            file("mods/a.jar", "https://cdn.modrinth.com/data/x/a.jar"),
            file(
                "mods/b.jar",
                "https://github.com/o/r/releases/download/v1/b.jar",
            ),
        ]
        .join(",");
        let p = pack(
            &index(&files, NEOFORGE),
            &[
                ("overrides/config/a.toml", "x=1"),
                ("overrides/options.txt", "lang:zh_cn"),
                ("client-overrides/options.txt", "lang:en_us"),
            ],
            false,
        );
        let plan = inspect(p.path()).unwrap();
        assert_eq!(plan.preview.name, "Test Pack");
        assert_eq!(plan.preview.game_version, "1.21.1");
        assert_eq!(
            plan.preview.loader,
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.248".to_owned()
            })
        );
        assert_eq!(plan.preview.file_count, 2);
        assert_eq!(
            plan.preview.override_count, 3,
            "client-overrides 覆盖 overrides 是允许的"
        );
        assert_eq!(plan.overrides.last().unwrap().1, "options.txt");
    }

    #[test]
    fn skips_files_the_client_does_not_need() {
        let server_only = r#"{"path":"mods/server.jar","hashes":{},"env":{"client":"unsupported"},"downloads":[]}"#;
        let p = pack(&index(server_only, NEOFORGE), &[], false);
        assert_eq!(inspect(p.path()).unwrap().preview.file_count, 0);
    }

    #[test]
    fn rejects_path_traversal_in_files_and_overrides() {
        let p = pack(
            &index(&file("../evil.jar", "https://cdn.modrinth.com/x"), NEOFORGE),
            &[],
            false,
        );
        assert!(matches!(inspect(p.path()), Err(ModpackError::Rejected(_))));

        let p = pack(
            &index(
                &file("C:/Windows/evil.dll", "https://cdn.modrinth.com/x"),
                NEOFORGE,
            ),
            &[],
            false,
        );
        assert!(matches!(inspect(p.path()), Err(ModpackError::Rejected(_))));

        let p = pack(
            &index("", NEOFORGE),
            &[("overrides/config/../../evil.txt", "x")],
            false,
        );
        assert!(matches!(inspect(p.path()), Err(ModpackError::Rejected(_))));
    }

    #[test]
    fn rejects_symlinks_and_shortcuts() {
        let p = pack(
            &index("", NEOFORGE),
            &[("overrides/link", "C:/Windows")],
            true,
        );
        let error = inspect(p.path()).unwrap_err();
        assert!(error.to_string().contains("符号链接"), "{error}");

        let p = pack(
            &index("", NEOFORGE),
            &[("overrides/打开我.lnk", "x")],
            false,
        );
        assert!(
            inspect(p.path())
                .unwrap_err()
                .to_string()
                .contains("快捷方式")
        );
    }

    #[test]
    fn rejects_case_insensitive_name_clashes() {
        let files = [
            file("mods/Sodium.jar", "https://cdn.modrinth.com/a"),
            file("mods/sodium.jar", "https://cdn.modrinth.com/b"),
        ]
        .join(",");
        let p = pack(&index(&files, NEOFORGE), &[], false);
        assert!(
            inspect(p.path())
                .unwrap_err()
                .to_string()
                .contains("同名冲突")
        );

        let p = pack(
            &index("", NEOFORGE),
            &[
                ("overrides/Options.txt", "a"),
                ("client-overrides/options.txt", "b"),
            ],
            false,
        );
        assert!(
            inspect(p.path())
                .unwrap_err()
                .to_string()
                .contains("同名冲突"),
            "跨层只差大小写"
        );
    }

    #[test]
    fn only_allows_modrinth_download_hosts() {
        for bad in [
            "http://cdn.modrinth.com/a.jar",
            "https://evil.example.com/a.jar",
            "https://cdn.modrinth.com.evil.com/a.jar",
            "https://user@evil.com/cdn.modrinth.com/a.jar",
        ] {
            let p = pack(&index(&file("mods/a.jar", bad), NEOFORGE), &[], false);
            assert!(
                matches!(inspect(p.path()), Err(ModpackError::Rejected(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn requires_a_usable_hash() {
        let no_hash = r#"{"path":"mods/a.jar","hashes":{"sha512":"abc"},"downloads":["https://cdn.modrinth.com/a"]}"#;
        let p = pack(&index(no_hash, NEOFORGE), &[], false);
        assert!(inspect(p.path()).unwrap_err().to_string().contains("SHA-1"));
    }

    #[test]
    fn reads_all_supported_pack_loaders() {
        for (dependency, kind) in [
            ("fabric-loader", LoaderKind::Fabric),
            ("quilt-loader", LoaderKind::Quilt),
            ("forge", LoaderKind::Forge),
        ] {
            let dependencies = format!(r#""minecraft":"1.21.1","{dependency}":"1.2.3""#);
            let p = pack(&index("", &dependencies), &[], false);
            assert_eq!(
                inspect(p.path()).unwrap().preview.loader,
                Some(LoaderRef {
                    kind,
                    version: "1.2.3".to_owned()
                }),
            );
        }
        let p = pack(
            &index("", r#""minecraft":"1.21.1","liteloader":"2.0""#),
            &[],
            false,
        );
        assert!(matches!(
            inspect(p.path()),
            Err(ModpackError::UnsupportedLoader(ref l)) if l == "liteloader 2.0"
        ));
        let p = pack(&index("", r#""minecraft":"1.21.1""#), &[], false);
        assert_eq!(
            inspect(p.path()).unwrap().preview.loader,
            None,
            "原版整合包"
        );
    }

    #[test]
    fn rejects_files_that_are_not_modrinth_packs() {
        let out = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(out.path(), b"not a zip").unwrap();
        assert!(matches!(inspect(out.path()), Err(ModpackError::Invalid(_))));
    }
}
