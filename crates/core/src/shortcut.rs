//! 桌面快捷方式（D17k）：双击直接进某个实例、某个世界。
//!
//! - 快捷方式文件（`.lnk`）按微软公开的 Shell Link 格式（MS-SHLLINK）自己写：只写「目标路径 + 参数 +
//!   起始位置 + 图标 + 说明」。不调 PowerShell、不碰 COM：杀毒软件常把「程序叫 PowerShell 往桌面放快捷方式」
//!   当成可疑行为，也用不着 `unsafe`。
//! - 世界的快捷方式用世界自己的图标（`icon.png`，游戏第一次进世界时截的），转成带圆角的 `.ico`。
//! - 启动器收到 `--play <实例> [--world <存档文件夹>]` 就直接启动（见 [`parse_play_args`]）。

use std::path::Path;

use image::imageops::FilterType;
use image::{ImageBuffer, Rgba, RgbaImage};

use crate::fs::{AtomicWriteError, write_atomic};

pub const PLAY_ARG: &str = "--play";
pub const WORLD_ARG: &str = "--world";

#[derive(Debug, thiserror::Error)]
pub enum ShortcutError {
    #[error("快捷方式里的文字太长")]
    TooLong,
    #[error("世界图标读不了")]
    Icon(#[source] image::ImageError),
    #[error("写快捷方式失败")]
    Write(#[from] AtomicWriteError),
}

/// 快捷方式要的东西。
pub struct Shortcut<'a> {
    /// 要运行的程序（完整路径）
    pub target: &'a Path,
    /// 参数（会按 Windows 的规则加引号拼起来）
    pub args: &'a [String],
    pub working_dir: &'a Path,
    /// 图标文件（`.ico`）；`None` 用程序自己的图标
    pub icon: Option<&'a Path>,
    /// 鼠标停上去显示的说明
    pub description: &'a str,
}

/// 双击快捷方式要做的事。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PlayRequest {
    pub instance_id: String,
    /// 存档文件夹名；`None` 是只启动实例
    pub world: Option<String>,
}

/// 快捷方式的参数。
pub fn play_args(request: &PlayRequest) -> Vec<String> {
    let mut args = vec![PLAY_ARG.to_owned(), request.instance_id.clone()];
    if let Some(world) = &request.world {
        args.extend([WORLD_ARG.to_owned(), world.clone()]);
    }
    args
}

/// 从命令行参数里找 `--play <实例> [--world <文件夹>]`（第一个是程序自己，跳过）。没有就是 `None`。
pub fn parse_play_args<I, S>(args: I) -> Option<PlayRequest>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .skip(1)
        .map(|a| a.as_ref().to_owned())
        .collect();
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
    };
    Some(PlayRequest {
        instance_id: value_of(PLAY_ARG)?,
        world: value_of(WORLD_ARG),
    })
}

/// 按 Windows 拆参数的规则（`CommandLineToArgvW`）把参数拼成一行：带空格、引号的加引号，
/// 引号前的反斜杠加倍。
pub fn quote_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {}
        }
        if c != '\\' {
            out.extend(std::iter::repeat_n('\\', slashes));
            slashes = 0;
            out.push(c);
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// 能当文件名的快捷方式名字：去掉 Windows 不允许的字符，最长 80 个字。
pub fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "Minecraft".to_owned()
    } else {
        trimmed.to_owned()
    }
}

// ---------- .lnk ----------

/// 有 LinkInfo、有说明、有起始位置、有参数、有图标、字符串是 UTF-16、有环境变量路径块
/// （没有 IDList 时系统靠最后这一块找到目标，LinkInfo 只给属性里显示用）
const FLAGS: u32 = 0x02 | 0x04 | 0x10 | 0x20 | 0x40 | 0x80 | 0x200;
/// 环境变量路径块里路径最多多少个字（含结尾的 0）
const EXP_CHARS: usize = 260;
const CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const SW_SHOWNORMAL: u32 = 1;
const DRIVE_FIXED: u32 = 3;

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// 带长度的 UTF-16 字符串（StringData）
fn put_string(out: &mut Vec<u8>, text: &str) -> Result<(), ShortcutError> {
    let units = utf16(text);
    put_u16(
        out,
        u16::try_from(units.len()).map_err(|_| ShortcutError::TooLong)?,
    );
    for unit in units {
        put_u16(out, unit);
    }
    Ok(())
}

/// LinkInfo：目标的本地路径。旧的单字节路径里不是 ASCII 的字换成「?」，系统用后面的 UTF-16 路径。
fn link_info(target: &str) -> Result<Vec<u8>, ShortcutError> {
    const HEADER: u32 = 0x24;
    // 卷信息：固定磁盘、没有卷标
    let mut volume = Vec::new();
    put_u32(&mut volume, 0x11);
    put_u32(&mut volume, DRIVE_FIXED);
    put_u32(&mut volume, 0);
    put_u32(&mut volume, 0x10);
    volume.push(0);

    let mut ansi: Vec<u8> = target
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect();
    ansi.push(0);
    let mut wide = Vec::new();
    for unit in utf16(target) {
        put_u16(&mut wide, unit);
    }
    put_u16(&mut wide, 0);

    let len = |n: usize| u32::try_from(n).map_err(|_| ShortcutError::TooLong);
    let volume_at = HEADER;
    let base_at = volume_at + len(volume.len())?;
    let suffix_at = base_at + len(ansi.len())?;
    let base_wide_at = suffix_at + 1;
    let suffix_wide_at = base_wide_at + len(wide.len())?;
    let total = suffix_wide_at + 2;

    let mut out = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    put_u32(&mut out, total);
    put_u32(&mut out, HEADER);
    put_u32(&mut out, 0x1); // VolumeIDAndLocalBasePath
    put_u32(&mut out, volume_at);
    put_u32(&mut out, base_at);
    put_u32(&mut out, 0); // 不是网络路径
    put_u32(&mut out, suffix_at);
    put_u32(&mut out, base_wide_at);
    put_u32(&mut out, suffix_wide_at);
    out.extend_from_slice(&volume);
    out.extend_from_slice(&ansi);
    out.push(0); // 空的路径后缀
    out.extend_from_slice(&wide);
    put_u16(&mut out, 0);
    Ok(out)
}

/// 拼出整个 `.lnk` 文件。
pub fn lnk_bytes(shortcut: &Shortcut) -> Result<Vec<u8>, ShortcutError> {
    let target = shortcut.target.to_string_lossy();
    let mut out = Vec::with_capacity(512);
    put_u32(&mut out, 0x4C);
    out.extend_from_slice(&CLSID);
    put_u32(&mut out, FLAGS);
    put_u32(&mut out, FILE_ATTRIBUTE_ARCHIVE);
    out.extend_from_slice(&[0; 24]); // 创建、访问、修改时间
    put_u32(&mut out, 0); // 文件大小
    put_u32(&mut out, 0); // 图标序号
    put_u32(&mut out, SW_SHOWNORMAL);
    put_u16(&mut out, 0); // 快捷键
    out.extend_from_slice(&[0; 10]);
    out.extend_from_slice(&link_info(&target)?);
    put_string(&mut out, shortcut.description)?;
    put_string(&mut out, &shortcut.working_dir.to_string_lossy())?;
    put_string(&mut out, &quote_args(shortcut.args))?;
    let icon = shortcut.icon.unwrap_or(shortcut.target);
    put_string(&mut out, &icon.to_string_lossy())?;
    out.extend_from_slice(&exp_block(&target)?);
    put_u32(&mut out, 0); // 结束
    Ok(out)
}

/// 环境变量路径块（EnvironmentVariableDataBlock）：单字节和 UTF-16 各一份目标路径，各占固定 260 个字。
fn exp_block(target: &str) -> Result<Vec<u8>, ShortcutError> {
    let wide = utf16(target);
    if wide.len() >= EXP_CHARS {
        return Err(ShortcutError::TooLong);
    }
    let mut out = Vec::with_capacity(0x314);
    put_u32(&mut out, 0x314);
    put_u32(&mut out, 0xA000_0001);
    let mut ansi = [0u8; EXP_CHARS];
    for (slot, c) in ansi.iter_mut().zip(target.chars()) {
        *slot = if c.is_ascii() { c as u8 } else { b'?' };
    }
    out.extend_from_slice(&ansi);
    let mut units = [0u16; EXP_CHARS];
    units[..wide.len()].copy_from_slice(&wide);
    for unit in units {
        put_u16(&mut out, unit);
    }
    Ok(out)
}

/// 写快捷方式（原子写：不会留下半个文件；同名的换成新的）。
pub fn write_lnk(path: &Path, shortcut: &Shortcut) -> Result<(), ShortcutError> {
    write_atomic(path, &lnk_bytes(shortcut)?)?;
    Ok(())
}

// ---------- 图标 ----------

/// 图标里放的几种大小（桌面一般用 48，高分屏 64 / 96，大图标 256）
const ICON_SIZES: [u32; 5] = [16, 32, 48, 64, 256];

/// 把世界图标转成带圆角的 `.ico`。
pub fn world_icon(png: &Path, out: &Path) -> Result<(), ShortcutError> {
    let source = image::open(png).map_err(ShortcutError::Icon)?.to_rgba8();
    let bytes = icon_bytes(&source)?;
    write_atomic(out, &bytes)?;
    Ok(())
}

fn icon_bytes(source: &RgbaImage) -> Result<Vec<u8>, ShortcutError> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    // 世界图标一般是 64×64 的正方形；不是正方形的从中间裁
    let side = source.width().min(source.height()).max(1);
    let square = image::imageops::crop_imm(
        source,
        (source.width() - side) / 2,
        (source.height() - side) / 2,
        side,
        side,
    )
    .to_image();
    let mut frames = Vec::new();
    for size in ICON_SIZES {
        let mut scaled = image::imageops::resize(&square, size, size, FilterType::CatmullRom);
        round_corners(&mut scaled);
        let frame = IcoFrame::as_png(scaled.as_raw(), size, size, image::ExtendedColorType::Rgba8)
            .map_err(ShortcutError::Icon)?;
        frames.push(frame);
    }
    let mut bytes = Vec::new();
    IcoEncoder::new(&mut bytes)
        .encode_images(&frames)
        .map_err(ShortcutError::Icon)?;
    Ok(bytes)
}

/// 四个角切成圆角（半径是边长的 18%，边缘一像素做半透明过渡）
fn round_corners(image: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let size = image.width() as f32;
    let radius = (size * 0.18).max(2.0);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        let cx = px.clamp(radius, size - radius);
        let cy = py.clamp(radius, size - radius);
        let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
        if coverage < 1.0 {
            pixel[3] = (f32::from(pixel[3]) * coverage).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn quotes_like_windows_splits() {
        let q = |a: &str| quote_arg(a);
        assert_eq!(q("plain"), "plain");
        assert_eq!(q("新的世界"), "新的世界");
        assert_eq!(q("有 空格"), "\"有 空格\"");
        assert_eq!(q(""), "\"\"");
        assert_eq!(q("a\"b"), "\"a\\\"b\"");
        assert_eq!(q("a b\\"), "\"a b\\\\\"", "结尾的反斜杠加倍");
        assert_eq!(q("a\\b c"), "\"a\\b c\"", "中间的反斜杠不动");
        assert_eq!(
            quote_args(&play_args(&PlayRequest {
                instance_id: "生存 1.21".to_owned(),
                world: Some("新的世界".to_owned()),
            })),
            "--play \"生存 1.21\" --world 新的世界"
        );
    }

    #[test]
    fn reads_play_args_back() {
        let exe = "launcher.exe";
        assert_eq!(
            parse_play_args([exe, "--play", "生存", "--world", "新的 世界"]),
            Some(PlayRequest {
                instance_id: "生存".to_owned(),
                world: Some("新的 世界".to_owned())
            })
        );
        assert_eq!(
            parse_play_args([exe, "--play", "生存"]),
            Some(PlayRequest {
                instance_id: "生存".to_owned(),
                world: None
            })
        );
        assert_eq!(parse_play_args([exe]), None);
        assert_eq!(parse_play_args([exe, "--play"]), None);
        assert_eq!(parse_play_args([exe, "--play", "--world", "x"]), None);
        assert_eq!(parse_play_args(["--play", "x"]), None, "第一个是程序自己");
    }

    #[test]
    fn safe_file_names() {
        assert_eq!(file_name("生存 · 新的世界"), "生存 · 新的世界");
        assert_eq!(file_name("a/b:c?"), "a_b_c_");
        assert_eq!(file_name(" ... "), "Minecraft");
        assert_eq!(file_name(&"长".repeat(100)).chars().count(), 80);
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    /// 按格式把 `.lnk` 读回来：路径（UTF-16 的那份）、说明、起始位置、参数、图标
    fn read_back(bytes: &[u8]) -> (String, Vec<String>) {
        assert_eq!(u32_at(bytes, 0), 0x4C);
        assert_eq!(&bytes[4..20], &CLSID);
        assert_eq!(u32_at(bytes, 20), FLAGS);
        let info = 76;
        let info_size = u32_at(bytes, info) as usize;
        let wide_at = info + u32_at(bytes, info + 28) as usize;
        let mut units = Vec::new();
        let mut at = wide_at;
        while u16_at(bytes, at) != 0 {
            units.push(u16_at(bytes, at));
            at += 2;
        }
        let path = String::from_utf16(&units).unwrap();
        let mut at = info + info_size;
        let mut strings = Vec::new();
        for _ in 0..4 {
            let len = u16_at(bytes, at) as usize;
            let units: Vec<u16> = (0..len).map(|i| u16_at(bytes, at + 2 + i * 2)).collect();
            strings.push(String::from_utf16(&units).unwrap());
            at += 2 + len * 2;
        }
        assert_eq!(u32_at(bytes, at), 0x314, "环境变量路径块");
        assert_eq!(u32_at(bytes, at + 4), 0xA000_0001);
        let exp: Vec<u16> = (0..EXP_CHARS)
            .map(|i| u16_at(bytes, at + 8 + EXP_CHARS + i * 2))
            .take_while(|&u| u != 0)
            .collect();
        assert_eq!(String::from_utf16(&exp).unwrap(), path);
        let at = at + 0x314;
        assert_eq!(u32_at(bytes, at), 0, "最后是结束块");
        assert_eq!(at + 4, bytes.len());
        (path, strings)
    }

    #[test]
    fn writes_a_readable_lnk_with_chinese_paths() {
        let target = PathBuf::from(r"D:\游戏\我的启动器\launcher.exe");
        let args = play_args(&PlayRequest {
            instance_id: "生存".to_owned(),
            world: Some("新的世界".to_owned()),
        });
        let icon = PathBuf::from(r"C:\icons\a.ico");
        let bytes = lnk_bytes(&Shortcut {
            target: &target,
            args: &args,
            working_dir: Path::new(r"D:\游戏\我的启动器"),
            icon: Some(&icon),
            description: "进入「新的世界」",
        })
        .unwrap();
        let (path, strings) = read_back(&bytes);
        assert_eq!(path, r"D:\游戏\我的启动器\launcher.exe");
        assert_eq!(
            strings,
            vec![
                "进入「新的世界」".to_owned(),
                r"D:\游戏\我的启动器".to_owned(),
                "--play 生存 --world 新的世界".to_owned(),
                r"C:\icons\a.ico".to_owned(),
            ]
        );
        // 旧的单字节路径里中文换成了问号
        let info = 76;
        let ansi_at = info + u32_at(&bytes, info + 16) as usize;
        assert_eq!(&bytes[ansi_at..ansi_at + 6], b"D:\\??\\");

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("a.lnk");
        write_lnk(
            &out,
            &Shortcut {
                target: &target,
                args: &args,
                working_dir: dir.path(),
                icon: None,
                description: "",
            },
        )
        .unwrap();
        let (_, strings) = read_back(&std::fs::read(&out).unwrap());
        assert_eq!(
            strings[3],
            target.to_string_lossy(),
            "没有图标时用程序自己的"
        );
    }

    #[test]
    fn icons_have_every_size_and_round_corners() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("icon.png");
        RgbaImage::from_pixel(64, 64, Rgba([40, 160, 80, 255]))
            .save(&png)
            .unwrap();
        let ico = dir.path().join("a.ico");
        world_icon(&png, &ico).unwrap();
        let bytes = std::fs::read(&ico).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 1, 0], "ICO 文件头");
        assert_eq!(u16_at(&bytes, 4), 5, "五种大小");

        let mut square = RgbaImage::from_pixel(48, 48, Rgba([255, 255, 255, 255]));
        round_corners(&mut square);
        assert_eq!(square.get_pixel(0, 0)[3], 0, "角上透明");
        assert_eq!(square.get_pixel(24, 24)[3], 255, "中间不透明");
        assert_eq!(square.get_pixel(24, 0)[3], 255, "边的中间不切");

        std::fs::write(&png, b"not a png").unwrap();
        assert!(matches!(
            world_icon(&png, &ico),
            Err(ShortcutError::Icon(_))
        ));
    }

    #[test]
    #[ignore = "手动：生成一个真的快捷方式给 Windows 读"]
    fn write_real_shortcut() {
        let out = PathBuf::from(std::env::var("MCL_LNK_OUT").unwrap());
        let target = PathBuf::from(std::env::var("MCL_LNK_TARGET").unwrap());
        let args: Vec<String> = std::env::var("MCL_LNK_ARGS")
            .unwrap()
            .split('|')
            .map(String::from)
            .collect();
        let icon = std::env::var("MCL_LNK_ICON").ok().map(PathBuf::from);
        write_lnk(
            &out,
            &Shortcut {
                target: &target,
                args: &args,
                working_dir: target.parent().unwrap(),
                icon: icon.as_deref(),
                description: "测试「快捷方式」",
            },
        )
        .unwrap();
    }
}
