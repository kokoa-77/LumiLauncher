//! 世界的背景图：自己选的图（自定义背景）和玩的时候自动截的游戏画面（自动封面）。
//!
//! 都放在实例自己的 `Lumi/covers/` 里，不碰游戏的 `screenshots/`：
//! - `custom.json` + `custom/<编号>-<时间>.<png|jpg>`：每个世界自己选的图和显示方式
//! - `auto/<编号>/<时间>-<细节分>.jpg`：每个世界最近几张自动封面
//!
//! 编号是存档文件夹名的哈希：文件夹名里什么字符都可能有，不拿它当路径。
//! 每次换图都换一个新文件名，图片地址跟着变，界面不会拿到浏览器缓存里的旧图。

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, RgbImage};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::thumb::has_transparency;
use crate::fs::{
    AtomicWriteError, ReadJsonError, read_json, safe_join, write_atomic, write_json_atomic,
};
use crate::instance::META_DIR;

const COVERS_DIR: &str = "covers";
const AUTO_DIR: &str = "auto";
const CUSTOM_DIR: &str = "custom";
const CUSTOM_INDEX: &str = "custom.json";
/// 每个世界留几张自动封面（扩到 5 张供手动精选）
pub const AUTO_KEEP: usize = 5;
/// 自动封面存多宽（再宽首页也用不上；界面要多宽还会再缩）
const AUTO_MAX_WIDTH: u32 = 1920;
const JPEG_QUALITY: u8 = 85;
/// 自己选的图最大多少
pub const CUSTOM_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// 解码时最多占多少内存（8K 的 PNG 解开约 130 MB）
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// 图片至少这么宽、这么高
const MIN_SIDE: u32 = 64;
/// 宽高比小于这个（竖图、方图）默认完整显示：立绘铺满会被裁掉头和脚
const WIDE_ENOUGH: f32 = 1.25;
/// 算细节分时先缩到这么宽
const SCORE_WIDTH: u32 = 160;
/// 整张图亮度的离散程度低于这个，当作黑屏或纯色的加载画面
const MIN_SPREAD: f32 = 6.0;
/// 挑自动封面时只和这么久以内的几张比（同一次游玩、差不多的场景）
const SAME_SESSION_MS: u64 = 15 * 60 * 1000;

/// 背景图怎么显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ImageFit {
    /// 铺满，多出来的部分裁掉（截图、横的壁纸）
    #[default]
    Cover,
    /// 整张显示、靠右，后面垫着它自己的颜色（立绘、竖图、透明底的图）
    Contain,
}

#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    #[error("读不了这张图片：{0}")]
    Io(#[from] std::io::Error),
    #[error("图片太大了（最大 32 MB，边长不超过 8000 像素左右）")]
    TooLarge,
    #[error("只支持 PNG 和 JPG 图片")]
    Unsupported,
    #[error("图片太小了（至少 64 × 64 像素）")]
    TooSmall,
    #[error("这张图片打不开，可能已经损坏：{0}")]
    Image(#[from] image::ImageError),
    #[error("保存背景图失败")]
    Write(#[from] AtomicWriteError),
    #[error("读取背景图设置失败")]
    Read(#[from] ReadJsonError),
}

/// `custom.json`：存档文件夹名 → 选的图 / 锁定的自动封面；另含实例级默认自定义封面
#[derive(Debug, Default, Serialize, Deserialize)]
struct CustomIndex {
    #[serde(default)]
    worlds: BTreeMap<String, CustomEntry>,
    /// 锁定了某张自动封面的世界：存档文件夹名 → `auto/<world_key>/` 下的文件名
    #[serde(default)]
    locked_auto: BTreeMap<String, String>,
    /// 实例自身选定的自定义封面（或整合包官方封面缓存）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instance: Option<CustomEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CustomEntry {
    /// `custom/` 里的文件名
    file: String,
    fit: ImageFit,
}

/// 封面都放在哪：`<实例游戏目录>/Lumi/covers`
pub fn covers_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join(META_DIR).join(COVERS_DIR)
}

/// 存档文件夹名的哈希（取前 16 位），当文件名用
fn world_key(folder: &str) -> String {
    let hex = crate::net::to_hex(&Sha1::digest(folder.as_bytes()));
    hex[..16].to_owned()
}

fn auto_dir(instance_dir: &Path, folder: &str) -> PathBuf {
    covers_dir(instance_dir)
        .join(AUTO_DIR)
        .join(world_key(folder))
}

fn index_path(instance_dir: &Path) -> PathBuf {
    covers_dir(instance_dir).join(CUSTOM_INDEX)
}

fn load_index(instance_dir: &Path) -> Result<CustomIndex, ReadJsonError> {
    Ok(read_json(&index_path(instance_dir))?.unwrap_or_default())
}

/// 删一个文件，删不掉只记日志（旧封面多留一张不影响使用）
fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, file = %path.display(), "删旧背景图失败");
    }
}

// ---------- 列存档时用：每个世界该用哪张 ----------

/// 一个实例的封面信息，列存档时读一次，给每个世界用。
#[derive(Debug, Default)]
pub struct Covers {
    dir: PathBuf,
    custom: BTreeMap<String, CustomEntry>,
    locked_auto: BTreeMap<String, String>,
}

impl Covers {
    /// 读实例的封面设置；读不了就当没有（记日志，不影响列存档）
    pub fn load(instance_dir: &Path) -> Self {
        let index = match load_index(instance_dir) {
            Ok(index) => index,
            Err(error) => {
                tracing::warn!(%error, "读取背景图设置失败");
                CustomIndex::default()
            }
        };
        Self {
            dir: covers_dir(instance_dir),
            custom: index.worlds,
            locked_auto: index.locked_auto,
        }
    }

    /// 这个世界自己选的图（文件还在才算）
    pub fn custom(&self, folder: &str) -> Option<(PathBuf, ImageFit)> {
        let entry = self.custom.get(folder)?;
        // 文件名来自我们自己写的 JSON，也按不可信处理
        let path = safe_join(&self.dir.join(CUSTOM_DIR), &entry.file).ok()?;
        path.is_file().then_some((path, entry.fit))
    }

    /// 这个世界用户手动锁定的那张自动封面（文件还在才算）
    pub fn locked_auto(&self, folder: &str) -> Option<PathBuf> {
        let file = self.locked_auto.get(folder)?;
        let path = safe_join(&self.dir.join(AUTO_DIR).join(world_key(folder)), file).ok()?;
        path.is_file().then_some(path)
    }

    /// 这个世界的自动封面里最合适的一张（已锁定则优先返回锁定的那张）
    pub fn auto(&self, folder: &str) -> Option<PathBuf> {
        if self.dir.as_os_str().is_empty() {
            return None;
        }
        if let Some(locked) = self.locked_auto(folder) {
            return Some(locked);
        }
        let entries = auto_entries(&self.dir.join(AUTO_DIR).join(world_key(folder)));
        best_auto(&entries).map(|e| e.path.clone())
    }
}

// ---------- 自定义背景 ----------

fn validate_image_bytes(bytes: &[u8]) -> Result<(&'static str, ImageFit), CoverError> {
    if bytes.len() as u64 > CUSTOM_MAX_BYTES {
        return Err(CoverError::TooLarge);
    }
    // 按文件内容认格式，不看扩展名
    let format = image::guess_format(bytes).map_err(|_| CoverError::Unsupported)?;
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        _ => return Err(CoverError::Unsupported),
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let picture = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => CoverError::TooLarge,
        other => CoverError::Image(other),
    })?;
    let (width, height) = picture.dimensions();
    if width < MIN_SIDE || height < MIN_SIDE {
        return Err(CoverError::TooSmall);
    }
    Ok((ext, default_fit(&picture)))
}

/// 给一个世界换成自己选的图：检查是真的 PNG / JPG、大小合适，原样复制一份到实例自己的文件夹。
/// 返回默认的显示方式（竖图、透明底的图完整显示，其余铺满）。`now_ms` 用来起新文件名。
pub fn set_custom(
    instance_dir: &Path,
    folder: &str,
    source: &Path,
    now_ms: u64,
) -> Result<ImageFit, CoverError> {
    if std::fs::metadata(source)?.len() > CUSTOM_MAX_BYTES {
        return Err(CoverError::TooLarge);
    }
    let bytes = std::fs::read(source)?;
    let (ext, fit) = validate_image_bytes(&bytes)?;

    let custom_dir = covers_dir(instance_dir).join(CUSTOM_DIR);
    let file = format!("{}-{now_ms}.{ext}", world_key(folder));
    write_atomic(&custom_dir.join(&file), &bytes)?;
    let mut index = load_index(instance_dir)?;
    let old = index.worlds.insert(
        folder.to_owned(),
        CustomEntry {
            file: file.clone(),
            fit,
        },
    );
    write_json_atomic(&index_path(instance_dir), &index)?;
    if let Some(old) = old
        && old.file != file
        && let Ok(path) = safe_join(&custom_dir, &old.file)
    {
        remove_quietly(&path);
    }
    Ok(fit)
}

/// 读取实例级自定义封面（文件仍在磁盘上时返回路径与显示方式）。
pub fn instance_custom(instance_dir: &Path) -> Option<(PathBuf, ImageFit)> {
    let index = load_index(instance_dir).ok()?;
    let entry = index.instance?;
    let path = safe_join(&covers_dir(instance_dir).join(CUSTOM_DIR), &entry.file).ok()?;
    path.is_file().then_some((path, entry.fit))
}

/// 给实例本身设置自定义封面（从本地图片文件校验并复制进 `Lumi/covers/custom/`）。
pub fn set_instance_custom(
    instance_dir: &Path,
    source: &Path,
    now_ms: u64,
) -> Result<ImageFit, CoverError> {
    if std::fs::metadata(source)?.len() > CUSTOM_MAX_BYTES {
        return Err(CoverError::TooLarge);
    }
    let bytes = std::fs::read(source)?;
    set_instance_custom_bytes(instance_dir, &bytes, now_ms)
}

/// 给实例本身设置自定义封面（从内存图片字节校验并保存，供整合包封面缓存复用）。
pub fn set_instance_custom_bytes(
    instance_dir: &Path,
    bytes: &[u8],
    now_ms: u64,
) -> Result<ImageFit, CoverError> {
    let (ext, fit) = validate_image_bytes(bytes)?;
    let custom_dir = covers_dir(instance_dir).join(CUSTOM_DIR);
    let file = format!("instance-{now_ms}.{ext}");
    write_atomic(&custom_dir.join(&file), bytes)?;
    let mut index = load_index(instance_dir)?;
    let old = index.instance.replace(CustomEntry {
        file: file.clone(),
        fit,
    });
    write_json_atomic(&index_path(instance_dir), &index)?;
    if let Some(old) = old
        && old.file != file
        && let Ok(path) = safe_join(&custom_dir, &old.file)
    {
        remove_quietly(&path);
    }
    Ok(fit)
}

/// 清除实例级自定义封面，恢复自动/预设封面。
pub fn clear_instance_custom(instance_dir: &Path) -> Result<bool, CoverError> {
    let mut index = load_index(instance_dir)?;
    let Some(old) = index.instance.take() else {
        return Ok(false);
    };
    write_json_atomic(&index_path(instance_dir), &index)?;
    if let Ok(path) = safe_join(&covers_dir(instance_dir).join(CUSTOM_DIR), &old.file) {
        remove_quietly(&path);
    }
    Ok(true)
}

/// 竖图、方图、透明底的图（立绘）完整显示，横图铺满
fn default_fit(picture: &DynamicImage) -> ImageFit {
    let (width, height) = picture.dimensions();
    if has_transparency(picture) || (width as f32) < (height as f32) * WIDE_ENOUGH {
        ImageFit::Contain
    } else {
        ImageFit::Cover
    }
}

/// 改自己选的图的显示方式；这个世界没选过图时返回 `false`
pub fn set_fit(instance_dir: &Path, folder: &str, fit: ImageFit) -> Result<bool, CoverError> {
    let mut index = load_index(instance_dir)?;
    let Some(entry) = index.worlds.get_mut(folder) else {
        return Ok(false);
    };
    if entry.fit != fit {
        entry.fit = fit;
        write_json_atomic(&index_path(instance_dir), &index)?;
    }
    Ok(true)
}

/// 恢复默认背景（删掉自己选的图）；本来就没选过时返回 `false`
pub fn clear_custom(instance_dir: &Path, folder: &str) -> Result<bool, CoverError> {
    let mut index = load_index(instance_dir)?;
    let Some(old) = index.worlds.remove(folder) else {
        return Ok(false);
    };
    write_json_atomic(&index_path(instance_dir), &index)?;
    if let Ok(path) = safe_join(&covers_dir(instance_dir).join(CUSTOM_DIR), &old.file) {
        remove_quietly(&path);
    }
    Ok(true)
}

// ---------- 自动封面 ----------

/// 一张自动封面
#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoEntry {
    path: PathBuf,
    /// 截图时间（毫秒）
    time: u64,
    score: u32,
}

/// 文件名 `<时间>-<细节分>.jpg`
fn parse_auto(path: &Path) -> Option<AutoEntry> {
    if !path.extension().is_some_and(|x| x == "jpg") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let (time, score) = stem.split_once('-')?;
    Some(AutoEntry {
        path: path.to_path_buf(),
        time: time.parse().ok()?,
        score: score.parse().ok()?,
    })
}

/// 一个世界的自动封面，新的在前
fn auto_entries(dir: &Path) -> Vec<AutoEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<AutoEntry> = entries
        .flatten()
        .filter_map(|e| parse_auto(&e.path()))
        .collect();
    list.sort_by_key(|e| std::cmp::Reverse(e.time));
    list
}

/// 挑最新的、而且不像是开着菜单或背包的那张：
/// 和同一次游玩里的另外几张比，细节分不到最高的六成，多半是菜单把画面压暗或糊掉了，跳过。
fn best_auto(entries: &[AutoEntry]) -> Option<&AutoEntry> {
    let newest = entries.first()?;
    let recent: Vec<&AutoEntry> = entries
        .iter()
        .take_while(|e| newest.time.saturating_sub(e.time) <= SAME_SESSION_MS)
        .collect();
    let max = recent.iter().map(|e| e.score).max()?;
    recent
        .into_iter()
        .find(|e| u64::from(e.score) * 10 >= u64::from(max) * 6)
}

/// 画面的「细节分」：黑屏、纯色的加载画面返回 `None`。
///
/// 只看左右两边各四分之一（去掉最下面的快捷栏和聊天框）：背包、暂停菜单都在中间，
/// 打开时整个画面会被压暗（旧版本）或者糊掉（1.20.5 起），两边的明暗变化一下子少很多。
pub fn detail_score(image: &RgbImage) -> Option<u32> {
    if image.width() < MIN_SIDE || image.height() < MIN_SIDE {
        return None;
    }
    let height = ((u64::from(image.height()) * u64::from(SCORE_WIDTH)) / u64::from(image.width()))
        .clamp(8, 4 * u64::from(SCORE_WIDTH)) as u32;
    let small = image::imageops::resize(image, SCORE_WIDTH, height, FilterType::Triangle);
    let luma: Vec<f32> = small
        .pixels()
        .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
        .collect();
    let count = luma.len() as f32;
    let mean = luma.iter().sum::<f32>() / count;
    let spread = (luma.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / count).sqrt();
    if spread < MIN_SPREAD {
        return None;
    }

    let (w, h) = (SCORE_WIDTH as usize, height as usize);
    let side = w / 4;
    let bottom = h * 88 / 100;
    let mut sum = 0.0_f32;
    let mut n = 0_u32;
    for y in 0..bottom.saturating_sub(1) {
        for x in (0..side).chain(w - side..w - 1) {
            let i = y * w + x;
            sum += (luma[i + 1] - luma[i]).abs() + (luma[i + w] - luma[i]).abs();
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    Some((sum / n as f32 * 100.0).round() as u32)
}

/// 给精选弹窗展示的一张自动封面
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCoverItem {
    pub file: String,
    pub path: PathBuf,
    /// 截图时间（毫秒）
    pub time: u64,
    pub score: u32,
    /// 当前是否被选为这个世界的自动封面
    pub active: bool,
    /// 是否由用户手动锁定
    pub locked: bool,
}

/// 列出一个世界最近保存的自动封面（最多 [`AUTO_KEEP`] 张，新的在前）。
pub fn list_auto(instance_dir: &Path, folder: &str) -> Vec<AutoCoverItem> {
    let dir = auto_dir(instance_dir, folder);
    let entries = auto_entries(&dir);
    let locked_file = load_index(instance_dir)
        .ok()
        .and_then(|idx| idx.locked_auto.get(folder).cloned());
    let has_valid_lock = locked_file.as_deref().is_some_and(|name| {
        entries
            .iter()
            .any(|e| e.path.file_name().and_then(|n| n.to_str()) == Some(name))
    });
    let best_path = if has_valid_lock {
        None
    } else {
        best_auto(&entries).map(|e| e.path.clone())
    };
    entries
        .into_iter()
        .take(AUTO_KEEP)
        .filter_map(|entry| {
            let file = entry.path.file_name()?.to_str()?.to_owned();
            let locked = has_valid_lock && locked_file.as_deref() == Some(file.as_str());
            let active = if has_valid_lock {
                locked
            } else {
                best_path.as_deref() == Some(entry.path.as_path())
            };
            Some(AutoCoverItem {
                file,
                path: entry.path,
                time: entry.time,
                score: entry.score,
                active,
                locked,
            })
        })
        .collect()
}

/// 锁定某张自动封面为固定封面；若此前设置了自定义背景则一并清除，让这张封面立即生效。
/// 找不到该文件或文件名不合法时返回 `Ok(false)`。
pub fn lock_auto(instance_dir: &Path, folder: &str, file: &str) -> Result<bool, CoverError> {
    let dir = auto_dir(instance_dir, folder);
    let Ok(path) = safe_join(&dir, file) else {
        return Ok(false);
    };
    if !path.is_file() || parse_auto(&path).is_none() {
        return Ok(false);
    }
    let mut index = load_index(instance_dir)?;
    index.locked_auto.insert(folder.to_owned(), file.to_owned());
    let old_custom = index.worlds.remove(folder);
    write_json_atomic(&index_path(instance_dir), &index)?;
    if let Some(old) = old_custom
        && let Ok(old_path) = safe_join(&covers_dir(instance_dir).join(CUSTOM_DIR), &old.file)
    {
        remove_quietly(&old_path);
    }
    Ok(true)
}

/// 解除自动封面锁定，恢复自动精选与自动更新。未锁定时返回 `Ok(false)`。
pub fn unlock_auto(instance_dir: &Path, folder: &str) -> Result<bool, CoverError> {
    let mut index = load_index(instance_dir)?;
    if index.locked_auto.remove(folder).is_none() {
        return Ok(false);
    }
    write_json_atomic(&index_path(instance_dir), &index)?;
    Ok(true)
}

/// 这个世界的自动封面有没有被用户锁定（且锁定的文件仍在）
pub fn is_auto_locked(instance_dir: &Path, folder: &str) -> bool {
    Covers::load(instance_dir).locked_auto(folder).is_some()
}

/// 准星贴图的边长（界面像素）。四周再多补 1 格，模组换的稍大一点的准星也能盖住
const CROSSHAIR_GUI: u32 = 15;
/// 最下面游戏自己画的一截（界面像素，从底边往上量）：快捷栏 22、经验条、血量和饥饿、护甲和氧气泡
const HUD_GUI: u32 = 52;

/// 实例 `options.txt` 里的界面尺寸（`guiScale`，0 是自动）。读不到按自动算
fn gui_scale_setting(instance_dir: &Path) -> u32 {
    std::fs::read_to_string(instance_dir.join("options.txt"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("guiScale:"))
                .and_then(|value| value.trim().parse().ok())
        })
        .unwrap_or(0)
}

/// 和游戏一样算界面缩放：在缩放后宽 ≥ 320、高 ≥ 240 的前提下尽量大，设置了就不超过设置的值
fn gui_scale(width: u32, height: u32, setting: u32) -> u32 {
    let mut scale = 1;
    while scale != setting
        && scale < width
        && scale < height
        && width / (scale + 1) >= 320
        && height / (scale + 1) >= 240
    {
        scale += 1;
    }
    scale
}

/// 用方块四条边外面一圈的颜色把方块里面补上（左右、上下各插值一次再平均）。
/// 方块贴着图片边缘时不动（没有外面那一圈）。
fn fill_from_edges(image: &mut RgbImage, x0: u32, y0: u32, width: u32, height: u32) {
    let (w, h) = image.dimensions();
    if x0 == 0 || y0 == 0 || x0 + width >= w || y0 + height >= h {
        return;
    }
    let left: Vec<[u8; 3]> = (0..height)
        .map(|dy| image.get_pixel(x0 - 1, y0 + dy).0)
        .collect();
    let right: Vec<[u8; 3]> = (0..height)
        .map(|dy| image.get_pixel(x0 + width, y0 + dy).0)
        .collect();
    let top: Vec<[u8; 3]> = (0..width)
        .map(|dx| image.get_pixel(x0 + dx, y0 - 1).0)
        .collect();
    let bottom: Vec<[u8; 3]> = (0..width)
        .map(|dx| image.get_pixel(x0 + dx, y0 + height).0)
        .collect();
    for (dy, (l, r)) in left.iter().zip(&right).enumerate() {
        let ty = (dy as f32 + 1.0) / (height as f32 + 1.0);
        for (dx, (t, b)) in top.iter().zip(&bottom).enumerate() {
            let tx = (dx as f32 + 1.0) / (width as f32 + 1.0);
            let mut pixel = [0_u8; 3];
            for (c, out) in pixel.iter_mut().enumerate() {
                let across = f32::from(l[c]) * (1.0 - tx) + f32::from(r[c]) * tx;
                let down = f32::from(t[c]) * (1.0 - ty) + f32::from(b[c]) * ty;
                *out = ((across + down) / 2.0).round() as u8;
            }
            image.put_pixel(x0 + dx as u32, y0 + dy as u32, image::Rgb(pixel));
        }
    }
}

/// 去掉截图里游戏自己画的界面，让封面像一张风景照：
/// 正中间的准星用周围的颜色补上，最下面的快捷栏、血条一截裁掉（裁完不到一半高就不裁）。
/// 位置都按游戏自己的界面缩放算，`setting` 是 `options.txt` 的 `guiScale`。
fn without_hud(image: &RgbImage, setting: u32) -> RgbImage {
    let (w, h) = image.dimensions();
    let scale = gui_scale(w, h, setting);
    let mut clean = image.clone();
    // 准星画在界面坐标 ((界面宽 - 15) / 2, (界面高 - 15) / 2)，界面宽高向上取整
    let x0 = ((w.div_ceil(scale).saturating_sub(CROSSHAIR_GUI) / 2) * scale).saturating_sub(scale);
    let y0 = ((h.div_ceil(scale).saturating_sub(CROSSHAIR_GUI) / 2) * scale).saturating_sub(scale);
    let side = (CROSSHAIR_GUI + 2) * scale;
    fill_from_edges(&mut clean, x0, y0, side, side);
    let keep = h.saturating_sub(HUD_GUI * scale);
    if keep < h / 2 {
        return clean;
    }
    image::imageops::crop_imm(&clean, 0, 0, w, keep).to_image()
}

/// 存一张自动封面（去掉准星和快捷栏，缩到 1920 宽以内的 JPEG），每个世界只留最近 [`AUTO_KEEP`] 张。
/// 若用户已锁定固定自动封面，或截到黑屏 / 纯色画面，不存并返回 `Ok(None)`。
pub fn save_auto(
    instance_dir: &Path,
    folder: &str,
    image: &RgbImage,
    now_ms: u64,
) -> Result<Option<PathBuf>, CoverError> {
    if is_auto_locked(instance_dir, folder) {
        return Ok(None);
    }
    let Some(score) = detail_score(image) else {
        return Ok(None);
    };
    let image = &without_hud(image, gui_scale_setting(instance_dir));
    let resized;
    let picture = if image.width() > AUTO_MAX_WIDTH {
        resized = image::imageops::resize(
            image,
            AUTO_MAX_WIDTH,
            (u64::from(image.height()) * u64::from(AUTO_MAX_WIDTH) / u64::from(image.width()))
                .max(1) as u32,
            FilterType::Triangle,
        );
        &resized
    } else {
        image
    };
    let mut bytes = Vec::new();
    picture.write_with_encoder(JpegEncoder::new_with_quality(
        &mut Cursor::new(&mut bytes),
        JPEG_QUALITY,
    ))?;
    let dir = auto_dir(instance_dir, folder);
    let path = dir.join(format!("{now_ms}-{score}.jpg"));
    write_atomic(&path, &bytes)?;
    for old in auto_entries(&dir).into_iter().skip(AUTO_KEEP) {
        remove_quietly(&old.path);
    }
    Ok(Some(path))
}

/// 现在开着的是哪个单人世界：游戏打开世界时会改写存档里的 `session.lock`，
/// 取改写时间最新、而且不早于 `since` 的那个。多人游戏没有存档，返回 `None`。
/// 只看修改时间，**不打开这个文件**：锁住它会让游戏打不开世界。
pub fn open_world(instance_dir: &Path, since: SystemTime) -> Option<String> {
    std::fs::read_dir(instance_dir.join("saves"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let locked = e
                .path()
                .join("session.lock")
                .metadata()
                .ok()?
                .modified()
                .ok()?;
            if locked < since {
                return None;
            }
            Some((e.file_name().to_str()?.to_owned(), locked))
        })
        .max_by_key(|(_, locked)| *locked)
        .map(|(folder, _)| folder)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    use super::*;

    /// 有细节的「游戏画面」：棋盘格（不管多大，缩小算分时格子都一样大）加一点渐变
    fn scene(width: u32, height: u32) -> RgbImage {
        let block = (width / 40).max(1);
        RgbImage::from_fn(width, height, |x, y| {
            let check = ((x / block + y / block) % 2) as u8 * 90;
            Rgb([60 + check, 90 + check, (x % 200) as u8])
        })
    }

    /// 开着菜单：整个画面压暗到四分之一
    fn darkened(image: &RgbImage) -> RgbImage {
        RgbImage::from_fn(image.width(), image.height(), |x, y| {
            let p = image.get_pixel(x, y);
            Rgb([p[0] / 4, p[1] / 4, p[2] / 4])
        })
    }

    fn write_png(path: &Path, image: &DynamicImage) {
        image.save_with_format(path, ImageFormat::Png).unwrap();
    }

    #[test]
    fn blank_screens_have_no_score_and_menus_score_lower() {
        let black = RgbImage::from_pixel(640, 360, Rgb([0, 0, 0]));
        assert_eq!(detail_score(&black), None, "黑屏");
        let loading = RgbImage::from_pixel(640, 360, Rgb([70, 50, 40]));
        assert_eq!(detail_score(&loading), None, "纯色加载画面");
        assert_eq!(detail_score(&scene(32, 32)), None, "太小");

        let playing = scene(1280, 720);
        let menu = darkened(&playing);
        let a = detail_score(&playing).unwrap();
        let b = detail_score(&menu).unwrap();
        assert!(b * 10 < a * 6, "压暗后细节分明显变低：{a} → {b}");
    }

    #[test]
    fn keeps_the_newest_few_and_skips_menu_shots() {
        let dir = tempfile::tempdir().unwrap();
        let playing = scene(2560, 1440);
        let menu = darkened(&playing);
        let minute = 60_000;
        let base = 1_800_000_000_000_u64;

        let first = save_auto(dir.path(), "New World", &playing, base)
            .unwrap()
            .unwrap();
        let saved = image::image_dimensions(&first).unwrap();
        // 2560×1440 自动界面缩放是 6：底下 52×6 = 312 像素的快捷栏裁掉，再缩到 1920 宽
        assert_eq!(saved, (1920, 846), "去掉快捷栏后缩到 1920 宽");
        save_auto(dir.path(), "New World", &playing, base + 3 * minute).unwrap();
        // 最新的一张刚好开着背包：跳过它，用前一张
        save_auto(dir.path(), "New World", &menu, base + 6 * minute).unwrap();
        let covers = Covers::load(dir.path());
        let best = covers.auto("New World").unwrap();
        assert!(
            best.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&(base + 3 * minute).to_string()),
            "选的是 {}",
            best.display()
        );

        save_auto(dir.path(), "New World", &playing, base + 9 * minute).unwrap();
        save_auto(dir.path(), "New World", &playing, base + 12 * minute).unwrap();
        save_auto(dir.path(), "New World", &playing, base + 15 * minute).unwrap();
        let left = auto_entries(&auto_dir(dir.path(), "New World"));
        assert_eq!(left.len(), AUTO_KEEP, "只留最近 5 张");
        assert_eq!(left[0].time, base + 15 * minute);
        assert_eq!(
            covers.auto("New World").unwrap(),
            left[0].path,
            "最新的正常画面"
        );

        assert_eq!(covers.auto("别的世界"), None);
        assert_eq!(
            save_auto(
                dir.path(),
                "New World",
                &RgbImage::new(800, 600),
                base + 18 * minute
            )
            .unwrap(),
            None,
            "黑屏不存"
        );
    }

    #[test]
    fn locking_auto_cover_pins_it_and_pauses_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let playing = scene(1280, 720);
        let base = 1_800_000_000_000_u64;
        let minute = 60_000;

        save_auto(dir.path(), "W", &playing, base).unwrap();
        save_auto(dir.path(), "W", &playing, base + minute).unwrap();
        let items = list_auto(dir.path(), "W");
        assert_eq!(items.len(), 2);
        let older = items[1].file.clone();

        assert!(lock_auto(dir.path(), "W", &older).unwrap());
        assert!(is_auto_locked(dir.path(), "W"));
        assert_eq!(
            Covers::load(dir.path())
                .auto("W")
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            older
        );
        // 锁定期间不再写入新的自动封面
        assert_eq!(
            save_auto(dir.path(), "W", &playing, base + 2 * minute).unwrap(),
            None
        );
        assert_eq!(list_auto(dir.path(), "W").len(), 2);
        // 非法路径与不存在的文件被拒绝
        assert!(!lock_auto(dir.path(), "W", "../evil.jpg").unwrap());
        assert!(!lock_auto(dir.path(), "W", "999-1.jpg").unwrap());

        assert!(unlock_auto(dir.path(), "W").unwrap());
        assert!(!is_auto_locked(dir.path(), "W"));
        assert!(!unlock_auto(dir.path(), "W").unwrap());
        assert!(
            save_auto(dir.path(), "W", &playing, base + 3 * minute)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn old_shots_from_another_session_do_not_outvote_new_ones() {
        let entries = vec![
            AutoEntry {
                path: "c".into(),
                time: 100 * 60_000,
                score: 100,
            },
            AutoEntry {
                path: "b".into(),
                time: 40 * 60_000,
                score: 900,
            },
            AutoEntry {
                path: "a".into(),
                time: 37 * 60_000,
                score: 900,
            },
        ];
        assert_eq!(
            best_auto(&entries).unwrap().path,
            PathBuf::from("c"),
            "上一次游玩细节再多，也不压过这次的"
        );
        assert_eq!(best_auto(&[]), None);
    }

    #[test]
    fn custom_backgrounds_are_copied_checked_and_can_be_reset() {
        let dir = tempfile::tempdir().unwrap();
        let pictures = tempfile::tempdir().unwrap();
        let wide = pictures.path().join("wallpaper.png");
        write_png(&wide, &DynamicImage::ImageRgb8(scene(1600, 900)));
        let portrait = pictures.path().join("立绘.png");
        let art = RgbaImage::from_fn(600, 1000, |x, _| {
            if x < 100 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([200, 120, 160, 255])
            }
        });
        write_png(&portrait, &DynamicImage::ImageRgba8(art));

        assert_eq!(
            set_custom(dir.path(), "w", &wide, 1).unwrap(),
            ImageFit::Cover
        );
        let (first, fit) = Covers::load(dir.path()).custom("w").unwrap();
        assert_eq!(fit, ImageFit::Cover);
        assert!(
            first.starts_with(covers_dir(dir.path())),
            "复制到实例自己的文件夹"
        );

        // 换一张：新文件名（地址会变），旧文件删掉；立绘默认完整显示
        assert_eq!(
            set_custom(dir.path(), "w", &portrait, 2).unwrap(),
            ImageFit::Contain
        );
        let (second, fit) = Covers::load(dir.path()).custom("w").unwrap();
        assert_ne!(first, second);
        assert!(!first.exists(), "旧图删掉了");
        assert_eq!(fit, ImageFit::Contain);

        assert!(set_fit(dir.path(), "w", ImageFit::Cover).unwrap());
        assert_eq!(
            Covers::load(dir.path()).custom("w").unwrap().1,
            ImageFit::Cover
        );
        assert!(!set_fit(dir.path(), "没选过", ImageFit::Cover).unwrap());

        assert!(clear_custom(dir.path(), "w").unwrap());
        assert!(!second.exists());
        assert_eq!(Covers::load(dir.path()).custom("w"), None);
        assert!(!clear_custom(dir.path(), "w").unwrap(), "已经恢复过了");
    }

    #[test]
    fn rejects_files_that_are_not_pictures_or_too_small() {
        let dir = tempfile::tempdir().unwrap();
        let pictures = tempfile::tempdir().unwrap();
        let fake = pictures.path().join("fake.png");
        std::fs::write(&fake, b"not really a png").unwrap();
        assert!(matches!(
            set_custom(dir.path(), "w", &fake, 1),
            Err(CoverError::Unsupported)
        ));
        let tiny = pictures.path().join("tiny.png");
        write_png(&tiny, &DynamicImage::ImageRgb8(scene(32, 32)));
        assert!(matches!(
            set_custom(dir.path(), "w", &tiny, 1),
            Err(CoverError::TooSmall)
        ));
        assert_eq!(
            Covers::load(dir.path()).custom("w"),
            None,
            "失败时什么都不留"
        );
    }

    #[test]
    fn gui_scale_matches_the_game() {
        assert_eq!(gui_scale(1920, 1080, 0), 4);
        assert_eq!(gui_scale(1920, 1080, 2), 2, "设置了就不超过设置的值");
        assert_eq!(gui_scale(1280, 720, 0), 3);
        assert_eq!(gui_scale(854, 480, 0), 2);
        assert_eq!(gui_scale(640, 480, 0), 2);
        assert_eq!(gui_scale(320, 240, 0), 1);
    }

    #[test]
    fn reads_the_gui_scale_from_options() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            gui_scale_setting(dir.path()),
            0,
            "没有 options.txt 按自动算"
        );
        std::fs::write(
            dir.path().join("options.txt"),
            "version:3955\nguiScale:2\nfov:0.0\n",
        )
        .unwrap();
        assert_eq!(gui_scale_setting(dir.path()), 2);
    }

    #[test]
    fn auto_covers_drop_the_crosshair_and_hotbar() {
        // 1280×720 自动缩放是 3：准星 15×3 = 45 像素居中，快捷栏 22×3 = 66 像素贴底
        let sky = Rgb([120, 160, 220]);
        let mut shot = RgbImage::from_pixel(1280, 720, sky);
        for d in 0..45 {
            for t in 0..3 {
                shot.put_pixel(618 + d, 359 + t, Rgb([255, 255, 255]));
                shot.put_pixel(639 + t, 338 + d, Rgb([255, 255, 255]));
            }
        }
        for y in 654..720 {
            for x in 400..880 {
                shot.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        let clean = without_hud(&shot, 0);
        assert_eq!(clean.dimensions(), (1280, 720 - 52 * 3), "快捷栏那一截裁掉");
        assert!(clean.pixels().all(|p| *p == sky), "准星补成了周围的天空色");

        // 设置成 2 倍界面时准星和快捷栏都更小，裁掉的也更少
        assert_eq!(without_hud(&shot, 2).height(), 720 - 52 * 2);
        // 很扁的图（裁完不到一半高）只补准星、不裁
        let flat = RgbImage::from_pixel(320, 100, sky);
        assert_eq!(without_hud(&flat, 0).dimensions(), (320, 100));
    }

    #[test]
    fn finds_the_world_that_is_open_by_its_session_lock() {
        let dir = tempfile::tempdir().unwrap();
        let saves = dir.path().join("saves");
        for name in ["old", "now"] {
            std::fs::create_dir_all(saves.join(name)).unwrap();
            std::fs::write(saves.join(name).join("session.lock"), "☃").unwrap();
        }
        std::fs::create_dir_all(saves.join("no-lock")).unwrap();
        let old = std::fs::File::options()
            .write(true)
            .open(saves.join("old/session.lock"))
            .unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();

        let since = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(open_world(dir.path(), since).as_deref(), Some("now"));
        assert_eq!(
            open_world(dir.path(), SystemTime::now() + Duration::from_secs(60)),
            None,
            "进世界以后没有存档被打开（多人游戏）"
        );
        assert_eq!(open_world(&dir.path().join("nothing"), UNIX_EPOCH), None);
    }

    #[test]
    fn instance_custom_cover_saves_loads_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbImage::from_pixel(320, 180, Rgb([72, 130, 180]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let fit = set_instance_custom_bytes(dir.path(), &bytes, 1_000).unwrap();
        assert_eq!(fit, ImageFit::Cover);
        let (path, loaded_fit) = instance_custom(dir.path()).unwrap();
        assert!(path.is_file());
        assert_eq!(loaded_fit, ImageFit::Cover);

        assert!(clear_instance_custom(dir.path()).unwrap());
        assert!(instance_custom(dir.path()).is_none());
        assert!(!path.exists());
    }
}
