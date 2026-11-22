//! 整合包的安全检查（Modrinth、CurseForge 两种格式共用）：
//! 路径越界、符号链接、快捷方式、同名冲突（Windows 不分大小写）、解压大小上限。

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde::de::DeserializeOwned;

use super::ModpackError;
use crate::fs::safe_join;

/// 覆盖文件解压后的总大小上限（防压缩炸弹）
const MAX_OVERRIDES_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Windows 快捷方式：可能指向任何地方，一律拒绝
const SHORTCUT_EXTENSIONS: &[&str] = &[".lnk", ".url"];

pub(super) type Archive = zip::ZipArchive<std::fs::File>;

pub(super) fn open_archive(path: &Path) -> Result<Archive, ModpackError> {
    let file = std::fs::File::open(path).map_err(|source| ModpackError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    zip::ZipArchive::new(file).map_err(|e| ModpackError::Invalid(format!("不是有效的压缩包：{e}")))
}

/// 读压缩包里的一个 JSON 文件。`what` 是缺这个文件时给用户看的说明（「Modrinth 整合包」）。
pub(super) fn read_json_entry<T: DeserializeOwned>(
    archive: &mut Archive,
    name: &str,
    what: &str,
) -> Result<T, ModpackError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| ModpackError::Invalid(format!("缺少 {name}，不是{what}")))?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| ModpackError::Invalid(format!("{name} 读取失败：{e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ModpackError::Invalid(format!("{name} 格式不对：{e}")))
}

/// 相对路径必须能安全地放进实例目录
pub(super) fn check_relative(path: &str) -> Result<(), ModpackError> {
    safe_join(Path::new("instance"), path)
        .map(|_| ())
        .map_err(|reason| ModpackError::Rejected(format!("路径不安全（{reason}）")))
}

/// 文件来自哪一层。后面的层可以覆盖前面的层（同名、同大小写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layer {
    /// 要下载的文件（模组等）
    Files,
    Overrides,
    ClientOverrides,
}

/// 同名冲突检查（Windows 不分大小写）：
/// - 同一层里两个文件落到同一个位置 → 冲突
/// - 不同层的同一个文件（大小写完全一样）→ 按规范后面的层覆盖前面的，允许
/// - 不同层只差大小写（`mods/A.jar` 和 `mods/a.jar`）→ 在 Windows 上会互相覆盖，几乎肯定是包做错了，按冲突处理
#[derive(Default)]
pub(super) struct ConflictChecker {
    seen: HashMap<String, (String, Layer)>,
}

impl ConflictChecker {
    pub(super) fn add(&mut self, path: &str, layer: Layer) -> Result<(), ModpackError> {
        let normalized: String = path
            .split(['/', '\\'])
            .filter(|s| !s.is_empty() && *s != ".")
            .collect::<Vec<_>>()
            .join("/");
        let key = normalized.to_lowercase();
        if let Some((existing, existing_layer)) = self.seen.get(&key) {
            let same_layer = *existing_layer == layer;
            let case_only = *existing != normalized;
            if same_layer || case_only {
                return Err(ModpackError::Rejected(format!(
                    "同名冲突：{existing} 和 {normalized}"
                )));
            }
        }
        self.seen.insert(key, (normalized, layer));
        Ok(())
    }
}

/// 收集覆盖文件（配置等直接放进实例的文件），逐个做安全检查。
/// `layers` 是（压缩包里的前缀, 层），按覆盖顺序排列。返回（条目名, 实例里的相对路径）。
pub(super) fn collect_overrides(
    archive: &mut Archive,
    layers: &[(&str, Layer)],
    conflicts: &mut ConflictChecker,
) -> Result<Vec<(String, String)>, ModpackError> {
    let mut overrides = Vec::new();
    let mut total_bytes = 0u64;
    for (prefix, layer) in layers {
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| ModpackError::Invalid(format!("压缩包损坏：{e}")))?;
            let name = entry
                .name()
                .map_err(|_| ModpackError::Invalid("压缩包里有无法识别的文件名".to_owned()))?
                .into_owned();
            let Some(relative) = name.strip_prefix(prefix) else {
                continue;
            };
            if entry.is_dir() || relative.is_empty() {
                continue;
            }
            if entry.is_symlink() {
                return Err(ModpackError::Rejected(format!("包含符号链接：{name}")));
            }
            let lower = relative.to_lowercase();
            if SHORTCUT_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
                return Err(ModpackError::Rejected(format!("包含快捷方式：{name}")));
            }
            // Lumi 自己的实例设置文件夹不从包里解压：instance.json 能写启动前后运行的命令
            if relative
                .split(['/', '\\'])
                .find(|part| !part.is_empty() && *part != ".")
                .is_some_and(crate::instance::is_meta_dir)
            {
                tracing::warn!(entry = %name, "整合包里带了 Lumi 的实例设置文件夹，已跳过");
                continue;
            }
            check_relative(relative)?;
            conflicts.add(relative, *layer)?;
            total_bytes += entry.size();
            overrides.push((name.clone(), relative.to_owned()));
        }
    }
    if total_bytes > MAX_OVERRIDES_BYTES {
        return Err(ModpackError::Rejected(format!(
            "覆盖文件解压后有 {:.1} GB，超过上限",
            total_bytes as f64 / 1024.0 / 1024.0 / 1024.0
        )));
    }
    Ok(overrides)
}
