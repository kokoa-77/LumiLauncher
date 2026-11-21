//! 「一包东西」：玩家把一堆模组、资源包、光影包、地图、投影随手塞进一个 zip，
//! 直接拖进来。认出来以后拆到临时文件夹，里面每一样再按 [`super::dropped::inspect`] 认。
//!
//! 只拆游戏内容：`.jar`、`.zip`、`.mrpack`、投影文件，和带 `level.dat` 的地图文件夹（整个文件夹）。
//! 别的（配置文件、说明文档）不拆，只数一下有几个，告诉玩家没装。
//!
//! 压缩包是不可信的：条目名一律经过 `safe_join`，总大小、条目数都有上限，先写到临时名再改名。

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::fs::{safe_file_name, safe_join};

/// 一包东西里最多这么多个条目
const MAX_ENTRIES: usize = 200_000;
/// 拆出来的东西加起来最多这么大
pub const MAX_TOTAL: u64 = 4 * 1024 * 1024 * 1024;

/// 拆出来以后要单独认的文件（按扩展名挑出来，拆完再按内容认）
const ITEM_EXTS: [&str; 6] = [
    ".jar",
    ".zip",
    ".mrpack",
    ".litematic",
    ".schem",
    ".schematic",
];

/// 一个压缩包里有什么：地图文件夹、单个文件、别的文件有几个。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// 地图文件夹在压缩包里的前缀（以 `/` 结尾；压缩包根上就是地图时是空串）
    pub worlds: Vec<String>,
    /// 要拆出来单独认的文件（不在地图文件夹里）
    pub files: Vec<String>,
    /// 别的文件（配置文件、说明文档……）有几个
    pub skipped: usize,
}

impl Plan {
    /// 是「一包东西」：有要单独认的文件，或者不止一个地图
    pub fn is_bundle(&self) -> bool {
        !self.files.is_empty() || self.worlds.len() > 1
    }
}

/// 系统自动塞进压缩包的东西（macOS 的 `__MACOSX/`、`._` 开头的影子文件、缩略图缓存），不算
fn is_junk(name: &str) -> bool {
    name.starts_with("__MACOSX/")
        || name.rsplit('/').next().is_some_and(|f| {
            f.starts_with("._")
                || f.eq_ignore_ascii_case(".DS_Store")
                || f.eq_ignore_ascii_case("Thumbs.db")
        })
}

/// 看压缩包的条目名（已经把 `\` 换成 `/`）分出地图、文件、别的。
pub fn plan(names: &[String]) -> Plan {
    let mut roots: BTreeSet<String> = names
        .iter()
        .filter(|n| !is_junk(n))
        .filter_map(|n| {
            let file = n.rsplit('/').next()?;
            file.eq_ignore_ascii_case("level.dat")
                .then(|| n[..n.len() - file.len()].to_owned())
        })
        .collect();
    // 地图里面再套地图（不该有）：只要外面那层
    let all: Vec<String> = roots.iter().cloned().collect();
    roots.retain(|r| {
        !all.iter()
            .any(|outer| outer != r && r.starts_with(outer.as_str()))
    });
    let worlds: Vec<String> = roots.into_iter().collect();

    let mut files = Vec::new();
    let mut skipped = 0;
    for name in names {
        if name.ends_with('/') || is_junk(name) || name.starts_with("META-INF/") {
            continue;
        }
        if worlds.iter().any(|w| name.starts_with(w.as_str())) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if ITEM_EXTS.iter().any(|ext| lower.ends_with(ext)) {
            files.push(name.clone());
        } else {
            skipped += 1;
        }
    }
    files.sort();
    Plan {
        worlds,
        files,
        skipped,
    }
}

/// 拆出来的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    /// 拆出来的每一样（文件，或者地图文件夹），按压缩包里的顺序：文件在前，地图在后
    pub items: Vec<PathBuf>,
    /// 没拆的别的文件有几个
    pub skipped: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("压缩包打不开：{0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("拆压缩包时读写失败：{0}")]
    Io(#[from] io::Error),
    #[error("压缩包太大了（超过 4 GB）")]
    TooLarge,
}

/// 每个条目的名字（按序号，`\` 换成 `/`）；名字读不出来的给空串（不会被挑中）
fn entry_names(archive: &mut zip::ZipArchive<File>) -> Result<Vec<String>, BundleError> {
    let mut names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        names.push(
            entry
                .name()
                .map(|n| n.replace('\\', "/"))
                .unwrap_or_default(),
        );
    }
    Ok(names)
}

/// 把一包东西拆到 `dest`（不存在会建）：每个文件放进自己的 `f<序号>/`，每个地图放进 `w<序号>/<地图名>/`，
/// 名字原样保留、不会互相覆盖。`dest` 由调用的人负责删。
pub fn extract(path: &Path, dest: &Path) -> Result<Unpacked, BundleError> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    if archive.len() > MAX_ENTRIES {
        return Err(BundleError::TooLarge);
    }
    let names = entry_names(&mut archive)?;
    let plan = plan(&names);
    let in_world = |name: &str| {
        plan.worlds
            .iter()
            .position(|w| name.starts_with(w.as_str()))
    };

    // 先按压缩包自己写的大小估一遍，明显太大就别开始拆
    let mut declared = 0u64;
    for (index, name) in names.iter().enumerate() {
        if plan.files.contains(name) || in_world(name).is_some() {
            declared = declared.saturating_add(archive.by_index_raw(index)?.size());
        }
    }
    if declared > MAX_TOTAL {
        return Err(BundleError::TooLarge);
    }

    std::fs::create_dir_all(dest)?;
    let mut budget = MAX_TOTAL;
    let mut items = Vec::new();
    for (i, wanted) in plan.files.iter().enumerate() {
        let file = wanted.rsplit('/').next().unwrap_or(wanted);
        let Ok(file) = safe_file_name(file) else {
            tracing::warn!(name = %wanted, "压缩包里的文件名不安全，跳过");
            continue;
        };
        let Some(index) = names.iter().position(|n| n == wanted) else {
            continue;
        };
        let target = safe_join(&dest.join(format!("f{i}")), file).map_err(io::Error::other)?;
        write_entry(archive.by_index(index)?, &target, &mut budget)?;
        items.push(target);
    }
    for (i, prefix) in plan.worlds.iter().enumerate() {
        let folder = prefix
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|f| !f.is_empty())
            .and_then(|f| safe_file_name(f).ok())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("地图 {}", i + 1));
        let world = dest.join(format!("w{i}")).join(&folder);
        std::fs::create_dir_all(&world)?;
        for (index, name) in names.iter().enumerate() {
            if in_world(name) != Some(i) || is_junk(name) {
                continue;
            }
            let rel = &name[prefix.len()..];
            if rel.is_empty() {
                continue;
            }
            let target = safe_join(&world, rel).map_err(io::Error::other)?;
            if name.ends_with('/') {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            write_entry(archive.by_index(index)?, &target, &mut budget)?;
        }
        items.push(world);
    }
    Ok(Unpacked {
        items,
        skipped: plan.skipped,
    })
}
/// 写出一个条目，最多写 `budget` 字节（压缩包写的大小可能是假的，按真正写出来的算）
fn write_entry(entry: impl Read, target: &Path, budget: &mut u64) -> Result<(), BundleError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = File::create(target)?;
    let mut limited = entry.take(budget.saturating_add(1));
    let written = io::copy(&mut limited, &mut out)?;
    out.flush()?;
    if written > *budget {
        return Err(BundleError::TooLarge);
    }
    *budget -= written;
    Ok(())
}

/// 投影（Litematica 的 `.litematic`、WorldEdit 的 `.schem`、MCEdit 的 `.schematic`）：
/// gzip 压缩的 NBT，看解压后开头一段里有没有这几种格式必有的标签名
pub fn looks_like_schematic(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut head = Vec::new();
    let mut gz = flate2::read::GzDecoder::new(file).take(256 * 1024);
    if gz.read_to_end(&mut head).is_err() && head.is_empty() {
        return false;
    }
    // NBT 第一个字节是 10（复合标签）
    if head.first() != Some(&10) {
        return false;
    }
    let tag = |name: &str| {
        let mut needle = (name.len() as u16).to_be_bytes().to_vec();
        needle.extend_from_slice(name.as_bytes());
        head.windows(needle.len()).any(|w| w == needle.as_slice())
    };
    (tag("Metadata") && tag("Regions"))
        || tag("Schematic")
        || (tag("Palette") && tag("BlockData"))
        || (tag("Materials") && tag("Blocks"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn plans_what_to_take_out_of_a_bundle() {
        let plan = plan(&names(&[
            "懒人包/",
            "懒人包/mods/a.jar",
            "懒人包/mods/b.JAR",
            "懒人包/shaderpacks/photon.zip",
            "懒人包/config/a.toml",
            "懒人包/说明.txt",
            "懒人包/saves/空岛/level.dat",
            "懒人包/saves/空岛/region/r.0.0.mca",
            "懒人包/saves/空岛/datapacks/x.zip",
            "懒人包/saves/空岛/DIM1/level.dat",
            "懒人包/基地.litematic",
            "__MACOSX/懒人包/mods/._a.jar",
            "META-INF/MANIFEST.MF",
        ]));
        assert_eq!(plan.worlds, ["懒人包/saves/空岛/"]);
        assert_eq!(
            plan.files,
            [
                "懒人包/mods/a.jar",
                "懒人包/mods/b.JAR",
                "懒人包/shaderpacks/photon.zip",
                "懒人包/基地.litematic",
            ]
        );
        assert_eq!(plan.skipped, 2);
        assert!(plan.is_bundle());

        let one_world = plan_of(&["My World/level.dat", "My World/region/r.0.0.mca"]);
        assert!(
            !one_world.is_bundle(),
            "只有一个地图：就是地图，不是一包东西"
        );
        let two_worlds = plan_of(&["a/level.dat", "b/level.dat"]);
        assert!(two_worlds.is_bundle());
    }

    fn plan_of(list: &[&str]) -> Plan {
        plan(&names(list))
    }

    fn zip_file(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, body) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_each_item_into_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("懒人包.zip");
        zip_file(
            &bundle,
            &[
                ("懒人包/mods/a.jar", b"jar a"),
                ("懒人包/备用/a.jar", b"another a"),
                ("懒人包/saves/空岛/level.dat", b"nbt"),
                ("懒人包/saves/空岛/region/r.0.0.mca", b"region"),
                ("懒人包/config/x.toml", b"x"),
            ],
        );
        let dest = dir.path().join("拆开");
        let unpacked = extract(&bundle, &dest).unwrap();
        assert_eq!(unpacked.skipped, 1);
        assert_eq!(
            unpacked.items,
            [
                dest.join("f0").join("a.jar"),
                dest.join("f1").join("a.jar"),
                dest.join("w0").join("空岛"),
            ]
        );
        assert_eq!(
            std::fs::read(&unpacked.items[0]).unwrap(),
            b"jar a",
            "按名字排序"
        );
        assert!(dest.join("w0/空岛/region/r.0.0.mca").is_file());
        assert!(!dest.join("f0/config").exists(), "配置文件不拆");
    }

    #[test]
    fn entries_cannot_escape_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("坏的.zip");
        zip_file(
            &bundle,
            &[
                ("w/level.dat", b"nbt"),
                ("w/../../逃出去.txt", b"x"),
                ("v/level.dat", b"nbt"),
            ],
        );
        let dest = dir.path().join("拆开");
        assert!(extract(&bundle, &dest).is_err());
        assert!(!dir.path().join("逃出去.txt").exists());
    }
}
