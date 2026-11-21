//! 识别实例里的文件是什么。
//!
//! 顺序：本启动器装的（有来源记录）→ 缓存 → 按 SHA-1 问 Modrinth → 按指纹问 CurseForge
//! → 读文件里自带的说明（NeoForge / Forge 的 `mods.toml`、Fabric 的 `fabric.mod.json`、材质包的 `pack.mcmeta`）。
//! 结果按哈希缓存在内容仓库里；「文件名 + 大小 + 修改时间 → 哈希」也缓存，打开列表不用重新算哈希。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::{read_json, write_atomic, write_json_atomic};

pub(crate) const CACHE_FILE: &str = "identity.json";
const CACHE_FORMAT: u32 = 1;
/// 图标超过这个大小就不存（界面里图标只有 36 像素，大图没必要）
const MAX_ICON_BYTES: usize = 256 * 1024;

/// 识别出来的身份
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub title: String,
    pub version: Option<String>,
    /// Modrinth 的图标地址
    pub icon_url: Option<String>,
    /// 从文件里取出的图标（存在仓库的 icons/ 下，文件名是内容的 SHA-1）
    pub icon_file: Option<String>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub source: IdentitySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum IdentitySource {
    /// 在 Modrinth 上找到了
    Modrinth,
    /// Modrinth 上没有，在 CurseForge 上按指纹找到了
    CurseForge,
    /// 两个平台上都没有，从文件自带的说明里读的
    File,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCache {
    #[serde(default)]
    format: u32,
    /// 哈希 → 身份
    #[serde(default)]
    by_hash: BTreeMap<String, Identity>,
    /// 「文件夹/文件名|大小|修改时间」→ 哈希
    #[serde(default)]
    file_hashes: BTreeMap<String, String>,
    /// 问过 Modrinth 但没找到的哈希（别每次都问）
    #[serde(default)]
    not_on_modrinth: Vec<String>,
    /// 问过 CurseForge 但没找到的哈希
    #[serde(default)]
    not_on_curseforge: Vec<String>,
    /// 项目作者：「modrinth:编号」「curseforge:编号」→ 作者名（内容页显示用，刷新时补）
    #[serde(default)]
    authors: BTreeMap<String, String>,
    /// 哈希 → CurseForge 项目编号（按指纹认出来的；同一个文件从 Modrinth 装的也算）
    #[serde(default)]
    curseforge_projects: BTreeMap<String, u32>,
    /// 哈希 → 模组自带语言文件里认出的中文名；认过但没认出是空字符串
    #[serde(default)]
    zh_names: BTreeMap<String, String>,
}

impl IdentityCache {
    pub fn load(store_root: &Path) -> Self {
        match read_json::<IdentityCache>(&store_root.join(CACHE_FILE)) {
            Ok(Some(cache)) if cache.format == CACHE_FORMAT => cache,
            Ok(_) => Self::default(),
            Err(error) => {
                tracing::warn!(%error, "识别缓存损坏，重新开始");
                Self::default()
            }
        }
    }

    pub fn save(&mut self, store_root: &Path) -> Result<(), crate::fs::AtomicWriteError> {
        self.format = CACHE_FORMAT;
        write_json_atomic(&store_root.join(CACHE_FILE), self)
    }

    /// 作者缓存的键
    pub fn author_key(source: super::ContentSource, project_id: &str) -> String {
        match source {
            super::ContentSource::Modrinth => format!("modrinth:{project_id}"),
            super::ContentSource::CurseForge => format!("curseforge:{project_id}"),
        }
    }

    pub fn author(&self, key: &str) -> Option<&String> {
        self.authors.get(key).filter(|a| !a.is_empty())
    }

    /// 这个项目的作者问过没有（问过但平台没给也算问过）
    pub fn knows_author(&self, key: &str) -> bool {
        self.authors.contains_key(key)
    }

    pub fn remember_author(&mut self, key: String, author: String) {
        self.authors.insert(key, author.trim().to_owned());
    }

    pub fn hash_of(&self, key: &str) -> Option<&String> {
        self.file_hashes.get(key)
    }

    pub fn remember_hash(&mut self, key: String, sha1: String) {
        self.file_hashes.insert(key, sha1);
    }

    pub fn identity(&self, sha1: &str) -> Option<&Identity> {
        self.by_hash.get(sha1)
    }

    pub fn remember(&mut self, sha1: String, identity: Identity) {
        self.not_on_modrinth.retain(|h| h != &sha1);
        self.by_hash.insert(sha1, identity);
    }

    pub fn asked_modrinth(&self, sha1: &str) -> bool {
        self.not_on_modrinth.iter().any(|h| h == sha1)
            || self
                .by_hash
                .get(sha1)
                .is_some_and(|i| i.source == IdentitySource::Modrinth)
    }

    pub fn mark_not_on_modrinth(&mut self, sha1: &str) {
        if !self.not_on_modrinth.iter().any(|h| h == sha1) {
            self.not_on_modrinth.push(sha1.to_owned());
        }
    }

    /// 还要不要问 CurseForge：没认出来，或者只是从文件里读到的说明，而且没问过
    pub fn should_ask_curseforge(&self, sha1: &str) -> bool {
        let identified = self
            .by_hash
            .get(sha1)
            .is_some_and(|i| i.source != IdentitySource::File);
        !identified && !self.not_on_curseforge.iter().any(|h| h == sha1)
    }

    pub fn curseforge_project(&self, sha1: &str) -> Option<u32> {
        self.curseforge_projects.get(sha1).copied()
    }

    /// 问过 CurseForge 这个文件（找到了或者确认没有）
    pub fn knows_curseforge(&self, sha1: &str) -> bool {
        self.curseforge_projects.contains_key(sha1)
            || self.not_on_curseforge.iter().any(|h| h == sha1)
    }

    pub fn remember_curseforge_project(&mut self, sha1: &str, project: u32) {
        self.not_on_curseforge.retain(|h| h != sha1);
        self.curseforge_projects.insert(sha1.to_owned(), project);
    }

    /// 全部认出来的文件：哈希 → 身份
    pub fn identities(&self) -> impl Iterator<Item = (&String, &Identity)> {
        self.by_hash.iter()
    }

    /// 文件自带的中文名（认过但没有时也是 `None`）
    pub fn zh_name(&self, sha1: &str) -> Option<&str> {
        self.zh_names
            .get(sha1)
            .map(String::as_str)
            .filter(|n| !n.is_empty())
    }

    /// 这个文件的中文名认过没有
    pub fn knows_zh(&self, sha1: &str) -> bool {
        self.zh_names.contains_key(sha1)
    }

    pub fn remember_zh(&mut self, sha1: &str, name: Option<String>) {
        self.zh_names
            .insert(sha1.to_owned(), name.unwrap_or_default());
    }

    pub fn mark_not_on_curseforge(&mut self, sha1: &str) {
        if !self.not_on_curseforge.iter().any(|h| h == sha1) {
            self.not_on_curseforge.push(sha1.to_owned());
        }
    }
}

/// 缓存键：同一个文件没改过（名字、大小、修改时间都一样）就不用重新算哈希
pub fn file_key(folder: &str, disk_name: &str, metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    format!("{folder}/{disk_name}|{}|{modified}", metadata.len())
}

/// 文件自带的说明（读不到的项为空）
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LocalMeta {
    pub title: Option<String>,
    pub version: Option<String>,
    pub icon: Option<Vec<u8>>,
}

/// 读模组 / 材质包 / 光影包里自带的说明。读不了（不是压缩包、格式不认识）返回默认值。
pub fn read_local_meta(file: &Path) -> LocalMeta {
    let Ok(handle) = std::fs::File::open(file) else {
        return LocalMeta::default();
    };
    let Ok(mut archive) = zip::ZipArchive::new(handle) else {
        return LocalMeta::default();
    };
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut entry = archive.by_name(name).ok()?;
        if entry.size() > 8 * 1024 * 1024 {
            return None;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    };

    // NeoForge / Forge
    for toml in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(text) = read(toml).map(|b| String::from_utf8_lossy(&b).into_owned()) {
            let fields = first_mods_table(&text);
            let mut version = fields.get("version").cloned();
            if version.as_deref().is_some_and(|v| v.contains("${")) {
                version = read("META-INF/MANIFEST.MF")
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .and_then(|m| manifest_value(&m, "Implementation-Version"));
            }
            let icon = fields
                .get("logoFile")
                .and_then(|f| read(f.trim_start_matches('/')));
            return LocalMeta {
                title: fields.get("displayName").or(fields.get("modId")).cloned(),
                version,
                icon: icon.filter(|i| i.len() <= MAX_ICON_BYTES),
            };
        }
    }
    // Fabric
    if let Some(json) =
        read("fabric.mod.json").and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
    {
        let icon_path = match &json["icon"] {
            serde_json::Value::String(path) => Some(path.clone()),
            // {"16": "a.png", "128": "b.png"}：取最大的
            serde_json::Value::Object(sizes) => sizes
                .iter()
                .max_by_key(|(size, _)| size.parse::<u32>().unwrap_or(0))
                .and_then(|(_, path)| path.as_str().map(str::to_owned)),
            _ => None,
        };
        return LocalMeta {
            title: json["name"]
                .as_str()
                .or(json["id"].as_str())
                .map(str::to_owned),
            version: json["version"].as_str().map(str::to_owned),
            icon: icon_path
                .and_then(|p| read(p.trim_start_matches('/')))
                .filter(|i| i.len() <= MAX_ICON_BYTES),
        };
    }
    // 材质包
    if read("pack.mcmeta").is_some() {
        return LocalMeta {
            title: None,
            version: None,
            icon: read("pack.png").filter(|i| i.len() <= MAX_ICON_BYTES),
        };
    }
    LocalMeta::default()
}

/// 取 `mods.toml` 里第一个 `[[mods]]` 表的单行键值（够用了：名字、版本、图标都是单行）
pub(crate) fn first_mods_table(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if inside {
                break;
            }
            // 官方模板写的是「[[mods]] #mandatory」：表头后面也可以跟注释
            let header = line.split('#').next().unwrap_or(line).trim();
            inside = header.replace(' ', "") == "[[mods]]";
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(v) = quoted(value.trim()) {
            fields.insert(key.trim().to_owned(), v.to_owned());
        }
    }
    fields
}

/// 引号里的值（后面可以跟 `#注释`）；多行字符串（三个引号）不要
fn quoted(value: &str) -> Option<&str> {
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    if value.starts_with("'''") || value.starts_with("\"\"\"") {
        return None;
    }
    let rest = &value[1..];
    rest.find(quote).map(|end| &rest[..end])
}

pub(crate) fn manifest_value(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim().to_owned())
    })
}

/// 把图标存进仓库的 icons/ 下，返回文件名
pub fn save_icon(store_root: &Path, bytes: &[u8]) -> Option<String> {
    use sha1::{Digest, Sha1};
    let name = format!("{}.png", crate::net::to_hex(&Sha1::digest(bytes)));
    let path = icon_path(store_root, &name)?;
    if !path.exists()
        && let Err(error) = write_atomic(&path, bytes)
    {
        tracing::warn!(%error, "保存图标失败");
        return None;
    }
    Some(name)
}

fn icon_path(store_root: &Path, name: &str) -> Option<PathBuf> {
    crate::fs::safe_join(&store_root.join("icons"), name).ok()
}

/// 图标转成 `data:` 地址给界面用（界面不碰路径）
pub fn icon_data_url(store_root: &Path, name: &str) -> Option<String> {
    let bytes = std::fs::read(icon_path(store_root, name)?).ok()?;
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else {
        "image/jpeg"
    };
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn jar(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let out = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(out.reopen().unwrap());
        for (name, body) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        out
    }

    fn identity(source: IdentitySource) -> Identity {
        Identity {
            title: "Mod".to_owned(),
            version: None,
            icon_url: None,
            icon_file: None,
            project_id: None,
            version_id: None,
            source,
        }
    }

    #[test]
    fn asks_curseforge_only_when_still_unknown() {
        let mut cache = IdentityCache::default();
        let (unknown, from_file, on_modrinth, on_cf) = (
            "a".repeat(40),
            "b".repeat(40),
            "c".repeat(40),
            "d".repeat(40),
        );
        cache.remember(from_file.clone(), identity(IdentitySource::File));
        cache.remember(on_modrinth.clone(), identity(IdentitySource::Modrinth));
        cache.remember(on_cf.clone(), identity(IdentitySource::CurseForge));
        assert!(cache.should_ask_curseforge(&unknown));
        assert!(
            cache.should_ask_curseforge(&from_file),
            "只从文件里读到说明的也再问一次"
        );
        assert!(!cache.should_ask_curseforge(&on_modrinth));
        assert!(!cache.should_ask_curseforge(&on_cf));
        cache.mark_not_on_curseforge(&unknown);
        cache.mark_not_on_curseforge(&unknown);
        assert!(!cache.should_ask_curseforge(&unknown), "问过没有就别再问");
        assert_eq!(cache.not_on_curseforge.len(), 1);
    }

    #[test]
    fn reads_neoforge_mods_toml() {
        let toml = br#"modLoader="javafml"
[[mods]]
modId="waystones"
version="21.1.46"
displayName="Waystones"
logoFile="waystones.png"
description='''Teleport back'''
[[dependencies.waystones]]
modId="balm"
"#;
        let file = jar(&[
            ("META-INF/neoforge.mods.toml", toml),
            ("waystones.png", b"\x89PNGicon"),
        ]);
        let meta = read_local_meta(file.path());
        assert_eq!(meta.title.as_deref(), Some("Waystones"));
        assert_eq!(meta.version.as_deref(), Some("21.1.46"));
        assert_eq!(meta.icon.as_deref(), Some(&b"\x89PNGicon"[..]));
    }

    #[test]
    fn mods_toml_values_can_have_trailing_comments() {
        // 官方模板的写法：表头和值后面都带注释
        let toml =
            "[[mods]] #mandatory\nmodId=\"antarchy\" #mandatory\ndisplayName = 'Antarchy' # 名字\n";
        let fields = first_mods_table(toml);
        assert_eq!(fields.get("modId").map(String::as_str), Some("antarchy"));
        assert_eq!(
            fields.get("displayName").map(String::as_str),
            Some("Antarchy")
        );
    }

    #[test]
    fn version_placeholder_falls_back_to_the_manifest() {
        let toml = b"[[mods]]\nmodId=\"examplemod\"\nversion=\"${file.jarVersion}\"\n";
        let file = jar(&[
            ("META-INF/mods.toml", toml),
            (
                "META-INF/MANIFEST.MF",
                b"Manifest-Version: 1.0\r\nImplementation-Version: 2.3.4\r\n",
            ),
        ]);
        let meta = read_local_meta(file.path());
        assert_eq!(
            meta.title.as_deref(),
            Some("examplemod"),
            "没有显示名时用 modId"
        );
        assert_eq!(meta.version.as_deref(), Some("2.3.4"));
    }

    #[test]
    fn reads_fabric_mod_json_with_sized_icons() {
        let json = br#"{"id":"sodium","name":"Sodium","version":"0.6.0","icon":{"16":"s.png","128":"l.png"}}"#;
        let file = jar(&[
            ("fabric.mod.json", json),
            ("l.png", b"large"),
            ("s.png", b"small"),
        ]);
        let meta = read_local_meta(file.path());
        assert_eq!(meta.title.as_deref(), Some("Sodium"));
        assert_eq!(meta.icon.as_deref(), Some(&b"large"[..]));
    }

    #[test]
    fn non_archives_have_no_metadata() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"not a zip").unwrap();
        assert_eq!(read_local_meta(file.path()), LocalMeta::default());
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn icons_round_trip_as_data_urls() {
        let dir = tempfile::tempdir().unwrap();
        let name = save_icon(dir.path(), b"\x89PNG....").unwrap();
        let url = icon_data_url(dir.path(), &name).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }
}
