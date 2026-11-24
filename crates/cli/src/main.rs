//! `mcl`：启动器命令行调试工具。
//!
//! 每个核心功能（下载、安装、启动、内容管理）都先在这里加一个子命令跑通，再接到界面。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use launcher_core::config::{DownloadSource, SettingsStore};
use launcher_core::content::claim::{ClaimParams, ClaimSession, ClaimState};
use launcher_core::content::curseforge::CurseForge;
use launcher_core::content::store::{ContentStore, STORE_DIR};
use launcher_core::fs::{safe_join, write_atomic};
use launcher_core::install::{
    InstallError, InstallPhase, InstalledVersion, LoaderContext, LoaderProgress, install_files,
    load_or_fetch_version, loaders, neoforge,
};
use launcher_core::instance::{self, LoaderKind, LoaderRef};
use launcher_core::java::{self, JavaContext, JavaInstall, JavaLocator, JavaRequest};
use launcher_core::meta::{Environment, fetch_manifest, fetch_version_json, rules_allow};
use launcher_core::modpack::{
    self, ImportContext, ImportProgress, ModpackError, PackFormat, PackPlan, curseforge, mrpack,
};
use launcher_core::net::{
    DownloadError, DownloadItem, DownloadProgress, Downloader, build_client, candidate_urls,
};
use launcher_core::paths::GameDir;
use launcher_core::redact::redact;
use tokio_util::sync::CancellationToken;

mod zh_names;

#[derive(Parser)]
#[command(name = "mcl", version, about = "启动器命令行调试工具")]
struct Cli {
    /// 调试数据目录（默认是当前目录下的 .mcl）
    #[arg(long, global = true, default_value = ".mcl")]
    data_dir: PathBuf,

    /// 下载源
    #[arg(long, global = true, value_enum, default_value_t = SourceArg::Auto)]
    source: SourceArg,

    /// 同时下载的文件数
    #[arg(long, global = true, default_value_t = 32)]
    concurrency: usize,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceArg {
    /// 官方优先，失败换镜像
    Auto,
    Official,
    /// 国内镜像（BMCLAPI）优先
    Mirror,
    /// 只用官方，不用镜像
    OfficialOnly,
}

impl From<SourceArg> for DownloadSource {
    fn from(value: SourceArg) -> Self {
        match value {
            SourceArg::Auto => DownloadSource::Auto,
            SourceArg::Official => DownloadSource::Official,
            SourceArg::OfficialOnly => DownloadSource::OfficialOnly,
            SourceArg::Mirror => DownloadSource::Mirror,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// 显示版本和数据目录
    Info,
    /// 显示当前设置（不存在时显示默认值）
    Settings,
    /// 检查一个外部路径能否安全地拼到目录下
    SafePath {
        /// 目标目录
        base: PathBuf,
        /// 外部给的相对路径，例如整合包里的文件名
        path: String,
    },
    /// 对一段文本做日志脱敏
    Redact {
        /// 要脱敏的文本
        text: String,
    },
    /// 列出 Minecraft 版本
    Versions {
        /// 也列出快照版
        #[arg(long)]
        snapshots: bool,
        /// 最多列几个
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// 下载单个文件（带校验、断点续传）
    Download {
        url: String,
        /// 保存到哪里
        out: PathBuf,
        /// 期望的 SHA-1
        #[arg(long)]
        sha1: Option<String>,
        /// 期望的大小（字节）
        #[arg(long)]
        size: Option<u64>,
    },
    /// 拉取一个版本：下载描述文件和客户端 jar 到游戏目录，并打印摘要
    Fetch {
        /// 版本号，例如 1.21.1
        version: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
    },
    /// 列出这台电脑上找到的 Java
    Java,
    /// 找一个能用的 Java，没有就下载 Mojang 官方运行时到游戏目录的 .mcl-java 里
    JavaInstall {
        /// 主版本号，例如 21
        major: u32,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
        /// 运行时组件名，例如 java-runtime-delta；不填按主版本号找
        #[arg(long)]
        component: Option<String>,
        /// 不用电脑上已有的 Java（模拟一台没装 Java 的电脑）
        #[arg(long)]
        no_system: bool,
    },
    /// 安装一个版本（完整校验每个文件），可以同时装 NeoForge
    Install {
        /// 游戏版本号，例如 1.21.1
        version: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
        /// 同时安装 NeoForge：写版本号（例如 21.1.265），或只写 --neoforge 装最新正式版
        #[arg(long, num_args = 0..=1, default_missing_value = "latest")]
        neoforge: Option<String>,
        /// 运行安装处理器用的 Java；不填自动选择
        #[arg(long)]
        java: Option<PathBuf>,
    },
    /// 列出某个游戏版本可用的 NeoForge
    Neoforge {
        /// 游戏版本号，例如 1.21.1；不填列出全部
        #[arg(long)]
        mc: Option<String>,
        /// 最多列几个
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// 游戏启动暂由 Lumi 桌面端完成
    Launch {
        /// 实例 ID 或版本号
        version: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
    },
    /// 把实例导出成 Modrinth 整合包（.mrpack）。不写 --entry 时带上默认勾选的全部「别的文件」
    Export {
        /// 实例 ID（versions 下的文件夹名）
        id: String,
        /// 导出到哪个文件
        out: PathBuf,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
        /// 整合包自己的版本号
        #[arg(long, default_value = "1.0.0")]
        version: String,
        /// 只带这几项别的文件（第一层的名字，可以写多次）；写 --no-entries 一项都不带
        #[arg(long)]
        entry: Vec<String>,
        #[arg(long)]
        no_entries: bool,
        /// 带上的世界（存档文件夹名，可以写多次）
        #[arg(long)]
        world: Vec<String>,
        /// 只列出会导出什么，不写文件
        #[arg(long)]
        dry_run: bool,
    },
    /// 导入整合包（Modrinth .mrpack 或 CurseForge .zip），建成一个新实例。
    /// CurseForge 包要设环境变量 MCL_CURSEFORGE_KEY
    Import {
        /// 整合包文件
        pack: PathBuf,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
        /// 实例名；不填用整合包的名字
        #[arg(long)]
        name: Option<String>,
        /// 运行安装处理器用的 Java；不填自动选择
        #[arg(long)]
        java: Option<PathBuf>,
        /// 只检查、显示信息，不导入
        #[arg(long)]
        dry_run: bool,
        /// 补件时盯着的下载文件夹（默认是系统的「下载」文件夹）
        #[arg(long)]
        downloads: Option<PathBuf>,
    },
    /// 列出游戏目录里的实例
    Instances {
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
    },
    /// 列出一个实例里的单人世界（最近玩过的在前）
    Worlds {
        /// 实例名
        instance: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".mcl/game")]
        game_dir: PathBuf,
        /// 玩家官方 UUID（可选，仅用于匹配本地存档里的游玩时长统计）
        #[arg(long)]
        player_uuid: Option<String>,
    },
    /// 看一个别家的 .minecraft 里有哪些版本、各用哪个游戏目录（挂上）；不给路径时在电脑上找
    LinkScan { root: Option<PathBuf> },
    /// 列出别家启动器（Prism、CurseForge、ATLauncher、Modrinth App）的实例；给路径时只看那个文件夹
    ForeignList { folder: Option<PathBuf> },
    /// 认拖进窗口的文件是什么（整合包、模组、资源包、光影包、数据包、地图），和拖进启动器时一样
    DropInspect {
        /// 文件或文件夹，可以好几个
        paths: Vec<PathBuf>,
    },
    /// 截一个进程的窗口内容区（自动封面的截图方式），存成 PNG，顺便算细节分
    CaptureWindow {
        /// 进程号
        pid: u32,
        /// 存到哪
        #[arg(long, default_value = "capture.png")]
        out: PathBuf,
        /// 只在它是最前面的窗口时截（自动封面的规则）
        #[arg(long)]
        focused: bool,
    },
    /// 看内存：总共、可用（和任务管理器对照）、自动模式最多给多少、占内存最多的带窗口程序；
    /// 给了实例就顺便做一次启动前检查
    Memory {
        /// 实例名（做启动前检查用）
        instance: Option<String>,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".minecraft")]
        game_dir: PathBuf,
        /// 手动指定给游戏多少（MB）；不填按自动模式检查
        #[arg(long)]
        memory: Option<u32>,
    },
    /// 复制一个实例（不带世界），和界面里「⋯ → 复制」一样
    CopyInstance {
        /// 原实例名
        from: String,
        /// 新实例名
        to: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".minecraft")]
        game_dir: PathBuf,
    },
    /// 检查并更新实例里的模组（和界面里「检查更新 → 全部更新」一样）。
    /// 开发版设 `MCL_DEBUG_CRASH_AT_STEP=3` 可以在替换到第 3 步时直接退出，模拟启动器被关掉
    ContentUpdate {
        /// 实例名
        instance: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".minecraft")]
        game_dir: PathBuf,
        /// 只列出有哪些更新，不更新
        #[arg(long)]
        dry_run: bool,
    },
    /// 把实例上次没做完的改动退回改之前（启动器启动时也会自动做）
    ContentRecover {
        /// 实例名
        instance: String,
        /// 游戏目录（.minecraft）
        #[arg(long, default_value = ".minecraft")]
        game_dir: PathBuf,
    },
    /// 问一下我的世界服务器的状态（在线人数、延迟；没写端口时先查 SRV）
    ServerStatus {
        /// 服务器地址，例如 mc.example.com 或 127.0.0.1:25565
        address: String,
    },
    /// 看一个文件夹里的模组各认出什么中文名（读模组自带的语言文件，不联网）
    ZhScan {
        /// 放模组的文件夹（会往下找子文件夹）
        dir: PathBuf,
    },
    /// 从两个平台下载量最高的模组生成中文名表（要 CurseForge 的 Key）
    ZhNamesBuild {
        /// 每个平台取前多少个
        #[arg(long, default_value_t = 1000)]
        top: u32,
        /// 名表写到哪（已有的「我们翻的」会保留）
        #[arg(long, default_value = "crates/core/data/zh-names.json")]
        out: PathBuf,
        /// 核对用的表格（制表符分隔）
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn,xcap=off")
        .with_target(false)
        .init();
    let cli = Cli::parse();
    let source = DownloadSource::from(cli.source);

    match cli.command {
        Command::Info => {
            println!("mcl {}", env!("CARGO_PKG_VERSION"));
            println!("数据目录：{}", cli.data_dir.display());
        }
        Command::Settings => {
            let store = SettingsStore::open(cli.data_dir.join("settings.json"));
            println!("{}", serde_json::to_string_pretty(&store.get())?);
        }
        Command::SafePath { base, path } => match safe_join(&base, &path) {
            Ok(joined) => println!("安全：{}", joined.display()),
            Err(reason) => {
                println!("拒绝：{reason}");
                std::process::exit(2);
            }
        },
        Command::Redact { text } => println!("{}", redact(&text)),
        Command::Versions { snapshots, limit } => list_versions(source, snapshots, limit).await?,
        Command::Download {
            url,
            out,
            sha1,
            size,
        } => {
            let item = DownloadItem {
                urls: candidate_urls(&url, source),
                path: out,
                sha1,
                size,
            };
            run_downloads(&[item], cli.concurrency).await?;
            println!("完成");
        }
        Command::Fetch { version, game_dir } => {
            fetch_version(&version, &game_dir, source, cli.concurrency).await?;
        }
        Command::Java => list_java(),
        Command::JavaInstall {
            major,
            game_dir,
            component,
            no_system,
        } => {
            let game = game_dir_from(&game_dir)?;
            let request = JavaRequest { major, component };
            install_java(&game, &request, !no_system, source, cli.concurrency).await?;
        }
        Command::Neoforge { mc, limit } => list_neoforge(mc.as_deref(), limit, source).await?,
        Command::Install {
            version,
            game_dir,
            neoforge: loader,
            java,
        } => {
            let game = game_dir_from(&game_dir)?;
            let started = Instant::now();
            let id = match loader {
                Some(requested) => {
                    let loader_version = resolve_neoforge(&version, &requested, source).await?;
                    install_neoforge(
                        &game,
                        &loader_version,
                        source,
                        cli.concurrency,
                        java.as_deref(),
                    )
                    .await?
                }
                None => version,
            };
            let installed = install_version(&game, &id, source, cli.concurrency, false).await?;
            println!(
                "{} 安装完成：依赖库 + 客户端共 {} 个进类路径的文件，用时 {:.1} 秒",
                installed.version.id,
                installed.classpath.len(),
                started.elapsed().as_secs_f64()
            );
        }
        Command::Launch { .. } => {
            bail!("命令行暂未接通 Microsoft 账号验证。请在 Lumi 桌面端登录后启动游戏");
        }
        Command::Export {
            id,
            out,
            game_dir,
            version,
            entry,
            no_entries,
            world,
            dry_run,
        } => {
            use launcher_core::modpack::export;
            let game = game_dir_from(&game_dir)?;
            let instance = instance::load(&game, &id)?.context("这不是本启动器的实例")?;
            let started = Instant::now();
            let plan = export::plan(&build_client()?, &game, &instance).await?;
            let preview = plan.preview();
            let mb = |bytes: f64| bytes / 1024.0 / 1024.0;
            println!(
                "{}（{}{}）：{} 个从 Modrinth 下载，{} 个直接打包（{:.1} MB），其中停用的 {} 个；查好用了 {:.1} 秒",
                preview.name,
                preview.game_version,
                preview
                    .loader
                    .as_deref()
                    .map(|l| format!(" · {l}"))
                    .unwrap_or_default(),
                preview.linked,
                preview.bundled.len(),
                mb(preview.bundled_bytes),
                preview.disabled,
                started.elapsed().as_secs_f64()
            );
            for name in &preview.bundled {
                println!("  直接打包：{name}");
            }
            for e in &preview.entries {
                println!(
                    "  别的文件：{}{}（{:.1} MB）{}",
                    e.name,
                    if e.folder { "/" } else { "" },
                    mb(e.size),
                    if e.default_on { "" } else { " 默认不带" }
                );
            }
            for w in &preview.worlds {
                println!("  世界：{}（{}，{:.1} MB）", w.name, w.folder, mb(w.size));
            }
            if dry_run {
                return Ok(());
            }
            let entries = if no_entries {
                Vec::new()
            } else if entry.is_empty() {
                preview
                    .entries
                    .iter()
                    .filter(|e| e.default_on)
                    .map(|e| e.name.clone())
                    .collect()
            } else {
                entry
            };
            let options = export::ExportOptions {
                name: preview.name.clone(),
                version,
                summary: None,
                entries,
                worlds: world,
            };
            let started = Instant::now();
            let summary = tokio::task::spawn_blocking(move || {
                plan.write(
                    &options,
                    &out,
                    &tokio_util::sync::CancellationToken::new(),
                    |_| {},
                )
            })
            .await??;
            println!(
                "已导出 {}（{:.1} MB，{:.1} 秒）",
                summary.path.display(),
                mb(summary.bytes as f64),
                started.elapsed().as_secs_f64()
            );
        }
        Command::Import {
            pack,
            game_dir,
            name,
            java,
            dry_run,
            downloads,
        } => {
            let game = game_dir_from(&game_dir)?;
            let options = ImportOptions {
                name,
                java,
                dry_run,
                downloads,
            };
            import_pack(&game, &pack, &options, source, cli.concurrency).await?;
        }
        Command::LinkScan { root } => {
            use launcher_core::external::root;
            let roots = match root {
                Some(path) => vec![path],
                None => root::discover(&[]),
            };
            for path in roots {
                let scanned = root::scan(&path)?;
                println!(
                    "{}（{}，分组「{}」）",
                    path.display(),
                    scanned.launcher.label(),
                    root::group_name(&path, scanned.launcher)
                );
                for v in &scanned.versions {
                    let isolated = v.game_dir != path;
                    println!(
                        "  {} · {}{} · {}",
                        v.folder,
                        v.game_version,
                        v.loader
                            .as_ref()
                            .map(|l| format!(" · {} {}", l.kind.label(), l.version))
                            .unwrap_or_default(),
                        if isolated { "隔离" } else { "不隔离" }
                    );
                }
                for (folder, reason) in &scanned.skipped {
                    println!("  （跳过 {folder}：{reason}）");
                }
            }
        }
        Command::ForeignList { folder } => {
            use launcher_core::external::foreign;
            let found = match folder {
                Some(path) => foreign::inspect_any(&path),
                None => foreign::known_instances(),
            };
            println!("{} 个实例", found.len());
            for i in found {
                println!(
                    "  {} · {} · {}{}",
                    i.launcher.label(),
                    i.name,
                    i.game_version,
                    i.loader
                        .as_ref()
                        .map(|l| format!(" · {} {}", l.kind.label(), l.version))
                        .unwrap_or_default()
                );
            }
        }
        Command::DropInspect { paths } => {
            use launcher_core::content::{bundle, dropped};
            let line = |found: dropped::Dropped, indent: &str| {
                println!(
                    "{indent}{}：{:?}{}{}{}{}",
                    found.name,
                    found.kind,
                    found
                        .problem
                        .map(|p| format!("（{p:?}）"))
                        .unwrap_or_default(),
                    found.title.map(|t| format!(" · {t}")).unwrap_or_default(),
                    found.version.map(|v| format!(" · {v}")).unwrap_or_default(),
                    if found.loaders.is_empty() {
                        String::new()
                    } else {
                        format!(" · 加载器 {}", found.loaders.join("/"))
                    },
                );
            };
            for path in paths {
                let found = dropped::inspect(&path);
                let is_bundle = found.kind == dropped::DropKind::Bundle;
                line(found, "");
                if !is_bundle {
                    continue;
                }
                // 一包东西：拆到临时文件夹，里面每一样再认一遍
                let dir = tempfile::tempdir()?;
                let started = Instant::now();
                match bundle::extract(&path, dir.path()) {
                    Ok(unpacked) => {
                        println!(
                            "  拆出 {} 样、另外 {} 个文件没拆，用时 {:.1} 秒",
                            unpacked.items.len(),
                            unpacked.skipped,
                            started.elapsed().as_secs_f64()
                        );
                        for item in &unpacked.items {
                            line(dropped::inspect(item), "  ");
                        }
                    }
                    Err(error) => println!("  拆不开：{error}"),
                }
            }
        }
        Command::Memory {
            instance: id,
            game_dir,
            memory,
        } => {
            use launcher_core::launch::memory;
            let gb = |mb: u32| f64::from(mb) / 1024.0;
            let Some(status) = memory::status() else {
                bail!("读不到这台电脑的内存情况");
            };
            println!(
                "总共 {:.1} GB，可用 {:.1} GB（任务管理器「性能 → 内存」里的「可用」），自动模式最多给 {:.1} GB",
                gb(status.total_mb),
                gb(status.available_mb),
                gb(status.auto_max_mb)
            );
            if let Some(id) = id {
                let game = game_dir_from(&game_dir)?;
                let found = instance::load(&game, &id)?;
                let dir = match &found {
                    Some(instance) => instance.game_dir(&game)?,
                    None => game.version_dir(&id)?,
                };
                let recommended = found.and_then(|i| i.config.recommended_memory_mb);
                let choice = memory.map_or(
                    launcher_core::launch::MemoryChoice::Auto,
                    launcher_core::launch::MemoryChoice::Manual,
                );
                if let Some(check) = memory::check(choice, &dir, recommended) {
                    println!(
                        "启动前检查（{}，{} 个模组{}）：想要 {:.1} GB，空得出 {:.1} GB → {}；少给的话给 {:.1} GB",
                        if check.auto { "自动" } else { "手动" },
                        check.mods,
                        if check.from_pack {
                            "，按整合包作者的推荐"
                        } else {
                            ""
                        },
                        gb(check.wanted_mb),
                        gb(check.spare_mb),
                        if check.enough { "够" } else { "不够" },
                        gb(check.fallback_mb)
                    );
                }
            }
            let started = Instant::now();
            let programs = memory::top_programs(8);
            println!(
                "占内存最多的带窗口程序（用时 {} 毫秒）：",
                started.elapsed().as_millis()
            );
            for program in programs {
                println!(
                    "  {:<40} 约 {:.1} GB（{} 个进程）",
                    program.name,
                    gb(program.memory_mb),
                    program.processes
                );
            }
        }
        Command::CaptureWindow { pid, out, focused } => {
            let started = Instant::now();
            let shot = if focused {
                launcher_core::capture::focused_window(pid)?
            } else {
                launcher_core::capture::any_window(pid)?
            };
            let Some(image) = shot else {
                println!("没截：找不到这个进程的窗口，或者它不在最前面、最小化了");
                return Ok(());
            };
            let took = started.elapsed();
            image.save(&out).context("保存截图失败")?;
            println!(
                "截好了：{} × {}，用时 {:.0} 毫秒，细节分 {:?}，存到 {}",
                image.width(),
                image.height(),
                took.as_secs_f64() * 1000.0,
                launcher_core::saves::cover::detail_score(&image),
                out.display()
            );
        }
        Command::Worlds {
            instance: id,
            game_dir,
            player_uuid,
        } => {
            let game = game_dir_from(&game_dir)?;
            let instance = instance::load(&game, &id)?.context("找不到这个实例")?;
            let dir = instance.game_dir(&game)?;
            let uuid = player_uuid.unwrap_or_default();
            let worlds = launcher_core::saves::list(&dir, &uuid);
            println!(
                "{} 个世界（Quick Play：{}）：",
                worlds.len(),
                if launcher_core::saves::supports_quick_play(&instance.config.game_version) {
                    "支持"
                } else {
                    "不支持"
                }
            );
            for w in &worlds {
                println!(
                    "  {} （文件夹 {}）第 {} 天 · {:?}{} · 版本 {} · 玩了 {} · 背景 {}",
                    w.name,
                    w.folder,
                    w.day,
                    w.game_mode,
                    if w.hardcore { " · 极限" } else { "" },
                    w.version.as_deref().unwrap_or("?"),
                    w.play_seconds
                        .map_or_else(|| "?".to_owned(), |s| format!("{:.1} 分钟", s / 60.0)),
                    w.image.as_ref().map_or_else(
                        || "无".to_owned(),
                        |i| format!("{:?} {}", i.kind, i.path.display())
                    )
                );
            }
        }
        Command::CopyInstance { from, to, game_dir } => {
            let game = game_dir_from(&game_dir)?;
            let options = instance::CopyOptions {
                name: to.clone(),
                id: to,
                include_worlds: false,
            };
            let copied = instance::copy(&game, &from, &options, &CancellationToken::new(), |_| {})?;
            println!("复制好了：{}", copied.id);
        }
        Command::ContentUpdate {
            instance: id,
            game_dir,
            dry_run,
        } => {
            use launcher_core::content::{ContentKind, InstanceContent};
            let game = game_dir_from(&game_dir)?;
            let found = instance::load(&game, &id)?.context("找不到这个实例")?;
            let content = InstanceContent::for_game(&game, found.game_dir(&game)?);
            let client = build_client()?;
            let curseforge =
                launcher_core::content::curseforge::CurseForge::from_env(client.clone());
            let loader = found.config.loader.as_ref().map(|l| l.kind.slug());
            let result = content
                .check_updates(
                    &client,
                    &curseforge,
                    ContentKind::Mod,
                    &found.config.game_version,
                    loader,
                )
                .await?;
            println!(
                "已检查 {} 项，暂时无法检查 {} 项",
                result.checked, result.unchecked
            );
            if result.lookup_failed {
                bail!("检查更新失败");
            }
            let updates = result.updates;
            println!("{} 个模组有更新：", updates.len());
            for u in &updates {
                println!(
                    "  {}  {} → {}",
                    u.title,
                    u.current_version.as_deref().unwrap_or("?"),
                    u.version_number
                );
            }
            if dry_run || updates.is_empty() {
                return Ok(());
            }
            let downloader = Downloader::new(client.clone(), cli.concurrency);
            let started = Instant::now();
            content
                .update(
                    &curseforge,
                    &downloader,
                    &updates,
                    None,
                    &CancellationToken::new(),
                    |_| {},
                )
                .await?;
            println!("更新好了，用时 {:.1} 秒", started.elapsed().as_secs_f64());
        }
        Command::ContentRecover {
            instance: id,
            game_dir,
        } => {
            use launcher_core::content::InstanceContent;
            let game = game_dir_from(&game_dir)?;
            let found = instance::load(&game, &id)?.context("找不到这个实例")?;
            let content = InstanceContent::for_game(&game, found.game_dir(&game)?);
            match content.recover().await? {
                Some(recovered) => println!("退回了上次没做完的「{}」", recovered.summary),
                None => println!("没有没做完的改动"),
            }
        }
        Command::ZhScan { dir } => zh_names::scan(&dir)?,
        Command::ZhNamesBuild { top, out, report } => {
            zh_names::build(top, &out, report.as_deref()).await?
        }
        Command::ServerStatus { address } => {
            use launcher_core::net::status::{self, ServerStatus};
            let started = std::time::Instant::now();
            if !address.contains(':') {
                match launcher_core::net::srv::lookup(&address).await {
                    Some(target) => println!("SRV：{}:{}", target.host, target.port),
                    None => println!("SRV：没有"),
                }
            }
            match status::ping(&address).await {
                ServerStatus::Online {
                    latency_ms,
                    players_online,
                    players_max,
                    version,
                    motd,
                    favicon,
                } => {
                    println!("在线：{players_online}/{players_max} 人，延迟 {latency_ms} 毫秒");
                    println!("版本：{}", version.unwrap_or_default());
                    println!("简介：{}", motd.replace('\n', " / "));
                    println!(
                        "图标：{}",
                        favicon.map_or("没有".to_owned(), |f| format!("{} 字", f.len()))
                    );
                }
                ServerStatus::Offline { reason } => println!("连不上：{reason:?}"),
            }
            println!("用时 {:.2} 秒", started.elapsed().as_secs_f64());
        }
        Command::Instances { game_dir } => {
            let game = game_dir_from(&game_dir)?;
            let instances = instance::list(&game);
            println!("{} 个实例：", instances.len());
            for i in &instances {
                let loader = i
                    .config
                    .loader
                    .as_ref()
                    .map(|l| format!("{} {}", l.kind.label(), l.version))
                    .unwrap_or_else(|| "原版".to_owned());
                let from = i
                    .config
                    .source
                    .as_ref()
                    .map(|s| format!("  来自整合包 {} {}", s.name, s.version))
                    .unwrap_or_default();
                println!(
                    "  {:<24} Minecraft {} · {loader}{from}",
                    i.id, i.config.game_version
                );
            }
        }
    }
    Ok(())
}

struct ImportOptions {
    name: Option<String>,
    java: Option<PathBuf>,
    dry_run: bool,
    downloads: Option<PathBuf>,
}

/// 读取并检查整合包；CurseForge 包还要联网查清每个文件
async fn inspect_pack(pack: &Path, curseforge: &CurseForge) -> Result<PackPlan> {
    let path = pack.to_path_buf();
    let format = tokio::task::spawn_blocking(move || modpack::detect(&path))
        .await
        .context("检查任务异常结束")??;
    let path = pack.to_path_buf();
    Ok(match format {
        PackFormat::Modrinth => tokio::task::spawn_blocking(move || mrpack::inspect(&path))
            .await
            .context("检查任务异常结束")??,
        PackFormat::CurseForge => {
            let cf_pack = tokio::task::spawn_blocking(move || curseforge::inspect(&path))
                .await
                .context("检查任务异常结束")??;
            curseforge::resolve(&curseforge.http(), curseforge, cf_pack).await?
        }
    })
}

/// 系统的「下载」文件夹（补件默认盯着这里）
fn default_downloads() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Downloads"))
}

async fn import_pack(
    game: &GameDir,
    pack: &Path,
    options: &ImportOptions,
    source: DownloadSource,
    concurrency: usize,
) -> Result<()> {
    let started = Instant::now();
    let client = build_client()?;
    let curseforge = CurseForge::from_env(client.clone());
    let plan = inspect_pack(pack, &curseforge).await?;
    let p = &plan.preview;
    println!("整合包：{} {}", p.name, p.version);
    if let Some(summary) = &p.summary {
        println!("  简介：{summary}");
    }
    println!(
        "  Minecraft {} · {}",
        p.game_version,
        p.loader
            .as_ref()
            .map(|l| format!("{} {}", l.kind.label(), l.version))
            .unwrap_or_else(|| "原版".to_owned())
    );
    println!(
        "  要下载 {} 个文件（{:.1} MB），覆盖文件 {} 个；安全检查通过（{} 毫秒）",
        p.file_count,
        p.download_bytes / 1024.0 / 1024.0,
        p.override_count,
        started.elapsed().as_millis()
    );
    if !p.optional_files.is_empty() {
        println!("  可选文件：{}", p.optional_files.join("、"));
    }
    if p.from_modrinth > 0 {
        println!(
            "  有 {} 个文件作者不允许第三方启动器下载，但 Modrinth 上有一模一样的，改从 Modrinth 下载",
            p.from_modrinth
        );
    }
    if !p.manual_files.is_empty() {
        println!(
            "  有 {} 个文件作者不允许第三方启动器下载，要在浏览器里下载：",
            p.manual_files.len()
        );
        for file in &p.manual_files {
            println!(
                "    {}{} · {}（{:.0} KB）\n      {}",
                if file.required { "" } else { "[可选] " },
                file.project_name,
                file.display_name,
                file.size / 1024.0,
                file.page_url.as_deref().unwrap_or("（没有下载页）")
            );
        }
    }
    if options.dry_run {
        return Ok(());
    }

    let vanilla = load_or_fetch_version(game, &p.game_version, &client, source)
        .await
        .with_context(|| format!("读取 {} 的版本描述失败", p.game_version))?;
    let java = pick_java(options.java.as_deref(), vanilla.required_java())?;
    let downloader =
        Downloader::new(client.clone(), concurrency).with_host_header(curseforge.download_header());
    let cancel = ctrl_c_token();
    let temp_root = game.root().join(".mcl-tmp");
    let session = (!p.manual_files.is_empty()).then(|| {
        let watch: Vec<PathBuf> = options
            .downloads
            .clone()
            .or_else(default_downloads)
            .into_iter()
            .collect();
        for dir in &watch {
            println!("  正在盯着 {}，下载好的文件会自动认领", dir.display());
        }
        ClaimSession::new(
            ClaimParams {
                job_id: "cli".to_owned(),
                title: p.name.clone(),
                files: p.manual_files.clone(),
                watch,
                store: ContentStore::new(game.root().join(STORE_DIR)),
                curseforge: curseforge.clone(),
                move_claimed: false,
            },
            |snapshot| {
                let claimed = snapshot
                    .entries
                    .iter()
                    .filter(|e| e.state == ClaimState::Claimed)
                    .count();
                eprintln!("\r  补件：{claimed}/{} 已认领", snapshot.entries.len());
                if let Some(notice) = &snapshot.notice {
                    eprintln!("  提示：{notice}");
                }
            },
        )
    });
    if let Some(session) = &session {
        let session = session.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { session.watch(&cancel).await });
    }
    let ctx = ImportContext {
        loader: LoaderContext {
            game,
            client: &client,
            downloader: &downloader,
            source,
            meta_source: source,
            java: &java.executable,
            work_dir: &temp_root,
            cancel: &cancel,
        },
        temp_root: &temp_root,
        manual: session.as_deref(),
    };

    let last_print = Mutex::new(Instant::now());
    let resume_key = modpack::file_resume_key(plan.pack_path());
    let result = modpack::import(
        &ctx,
        &plan,
        options.name.as_deref(),
        &resume_key,
        |progress| {
            let (label, done_files, total_files, done_bytes, total_bytes) = match &progress {
                ImportProgress::Loader(LoaderProgress::Processor { index, total, task }) => {
                    eprintln!(
                        "\r  [{:>5.1} 秒] 加载器处理器 {index}/{total}：{task}            ",
                        started.elapsed().as_secs_f64()
                    );
                    return;
                }
                ImportProgress::Loader(
                    LoaderProgress::GameJar(d)
                    | LoaderProgress::Installer(d)
                    | LoaderProgress::Libraries(d),
                ) => (
                    "安装加载器",
                    d.done_files,
                    d.total_files,
                    d.done_bytes,
                    d.total_bytes,
                ),
                ImportProgress::Files(d) => (
                    "整合包文件",
                    d.done_files,
                    d.total_files,
                    d.done_bytes,
                    d.total_bytes,
                ),
                ImportProgress::Overrides => {
                    eprintln!(
                        "\r  [{:>5.1} 秒] 解压覆盖文件                              ",
                        started.elapsed().as_secs_f64()
                    );
                    return;
                }
                ImportProgress::WaitingManual(n) => {
                    eprintln!(
                        "\r  [{:>5.1} 秒] 其他文件都下好了，等你补齐 {n} 个文件",
                        started.elapsed().as_secs_f64()
                    );
                    return;
                }
                ImportProgress::Game(_, d) => (
                    "游戏本体文件",
                    d.done_files,
                    d.total_files,
                    d.done_bytes,
                    d.total_bytes,
                ),
            };
            let mut last = last_print.lock().unwrap_or_else(|e| e.into_inner());
            if last.elapsed().as_millis() < 300 && done_files < total_files {
                return;
            }
            *last = Instant::now();
            eprint!(
                "\r  {label}：{done_files}/{total_files} 个文件  {:.1}/{:.1} MB      ",
                mb(done_bytes),
                mb(total_bytes)
            );
        },
    )
    .await;
    eprintln!();
    if let Some(session) = &session {
        session.close();
    }
    match result {
        Ok(instance) => {
            println!(
                "导入完成，用时 {:.1} 秒。实例名：{}（目录 {}）",
                started.elapsed().as_secs_f64(),
                instance.id,
                instance.game_dir(game)?.display()
            );
            println!("请在 Lumi 桌面端登录 Microsoft 账号后启动实例");
            Ok(())
        }
        Err(ModpackError::Install(InstallError::Processor { task, code, output })) => {
            eprintln!("处理器 {task} 失败（退出码 {code:?}），最后的输出：\n{output}");
            bail!("导入失败，未登记新实例。重新导入时会复用已通过校验的文件")
        }
        Err(ModpackError::Download(DownloadError::Failed(failures))) => {
            for failure in &failures {
                eprintln!("  失败：{} —— {}", failure.path.display(), failure.reason);
            }
            bail!(
                "有 {} 个文件下载失败，未登记新实例。重新导入时会复用已通过校验的文件",
                failures.len()
            )
        }
        Err(error) => {
            Err(error).context("导入失败，未登记新实例。重新导入时会复用已通过校验的文件")
        }
    }
}

/// 游戏目录转成绝对路径：游戏进程的工作目录会切过去，类路径里的相对路径就失效了
fn game_dir_from(path: &Path) -> Result<GameDir> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("无法解析游戏目录 {}", path.display()))?;
    let game = GameDir::new(absolute);
    // 和启动器一样：早先放在 versions/<实例名>/ 的实例先搬到 instances/
    let report = instance::migrate_legacy(&game);
    if report.moved > 0 || report.failed > 0 {
        eprintln!(
            "老实例搬到 instances/：搬了 {} 个，没搬成 {} 个（被占用，下次再搬）",
            report.moved, report.failed
        );
    }
    Ok(game)
}

async fn install_java(
    game: &GameDir,
    request: &JavaRequest,
    use_system: bool,
    source: DownloadSource,
    concurrency: usize,
) -> Result<JavaInstall> {
    let client = build_client()?;
    let downloader = Downloader::new(client.clone(), concurrency);
    let locator = JavaLocator {
        use_system,
        ..JavaLocator::for_game(game.root())
    };
    let cancel = CancellationToken::new();
    let ctx = JavaContext {
        locator: &locator,
        client: &client,
        downloader: &downloader,
        source,
        cancel: &cancel,
    };
    let started = Instant::now();
    let last_print = Mutex::new(Instant::now());
    let found = java::find_or_install(&ctx, request, |p| {
        let mut last = last_print.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed().as_millis() >= 1000 || p.done_files == p.total_files {
            *last = Instant::now();
            println!(
                "  下载 Java {}：{}/{} 个文件，{:.1}/{:.1} MB",
                request.major,
                p.done_files,
                p.total_files,
                p.done_bytes as f64 / 1e6,
                p.total_bytes as f64 / 1e6
            );
        }
    })
    .await
    .context("准备 Java 失败")?;
    println!(
        "Java {}（{}，来源：{}），用时 {:.1} 秒",
        found.version,
        found.executable.display(),
        found.origin.label(),
        started.elapsed().as_secs_f64()
    );
    Ok(found)
}

fn list_java() {
    let started = Instant::now();
    let installs = java::scan(&[], &[]);
    println!(
        "找到 {} 个 Java（用时 {} 毫秒）：",
        installs.len(),
        started.elapsed().as_millis()
    );
    for install in &installs {
        println!(
            "  Java {:<3} {:<14} {}  {}  [{}]",
            install.major,
            install.version,
            if install.is_64bit { "64 位" } else { "32 位" },
            install.home.display(),
            install.origin.label()
        );
    }
}

async fn list_neoforge(
    game_version: Option<&str>,
    limit: usize,
    source: DownloadSource,
) -> Result<()> {
    let started = Instant::now();
    let versions = neoforge::fetch_versions(&build_client()?, source)
        .await
        .context("获取 NeoForge 版本列表失败")?;
    let matching: Vec<_> = versions
        .iter()
        .filter(|v| game_version.is_none_or(|g| v.game_version == g))
        .collect();
    println!(
        "共 {} 个 NeoForge 版本，符合条件 {} 个（用时 {:.1} 秒）",
        versions.len(),
        matching.len(),
        started.elapsed().as_secs_f64()
    );
    for v in matching.iter().take(limit) {
        println!(
            "  {:<16} Minecraft {:<8} {}",
            v.version,
            v.game_version,
            if v.stable { "正式版" } else { "测试版" }
        );
    }
    Ok(())
}

/// `latest` → 这个游戏版本最新的 NeoForge；写了具体版本号就检查它和游戏版本对得上
async fn resolve_neoforge(
    game_version: &str,
    requested: &str,
    source: DownloadSource,
) -> Result<String> {
    if requested != "latest" {
        let expected = neoforge::game_version_of(requested)
            .with_context(|| format!("认不出 NeoForge 版本号 {requested}"))?;
        if expected != game_version {
            bail!("NeoForge {requested} 对应的是 Minecraft {expected}，不是 {game_version}");
        }
        return Ok(requested.to_owned());
    }
    let versions = neoforge::fetch_versions(&build_client()?, source)
        .await
        .context("获取 NeoForge 版本列表失败")?;
    let latest = neoforge::latest_for(&versions, game_version)
        .with_context(|| format!("Minecraft {game_version} 没有可用的 NeoForge"))?;
    println!("NeoForge 最新版：{}", latest.version);
    Ok(latest.version.clone())
}

/// 安装 NeoForge（已装好时瞬间返回），返回版本 ID
async fn install_neoforge(
    game: &GameDir,
    loader_version: &str,
    source: DownloadSource,
    concurrency: usize,
    java_override: Option<&Path>,
) -> Result<String> {
    let game_version = neoforge::game_version_of(loader_version)
        .with_context(|| format!("认不出 NeoForge 版本号 {loader_version}"))?;
    let loader = LoaderRef {
        kind: LoaderKind::NeoForge,
        version: loader_version.to_owned(),
    };
    install_loader(
        game,
        &game_version,
        &loader,
        source,
        concurrency,
        java_override,
    )
    .await
}

async fn install_loader(
    game: &GameDir,
    game_version: &str,
    loader: &LoaderRef,
    source: DownloadSource,
    concurrency: usize,
    java_override: Option<&Path>,
) -> Result<String> {
    loaders::validate(game_version, loader)?;
    if loaders::is_installed(game, game_version, loader) {
        return Ok(loader.version_id(game_version));
    }
    let client = build_client()?;
    let java = if loader.kind.uses_processor() {
        let vanilla = load_or_fetch_version(game, game_version, &client, source)
            .await
            .with_context(|| format!("读取 {game_version} 的版本描述失败"))?;
        Some(pick_java(java_override, vanilla.required_java())?)
    } else {
        None
    };
    let label = loader.kind.label();
    let downloader = Downloader::new(client.clone(), concurrency);
    let cancel = ctrl_c_token();
    let work_dir = game.root().join(".mcl-tmp");
    let ctx = LoaderContext {
        game,
        client: &client,
        downloader: &downloader,
        source,
        meta_source: source,
        java: java
            .as_ref()
            .map_or_else(|| Path::new(""), |j| j.executable.as_path()),
        work_dir: &work_dir,
        cancel: &cancel,
    };

    let started = Instant::now();
    let last_print = Mutex::new(Instant::now());
    let result = loaders::install(&ctx, game_version, loader, |progress| {
        let (label, p) = match progress {
            LoaderProgress::GameJar(p) => ("原版客户端", p),
            LoaderProgress::Installer(p) => ("加载器安装器", p),
            LoaderProgress::Libraries(p) => ("安装用的库", p),
            LoaderProgress::Processor { index, total, task } => {
                eprintln!(
                    "\r  [{:>5.1} 秒] 处理器 {index}/{total}：{task}            ",
                    started.elapsed().as_secs_f64()
                );
                return;
            }
        };
        let mut last = last_print.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed().as_millis() < 300 && p.done_files < p.total_files {
            return;
        }
        *last = Instant::now();
        eprint!(
            "\r  {label}：{}/{} 个文件  {:.1}/{:.1} MB    ",
            p.done_files,
            p.total_files,
            mb(p.done_bytes),
            mb(p.total_bytes)
        );
    })
    .await;
    match result {
        Ok(id) => {
            let secs = started.elapsed().as_secs_f64();
            if secs > 1.0 {
                eprintln!();
                println!(
                    "{label} {} 安装完成，用时 {secs:.1} 秒，版本名 {id}",
                    loader.version
                );
            }
            Ok(id)
        }
        Err(InstallError::Processor { task, code, output }) => {
            eprintln!("\n处理器 {task} 失败（退出码 {code:?}），最后的输出：\n{output}");
            bail!("{label} 安装失败")
        }
        Err(InstallError::Download(DownloadError::Cancelled)) => bail!("已取消"),
        Err(error) => Err(error).with_context(|| format!("{label} 安装失败")),
    }
}

/// 读版本描述 → 补全全部文件。`quick` 为真时已存在的文件只比大小（启动前用）。
async fn install_version(
    game: &GameDir,
    id: &str,
    source: DownloadSource,
    concurrency: usize,
    quick: bool,
) -> Result<InstalledVersion> {
    let client = build_client()?;
    let version = load_or_fetch_version(game, id, &client, source)
        .await
        .with_context(|| format!("读取 {id} 的版本描述失败"))?;
    let downloader = Downloader::new(client, concurrency).quick_verify(quick);
    let env = Environment::current();
    let cancel = ctrl_c_token();

    let started = Instant::now();
    let last_print = Mutex::new(Instant::now());
    let result = install_files(
        version,
        game,
        &env,
        &downloader,
        source,
        &cancel,
        |phase, p| {
            let mut last = last_print.lock().unwrap_or_else(|e| e.into_inner());
            if last.elapsed().as_millis() < 300 && p.done_files < p.total_files {
                return;
            }
            *last = Instant::now();
            let label = match phase {
                InstallPhase::AssetIndex => "资源索引",
                InstallPhase::Files => "补全文件",
                InstallPhase::Natives => "解压本地库",
            };
            let secs = started.elapsed().as_secs_f64().max(0.001);
            eprint!(
                "\r  {label}：{}/{} 个文件  {:.1}/{:.1} MB  {:.1} MB/s    ",
                p.done_files,
                p.total_files,
                mb(p.done_bytes),
                mb(p.total_bytes),
                mb(p.done_bytes) / secs
            );
        },
    )
    .await;
    eprintln!();
    match result {
        Ok(installed) => Ok(installed),
        Err(InstallError::Download(DownloadError::Cancelled)) => bail!("已取消"),
        Err(InstallError::Download(DownloadError::Failed(failures))) => {
            for failure in &failures {
                eprintln!("  失败：{} —— {}", failure.path.display(), failure.reason);
            }
            bail!("有 {} 个文件下载失败", failures.len())
        }
        Err(error) => Err(error).context("补全文件失败"),
    }
}

fn pick_java(chosen: Option<&Path>, required: u32) -> Result<JavaInstall> {
    if let Some(path) = chosen {
        // 允许传 java.exe 或 Java 主目录
        let home = java::home_of(path);
        return java::inspect(&home, java::JavaOrigin::Manual)
            .with_context(|| format!("{} 不是可用的 Java 目录", path.display()));
    }
    let installs = java::scan(&[], &[]);
    match java::select(&installs, required) {
        Some(found) => Ok(found.clone()),
        None => bail!(
            "没找到能用的 Java {required}（找到 {} 个其他版本）。可以用 --java 指定，或先装一个",
            installs.len()
        ),
    }
}

/// Ctrl+C 时取消，已下载的部分保留
fn ctrl_c_token() -> CancellationToken {
    let cancel = CancellationToken::new();
    let on_ctrl_c = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n正在取消……已下载的部分会保留，下次接着下");
            on_ctrl_c.cancel();
        }
    });
    cancel
}

async fn list_versions(source: DownloadSource, snapshots: bool, limit: usize) -> Result<()> {
    let started = Instant::now();
    let client = build_client()?;
    let manifest = fetch_manifest(&client, source)
        .await
        .context("获取版本清单失败")?;
    println!(
        "最新正式版：{}    最新快照：{}    （共 {} 个版本，用时 {:.1} 秒）",
        manifest.latest.release,
        manifest.latest.snapshot,
        manifest.versions.len(),
        started.elapsed().as_secs_f64()
    );
    for entry in manifest
        .versions
        .iter()
        .filter(|v| snapshots || v.is_release())
        .take(limit)
    {
        let date = entry
            .release_time
            .as_deref()
            .and_then(|t| t.get(..10))
            .unwrap_or("");
        println!("  {:<14} {:<10} {}", entry.id, entry.kind, date);
    }
    Ok(())
}

async fn fetch_version(
    id: &str,
    game_dir: &Path,
    source: DownloadSource,
    concurrency: usize,
) -> Result<()> {
    let client = build_client()?;
    let manifest = fetch_manifest(&client, source)
        .await
        .context("获取版本清单失败")?;
    let Some(entry) = manifest.find(id) else {
        bail!("版本清单里没有 {id}");
    };

    let (version, raw) = fetch_version_json(&client, entry, source).await?;
    let version_dir = safe_join(&game_dir.join("versions"), id)?;
    let json_path = version_dir.join(format!("{id}.json"));
    write_atomic(&json_path, &raw)?;
    println!(
        "版本描述文件已保存（SHA-1 校验通过）：{}",
        json_path.display()
    );

    let Some(client_jar) = version.downloads.as_ref().and_then(|d| d.client.as_ref()) else {
        bail!("{id} 没有客户端下载信息");
    };
    let item = DownloadItem {
        urls: candidate_urls(&client_jar.url, source),
        path: version_dir.join(format!("{id}.jar")),
        sha1: client_jar.sha1.clone(),
        size: client_jar.size,
    };
    run_downloads(&[item], concurrency).await?;
    println!("客户端已下载并校验");

    let env = Environment::current();
    let allowed = version
        .libraries
        .iter()
        .filter(|l| rules_allow(l.rules.as_deref(), &env))
        .count();
    println!();
    println!(
        "版本：{}（{}）",
        version.id,
        version.kind.as_deref().unwrap_or("未知类型")
    );
    println!(
        "主类：{}",
        version.main_class.as_deref().unwrap_or("（无）")
    );
    println!("需要 Java：{}", version.required_java());
    println!(
        "资源索引：{}",
        version
            .asset_index
            .as_ref()
            .map_or("（无）", |a| a.id.as_str())
    );
    println!(
        "依赖库：共 {} 个，当前系统（{} {}）需要 {} 个",
        version.libraries.len(),
        env.os_name,
        env.os_arch,
        allowed
    );
    Ok(())
}

/// 下载一批文件，在终端里显示进度；按 Ctrl+C 安全取消（已下的部分下次续传）。
async fn run_downloads(items: &[DownloadItem], concurrency: usize) -> Result<()> {
    let cancel = ctrl_c_token();
    let started = Instant::now();
    let last_print = Mutex::new(Instant::now());
    let print = |p: DownloadProgress, force: bool| {
        let mut last = last_print.lock().unwrap_or_else(|e| e.into_inner());
        if !force && last.elapsed().as_millis() < 300 {
            return;
        }
        *last = Instant::now();
        let secs = started.elapsed().as_secs_f64().max(0.001);
        eprint!(
            "\r  {}/{} 个文件  {:.1}/{:.1} MB  {:.1} MB/s    ",
            p.done_files,
            p.total_files,
            mb(p.done_bytes),
            mb(p.total_bytes),
            mb(p.done_bytes) / secs
        );
    };

    let downloader = Downloader::new(build_client()?, concurrency);
    let result = downloader
        .download_all(items, &cancel, |p| print(p, false))
        .await;
    eprintln!();
    match result {
        Ok(report) => {
            println!(
                "下载 {} 个，跳过 {} 个（已存在且校验通过），收到 {:.1} MB，用时 {:.1} 秒",
                report.downloaded,
                report.skipped,
                mb(report.bytes_received),
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        Err(DownloadError::Cancelled) => bail!("已取消"),
        Err(DownloadError::Failed(failures)) => {
            for failure in &failures {
                eprintln!("  失败：{} —— {}", failure.path.display(), failure.reason);
            }
            bail!("有 {} 个文件下载失败", failures.len())
        }
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}
