//! 截图缩略图：按界面要的宽度缩小，存成 JPEG 缓存起来。
//!
//! 首页铺满整窗的截图如果直接用原图（1080p 截图解码后 7 MB，4K 截图 30 MB 多），
//! 网页内核的渲染进程和显卡进程都要各存一份；缩到窗口大小能省下一大半，2K、4K 屏省得更多（实测）。
//! 缓存键包含文件大小和修改时间，截图被替换后自然换一张新的缩略图。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader};

use crate::fs::write_atomic;

/// JPEG 质量：截图缩小后肉眼看不出和原图的区别，文件只有原来的十分之一左右
const QUALITY: u8 = 85;
/// 解码时最多占多少内存（8K 截图 RGBA 约 130 MB，再大的不是正常截图）
const MAX_ALLOC: u64 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ThumbError {
    #[error("读不了图片：{0}")]
    Io(#[from] std::io::Error),
    #[error("图片格式不对：{0}")]
    Image(#[from] image::ImageError),
    #[error("保存缩略图失败")]
    Write(#[from] crate::fs::AtomicWriteError),
}

/// 返回不超过 `max_width` 宽的版本：原图本来就不宽于它时返回原图路径，否则返回（必要时生成的）缓存文件。
pub fn thumbnail(source: &Path, max_width: u32, cache_dir: &Path) -> Result<PathBuf, ThumbError> {
    let (width, _) = image::image_dimensions(source)?;
    if width <= max_width {
        return Ok(source.to_path_buf());
    }
    let meta = std::fs::metadata(source)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    modified.hash(&mut hasher);
    let stem = format!("{:016x}-{max_width}", hasher.finish());
    // 透明底的图存成 PNG，其余 JPEG：解码前不知道是哪种，两个都看一眼
    for ext in ["jpg", "png"] {
        let cached = cache_dir.join(format!("{stem}.{ext}"));
        if cached.is_file() {
            return Ok(cached);
        }
    }

    let mut reader = ImageReader::open(source)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let picture = reader.decode()?;
    // Triangle（双线性）缩小截图足够清楚，比 Lanczos 快好几倍
    let small = picture.resize(max_width, u32::MAX, FilterType::Triangle);
    let mut bytes = Vec::new();
    // 透明底的图（立绘之类）存成 PNG 保留透明，转 JPEG 透明的地方会变黑
    let cached = if has_transparency(&small) {
        small
            .into_rgba8()
            .write_with_encoder(PngEncoder::new(&mut Cursor::new(&mut bytes)))?;
        cache_dir.join(format!("{stem}.png"))
    } else {
        small
            .into_rgb8()
            .write_with_encoder(JpegEncoder::new_with_quality(
                &mut Cursor::new(&mut bytes),
                QUALITY,
            ))?;
        cache_dir.join(format!("{stem}.jpg"))
    };
    write_atomic(&cached, &bytes)?;
    Ok(cached)
}

/// 图里有没有透明的地方（立绘常见）：带透明通道、而且真有不是完全不透明的像素才算
pub fn has_transparency(picture: &DynamicImage) -> bool {
    if !picture.color().has_alpha() {
        return false;
    }
    match picture.as_rgba8() {
        Some(rgba) => rgba.pixels().any(|p| p[3] < 250),
        None => picture.to_rgba8().pixels().any(|p| p[3] < 250),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn screenshot(path: &Path, width: u32, height: u32) {
        let img = RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        img.save(path).unwrap();
    }

    #[test]
    fn shrinks_wide_screenshots_and_caches_them() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("2026-11-11_09.12.53.png");
        screenshot(&shot, 1920, 1009);
        let cache = dir.path().join("thumbs");

        let small = thumbnail(&shot, 1280, &cache).unwrap();
        assert_ne!(small, shot);
        assert_eq!(image::image_dimensions(&small).unwrap(), (1280, 673));

        // 第二次直接用缓存
        let modified = std::fs::metadata(&small).unwrap().modified().unwrap();
        assert_eq!(thumbnail(&shot, 1280, &cache).unwrap(), small);
        assert_eq!(
            std::fs::metadata(&small).unwrap().modified().unwrap(),
            modified
        );

        // 不同宽度是不同的文件
        assert_ne!(thumbnail(&shot, 640, &cache).unwrap(), small);
    }

    #[test]
    fn transparent_pictures_stay_transparent() {
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("立绘.png");
        image::RgbaImage::from_fn(1200, 1600, |x, _| {
            image::Rgba([220, 140, 180, if x < 400 { 0 } else { 255 }])
        })
        .save(&art)
        .unwrap();
        let small = thumbnail(&art, 320, &dir.path().join("thumbs")).unwrap();
        assert!(
            small.extension().is_some_and(|x| x == "png"),
            "{}",
            small.display()
        );
        let decoded = image::open(&small).unwrap();
        assert!(has_transparency(&decoded), "透明的地方没变黑");
        assert_eq!(
            thumbnail(&art, 320, &dir.path().join("thumbs")).unwrap(),
            small,
            "缓存"
        );
    }

    #[test]
    fn small_pictures_are_served_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let icon = dir.path().join("icon.png");
        screenshot(&icon, 64, 64);
        assert_eq!(
            thumbnail(&icon, 1280, &dir.path().join("thumbs")).unwrap(),
            icon
        );
    }

    #[test]
    fn broken_files_are_errors_not_panics() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"not a png").unwrap();
        assert!(thumbnail(&bad, 1280, &dir.path().join("thumbs")).is_err());
    }
}
