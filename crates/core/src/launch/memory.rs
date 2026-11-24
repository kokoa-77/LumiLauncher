//! 给游戏分多少内存、启动前够不够、谁占着内存。
//!
//! 自动模式（按需分配，上限 16 GB）：先定「想要多少」——按模组数量估一个，
//! 整合包作者写了推荐内存（CurseForge 清单的 `recommendedRam`）而且比估的多，就按作者的；2–16 GB。
//! 再按这台电脑的情况封顶：不超过总内存的一半，也不超过现在空闲内存减去 1 GB（给系统和别的程序留余地）。
//! 不像 HMCL 那样「空多少给多少」：原版也拿十来 GB 的话，Java 会慢慢把它占满，别的程序就挤了。
//!
//! 「空闲」用的是系统的「可用内存」（待机 + 空闲），和任务管理器「性能 → 内存」里的「可用」是同一个数。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

const MB: u64 = 1024 * 1024;
/// 没装模组时的起点
const BASE_MB: u64 = 2560;
/// 每个模组多给一点
const PER_MOD_MB: u64 = 24;
const AUTO_MAX_MB: u64 = 16384;
const AUTO_MIN_MB: u64 = 2048;
/// 给系统和其他程序留的空闲内存
const HEADROOM_MB: u64 = 1024;
/// 查不到电脑内存时最多给这么多
const UNKNOWN_SYSTEM_MAX_MB: u64 = 4096;
const STEP_MB: u64 = 256;

/// 内存设置：自动，或者用户手动指定（MB）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryChoice {
    Auto,
    Manual(u32),
}

/// 这台电脑的内存情况（MB）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemMemory {
    pub total_mb: u64,
    pub available_mb: u64,
}

/// 读这台电脑的内存。读不到（极少见）返回 `None`。
pub fn system_memory() -> Option<SystemMemory> {
    let mut system = match sysinfo::System::new() {
        Ok(system) => system,
        Err(error) => {
            tracing::warn!(%error, "读不到电脑的内存情况");
            return None;
        }
    };
    system.refresh_memory();
    let total_mb = system.total_memory() / MB;
    (total_mb > 0).then(|| SystemMemory {
        total_mb,
        available_mb: system.available_memory() / MB,
    })
}

/// 只按模组数量估的（没封顶）
fn by_mods(mods: usize) -> u64 {
    let mods = u64::try_from(mods).unwrap_or(u64::MAX);
    BASE_MB.saturating_add(PER_MOD_MB.saturating_mul(mods))
}

/// 「想要多少」（还没按电脑情况封顶）：按模组数量估的、整合包作者推荐的，取大的，2–16 GB
pub fn wanted_mb(mods: usize, recommended: Option<u32>) -> u64 {
    by_mods(mods)
        .max(recommended.map_or(0, u64::from))
        .clamp(AUTO_MIN_MB, AUTO_MAX_MB)
}

/// 「想要多少」是不是按整合包作者的推荐定的
fn from_pack(mods: usize, recommended: Option<u32>) -> bool {
    recommended.is_some_and(|mb| u64::from(mb) > by_mods(mods))
}

fn round_down(mb: u64) -> u64 {
    (mb / STEP_MB * STEP_MB).max(STEP_MB)
}

fn to_u32(mb: u64) -> u32 {
    u32::try_from(mb).unwrap_or(u32::MAX)
}

/// 现在能放心给游戏多少：空闲内存减去留给系统的 1 GB（256 的整数倍）
fn spare_mb(system: SystemMemory) -> u64 {
    round_down(system.available_mb.saturating_sub(HEADROOM_MB))
}

/// 自动模式给多少（MB，256 的整数倍）。`wanted` 来自 [`wanted_mb`]。
pub fn auto_memory_mb(system: Option<SystemMemory>, wanted: u64) -> u32 {
    let amount = match system {
        None => wanted.min(UNKNOWN_SYSTEM_MAX_MB),
        Some(system) => {
            let half = system.total_mb / 2;
            let limit = half.min(system.available_mb.saturating_sub(HEADROOM_MB));
            // 再紧张也给到 2 GB（否则游戏起不来）；整台电脑不到 4 GB 时给一半
            wanted.min(limit).max(AUTO_MIN_MB.min(half))
        }
    };
    to_u32(round_down(amount))
}

/// 实例里启用了多少个模组（只数文件，不解析）。
pub fn count_mods(game_dir: &Path) -> usize {
    std::fs::read_dir(game_dir.join("mods"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".jar"))
                })
                .count()
        })
        .unwrap_or(0)
}

impl MemoryChoice {
    /// 这次启动实际用多少（MB）。`recommended` 是整合包作者推荐的内存（实例信息里记着）。
    pub fn resolve(self, game_dir: &Path, recommended: Option<u32>) -> u32 {
        match self {
            MemoryChoice::Manual(mb) => mb,
            MemoryChoice::Auto => {
                let system = system_memory();
                let mods = count_mods(game_dir);
                let amount = auto_memory_mb(system, wanted_mb(mods, recommended));
                tracing::info!(amount, mods, ?recommended, ?system, "自动分配内存");
                amount
            }
        }
    }
}

// ---------- 设置里的内存条 ----------

/// 设置里内存条要的数字（MB）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    /// 电脑总内存（系统能用的部分，比内存条标的容量少一点「硬件保留」）
    pub total_mb: u32,
    /// 可用（和任务管理器的「可用」是同一个数）
    pub available_mb: u32,
    /// 自动模式现在最多会给多少（16 GB、总内存一半、空闲 − 1 GB 里最小的，至少 2 GB）
    pub auto_max_mb: u32,
}

/// 读内存条要的数字。读不到电脑内存时返回 `None`。
pub fn status() -> Option<MemoryStatus> {
    let system = system_memory()?;
    Some(MemoryStatus {
        total_mb: to_u32(system.total_mb),
        available_mb: to_u32(system.available_mb),
        auto_max_mb: auto_memory_mb(Some(system), AUTO_MAX_MB),
    })
}

// ---------- 启动前检查 ----------

/// 启动前看一眼：这次想给游戏的内存，现在空得出来吗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MemoryCheck {
    /// 自动模式（按模组数量）还是手动设的
    pub auto: bool,
    /// 实例里启用的模组数
    pub mods: u32,
    /// 想给游戏多少：自动时按模组数量或整合包作者的推荐（不超过总内存一半），手动时就是设的值
    pub wanted_mb: u32,
    /// 自动模式下「想要多少」是按整合包作者的推荐定的
    pub from_pack: bool,
    /// 现在能放心给多少（可用 − 1 GB）
    pub spare_mb: u32,
    pub total_mb: u32,
    pub available_mb: u32,
    /// 想要的空得出来
    pub enough: bool,
    /// 空不出来时「这次少给一点」给多少（至少 2 GB）。自动模式下就是照样启动会给的数
    pub fallback_mb: u32,
}

/// 纯计算部分（方便测试）
pub fn check_with(
    system: SystemMemory,
    choice: MemoryChoice,
    mods: usize,
    recommended: Option<u32>,
) -> MemoryCheck {
    let half = system.total_mb / 2;
    let spare = spare_mb(system);
    let wanted_auto = wanted_mb(mods, recommended);
    let (auto, wanted) = match choice {
        // 自动模式本来就不超过总内存一半，超出的部分不算「不够」（关程序也解决不了）
        MemoryChoice::Auto => (true, round_down(wanted_auto.min(half))),
        MemoryChoice::Manual(mb) => (false, u64::from(mb)),
    };
    let fallback = match choice {
        MemoryChoice::Auto => u64::from(auto_memory_mb(Some(system), wanted_auto)),
        MemoryChoice::Manual(_) => spare.max(AUTO_MIN_MB.min(half)).min(wanted),
    };
    MemoryCheck {
        auto,
        mods: u32::try_from(mods).unwrap_or(u32::MAX),
        wanted_mb: to_u32(wanted),
        from_pack: auto && from_pack(mods, recommended),
        spare_mb: to_u32(spare),
        total_mb: to_u32(system.total_mb),
        available_mb: to_u32(system.available_mb),
        enough: wanted <= system.available_mb.saturating_sub(HEADROOM_MB),
        fallback_mb: to_u32(fallback),
    }
}

/// 启动前检查。读不到电脑内存时返回 `None`（不拦，照常启动）。
pub fn check(
    choice: MemoryChoice,
    game_dir: &Path,
    recommended: Option<u32>,
) -> Option<MemoryCheck> {
    let system = system_memory()?;
    Some(check_with(
        system,
        choice,
        count_mods(game_dir),
        recommended,
    ))
}

// ---------- 谁占着内存 ----------

/// 一个占内存的程序（同一个程序的多个进程合在一起，比如浏览器的几十个进程）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ProgramMemory {
    /// 程序自己写的名字（例如「Google Chrome」），读不到时用文件名
    pub name: String,
    /// 关掉它大约能腾出多少
    pub memory_mb: u32,
    /// 几个进程
    pub processes: u32,
}

/// 一个进程的原始数据（纯计算部分的输入）
#[derive(Debug, Clone)]
pub struct ProcessSample {
    pub pid: u32,
    pub parent: Option<u32>,
    pub exe: Option<PathBuf>,
    /// 关掉它能腾出的内存（字节）：工作集和私有内存里小的那个，接近任务管理器的「内存」列
    pub bytes: u64,
}

/// 占内存最多的、带窗口的程序（只有带窗口的才是用户能顺手关掉的）。
/// 不算：启动器自己和它带的子进程、Windows 自己的程序（装在系统目录里的，比如资源管理器）。
/// 要遍历全部进程和窗口（几十到一两百毫秒），放在后台线程里调用。
#[cfg(windows)]
pub fn top_programs(limit: usize) -> Vec<ProgramMemory> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

    let mut system = match sysinfo::System::new() {
        Ok(system) => system,
        Err(error) => {
            tracing::warn!(%error, "读不到进程列表");
            return Vec::new();
        }
    };
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );
    let samples: Vec<ProcessSample> = system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessSample {
            pid: pid.as_u32(),
            parent: process.parent().map(|p| p.as_u32()),
            exe: process.exe().map(Path::to_path_buf),
            bytes: process.memory().min(process.virtual_memory()),
        })
        .collect();

    let windows = match xcap::Window::all() {
        Ok(windows) => windows,
        Err(error) => {
            tracing::warn!(%error, "读不到窗口列表");
            return Vec::new();
        }
    };
    let window_names: HashMap<u32, String> = windows
        .iter()
        .filter_map(|w| {
            let pid = w.pid().ok()?;
            let name = w.app_name().unwrap_or_default();
            Some((pid, name))
        })
        .collect();

    let system_root = std::env::var_os("SystemRoot").map(PathBuf::from);
    group_programs(
        &samples,
        &window_names,
        std::process::id(),
        system_root.as_deref(),
        limit,
    )
}

#[cfg(not(windows))]
pub fn top_programs(_limit: usize) -> Vec<ProgramMemory> {
    Vec::new()
}

/// 把进程按程序（同一个 exe）合并，只留带窗口的，按占用从大到小。
pub fn group_programs(
    samples: &[ProcessSample],
    window_names: &HashMap<u32, String>,
    own_pid: u32,
    system_root: Option<&Path>,
    limit: usize,
) -> Vec<ProgramMemory> {
    let parents: HashMap<u32, Option<u32>> = samples.iter().map(|s| (s.pid, s.parent)).collect();
    // 启动器自己和它的子孙进程（网页内核、正在跑的游戏）
    let is_ours = |mut pid: u32| {
        for _ in 0..16 {
            if pid == own_pid {
                return true;
            }
            match parents.get(&pid).copied().flatten() {
                Some(parent) if parent != pid => pid = parent,
                _ => return false,
            }
        }
        false
    };
    let system_root = system_root.map(|p| p.to_string_lossy().to_lowercase());

    struct Group {
        name: Option<String>,
        fallback: String,
        bytes: u64,
        processes: u32,
        has_window: bool,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    for sample in samples {
        let Some(exe) = &sample.exe else { continue };
        if is_ours(sample.pid) {
            continue;
        }
        let key = exe.to_string_lossy().to_lowercase();
        if system_root
            .as_deref()
            .is_some_and(|root| key.starts_with(&format!("{root}\\")))
        {
            continue;
        }
        let group = groups.entry(key).or_insert_with(|| Group {
            name: None,
            fallback: exe
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bytes: 0,
            processes: 0,
            has_window: false,
        });
        group.bytes = group.bytes.saturating_add(sample.bytes);
        group.processes += 1;
        if let Some(name) = window_names.get(&sample.pid) {
            group.has_window = true;
            if group.name.is_none() && !name.trim().is_empty() {
                group.name = Some(name.trim().to_owned());
            }
        }
    }

    let mut programs: Vec<ProgramMemory> = groups
        .into_values()
        .filter(|g| g.has_window && g.bytes >= 64 * MB)
        .map(|g| ProgramMemory {
            name: g.name.unwrap_or(g.fallback),
            memory_mb: to_u32(g.bytes / MB),
            processes: g.processes,
        })
        .collect();
    programs.sort_by_key(|p| std::cmp::Reverse(p.memory_mb));
    programs.truncate(limit);
    programs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(total_gb: u64, available_gb: u64) -> SystemMemory {
        SystemMemory {
            total_mb: total_gb * 1024,
            available_mb: available_gb * 1024,
        }
    }

    fn pc(total_gb: u64, available_gb: u64) -> Option<SystemMemory> {
        Some(system(total_gb, available_gb))
    }

    /// 只按模组数量（没有整合包推荐）时自动给多少
    fn auto(system: Option<SystemMemory>, mods: usize) -> u32 {
        auto_memory_mb(system, wanted_mb(mods, None))
    }

    #[test]
    fn more_mods_get_more_memory_up_to_16_gb() {
        let roomy = pc(64, 40);
        assert_eq!(auto(roomy, 0), 2560);
        assert_eq!(auto(roomy, 8), 2560, "向下取到 256 的整数倍");
        assert_eq!(auto(roomy, 73), 4096);
        assert_eq!(auto(roomy, 136), 5632);
        assert_eq!(auto(roomy, 400), 12032, "几百个模组的大包不再卡在 8 GB");
        assert_eq!(auto(roomy, usize::MAX), 16384);
    }

    #[test]
    fn never_takes_more_than_the_pc_can_spare() {
        // 32 GB 的电脑：最多一半
        assert_eq!(auto(pc(32, 30), usize::MAX), 16384);
        assert_eq!(auto(pc(16, 14), 400), 8192);
        assert_eq!(auto(pc(8, 7), 400), 4096);
        // 空闲内存紧张：留 1 GB 给别的程序
        assert_eq!(auto(pc(32, 5), 400), 4096);
        // 再紧张也给 2 GB
        assert_eq!(auto(pc(32, 1), 200), 2048);
        // 整台电脑只有 3 GB：给一半
        assert_eq!(auto(pc(3, 2), 0), 1536);
    }

    #[test]
    fn unknown_system_stays_conservative() {
        assert_eq!(auto(None, 0), 2560);
        assert_eq!(auto(None, 300), 4096);
    }

    #[test]
    fn the_pack_authors_recommendation_wins_when_higher() {
        // Integrated Boogaloo 的清单写了 6016 MB
        assert_eq!(wanted_mb(50, Some(6016)), 6016);
        assert_eq!(auto_memory_mb(pc(32, 20), wanted_mb(50, Some(6016))), 5888);
        // 后来自己加了很多模组：按模组数量的更多，就按模组数量
        assert_eq!(wanted_mb(400, Some(6016)), 12_160);
        // 乱写的大数也不超过 16 GB，小数不低于 2 GB
        assert_eq!(wanted_mb(0, Some(100_000)), 16384);
        assert_eq!(wanted_mb(0, Some(1024)), 2560);
        assert!(from_pack(50, Some(6016)));
        assert!(!from_pack(400, Some(6016)));
        assert!(!from_pack(50, None));
    }

    #[test]
    fn counts_enabled_mods_only() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(count_mods(dir.path()), 0, "没有 mods 文件夹");
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("a.jar"), b"").unwrap();
        std::fs::write(mods.join("B.JAR"), b"").unwrap();
        std::fs::write(mods.join("c.jar.disabled"), b"").unwrap();
        std::fs::write(mods.join("readme.txt"), b"").unwrap();
        assert_eq!(count_mods(dir.path()), 2);
    }

    #[test]
    fn manual_choice_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(MemoryChoice::Manual(6144).resolve(dir.path(), None), 6144);
        assert!(MemoryChoice::Auto.resolve(dir.path(), Some(6016)) >= 256);
    }

    #[test]
    fn auto_check_is_enough_when_the_modpack_fits() {
        // 32 GB 电脑空 20 GB，136 个模组想要 5.5 GB：够
        let check = check_with(system(32, 20), MemoryChoice::Auto, 136, None);
        assert!(check.auto && check.enough && !check.from_pack);
        assert_eq!(check.wanted_mb, 5632);
        assert_eq!(check.spare_mb, 19 * 1024);
        assert_eq!(check.mods, 136);
    }

    #[test]
    fn auto_check_reports_when_it_had_to_cut() {
        // 只空 4 GB：想要 5.5 GB，只能给 3 GB
        let check = check_with(system(32, 4), MemoryChoice::Auto, 136, None);
        assert!(!check.enough);
        assert_eq!(check.wanted_mb, 5632);
        assert_eq!(check.fallback_mb, 3072, "照样启动时实际给的");
    }

    #[test]
    fn auto_check_does_not_blame_a_small_pc() {
        // 8 GB 的电脑，400 个模组想要 12 GB，但自动最多给一半（4 GB）；空 6 GB 时算够
        let check = check_with(system(8, 6), MemoryChoice::Auto, 400, None);
        assert_eq!(check.wanted_mb, 4096);
        assert!(check.enough, "超出总内存一半的部分关程序也解决不了，不提示");
    }

    #[test]
    fn manual_check_offers_a_smaller_amount() {
        // 手动设 8 GB，只空 5 GB：这次可以给 4 GB
        let check = check_with(system(32, 5), MemoryChoice::Manual(8192), 0, None);
        assert!(!check.auto && !check.enough);
        assert_eq!(check.wanted_mb, 8192);
        assert_eq!(check.fallback_mb, 4096);
        // 很紧张时也至少 2 GB，但不超过设的值
        let tight = check_with(system(32, 2), MemoryChoice::Manual(3072), 0, None);
        assert_eq!(tight.fallback_mb, 2048);
        let tiny = check_with(system(32, 1), MemoryChoice::Manual(1024), 0, None);
        assert_eq!(tiny.fallback_mb, 1024, "不会比设的还多");
    }

    #[test]
    fn manual_check_is_enough_with_room_to_spare() {
        let check = check_with(system(32, 10), MemoryChoice::Manual(8192), 0, None);
        assert!(check.enough);
        // 正好留出 1 GB 也算够
        let edge = check_with(system(32, 9), MemoryChoice::Manual(8192), 0, Some(16_000));
        assert!(edge.enough);
        assert!(!edge.from_pack, "手动模式不管整合包推荐");
    }

    #[test]
    fn auto_check_follows_the_pack_recommendation() {
        let check = check_with(system(32, 5), MemoryChoice::Auto, 50, Some(6016));
        assert!(check.from_pack && !check.enough);
        assert_eq!(check.wanted_mb, 5888);
        assert_eq!(check.fallback_mb, 4096);
    }

    fn sample(pid: u32, parent: Option<u32>, exe: &str, mb: u64) -> ProcessSample {
        ProcessSample {
            pid,
            parent,
            exe: Some(PathBuf::from(exe)),
            bytes: mb * MB,
        }
    }

    #[test]
    fn programs_are_grouped_by_exe_and_named_by_their_window() {
        let samples = [
            sample(10, None, r"C:\Program Files\Google\Chrome\chrome.exe", 300),
            sample(
                11,
                Some(10),
                r"C:\Program Files\Google\Chrome\chrome.exe",
                900,
            ),
            sample(
                12,
                Some(10),
                r"C:\Program Files\Google\Chrome\chrome.exe",
                800,
            ),
            sample(20, None, r"D:\Apps\QQ\QQ.exe", 500),
            // 没有窗口的后台程序：用户关不掉，不列
            sample(30, None, r"C:\Program Files\Service\agent.exe", 2000),
            // Windows 自己的程序
            sample(40, None, r"C:\Windows\explorer.exe", 400),
            // 启动器自己和它的子进程（网页内核、游戏）
            sample(50, None, r"D:\Launcher\launcher-app.exe", 200),
            sample(
                51,
                Some(50),
                r"C:\Program Files (x86)\WebView2\msedgewebview2.exe",
                300,
            ),
            sample(
                52,
                Some(50),
                r"D:\mc\.mcl-java\java-runtime-delta\bin\java.exe",
                3000,
            ),
            // 太小的不列
            sample(60, None, r"C:\Tools\tiny.exe", 20),
        ];
        let windows = HashMap::from([
            (10, "Google Chrome".to_owned()),
            (20, String::new()),
            (40, "Windows 资源管理器".to_owned()),
            (51, "Microsoft Edge WebView2".to_owned()),
            (52, "OpenJDK Platform binary".to_owned()),
            (60, "Tiny".to_owned()),
        ]);
        let programs = group_programs(&samples, &windows, 50, Some(Path::new(r"C:\Windows")), 5);
        assert_eq!(
            programs,
            vec![
                ProgramMemory {
                    name: "Google Chrome".into(),
                    memory_mb: 2000,
                    processes: 3
                },
                ProgramMemory {
                    name: "QQ".into(),
                    memory_mb: 500,
                    processes: 1
                },
            ]
        );
    }

    #[test]
    fn program_list_is_limited() {
        let samples: Vec<_> = (1..=10)
            .map(|i| sample(i, None, &format!(r"C:\Apps\app{i}.exe"), 100 * u64::from(i)))
            .collect();
        let windows: HashMap<u32, String> = (1..=10).map(|i| (i, format!("App {i}"))).collect();
        let programs = group_programs(&samples, &windows, 999, None, 3);
        assert_eq!(
            programs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["App 10", "App 9", "App 8"]
        );
    }
}
