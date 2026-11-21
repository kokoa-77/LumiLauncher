//! 存档：读实例里的单人世界（招牌特色 1「直达你的世界」）。
//!
//! - `saves/<文件夹>/level.dat`（gzip 压缩的 NBT）：世界名、上次游玩时间、游戏内天数、游戏模式、存档版本
//! - `saves/<文件夹>/stats/<玩家 UUID>.json`：玩家在这个世界里玩了多久
//! - 背景图，按这个顺序挑：
//!   1. 你给这个世界选的图（[`cover`]，自定义背景）
//!   2. 这个世界上次游玩期间你自己截的图（实例的 `screenshots/`，按 F2 截的）
//!   3. 启动器玩的时候自动截的游戏画面（[`cover`]，自动封面）
//!   4. 世界自带的 `icon.png`（游戏建世界时自动拍的 64×64 缩略图）
//!
//! 只读，不改任何存档文件。读不了的存档跳过并记日志，不影响其他存档。
//! 世界备份在 [`backup`]，世界笔记在 [`notes`]。

pub mod backup;
pub mod cover;
pub mod nbt;
pub mod notes;
pub mod servers;
pub mod thumb;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use cover::{Covers, ImageFit};
use nbt::Value;

/// `level.dat` 解压后最多读这么多（正常只有几 KB；防止坏文件或压缩炸弹）
const MAX_LEVEL_BYTES: u64 = 16 * 1024 * 1024;
/// 一个实例最多列多少个存档
const MAX_WORLDS: usize = 200;
/// 游戏里一天的刻数
const TICKS_PER_DAY: i64 = 24_000;
/// 截图离世界的「上次游玩时间」多远以内，才算是这次游玩期间截的
const SESSION_BEFORE: Duration = Duration::from_secs(12 * 3600);
const SESSION_AFTER: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    fn from_id(id: i64) -> Option<Self> {
        match id {
            0 => Some(GameMode::Survival),
            1 => Some(GameMode::Creative),
            2 => Some(GameMode::Adventure),
            3 => Some(GameMode::Spectator),
            _ => None,
        }
    }
}

/// 一个单人世界。
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    /// 存档文件夹名（Quick Play 用它）
    pub folder: String,
    /// 游戏里显示的世界名
    pub name: String,
    /// 上次游玩（毫秒时间戳）；`level.dat` 里没写时用文件修改时间
    pub last_played: f64,
    /// 游戏里第几天（从 1 开始数，和玩家口头说的「第几天」一致）
    pub day: f64,
    pub game_mode: Option<GameMode>,
    pub hardcore: bool,
    /// 存档用的游戏版本（例如 1.21.1）
    pub version: Option<String>,
    /// 玩家在这个世界里玩了多久（秒）；读不到统计时为 `None`
    pub play_seconds: Option<f64>,
    /// 背景图
    pub image: Option<WorldImage>,
    /// 世界种子（64 位整数，用文字传：界面的数字放不下）；读不到时为空
    pub seed: Option<String>,
    /// 世界笔记在首页显示的那一行
    pub note: Option<notes::NoteLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldImage {
    pub path: PathBuf,
    pub kind: ImageKind,
    /// 怎么显示：只有自己选的图可能是「完整显示」
    pub fit: ImageFit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ImageKind {
    /// 这个世界上次游玩期间自己截的图
    Screenshot,
    /// 世界自带的缩略图（64×64）
    Icon,
    /// 启动器玩的时候自动截的游戏画面
    Auto,
    /// 自己给这个世界选的图
    Custom,
}

/// 列出实例里的单人世界，最近玩过的在前。读文件比较多，在后台线程里调用。
pub fn list(instance_dir: &Path, player_uuid: &str) -> Vec<World> {
    let mut candidates = scan_worlds(instance_dir).collect::<Vec<_>>();
    if candidates.is_empty() {
        return Vec::new();
    }
    candidates.sort_by(|(_, a), (_, b)| b.last_played.total_cmp(&a.last_played));
    let screenshots = screenshots(&instance_dir.join("screenshots"));
    let covers = Covers::load(instance_dir);
    let mut all_notes = notes::load_all(instance_dir);
    candidates
        .into_iter()
        .map(|(dir, world)| {
            let note = all_notes.remove(&world.folder).and_then(|n| n.line());
            with_details(world, &dir, player_uuid, &screenshots, &covers, note)
        })
        .collect()
}

/// 首页只需要最新一个。先按存档时间选中，再读该世界的统计、封面与笔记。
pub fn recent(instance_dir: &Path, player_uuid: &str) -> Option<World> {
    let (dir, world) = scan_worlds(instance_dir)
        // 期望与 list 的稳定排序一致：同一时间取先读到的存档。
        .reduce(|current, next| {
            if next.1.last_played.total_cmp(&current.1.last_played).is_gt() {
                next
            } else {
                current
            }
        })?;
    let screenshots = screenshots(&instance_dir.join("screenshots"));
    let covers = Covers::load(instance_dir);
    let note = notes::load(instance_dir, &world.folder).line();
    Some(with_details(
        world,
        &dir,
        player_uuid,
        &screenshots,
        &covers,
        note,
    ))
}

fn scan_worlds(instance_dir: &Path) -> impl Iterator<Item = (PathBuf, World)> {
    std::fs::read_dir(instance_dir.join("saves"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        // 启动器恢复备份时用的临时文件夹
        .filter(|e| {
            !e.file_name()
                .to_string_lossy()
                .starts_with(backup::TEMP_PREFIX)
        })
        .take(MAX_WORLDS)
        .filter_map(|e| {
            let folder = e.file_name().to_str()?.to_owned();
            let path = e.path();
            match read_world(&path, &folder) {
                Ok(world) => Some((path, world)),
                Err(reason) => {
                    tracing::debug!(%folder, reason, "跳过读不了的存档");
                    None
                }
            }
        })
}

fn with_details(
    mut world: World,
    dir: &Path,
    player_uuid: &str,
    screenshots: &[(PathBuf, SystemTime)],
    covers: &Covers,
    note: Option<notes::NoteLine>,
) -> World {
    let played_at = UNIX_EPOCH + Duration::from_millis(world.last_played as u64);
    world.play_seconds = play_seconds(&dir.join("stats"), player_uuid);
    world.image = pick_image(dir, &world.folder, played_at, screenshots, covers);
    world.note = note;
    world
}

/// 读 `level.dat`：返回整棵 NBT 和文件修改时间
fn read_level(dir: &Path) -> Result<(Value, Option<SystemTime>), String> {
    let level = dir.join("level.dat");
    let file = std::fs::File::open(&level).map_err(|e| format!("没有 level.dat：{e}"))?;
    let modified = file.metadata().and_then(|m| m.modified()).ok();
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(file)
        .take(MAX_LEVEL_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("level.dat 解压失败：{e}"))?;
    let root = nbt::parse(&bytes).map_err(|e| format!("level.dat 格式不对：{e}"))?;
    Ok((root, modified))
}

/// 一个世界在游戏里显示的名字（只读 `level.dat`，比 [`list`] 快）；读不到就是文件夹名
pub fn world_name(instance_dir: &Path, folder: &str) -> String {
    world_dir(instance_dir, folder)
        .ok()
        .and_then(|dir| read_level(&dir).ok())
        .and_then(|(root, _)| {
            root.get("Data")?
                .get("LevelName")?
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| folder.to_owned())
}

fn read_world(dir: &Path, folder: &str) -> Result<World, String> {
    let (root, modified) = read_level(dir)?;
    let data = root.get("Data").ok_or("level.dat 里没有 Data")?;

    let last_played = data
        .get("LastPlayed")
        .and_then(Value::as_i64)
        .filter(|ms| *ms > 0)
        .map(|ms| ms as f64)
        .or_else(|| modified.map(millis))
        .unwrap_or(0.0);
    // DayTime 是「从第一天早上起过了多少刻」（睡觉跳过的夜晚也算）；没有就退回总刻数 Time
    let ticks = data
        .get("DayTime")
        .or_else(|| data.get("Time"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    Ok(World {
        folder: folder.to_owned(),
        name: data
            .get("LevelName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(folder)
            .to_owned(),
        last_played,
        day: (ticks / TICKS_PER_DAY + 1) as f64,
        game_mode: data
            .get("GameType")
            .and_then(Value::as_i64)
            .and_then(GameMode::from_id),
        hardcore: data
            .get("hardcore")
            .and_then(Value::as_i64)
            .is_some_and(|v| v != 0),
        version: data
            .get("Version")
            .and_then(|v| v.get("Name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        play_seconds: None,
        image: None,
        seed: seed(data).map(|s| s.to_string()),
        note: None,
    })
}

/// 世界种子：1.16 起在 `WorldGenSettings.seed`，更早是 `RandomSeed`
fn seed(data: &Value) -> Option<i64> {
    data.get("WorldGenSettings")
        .and_then(|g| g.get("seed"))
        .or_else(|| data.get("RandomSeed"))
        .and_then(Value::as_i64)
}

fn millis(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

/// 游玩时长：先看这个玩家的统计文件，没有就取所有统计里最长的（换过名字的离线玩家）。
fn play_seconds(stats: &Path, player_uuid: &str) -> Option<f64> {
    let own = stats.join(format!("{player_uuid}.json"));
    if let Some(ticks) = read_play_ticks(&own) {
        return Some(ticks as f64 / 20.0);
    }
    std::fs::read_dir(stats)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_play_ticks(&e.path()))
        .max()
        .map(|ticks| ticks as f64 / 20.0)
}

/// 统计文件里的游玩刻数。1.17 起叫 `play_time`，更早叫 `play_one_minute`（名字是分钟，数值其实是刻）；
/// 1.12 及以前是平铺的 `stat.playOneMinute`。
fn read_play_ticks(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let custom = json.get("stats").and_then(|s| s.get("minecraft:custom"));
    custom
        .and_then(|c| {
            c.get("minecraft:play_time")
                .or_else(|| c.get("minecraft:play_one_minute"))
        })
        .or_else(|| json.get("stat.playOneMinute"))
        .and_then(serde_json::Value::as_u64)
}

/// 一张截图。
#[derive(Debug, Clone, PartialEq)]
pub struct Screenshot {
    pub path: PathBuf,
    pub name: String,
    /// 截图时间（文件修改时间，毫秒）
    pub taken_at: f64,
}

/// 实例的截图（`screenshots` 文件夹里的 png / jpg），新的在前。
pub fn list_screenshots(instance_dir: &Path) -> Vec<Screenshot> {
    let mut shots: Vec<Screenshot> = screenshots(&instance_dir.join("screenshots"))
        .into_iter()
        .map(|(path, time)| Screenshot {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            taken_at: millis(time),
            path,
        })
        .collect();
    shots.sort_by(|a, b| b.taken_at.total_cmp(&a.taken_at));
    shots
}

/// 一个存档的文件夹（存档名是界面传回来的，按不可信处理）
pub fn world_dir(instance_dir: &Path, folder: &str) -> Result<PathBuf, crate::fs::UnsafePath> {
    crate::fs::safe_join(&instance_dir.join("saves"), folder)
}

/// 实例截图文件夹里的图片（路径和修改时间）
fn screenshots(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("png") || x.eq_ignore_ascii_case("jpg"))
        })
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect()
}

/// 背景图，顺序见模块说明：自己选的图 → 上次游玩期间自己截的图里最晚的一张 → 自动封面 → 世界自带的缩略图。
/// 自己截的图不记是在哪个世界截的，所以按时间判断：截图时间落在「上次游玩」前 12 小时到后 5 分钟之间。
/// 自动封面是按世界分开存的，不用按时间猜。
fn pick_image(
    world_dir: &Path,
    folder: &str,
    played_at: SystemTime,
    screenshots: &[(PathBuf, SystemTime)],
    covers: &Covers,
) -> Option<WorldImage> {
    let image = |path: PathBuf, kind: ImageKind| WorldImage {
        path,
        kind,
        fit: ImageFit::Cover,
    };
    if let Some((path, fit)) = covers.custom(folder) {
        return Some(WorldImage {
            path,
            kind: ImageKind::Custom,
            fit,
        });
    }
    if let Some(path) = covers.locked_auto(folder) {
        return Some(image(path, ImageKind::Auto));
    }
    let from = played_at.checked_sub(SESSION_BEFORE).unwrap_or(UNIX_EPOCH);
    let to = played_at + SESSION_AFTER;
    if let Some((path, _)) = screenshots
        .iter()
        .filter(|(_, time)| *time >= from && *time <= to)
        .max_by_key(|(_, time)| *time)
    {
        return Some(image(path.clone(), ImageKind::Screenshot));
    }
    if let Some(path) = covers.auto(folder) {
        return Some(image(path, ImageKind::Auto));
    }
    let icon = world_dir.join("icon.png");
    icon.is_file().then(|| image(icon, ImageKind::Icon))
}

/// 这个游戏版本支不支持 Quick Play（启动时直接进存档）：1.20（快照 23w14a）起支持。
/// 新的年份版本号（26.1 这种）也算支持；认不出的版本当作不支持，退回普通启动。
pub fn supports_quick_play(game_version: &str) -> bool {
    let mut parts = game_version.split(['.', '-', ' ']);
    let first = parts.next().unwrap_or_default();
    if let Some((year, rest)) = first.split_once('w') {
        // 快照：23w14a
        let week: String = rest.chars().take_while(char::is_ascii_digit).collect();
        return match (year.parse::<u32>(), week.parse::<u32>()) {
            (Ok(y), Ok(w)) => y > 23 || (y == 23 && w >= 14),
            _ => false,
        };
    }
    match first.parse::<u32>() {
        Ok(1) => parts
            .next()
            .and_then(|minor| minor.parse::<u32>().ok())
            .is_some_and(|minor| minor >= 20),
        Ok(major) => major >= 2,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::nbt::tests::Writer;
    use super::*;

    const UUID: &str = "a01e3843-e521-3998-958a-f459800e4d11";

    fn write_level(dir: &Path, build: impl FnOnce(&mut Writer)) {
        let mut w = Writer::new();
        w.begin("Data");
        build(&mut w);
        w.end();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&w.finish()).unwrap();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("level.dat"), gz.finish().unwrap()).unwrap();
    }

    #[test]
    fn lists_worlds_newest_first_with_their_details() {
        let dir = tempfile::tempdir().unwrap();
        let saves = dir.path().join("saves");
        write_level(&saves.join("New World"), |w| {
            w.string("LevelName", "新的世界")
                .long("LastPlayed", 1_800_000_000_000)
                .begin("WorldGenSettings")
                .long("seed", -4_172_144_997_902_289_642)
                .end()
                .long("DayTime", 24_000 * 41 + 6_000)
                .int("GameType", 0)
                .byte("hardcore", 1)
                .begin("Version")
                .string("Name", "1.21.1")
                .end();
        });
        write_level(&saves.join("old"), |w| {
            w.string("LevelName", "旧世界")
                .long("LastPlayed", 1_700_000_000_000)
                .int("GameType", 1);
        });
        std::fs::create_dir_all(saves.join("broken")).unwrap();
        std::fs::write(saves.join("broken/level.dat"), b"not gzip").unwrap();
        let stats = saves.join("New World/stats");
        std::fs::create_dir_all(&stats).unwrap();
        std::fs::write(
            stats.join(format!("{UUID}.json")),
            r#"{"stats":{"minecraft:custom":{"minecraft:play_time":72000}},"DataVersion":3955}"#,
        )
        .unwrap();

        let worlds = list(dir.path(), UUID);
        assert_eq!(worlds.len(), 2, "读不了的存档跳过");
        assert_eq!(recent(dir.path(), UUID).as_ref(), worlds.first());
        let w = &worlds[0];
        assert_eq!(w.folder, "New World");
        assert_eq!(w.name, "新的世界");
        assert_eq!(w.day, 42.0, "过了 41 天多一点，就是第 42 天");
        assert_eq!(w.game_mode, Some(GameMode::Survival));
        assert!(w.hardcore);
        assert_eq!(w.version.as_deref(), Some("1.21.1"));
        assert_eq!(w.play_seconds, Some(3600.0), "72000 刻 = 1 小时");
        assert_eq!(
            w.seed.as_deref(),
            Some("-4172144997902289642"),
            "种子超出界面数字的精度，用文字"
        );
        assert_eq!(worlds[1].name, "旧世界");
        assert_eq!(worlds[1].day, 1.0, "没写时间就是第 1 天");
        assert_eq!(worlds[1].play_seconds, None);
    }

    #[test]
    fn reads_older_stats_formats_and_other_players() {
        let dir = tempfile::tempdir().unwrap();
        let stats = dir.path().join("stats");
        std::fs::create_dir_all(&stats).unwrap();
        std::fs::write(
            stats.join("someone-else.json"),
            r#"{"stats":{"minecraft:custom":{"minecraft:play_one_minute":2400}}}"#,
        )
        .unwrap();
        assert_eq!(play_seconds(&stats, UUID), Some(120.0), "换过名字也能读到");
        std::fs::write(
            stats.join(format!("{UUID}.json")),
            r#"{"stat.playOneMinute":200}"#,
        )
        .unwrap();
        assert_eq!(play_seconds(&stats, UUID), Some(10.0), "1.12 的格式");
    }

    #[test]
    fn picks_a_screenshot_from_the_last_session_or_the_icon() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("saves/w");
        std::fs::create_dir_all(&world).unwrap();
        let played = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let shot = |name: &str, offset_secs: i64| {
            let time = if offset_secs >= 0 {
                played + Duration::from_secs(offset_secs as u64)
            } else {
                played - Duration::from_secs((-offset_secs) as u64)
            };
            (dir.path().join(name), time)
        };
        let shots = vec![
            shot("during.png", -600),
            shot("earlier-same-day.png", -3600),
            shot("last-week.png", -7 * 86400),
            shot("next-day.png", 86400),
        ];
        let none = Covers::default();
        let picked = pick_image(&world, "w", played, &shots, &none).unwrap();
        assert_eq!(picked.kind, ImageKind::Screenshot);
        assert!(
            picked.path.ends_with("during.png"),
            "取这次游玩期间最晚的一张"
        );

        assert_eq!(
            pick_image(&world, "w", played, &shots[2..], &none),
            None,
            "都不在这次游玩期间，也没有缩略图"
        );
        std::fs::write(world.join("icon.png"), b"png").unwrap();
        assert_eq!(
            pick_image(&world, "w", played, &shots[2..], &none)
                .unwrap()
                .kind,
            ImageKind::Icon
        );
    }

    #[test]
    fn own_choice_beats_screenshots_which_beat_auto_covers() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("saves/w");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("icon.png"), b"png").unwrap();
        let played = SystemTime::now();
        let own_shot = vec![(dir.path().join("screenshots/mine.png"), played)];

        // 自动封面：一张有细节的画面
        let scene = image::RgbImage::from_fn(640, 360, |x, y| {
            let check = ((x / 16 + y / 16) % 2) as u8 * 90;
            image::Rgb([60 + check, 90 + check, 120])
        });
        cover::save_auto(dir.path(), "w", &scene, 1_800_000_000_000).unwrap();
        let covers = Covers::load(dir.path());
        assert_eq!(
            pick_image(&world, "w", played, &[], &covers).unwrap().kind,
            ImageKind::Auto,
            "没有自己截的图时用自动封面，不用小图标"
        );
        assert_eq!(
            pick_image(&world, "w", played, &own_shot, &covers)
                .unwrap()
                .kind,
            ImageKind::Screenshot,
            "自己截的图比自动封面优先"
        );

        let picture = dir.path().join("立绘.png");
        image::RgbaImage::from_pixel(300, 500, image::Rgba([200, 120, 160, 255]))
            .save(&picture)
            .unwrap();
        cover::set_custom(dir.path(), "w", &picture, 1).unwrap();
        let covers = Covers::load(dir.path());
        let picked = pick_image(&world, "w", played, &own_shot, &covers).unwrap();
        assert_eq!(picked.kind, ImageKind::Custom, "自己选的图最优先");
        assert_eq!(picked.fit, ImageFit::Contain, "竖图默认完整显示");
    }

    #[test]
    fn knows_which_versions_support_quick_play() {
        for yes in [
            "1.20", "1.20.1", "1.21.1", "23w14a", "24w10a", "26.1", "26.3",
        ] {
            assert!(supports_quick_play(yes), "{yes}");
        }
        for no in [
            "1.19.4", "1.12.2", "23w13a", "22w45a", "b1.7.3", "", "neoforge",
        ] {
            assert!(!supports_quick_play(no), "{no}");
        }
    }
}
