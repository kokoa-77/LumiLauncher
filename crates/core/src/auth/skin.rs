//! 皮肤图片：检查尺寸、认体型（经典 / 纤细）、按内容的 SHA-256 存进 `skins/`。
//!
//! 皮肤文件很小（64×64 的 PNG 一般几 KB），存在应用数据目录，换游戏目录也还在。

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fs::{AtomicWriteError, write_atomic};

/// 皮肤文件最多多少字节（正常的几 KB，留足余量；再大就不是皮肤了）
pub const MAX_SKIN_BYTES: usize = 256 * 1024;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// 人物体型：经典（Steve，手臂 4 像素宽）、纤细（Alex，手臂 3 像素宽）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum SkinModel {
    #[default]
    Classic,
    Slim,
}

/// 存好的一张皮肤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skin {
    /// 文件内容的 SHA-256（十六进制小写），也是 `skins/` 里的文件名
    pub hash: String,
    pub model: SkinModel,
}

#[derive(Debug, thiserror::Error)]
pub enum SkinError {
    #[error("皮肤文件太大了，正常的皮肤只有几 KB")]
    TooLarge,
    #[error("不是 PNG 图片，皮肤要是 PNG 格式")]
    NotPng,
    #[error("皮肤图片要是 64×64 或 64×32 的，这张是 {0}×{1}")]
    Size(u32, u32),
    #[error("保存皮肤失败")]
    Write(#[from] AtomicWriteError),
}

/// 检查是不是一张能用的皮肤，返回猜出来的体型。
///
/// 先从 PNG 文件头读宽高，对了才真正解码（防止一张超大的图片把内存吃光）。
pub fn check(bytes: &[u8]) -> Result<SkinModel, SkinError> {
    if bytes.len() > MAX_SKIN_BYTES {
        return Err(SkinError::TooLarge);
    }
    let (width, height) = png_size(bytes).ok_or(SkinError::NotPng)?;
    if !matches!((width, height), (64, 64) | (64, 32)) {
        return Err(SkinError::Size(width, height));
    }
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|_| SkinError::NotPng)?
        .to_rgba8();
    Ok(guess_model(&image))
}

/// 检查是不是一张官方披风贴图（PNG 且宽高在官方披风范围内）。
pub fn check_cape(bytes: &[u8]) -> Result<(), SkinError> {
    if bytes.len() > MAX_SKIN_BYTES {
        return Err(SkinError::TooLarge);
    }
    let (width, height) = png_size(bytes).ok_or(SkinError::NotPng)?;
    if !(64..=512).contains(&width) || !(32..=256).contains(&height) {
        return Err(SkinError::Size(width, height));
    }
    image::load_from_memory_with_format(bytes, ImageFormat::Png).map_err(|_| SkinError::NotPng)?;
    Ok(())
}

/// 清理 `skins/` 中已无任何账户引用的孤儿贴图缓存。
pub fn gc_unused(dir: &Path, keep_hashes: &std::collections::HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("png") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.len() != 64 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let lower = stem.to_ascii_lowercase();
        if !keep_hashes.contains(&lower)
            && let Err(error) = std::fs::remove_file(&path)
        {
            tracing::warn!(%error, path = %path.display(), "清理孤儿皮肤或披风缓存失败");
        }
    }
}

/// PNG 文件头（IHDR）里的宽高
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(PNG_SIGNATURE) || bytes.get(12..16) != Some(b"IHDR") {
        return None;
    }
    let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

/// 认体型：纤细的手臂只有 3 像素宽，右臂背面右边那两列（x = 54、55，y = 20..32）是空的。
/// 64×32 的老格式只有经典体型。
fn guess_model(image: &RgbaImage) -> SkinModel {
    if image.height() < 64 {
        return SkinModel::Classic;
    }
    let empty = (54..56).all(|x| (20..32).all(|y| image.get_pixel(x, y).0[3] == 0));
    if empty {
        SkinModel::Slim
    } else {
        SkinModel::Classic
    }
}

/// 文件内容的 SHA-256（十六进制小写）
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 存进 `skins/`（已经有同样内容的就不再写），返回哈希。调用前先 [`check`]。
pub fn save(dir: &Path, bytes: &[u8]) -> Result<String, SkinError> {
    let hash = hash(bytes);
    let path = dir.join(format!("{hash}.png"));
    if !path.is_file() {
        write_atomic(&path, bytes)?;
    }
    Ok(hash)
}

/// `skins/` 里这张皮肤的位置。哈希不是 64 位十六进制时返回 `None`（不让外面传进来的值拼出别的路径）
pub fn path_of(dir: &Path, hash: &str) -> Option<PathBuf> {
    let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then(|| dir.join(format!("{}.png", hash.to_ascii_lowercase())))
}

/// 读出存好的皮肤（文件被删了返回 `None`）
pub fn read(dir: &Path, hash: &str) -> Option<Vec<u8>> {
    let path = path_of(dir, hash)?;
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, path = %path.display(), "读皮肤文件失败");
            }
            None
        }
    }
}

/// 给界面画人物用：`data:image/png;base64,…`（皮肤很小，直接给内容，不给路径）
pub fn data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(bytes))
}

/// 角色资料里的 `textures` 属性（base64 的 JSON）→ 皮肤地址和体型。没有皮肤返回 `None`
pub fn parse_textures(value: &str) -> Option<(String, SkinModel)> {
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        textures: Textures,
    }
    #[derive(Deserialize, Default)]
    struct Textures {
        #[serde(rename = "SKIN")]
        skin: Option<Texture>,
    }
    #[derive(Deserialize)]
    struct Texture {
        url: String,
        #[serde(default)]
        metadata: Option<Metadata>,
    }
    #[derive(Deserialize)]
    struct Metadata {
        model: Option<String>,
    }
    let json = STANDARD.decode(value.trim()).ok()?;
    let payload: Payload = serde_json::from_slice(&json).ok()?;
    let skin = payload.textures.skin?;
    let model = match skin.metadata.and_then(|m| m.model).as_deref() {
        Some("slim") => SkinModel::Slim,
        _ => SkinModel::Classic,
    };
    Some((skin.url, model))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 做一张测试用的皮肤：整张填满（经典），`slim` 时把右臂背面右边两列挖空
    pub(crate) fn sample_png(width: u32, height: u32, slim: bool) -> Vec<u8> {
        let mut image = RgbaImage::from_pixel(width, height, image::Rgba([120, 80, 40, 255]));
        if slim {
            for x in 54..56 {
                for y in 20..32 {
                    image.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                }
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut out, ImageFormat::Png)
            .unwrap_or_else(|e| panic!("写 PNG 失败：{e}"));
        out.into_inner()
    }

    #[test]
    fn accepts_both_sizes_and_guesses_the_model() {
        assert_eq!(
            check(&sample_png(64, 64, false)).ok(),
            Some(SkinModel::Classic)
        );
        assert_eq!(check(&sample_png(64, 64, true)).ok(), Some(SkinModel::Slim));
        assert_eq!(
            check(&sample_png(64, 32, false)).ok(),
            Some(SkinModel::Classic)
        );
    }

    #[test]
    fn rejects_wrong_sizes_and_non_png() {
        assert!(matches!(
            check(&sample_png(128, 128, false)),
            Err(SkinError::Size(128, 128))
        ));
        assert!(matches!(check(b"not a png"), Err(SkinError::NotPng)));
        assert!(matches!(
            check(&vec![0u8; MAX_SKIN_BYTES + 1]),
            Err(SkinError::TooLarge)
        ));
    }

    #[test]
    fn saves_by_hash_and_refuses_odd_names() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let png = sample_png(64, 64, false);
        let saved = save(dir.path(), &png).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(saved.len(), 64);
        assert_eq!(read(dir.path(), &saved), Some(png));
        assert!(path_of(dir.path(), "../../evil").is_none());
        assert!(path_of(dir.path(), &"g".repeat(64)).is_none());
    }

    #[test]
    fn reads_textures_property() {
        let json = r#"{"timestamp":1,"profileId":"x","profileName":"Alex",
            "textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/abc","metadata":{"model":"slim"}}}}"#;
        let value = STANDARD.encode(json);
        assert_eq!(
            parse_textures(&value),
            Some((
                "https://textures.minecraft.net/texture/abc".to_owned(),
                SkinModel::Slim
            ))
        );
        let empty = STANDARD.encode(r#"{"textures":{}}"#);
        assert_eq!(parse_textures(&empty), None);
    }

    #[test]
    fn validates_capes_and_gcs_unused_assets() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let cape_png = sample_png(64, 32, false);
        assert!(check_cape(&cape_png).is_ok());
        assert!(matches!(
            check_cape(&sample_png(32, 32, false)),
            Err(SkinError::Size(32, 32))
        ));
        let keep_hash = save(dir.path(), &cape_png).unwrap_or_else(|e| panic!("{e}"));
        let dead_hash = "0".repeat(64);
        std::fs::write(dir.path().join(format!("{dead_hash}.png")), b"stale").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"keep-non-png").unwrap();

        let keep = std::collections::HashSet::from([keep_hash.clone()]);
        gc_unused(dir.path(), &keep);

        assert!(path_of(dir.path(), &keep_hash).unwrap().is_file());
        assert!(!path_of(dir.path(), &dead_hash).unwrap().exists());
        assert!(dir.path().join("notes.txt").is_file());
    }
}
