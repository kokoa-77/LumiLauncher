//! 全局设置：读写 `settings.json`。
//!
//! - 文件不存在 → 用默认值；
//! - 文件损坏 → 把坏文件改名备份成 `settings.json.corrupt-<时间戳>`，再用默认值，不让启动器打不开；
//! - 带 `schemaVersion`，以后改格式时在 [`migrate`] 里兼容旧数据；
//! - 写入走原子写。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::fs::{AtomicWriteError, ReadJsonError, read_json, write_json_atomic};
use crate::now_millis;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
const MIN_CONCURRENCY: u32 = 1;
const MAX_CONCURRENCY: u32 = 256;
const MIN_MEMORY_MB: u32 = 512;
const MAX_MEMORY_MB: u32 = 65536;
/// 自定义参数最长多少个字符（防止误粘贴一大段东西进来）
const MAX_ARGS_CHARS: usize = 4000;
/// 游戏窗口宽高的范围（像素）
const MIN_WINDOW_WIDTH: u32 = 320;
const MIN_WINDOW_HEIGHT: u32 = 240;
const MAX_WINDOW_SIDE: u32 = 16384;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub appearance: Appearance,
    pub download: DownloadSettings,
    pub game: GameSettings,
    /// 世界与存储：自动备份与自定义备份目录
    pub worlds: WorldSettings,
    /// 本机跨实例同步
    pub sync: SyncSettings,
    /// 启动时自动静默检查启动器更新（默认开）
    pub auto_check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            appearance: Appearance::default(),
            download: DownloadSettings::default(),
            game: GameSettings::default(),
            worlds: WorldSettings::default(),
            sync: SyncSettings::default(),
            auto_check_updates: true,
        }
    }
}

/// 每个世界最多留几份自动备份的范围
const MIN_KEEP_BACKUPS: u32 = 1;
const MAX_KEEP_BACKUPS: u32 = 20;

/// 世界的设置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct WorldSettings {
    /// 自动备份：正常退出游戏后备份这次玩过的世界、从首页进世界前世界变过也先备一份。
    /// 默认关闭以节省磁盘空间；需要时可在设置中开启
    pub auto_backup: bool,
    /// 每个世界最多留几份自动备份（手动的、恢复前的不算）
    pub keep_backups: u32,
    /// 自定义世界备份存放根目录；`None` 时存放在各实例的 `Lumi/backups/`
    pub backup_dir: Option<String>,
}

impl Default for WorldSettings {
    fn default() -> Self {
        Self {
            auto_backup: false,
            keep_backups: 3,
            backup_dir: None,
        }
    }
}

/// `options.txt` 跨实例同步的细分分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct OptionsSyncCategories {
    /// 按键绑定（`key_*`）
    pub keybinds: bool,
    /// 音频与音量（`soundCategory_*` 等）
    pub audio: bool,
    /// 视频与画面（FOV、视距、帧率上限、全屏、画质等）
    pub video: bool,
    /// 辅助功能、字幕、聊天框与语言
    pub accessibility: bool,
}

impl Default for OptionsSyncCategories {
    fn default() -> Self {
        Self {
            keybinds: true,
            audio: true,
            video: false,
            accessibility: true,
        }
    }
}

/// 本机跨实例同步设置（纯本机、零云端，支持按资产独立开关）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SyncSettings {
    /// 总开关：开启后启动前拉取共享库、正常退出后增量回写共享库
    pub enabled: bool,
    /// 同步游戏设置（`options.txt`，按下方分类安全合并）
    pub options: bool,
    /// `options.txt` 分类开关
    pub options_categories: OptionsSyncCategories,
    /// 同步多人服务器列表（`servers.dat`，NBT 无损去重合并）
    pub servers: bool,
    /// 同步共享资源包（`resourcepacks/`，同盘硬链接）
    pub resource_packs: bool,
    /// 同步共享光影包（`shaderpacks/`，同盘硬链接）
    pub shader_packs: bool,
    /// 同步命令历史记录（`command_history.txt`，按行去重合并）
    #[serde(default)]
    pub command_history: bool,
    /// 同步保存的创造模式物品栏（`hotbar.nbt`，安全镜像 + 首次自动备份）
    #[serde(default)]
    pub hotbar: bool,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            options: true,
            options_categories: OptionsSyncCategories::default(),
            servers: true,
            resource_packs: false,
            shader_packs: false,
            command_history: false,
            hotbar: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub theme: ThemeMode,
    /// 减少动效（另外也会跟随系统的「减少动态效果」设置）。
    pub reduce_motion: bool,
    /// 画面效果：自动（老电脑用流畅）/ 流畅 / 精美
    pub effects: EffectsMode,
    /// 自动封面：玩的时候悄悄截游戏画面当这个世界的封面（默认开）
    pub auto_cover: bool,
    /// 主色跟着世界变（招牌 2，默认开）：进到有截图的世界时换成世界的颜色；关掉就一直用下面选的颜色
    pub accent_follow_world: bool,
    /// 自己选的主色：色相 0–360、彩度 0–0.3，亮度按对比度自动算。
    /// 跟着世界变时，没有截图的世界、还没进过世界时也用它
    pub accent_hue: f64,
    pub accent_chroma: f64,
}

/// 默认主色：天空蓝（和 `lib/themeEngine.ts` 的 `DEFAULT_ACCENT` 一样）
const DEFAULT_ACCENT_HUE: f64 = 245.0;
const DEFAULT_ACCENT_CHROMA: f64 = 0.15;
const MAX_ACCENT_CHROMA: f64 = 0.3;

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            reduce_motion: false,
            effects: EffectsMode::default(),
            auto_cover: true,
            accent_follow_world: true,
            accent_hue: DEFAULT_ACCENT_HUE,
            accent_chroma: DEFAULT_ACCENT_CHROMA,
        }
    }
}

/// 画面效果。流畅：过渡更短、不做位移和缩放动画、首页封面不铺满整窗，给低配电脑用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum EffectsMode {
    #[default]
    Auto,
    Lite,
    Rich,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    /// 用 DoH 解析下载域名，防 DNS 污染和劫持（不承诺提速）。问不到时自动退回系统 DNS（`net::doh`）。
    pub doh_enabled: bool,
    pub max_concurrency: u32,
    /// 补件时在下载文件夹里认领的文件：`true` 移进内容仓库（下载文件夹不留），
    /// `false`（默认）复制一份，原文件不动。拖进窗口、手动选的文件始终复制。
    #[serde(default)]
    pub move_claimed_files: bool,
    /// 首选平台：同一个项目两个平台都有时，资源中心显示、安装都先用它
    pub preferred_platform: Platform,
    /// 自动装前置：关掉后只装点的那个，前置自己装
    pub auto_dependencies: bool,
    /// 补件时盯着的文件夹；空 = 系统的「下载」文件夹。只由后端的「选择文件夹」窗口写入
    pub claim_folder: Option<String>,
    /// 网络代理设置：跟随系统 / 直连 / 自定义 HTTP 或 SOCKS5
    pub proxy: ProxySettings,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            doh_enabled: true,
            max_concurrency: 32,
            move_claimed_files: false,
            preferred_platform: Platform::Modrinth,
            auto_dependencies: true,
            claim_folder: None,
            proxy: ProxySettings::default(),
        }
    }
}

/// 网络代理模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    /// 跟随 Windows 系统代理
    #[default]
    System,
    /// 不使用代理，直连目标服务器
    Direct,
    /// 手动指定代理协议、主机与端口
    Custom,
}

/// 自定义代理协议
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ProxyScheme {
    #[default]
    Http,
    Socks5,
}

/// 代理配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    pub mode: ProxyMode,
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            mode: ProxyMode::System,
            scheme: ProxyScheme::Http,
            host: "127.0.0.1".to_owned(),
            port: 7890,
        }
    }
}

impl ProxySettings {
    pub fn normalized(mut self) -> Self {
        let cleaned: String = self
            .host
            .trim()
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control())
            .take(255)
            .collect();
        self.host = cleaned;
        if self.port == 0 {
            self.port = 7890;
        }
        self
    }

    /// 自定义模式下返回给 reqwest 使用的代理 URL；主机为空或含非法字符时返回 None。
    pub fn custom_url(&self) -> Option<String> {
        if self.mode != ProxyMode::Custom {
            return None;
        }
        let host = self.host.trim();
        if host.is_empty()
            || self.port == 0
            || host.contains("://")
            || host.contains('/')
            || host.contains('@')
            || host.contains('?')
            || host.contains('#')
        {
            return None;
        }
        let scheme = match self.scheme {
            ProxyScheme::Http => "http",
            ProxyScheme::Socks5 => "socks5h",
        };
        let formatted_host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        Some(format!("{scheme}://{formatted_host}:{}", self.port))
    }
}

/// 内容平台（「首选平台」）。和 `content::ContentSource` 的写法一样
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    #[default]
    Modrinth,
    CurseForge,
}

/// 游戏相关的默认设置（以后每个实例可以单独覆盖）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct GameSettings {
    /// 内存自动分配（按电脑内存和模组数量）；关掉就用下面手动指定的值
    pub memory_auto: bool,
    /// 手动指定的最大内存（MB）
    pub max_memory_mb: u32,
    /// 游戏目录（.minecraft）的完整路径；`None` 用默认位置。只由后端的「选择文件夹」窗口写入
    pub directory: Option<String>,
    /// 垃圾回收预设
    pub gc_preset: GcPreset,
    /// 自定义 JVM 参数（用户输入的原文，启动时按空格和引号拆成数组）
    pub jvm_args: String,
    /// 自定义游戏参数（同上，加在最后，例如 `--fullscreen`）
    pub game_args: String,
    /// 高性能显卡：启动前在 Windows「图形设置」里让 Java 用高性能显卡（双显卡笔记本有用，默认开）
    pub high_performance_gpu: bool,
    /// 游戏窗口：游戏默认 / 自定义宽高 / 全屏
    pub window: WindowSettings,
    /// 启动游戏时额外加的环境变量
    pub env_vars: Vec<EnvVar>,
    /// 兼容旧字段：与 `after_launch == AfterLaunchBehavior::Release` 同步。
    pub release_webview_while_playing: bool,
    /// 实例首次启动时的默认游戏语言（仅实例尚无语言配置时写入 `options.txt`）。
    pub default_language: GameLanguage,
    /// 游戏进入可玩界面后启动器窗口的行为：保持显示 / 最小化 / 释放界面。
    pub after_launch: AfterLaunchBehavior,
    /// 游戏退出后自动恢复并聚焦启动器窗口（默认开）。
    pub refocus_on_exit: bool,
    /// 启动钩子：启动前 / 退出后执行的本地命令（默认空）
    pub hooks: LaunchHooks,
    /// 自定义游戏窗口标题模板（空字符串表示使用 Minecraft 默认标题，支持 `{instance}` / `{version}` / `{loader}` / `{player}`）
    pub custom_window_title: String,
    /// Discord 活动状态（纯本地命名管道，默认关闭）
    pub discord_rpc: bool,
}

/// 启动钩子命令（在游戏启动前与退出后运行）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LaunchHooks {
    /// 启动前运行的命令（非零退出码会中止启动）
    pub pre_launch: String,
    /// 游戏退出后运行的命令
    pub post_exit: String,
}

impl LaunchHooks {
    pub fn normalized(mut self) -> Self {
        self.pre_launch = clean_hook_command(&self.pre_launch);
        self.post_exit = clean_hook_command(&self.post_exit);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.pre_launch.is_empty() && self.post_exit.is_empty()
    }
}

pub fn clean_hook_command(text: &str) -> String {
    text.split(['\r', '\n'])
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .chars()
        .filter(|c| *c != '\0')
        .take(1000)
        .collect()
}

pub fn clean_window_title(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect()
}

/// 实例首次启动时的默认游戏语言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum GameLanguage {
    /// 跟随启动器界面语言（中文用 `zh_cn`，其他用 `en_us`）
    #[default]
    FollowLauncher,
    /// 不预设，保留 Minecraft 默认语言
    GameDefault,
    ZhCn,
    ZhTw,
    EnUs,
    JaJp,
}

impl GameLanguage {
    /// 解析为写入 `options.txt` 的 Minecraft 语言代码；`GameDefault` 返回 `None`。
    pub fn resolve(self, ui_language: Option<&str>) -> Option<&'static str> {
        match self {
            Self::FollowLauncher => {
                if ui_language.is_some_and(|l| l.to_ascii_lowercase().starts_with("zh")) {
                    Some("zh_cn")
                } else {
                    Some("en_us")
                }
            }
            Self::GameDefault => None,
            Self::ZhCn => Some("zh_cn"),
            Self::ZhTw => Some("zh_tw"),
            Self::EnUs => Some("en_us"),
            Self::JaJp => Some("ja_jp"),
        }
    }
}

/// 游戏启动后的启动器窗口行为
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum AfterLaunchBehavior {
    /// 保持启动器窗口显示
    #[default]
    Keep,
    /// 最小化启动器窗口
    Minimize,
    /// 释放启动器界面以降低内存占用（可从系统托盘恢复）
    Release,
}

/// 一个环境变量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

/// 环境变量最多几个、名字和值最长多少个字符
const MAX_ENV_VARS: usize = 64;
const MAX_ENV_NAME_CHARS: usize = 256;

/// 整理环境变量：名字去掉首尾空白；名字为空、带「=」或控制字符的去掉（系统不认）；同名的留后面那个；太多、太长的截断
pub fn clean_env_vars(vars: &[EnvVar]) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = Vec::new();
    for var in vars {
        let name = var.name.trim();
        let valid = !name.is_empty()
            && name.chars().count() <= MAX_ENV_NAME_CHARS
            && !name.contains('=')
            && !name.chars().any(char::is_control);
        if !valid {
            continue;
        }
        let value: String = var
            .value
            .chars()
            .filter(|c| *c != '\0')
            .take(MAX_ARGS_CHARS)
            .collect();
        // Windows 的环境变量名不分大小写
        out.retain(|v| !v.name.eq_ignore_ascii_case(name));
        out.push(EnvVar {
            name: name.to_owned(),
            value,
        });
    }
    out.truncate(MAX_ENV_VARS);
    out
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            memory_auto: true,
            max_memory_mb: 4096,
            directory: None,
            gc_preset: GcPreset::default(),
            jvm_args: String::new(),
            game_args: String::new(),
            high_performance_gpu: true,
            window: WindowSettings::default(),
            env_vars: Vec::new(),
            release_webview_while_playing: false,
            default_language: GameLanguage::default(),
            after_launch: AfterLaunchBehavior::default(),
            refocus_on_exit: true,
            hooks: LaunchHooks::default(),
            custom_window_title: String::new(),
            discord_rpc: false,
        }
    }
}

/// 游戏窗口一开始多大。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum WindowMode {
    /// 不管，游戏自己定（854 × 480，或者上次在游戏里设的全屏）
    #[default]
    Default,
    /// 用下面的宽高
    Custom,
    /// 启动后自动最大化游戏窗口
    Maximized,
    /// 全屏启动（加 `--fullscreen`）
    Fullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct WindowSettings {
    pub mode: WindowMode,
    /// 自定义时的宽高（像素）；别的模式下只是记着，切回自定义时还在
    pub width: u32,
    pub height: u32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            mode: WindowMode::Default,
            width: 1280,
            height: 720,
        }
    }
}

impl WindowSettings {
    /// 宽高拉回合理范围（太小游戏会自己放大，太大没有意义）
    pub fn normalized(mut self) -> Self {
        self.width = self.width.clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_SIDE);
        self.height = self.height.clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_SIDE);
        self
    }
}

/// 垃圾回收预设。用户自己在 JVM 参数里写了垃圾回收器时，预设不加（两个会冲突，Java 直接起不来）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum GcPreset {
    /// G1，参数和官方启动器一样（所有 Java 版本都能用）
    #[default]
    Auto,
    /// ZGC：停顿更短，内存给得多时更明显；Java 15 起才有，更老的 Java 自动退回 G1
    Zgc,
    /// 不加，用 Java 自己的默认
    Off,
}

impl Settings {
    /// 把越界的值拉回合法范围，并写上当前格式版本。
    pub fn normalized(mut self) -> Self {
        self.schema_version = CURRENT_SCHEMA_VERSION;
        let hue = self.appearance.accent_hue;
        self.appearance.accent_hue = if hue.is_finite() {
            hue.rem_euclid(360.0)
        } else {
            DEFAULT_ACCENT_HUE
        };
        let chroma = self.appearance.accent_chroma;
        self.appearance.accent_chroma = if chroma.is_finite() {
            chroma.clamp(0.0, MAX_ACCENT_CHROMA)
        } else {
            DEFAULT_ACCENT_CHROMA
        };
        self.download.max_concurrency = self
            .download
            .max_concurrency
            .clamp(MIN_CONCURRENCY, MAX_CONCURRENCY);
        self.game.max_memory_mb = clamp_memory_mb(self.game.max_memory_mb);
        self.game.directory = self
            .game
            .directory
            .take()
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty());
        self.download.claim_folder = self
            .download
            .claim_folder
            .take()
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty());
        self.download.proxy = self.download.proxy.normalized();
        self.game.jvm_args = clean_args(&self.game.jvm_args);
        self.game.game_args = clean_args(&self.game.game_args);
        self.game.window = self.game.window.normalized();
        self.game.env_vars = clean_env_vars(&self.game.env_vars);
        self.game.release_webview_while_playing =
            self.game.after_launch == AfterLaunchBehavior::Release;
        self.game.hooks = self.game.hooks.normalized();
        self.game.custom_window_title = clean_window_title(&self.game.custom_window_title);
        self.worlds.keep_backups = self
            .worlds
            .keep_backups
            .clamp(MIN_KEEP_BACKUPS, MAX_KEEP_BACKUPS);
        self.worlds.backup_dir = self
            .worlds
            .backup_dir
            .take()
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty());
        self
    }

    /// 导出可迁移的偏好 JSON：不写只属于这台电脑的几项（见 [`Self::keep_machine_bound`]），且不包含任何登录凭据。
    pub fn export_portable(&self) -> Result<String, serde_json::Error> {
        let mut portable = self.clone().normalized();
        portable.keep_machine_bound(&Self::default());
        serde_json::to_string_pretty(&portable)
    }

    /// 从导出的偏好 JSON 解析设置：走版本迁移、缺省补齐和范围规范化，只属于这台电脑的几项保留本机原来的值。
    pub fn import_portable(&self, raw: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(raw)?;
        let mut imported = with_defaults(migrate(value))?.normalized();
        imported.keep_machine_bound(self);
        Ok(imported)
    }

    /// 只属于这台电脑、不随偏好文件走的几项，从 `from` 照搬：
    /// 本机目录路径，以及会在这台电脑上执行命令的启动前后命令、环境变量、自定义 Java 参数。
    /// 别人分享的偏好文件不能借这几项在导入的电脑上执行命令，导出时也不带出玩家自己填的值。
    fn keep_machine_bound(&mut self, from: &Self) {
        self.game.directory = from.game.directory.clone();
        self.download.claim_folder = from.download.claim_folder.clone();
        self.worlds.backup_dir = from.worlds.backup_dir.clone();
        self.game.hooks = from.game.hooks.clone();
        self.game.env_vars = from.game.env_vars.clone();
        self.game.jvm_args = from.game.jvm_args.clone();
    }
}

/// 内存设置的合法范围（实例单独设置也用它）
pub fn clamp_memory_mb(mb: u32) -> u32 {
    mb.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB)
}

/// 自定义参数：去掉首尾空白和换行（粘贴进来的多行变成一行），太长的截断
pub fn clean_args(text: &str) -> String {
    let joined = text.split(['\r', '\n']).collect::<Vec<_>>().join(" ");
    joined.trim().chars().take(MAX_ARGS_CHARS).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("读取设置失败")]
    Read(#[from] ReadJsonError),
    #[error("保存设置失败")]
    Write(#[from] AtomicWriteError),
}

/// 旧格式 → 新格式：
/// - 如果旧文件只写了 `game.releaseWebviewWhilePlaying: true` 而没有 `game.afterLaunch`，
///   平滑迁移为 `game.afterLaunch = "release"`。
fn migrate(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(game) = value.get_mut("game").and_then(|v| v.as_object_mut())
        && !game.contains_key("afterLaunch")
        && game
            .get("releaseWebviewWhilePlaying")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    {
        game.insert(
            "afterLaunch".to_owned(),
            serde_json::Value::String("release".to_owned()),
        );
    }
    value
}

/// 把文件里的值叠到默认设置上：文件里缺的字段用默认值补齐（老版本的设置文件不会因为新增字段而失效）。
/// 在这里补默认值，而不是给结构体加 `#[serde(default)]`，这样前端类型里的字段都是必填的。
fn with_defaults(value: serde_json::Value) -> Result<Settings, serde_json::Error> {
    let mut merged = serde_json::to_value(Settings::default())?;
    merge_json(&mut merged, value);
    serde_json::from_value(merged)
}

fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// 设置的读写入口，内存里缓存一份，写入时同时落盘。
pub struct SettingsStore {
    path: PathBuf,
    cached: Mutex<Settings>,
}

impl SettingsStore {
    /// 打开（或初始化）设置文件。损坏的文件会被备份并重置为默认值。
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let settings = load_or_recover(&path);
        Self {
            path,
            cached: Mutex::new(settings),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Settings> {
        self.cached.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self) -> Settings {
        self.lock().clone()
    }

    /// 整体替换并保存，返回规范化后的设置。保存失败时内存里的值保持不变。
    pub fn set(&self, settings: Settings) -> Result<Settings, SettingsError> {
        let normalized = settings.normalized();
        let mut cached = self.lock();
        write_json_atomic(&self.path, &normalized)?;
        *cached = normalized.clone();
        tracing::info!(path = %self.path.display(), "设置已保存");
        Ok(normalized)
    }
}

fn load_or_recover(path: &Path) -> Settings {
    match read_json::<serde_json::Value>(path) {
        Ok(None) => Settings::default(),
        Ok(Some(value)) => match with_defaults(migrate(value)) {
            Ok(settings) => settings.normalized(),
            Err(error) => {
                backup_corrupt(path, &error.to_string());
                Settings::default()
            }
        },
        Err(ReadJsonError::Parse { source, .. }) => {
            backup_corrupt(path, &source.to_string());
            Settings::default()
        }
        Err(error @ ReadJsonError::Io { .. }) => {
            tracing::error!(path = %path.display(), error = %error, "读取设置失败，本次使用默认设置");
            Settings::default()
        }
    }
}

fn backup_corrupt(path: &Path, reason: &str) {
    let backup = path.with_file_name(format!(
        "{}.corrupt-{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        now_millis()
    ));
    match std::fs::rename(path, &backup) {
        Ok(()) => tracing::warn!(
            path = %path.display(),
            backup = %backup.display(),
            reason,
            "设置文件已损坏，已备份并恢复默认设置"
        ),
        Err(error) => tracing::error!(
            path = %path.display(),
            error = %error,
            reason,
            "设置文件已损坏，备份失败，本次使用默认设置"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(dir.path().join("settings.json"));
        assert_eq!(store.get(), Settings::default());
        assert!(store.get().download.doh_enabled, "DoH 默认开启");
    }

    #[test]
    fn saves_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::open(&path);

        let mut changed = store.get();
        changed.appearance.theme = ThemeMode::Dark;
        changed.download.doh_enabled = false;
        store.set(changed.clone()).unwrap();

        assert_eq!(SettingsStore::open(&path).get(), changed);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(dir.path().join("settings.json"));
        let mut s = store.get();
        s.download.max_concurrency = 100_000;
        assert_eq!(
            store.set(s).unwrap().download.max_concurrency,
            MAX_CONCURRENCY
        );
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "appearance": { "theme": "light" } }"#).unwrap();

        let settings = SettingsStore::open(&path).get();
        assert_eq!(settings.appearance.theme, ThemeMode::Light);
        assert_eq!(
            settings.appearance.effects,
            EffectsMode::Auto,
            "老设置文件没有画面效果，按自动"
        );
        assert_eq!(settings.download, DownloadSettings::default());
    }

    #[test]
    fn old_download_settings_keep_their_values() {
        // 加了「补件认领后移走」并移除了旧下载源字段：以前保存的设置能正常忽略旧键并保留其余值
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{ "download": { "source": "official", "dohEnabled": false, "maxConcurrency": 8 } }"#,
        )
        .unwrap();

        let download = SettingsStore::open(&path).get().download;
        assert!(!download.doh_enabled);
        assert_eq!(download.max_concurrency, 8);
        assert!(!download.move_claimed_files, "默认复制，不移走");
        assert_eq!(download.preferred_platform, Platform::Modrinth);
        assert!(download.auto_dependencies, "默认自动装前置");
        assert_eq!(download.claim_folder, None);
    }

    #[test]
    fn accent_defaults_and_bounds() {
        // 后来加的主色：以前的设置文件没有，补成跟着世界变 + 天空蓝；越界的拉回来
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "appearance": { "theme": "dark" } }"#).unwrap();
        let appearance = SettingsStore::open(&path).get().appearance;
        assert!(appearance.accent_follow_world);
        assert!((appearance.accent_hue - 245.0).abs() < f64::EPSILON);

        let mut settings = Settings::default();
        settings.appearance.accent_hue = -30.0;
        settings.appearance.accent_chroma = 9.0;
        let settings = settings.normalized();
        assert!((settings.appearance.accent_hue - 330.0).abs() < 1e-9);
        assert!((settings.appearance.accent_chroma - 0.3).abs() < 1e-9);
    }

    #[test]
    fn old_game_settings_get_the_new_launch_options() {
        // 加了垃圾回收、自定义参数、高性能显卡：以前的设置文件没有这几个字段
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{ "game": { "playerName": "Alex", "memoryAuto": false, "maxMemoryMb": 6144 } }"#,
        )
        .unwrap();

        let game = SettingsStore::open(&path).get().game;
        assert!(!game.memory_auto);
        assert_eq!(game.max_memory_mb, 6144);
        assert_eq!(game.gc_preset, GcPreset::Auto);
        assert!(game.jvm_args.is_empty() && game.game_args.is_empty());
        assert!(game.high_performance_gpu, "高性能显卡默认开");
        assert_eq!(game.window, WindowSettings::default(), "后来加的游戏窗口");
        assert!(game.env_vars.is_empty(), "后来加的环境变量");
        assert!(!game.release_webview_while_playing, "释放界面默认关");
    }

    #[test]
    fn env_vars_are_tidied() {
        let var = |name: &str, value: &str| EnvVar {
            name: name.to_owned(),
            value: value.to_owned(),
        };
        let cleaned = clean_env_vars(&[
            var("  JAVA_TOOL_OPTIONS ", "-Dx=1"),
            var("", "空名字"),
            var("A=B", "名字带等号"),
            var("bad\nname", "控制字符"),
            var("java_tool_options", "-Dx=2"),
            var("MESA_GL_VERSION_OVERRIDE", "4.5"),
        ]);
        assert_eq!(
            cleaned,
            vec![
                var("java_tool_options", "-Dx=2"),
                var("MESA_GL_VERSION_OVERRIDE", "4.5")
            ],
            "同名（不分大小写）留后面那个"
        );
    }

    #[test]
    fn window_size_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(dir.path().join("settings.json"));
        let mut s = store.get();
        s.game.window = WindowSettings {
            mode: WindowMode::Custom,
            width: 10,
            height: 99_999,
        };
        let saved = store.set(s).unwrap().game.window;
        assert_eq!(
            (saved.width, saved.height),
            (MIN_WINDOW_WIDTH, MAX_WINDOW_SIDE)
        );
        assert_eq!(saved.mode, WindowMode::Custom);
    }

    #[test]
    fn custom_args_are_tidied() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(dir.path().join("settings.json"));
        let mut s = store.get();
        s.game.jvm_args = "  -XX:+UseZGC\r\n-Dfoo=bar  ".to_owned();
        s.game.game_args = "x".repeat(MAX_ARGS_CHARS + 10);
        let saved = store.set(s).unwrap();
        assert_eq!(saved.game.jvm_args, "-XX:+UseZGC  -Dfoo=bar");
        assert_eq!(saved.game.game_args.chars().count(), MAX_ARGS_CHARS);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ broken").unwrap();

        let store = SettingsStore::open(&path);

        assert_eq!(store.get(), Settings::default());
        assert!(!path.exists(), "坏文件应该被改名");
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.corrupt-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
    }

    #[test]
    fn legacy_release_webview_migrates_to_after_launch_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{ "game": { "releaseWebviewWhilePlaying": true } }"#,
        )
        .unwrap();

        let settings = SettingsStore::open(&path).get();
        assert_eq!(settings.game.after_launch, AfterLaunchBehavior::Release);
        assert!(settings.game.release_webview_while_playing);
        assert_eq!(settings.game.default_language, GameLanguage::FollowLauncher);
        assert!(settings.auto_check_updates);
    }

    #[test]
    fn proxy_settings_validate_custom_urls() {
        let mut p = ProxySettings::default();
        assert_eq!(p.custom_url(), None);

        p.mode = ProxyMode::Custom;
        p.scheme = ProxyScheme::Http;
        p.host = " 127.0.0.1 ".to_owned();
        p.port = 7897;
        assert_eq!(
            p.clone().normalized().custom_url().as_deref(),
            Some("http://127.0.0.1:7897")
        );

        p.scheme = ProxyScheme::Socks5;
        assert_eq!(
            p.clone().normalized().custom_url().as_deref(),
            Some("socks5h://127.0.0.1:7897")
        );

        p.host = "http://bad/path".to_owned();
        assert_eq!(p.normalized().custom_url(), None);
    }

    #[test]
    fn portable_export_strips_machine_paths_and_import_preserves_local_paths() {
        let mut local = Settings::default();
        local.game.directory = Some(r"D:\Games\MC".to_owned());
        local.download.claim_folder = Some(r"D:\Downloads".to_owned());
        local.appearance.theme = ThemeMode::Dark;
        local.game.default_language = GameLanguage::ZhCn;
        local.game.after_launch = AfterLaunchBehavior::Minimize;

        let exported = local.export_portable().unwrap();
        assert!(!exported.contains(r"D:\\Games\\MC"));
        assert!(!exported.contains(r"D:\\Downloads"));

        let mut target = Settings::default();
        target.game.directory = Some(r"E:\OtherPC\MC".to_owned());
        target.download.claim_folder = Some(r"E:\OtherDownloads".to_owned());
        let imported = target.import_portable(&exported).unwrap();
        assert_eq!(imported.appearance.theme, ThemeMode::Dark);
        assert_eq!(imported.game.default_language, GameLanguage::ZhCn);
        assert_eq!(imported.game.after_launch, AfterLaunchBehavior::Minimize);
        assert_eq!(imported.game.directory.as_deref(), Some(r"E:\OtherPC\MC"));
        assert_eq!(
            imported.download.claim_folder.as_deref(),
            Some(r"E:\OtherDownloads")
        );
    }

    #[test]
    fn portable_settings_never_carry_commands_or_env_vars() {
        let mut local = Settings::default();
        local.game.hooks.pre_launch = "echo local-pre".to_owned();
        local.game.hooks.post_exit = "echo local-post".to_owned();
        local.game.env_vars = vec![EnvVar {
            name: "MY_TOKEN".to_owned(),
            value: "secret-value".to_owned(),
        }];
        local.game.jvm_args = "-XX:+UseZGC".to_owned();

        // 导出：这几项不写进文件
        let exported = local.export_portable().unwrap();
        for leaked in [
            "local-pre",
            "local-post",
            "MY_TOKEN",
            "secret-value",
            "UseZGC",
        ] {
            assert!(!exported.contains(leaked), "导出的偏好里不该有 {leaked}");
        }

        // 别人分享的文件里写了命令、环境变量、Java 参数：一概不用，保留本机原来的
        let mut shared: serde_json::Value =
            serde_json::from_str(&Settings::default().export_portable().unwrap()).unwrap();
        shared["game"]["hooks"]["preLaunch"] = "calc.exe".into();
        shared["game"]["hooks"]["postExit"] = "calc.exe".into();
        shared["game"]["envVars"] =
            serde_json::json!([{ "name": "JAVA_TOOL_OPTIONS", "value": "-javaagent:x.jar" }]);
        shared["game"]["jvmArgs"] = "-XX:OnOutOfMemoryError=calc.exe".into();
        shared["appearance"]["theme"] = serde_json::to_value(ThemeMode::Dark).unwrap();
        let imported = local.import_portable(&shared.to_string()).unwrap();
        assert_eq!(imported.game.hooks, local.game.hooks);
        assert_eq!(imported.game.env_vars, local.game.env_vars);
        assert_eq!(imported.game.jvm_args, local.game.jvm_args);
        assert_eq!(
            imported.appearance.theme,
            ThemeMode::Dark,
            "其他偏好照常导入"
        );
    }

    #[test]
    fn game_language_resolves_for_ui_locale() {
        assert_eq!(
            GameLanguage::FollowLauncher.resolve(Some("zh-CN")),
            Some("zh_cn")
        );
        assert_eq!(
            GameLanguage::FollowLauncher.resolve(Some("en")),
            Some("en_us")
        );
        assert_eq!(GameLanguage::GameDefault.resolve(Some("zh-CN")), None);
        assert_eq!(GameLanguage::JaJp.resolve(Some("zh-CN")), Some("ja_jp"));
    }
}
