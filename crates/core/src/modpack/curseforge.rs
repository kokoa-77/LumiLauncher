//! CurseForge 整合包（`.zip`，里面有 `manifest.json`）：读取、检查、查清每个文件。
//!
//! 两步走：
//! 1. [`inspect`]（离线，后台线程）：读 `manifest.json`，检查覆盖文件（和 Modrinth 包同一套安全检查）
//! 2. [`resolve`]（联网，要 API Key）：清单里只有项目编号和文件编号，批量查出文件名、哈希、下载地址、
//!    放哪个文件夹。作者禁止第三方下载的文件（没有下载地址）**不自己拼 CDN 地址**：
//!    先按 SHA-1 找 Modrinth 上一模一样的文件（作者自己发在 Modrinth 上的），找到就从 Modrinth 下；
//!    剩下的列进补件清单，由用户在浏览器里下载
//!
//! 没有 Key 时第二步做不了：清单里没有哈希，用户手动放进来的文件也没法核对，所以直接拒绝，
//! 不做「不校验的导入」（docs/DESIGN.md 第 8 节第 4 条）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use reqwest::Client;

use serde::Deserialize;

use super::ModpackError;
use super::check::{
    Archive, ConflictChecker, Layer, check_relative, collect_overrides, open_archive,
    read_json_entry,
};
use super::plan::{PackFormat, PackPlan, PackPreview, PlannedFile};
use crate::content::ContentKind;
use crate::content::claim::ManualFile;
use crate::content::curseforge::{self, CurseForge, class};
use crate::content::modrinth::{self, VersionFile};
use crate::fs::safe_file_name;
use crate::instance::{LoaderKind, LoaderRef};

pub(super) const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    minecraft: Minecraft,
    #[serde(default)]
    manifest_type: String,
    #[serde(default)]
    manifest_version: u32,
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default = "default_overrides")]
    overrides: String,
}

fn default_overrides() -> String {
    "overrides".to_owned()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Minecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<ModLoader>,
    /// 作者推荐的内存（MB）；老的整合包没有
    #[serde(default)]
    recommended_ram: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestFile {
    #[serde(rename = "projectID")]
    project_id: u32,
    #[serde(rename = "fileID")]
    file_id: u32,
    #[serde(default = "yes")]
    required: bool,
}

fn yes() -> bool {
    true
}

/// 第一步的结果：清单读完、覆盖文件检查过，还没联网。
pub struct CfPack {
    manifest: Manifest,
    loader: Option<LoaderRef>,
    pack_path: PathBuf,
    overrides: Vec<(String, String)>,
    conflicts: ConflictChecker,
}

impl CfPack {
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
}

/// 读取并检查 CurseForge 整合包（离线，会读压缩包，在后台线程里调用）。
pub fn inspect(pack_path: &Path) -> Result<CfPack, ModpackError> {
    let mut archive = open_archive(pack_path)?;
    inspect_archive(&mut archive, pack_path)
}

fn inspect_archive(archive: &mut Archive, pack_path: &Path) -> Result<CfPack, ModpackError> {
    let manifest: Manifest = read_json_entry(archive, MANIFEST_FILE, " CurseForge 整合包")?;
    if manifest.manifest_type != "minecraftModpack" || manifest.manifest_version != 1 {
        return Err(ModpackError::Invalid(format!(
            "不支持的整合包格式（manifestType {}，manifestVersion {}）",
            manifest.manifest_type, manifest.manifest_version
        )));
    }
    if manifest.minecraft.version.trim().is_empty() {
        return Err(ModpackError::Invalid("整合包没写游戏版本".to_owned()));
    }
    let loader = loader_of(&manifest.minecraft.version, &manifest.minecraft.mod_loaders)?;
    let overrides_dir = manifest.overrides.trim_matches(['/', '\\']).to_owned();
    check_relative(&overrides_dir)?;
    let prefix = format!("{overrides_dir}/");
    let mut conflicts = ConflictChecker::default();
    let overrides = collect_overrides(archive, &[(&prefix, Layer::Overrides)], &mut conflicts)?;
    Ok(CfPack {
        manifest,
        loader,
        pack_path: pack_path.to_path_buf(),
        overrides,
        conflicts,
    })
}

/// 有多个加载器时用标了 `primary` 的那个。
fn loader_of(mc: &str, loaders: &[ModLoader]) -> Result<Option<LoaderRef>, ModpackError> {
    let Some(loader) = loaders
        .iter()
        .find(|l| l.primary)
        .or_else(|| loaders.first())
    else {
        return Ok(None);
    };
    let (kind, version) = loader
        .id
        .split_once('-')
        .ok_or_else(|| ModpackError::Invalid(format!("认不出加载器：{}", loader.id)))?;
    let kind = match kind {
        "neoforge" => LoaderKind::NeoForge,
        "forge" => LoaderKind::Forge,
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        other => {
            return Err(ModpackError::UnsupportedLoader(format!(
                "{other} {version}"
            )));
        }
    };
    if !kind.installable_for(mc) {
        return Err(ModpackError::UnsupportedLoader(format!(
            "{} {version}",
            kind.label()
        )));
    }
    Ok(Some(LoaderRef {
        kind,
        version: version.to_owned(),
    }))
}

/// 第二步：联网查清每个文件，得到导入计划。`client` 用来问 Modrinth 有没有一模一样的文件。
pub async fn resolve(
    client: &Client,
    cf: &CurseForge,
    pack: CfPack,
) -> Result<PackPlan, ModpackError> {
    let CfPack {
        manifest,
        loader,
        pack_path,
        overrides,
        mut conflicts,
    } = pack;
    let mut seen = HashSet::new();
    let entries: Vec<ManifestFile> = manifest
        .files
        .iter()
        .filter(|f| seen.insert(f.file_id))
        .cloned()
        .collect();
    let file_ids: Vec<u32> = entries.iter().map(|f| f.file_id).collect();
    let mut project_ids: Vec<u32> = entries.iter().map(|f| f.project_id).collect();
    project_ids.sort_unstable();
    project_ids.dedup();

    let files = cf.files(&file_ids).await?;
    let mods = cf.mods(&project_ids).await?;

    let mut downloads = Vec::new();
    let mut manual_files = Vec::new();
    let mut optional_files = Vec::new();
    let mut download_bytes = 0u64;
    let mut missing = Vec::new();
    for entry in &entries {
        let Some(file) = files.iter().find(|f| f.id == entry.file_id) else {
            if entry.required {
                missing.push(entry.file_id);
            } else {
                tracing::warn!(
                    file = entry.file_id,
                    "可选文件在 CurseForge 上找不到了，跳过"
                );
            }
            continue;
        };
        let project = mods.get(&entry.project_id);
        let project_name =
            project.map_or_else(|| format!("项目 {}", entry.project_id), |m| m.name.clone());
        let file_name = safe_file_name(&file.file_name)
            .map_err(|reason| ModpackError::Rejected(format!("文件名不安全（{reason}）")))?;
        let (kind, folder) =
            target_of(project.and_then(|m| m.class_id), file_name).ok_or_else(|| {
                ModpackError::Invalid(format!(
                    "暂不支持整合包里的这类内容：{project_name}（{file_name}）"
                ))
            })?;
        // 可选文件装上但默认停用（CurseForge 整合包里「不是必需」就是作者没勾选）
        let disabled = !entry.required && kind == ContentKind::Mod;
        let path = if disabled {
            format!("{folder}/{file_name}.disabled")
        } else {
            format!("{folder}/{file_name}")
        };
        check_relative(&path)?;
        conflicts.add(&path, Layer::Files)?;
        let sha1 = file.sha1().ok_or_else(|| {
            ModpackError::Invalid(format!("{file_name} 没有有效的 SHA-1，无法校验"))
        })?;
        if !entry.required {
            optional_files.push(path.clone());
        }
        match (file.usable_download_url(), &file.download_url) {
            (Some(url), _) => {
                download_bytes += file.file_length;
                downloads.push(PlannedFile {
                    path,
                    urls: vec![url.to_owned()],
                    sha1,
                    size: (file.file_length > 0).then_some(file.file_length),
                });
            }
            (None, Some(other)) => {
                return Err(ModpackError::Rejected(format!(
                    "{file_name} 的下载地址不是 CurseForge 的：{other}"
                )));
            }
            (None, None) => manual_files.push(ManualFile {
                project_id: entry.project_id,
                file_id: file.id,
                project_name,
                display_name: if file.display_name.is_empty() {
                    file_name.to_owned()
                } else {
                    file.display_name.clone()
                },
                file_name: file_name.to_owned(),
                size: file.file_length as f64,
                required: entry.required,
                kind,
                page_url: curseforge::download_page(
                    project.and_then(|m| m.links.website_url.as_deref()),
                    file.id,
                ),
                icon_url: project.and_then(curseforge::Mod::icon_url),
                sha1,
                path,
            }),
        }
    }
    if !missing.is_empty() {
        return Err(ModpackError::Invalid(format!(
            "整合包里有 {} 个文件在 CurseForge 上找不到了（可能被作者删除），这个版本装不了，换一个版本试试",
            missing.len()
        )));
    }

    // 作者禁止第三方下载的，先找 Modrinth 上一模一样的文件
    let found = if manual_files.is_empty() {
        HashMap::new()
    } else {
        let hashes: Vec<String> = manual_files.iter().map(|m| m.sha1.clone()).collect();
        modrinth::files_by_sha1(client, &hashes)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "问 Modrinth 有没有同样的文件失败（可能断网），全部走补件");
                HashMap::new()
            })
    };
    let (replaced, manual_files) = split_by_modrinth(manual_files, &found);
    let from_modrinth = u32::try_from(replaced.len()).unwrap_or(u32::MAX);
    download_bytes += replaced.iter().filter_map(|f| f.size).sum::<u64>();
    downloads.extend(replaced);
    tracing::info!(
        pack = %manifest.name,
        files = downloads.len(),
        from_modrinth,
        manual = manual_files.len(),
        "CurseForge 整合包已查清"
    );

    let preview = PackPreview {
        format: PackFormat::CurseForge,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        summary: (!manifest.author.trim().is_empty())
            .then(|| format!("作者：{}", manifest.author.trim())),
        game_version: manifest.minecraft.version.clone(),
        loader,
        file_count: u32::try_from(downloads.len()).unwrap_or(u32::MAX),
        download_bytes: download_bytes as f64,
        override_count: u32::try_from(overrides.len()).unwrap_or(u32::MAX),
        optional_files,
        manual_files,
        from_modrinth,
        // 只认看起来合理的数（512 MB – 64 GB），乱写的当没写
        recommended_memory_mb: manifest
            .minecraft
            .recommended_ram
            .filter(|mb| (512..=65_536).contains(mb))
            .and_then(|mb| u32::try_from(mb).ok()),
    };
    Ok(PackPlan {
        preview,
        origin: None,
        pack_path,
        downloads,
        overrides,
    })
}

/// 把补件清单分成两份：Modrinth 上有一模一样文件的（改成自动下载）、还得用户自己下的。
fn split_by_modrinth(
    manual: Vec<ManualFile>,
    found: &HashMap<String, VersionFile>,
) -> (Vec<PlannedFile>, Vec<ManualFile>) {
    let mut replaced = Vec::new();
    let mut still = Vec::new();
    for file in manual {
        match found.get(&file.sha1) {
            Some(on_modrinth) => {
                let size = if on_modrinth.size > 0 {
                    on_modrinth.size
                } else {
                    file.size as u64
                };
                replaced.push(PlannedFile {
                    path: file.path,
                    urls: vec![on_modrinth.url.clone()],
                    sha1: file.sha1,
                    size: (size > 0).then_some(size),
                });
            }
            None => still.push(file),
        }
    }
    (replaced, still)
}

/// 按项目类型决定放哪个文件夹。存档、数据包这些暂不支持（`None`）。
fn target_of(class_id: Option<u32>, file_name: &str) -> Option<(ContentKind, &'static str)> {
    match class_id {
        Some(class::MOD) => Some((ContentKind::Mod, "mods")),
        Some(class::RESOURCE_PACK) => Some((ContentKind::ResourcePack, "resourcepacks")),
        Some(class::SHADER) => Some((ContentKind::Shader, "shaderpacks")),
        Some(class::WORLD | class::DATA_PACK | class::MODPACK) => None,
        // 查不到类型的（项目被删了之类）：.jar 只可能是模组
        _ => file_name
            .to_ascii_lowercase()
            .ends_with(".jar")
            .then_some((ContentKind::Mod, "mods")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn manifest(loaders: &str, files: &str) -> String {
        format!(
            r#"{{"minecraft":{{"version":"1.21.1","modLoaders":[{loaders}],"recommendedRam":6016}},
               "manifestType":"minecraftModpack","manifestVersion":1,"name":"Integrated Boogaloo",
               "version":"1.1.6","author":"","files":[{files}],"overrides":"overrides"}}"#
        )
    }

    const NEOFORGE: &str = r#"{"id":"neoforge-21.1.264","primary":true}"#;

    fn pack(manifest_json: &str, entries: &[(&str, &str)]) -> tempfile::NamedTempFile {
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(out.reopen().unwrap());
        let options = SimpleFileOptions::default();
        zip.start_file(MANIFEST_FILE, options).unwrap();
        zip.write_all(manifest_json.as_bytes()).unwrap();
        for (name, body) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out
    }

    #[test]
    fn reads_a_real_manifest() {
        let files = r#"{"projectID":955501,"fileID":9533786,"required":true,"isLocked":false}"#;
        let p = pack(
            &manifest(NEOFORGE, files),
            &[
                ("overrides/config/a.toml", "x=1"),
                ("overrides/kubejs/startup.js", "//"),
                ("modlist.html", "<ul></ul>"),
            ],
        );
        let cf = inspect(p.path()).unwrap();
        assert_eq!(cf.name(), "Integrated Boogaloo");
        assert_eq!(
            cf.loader,
            Some(LoaderRef {
                kind: LoaderKind::NeoForge,
                version: "21.1.264".to_owned()
            })
        );
        assert_eq!(cf.overrides.len(), 2, "modlist.html 不在覆盖目录里");
        assert_eq!(cf.manifest.files[0].file_id, 9_533_786);
        assert_eq!(
            cf.manifest.minecraft.recommended_ram,
            Some(6016),
            "作者推荐的内存"
        );
    }

    #[test]
    fn reads_supported_loaders_and_rejects_old_forge() {
        for (id, kind, version) in [
            ("forge-52.1.20", LoaderKind::Forge, "52.1.20"),
            ("fabric-0.16.9", LoaderKind::Fabric, "0.16.9"),
            ("quilt-0.24.0", LoaderKind::Quilt, "0.24.0"),
        ] {
            let loaders = format!(r#"{{"id":"{id}","primary":true}}"#);
            let p = pack(&manifest(&loaders, ""), &[]);
            assert_eq!(
                inspect(p.path()).unwrap().loader,
                Some(LoaderRef {
                    kind,
                    version: version.to_owned()
                })
            );
        }
        let old = manifest(r#"{"id":"forge-14.23.5.2860","primary":true}"#, "")
            .replace("1.21.1", "1.12.2");
        let p = pack(&old, &[]);
        assert!(matches!(
            inspect(p.path()),
            Err(ModpackError::UnsupportedLoader(_))
        ));
        let p = pack(&manifest("", ""), &[]);
        assert_eq!(inspect(p.path()).unwrap().loader, None, "原版整合包");
    }

    #[test]
    fn rejects_unsafe_overrides() {
        let p = pack(
            &manifest(NEOFORGE, ""),
            &[("overrides/config/../../evil.txt", "x")],
        );
        assert!(matches!(inspect(p.path()), Err(ModpackError::Rejected(_))));
        let p = pack(&manifest(NEOFORGE, ""), &[("overrides/打开我.lnk", "x")]);
        assert!(matches!(inspect(p.path()), Err(ModpackError::Rejected(_))));

        let evil_dir =
            manifest(NEOFORGE, "").replace(r#""overrides":"overrides""#, r#""overrides":"../x""#);
        let p = pack(&evil_dir, &[]);
        assert!(matches!(inspect(p.path()), Err(ModpackError::Rejected(_))));
    }

    #[test]
    fn rejects_other_manifest_types() {
        let p = pack(
            &manifest(NEOFORGE, "").replace("minecraftModpack", "somethingElse"),
            &[],
        );
        assert!(matches!(inspect(p.path()), Err(ModpackError::Invalid(_))));
    }

    #[test]
    fn picks_the_folder_by_project_type() {
        assert_eq!(
            target_of(Some(class::MOD), "a.jar"),
            Some((ContentKind::Mod, "mods"))
        );
        assert_eq!(
            target_of(Some(class::RESOURCE_PACK), "[1.6] Enhanced Boss Bars.zip"),
            Some((ContentKind::ResourcePack, "resourcepacks"))
        );
        assert_eq!(
            target_of(Some(class::SHADER), "BSL.zip"),
            Some((ContentKind::Shader, "shaderpacks"))
        );
        assert_eq!(target_of(Some(class::WORLD), "map.zip"), None);
        assert_eq!(target_of(None, "a.jar"), Some((ContentKind::Mod, "mods")));
        assert_eq!(target_of(None, "a.zip"), None);
    }

    #[test]
    fn blocked_files_found_on_modrinth_become_downloads() {
        let manual = |sha1: &str, name: &str| ManualFile {
            project_id: 1,
            file_id: 2,
            project_name: "Mod".to_owned(),
            display_name: name.to_owned(),
            file_name: name.to_owned(),
            size: 10.0,
            required: true,
            kind: ContentKind::Mod,
            page_url: None,
            icon_url: None,
            sha1: sha1.to_owned(),
            path: format!("mods/{name}"),
        };
        let on_modrinth = "a".repeat(40);
        let found = HashMap::from([(
            on_modrinth.clone(),
            VersionFile {
                url: "https://cdn.modrinth.com/data/x/versions/y/a.jar".to_owned(),
                filename: "a-renamed.jar".to_owned(),
                primary: true,
                size: 10,
                hashes: HashMap::new(),
            },
        )]);
        let (replaced, still) = split_by_modrinth(
            vec![
                manual(&on_modrinth, "a.jar"),
                manual(&"b".repeat(40), "b.jar"),
            ],
            &found,
        );
        assert_eq!(replaced.len(), 1);
        assert_eq!(
            replaced[0].path, "mods/a.jar",
            "放的位置和文件名按整合包的来"
        );
        assert_eq!(replaced[0].sha1, on_modrinth, "照样按哈希校验");
        assert!(replaced[0].urls[0].starts_with("https://cdn.modrinth.com/"));
        assert_eq!(still.len(), 1);
        assert_eq!(still[0].file_name, "b.jar");
    }

    #[tokio::test]
    async fn resolving_without_a_key_is_refused() {
        let p = pack(
            &manifest(NEOFORGE, r#"{"projectID":1,"fileID":2,"required":true}"#),
            &[],
        );
        let cf_pack = inspect(p.path()).unwrap();
        let client = reqwest::Client::new();
        // 没 Key 会改问 MCIM；「仅官方源」时才直接拒绝（这样测不用联网）
        crate::content::mirror::curseforge()
            .set_policy(crate::config::DownloadSource::OfficialOnly);
        let cf = CurseForge::new(client.clone(), None);
        let error = resolve(&client, &cf, cf_pack).await.unwrap_err();
        assert!(error.to_string().contains("API Key"), "{error}");
    }
}
