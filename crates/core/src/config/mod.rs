//! 启动器配置。

pub mod settings;

pub use settings::{
    AfterLaunchBehavior, Appearance, DownloadSettings, DownloadSource, EffectsMode, EnvVar,
    GameLanguage, GameSettings, GcPreset, LaunchHooks, OptionsSyncCategories, Platform, ProxyMode,
    ProxyScheme, ProxySettings, Settings, SettingsError, SettingsStore, SyncSettings, ThemeMode,
    WindowMode, WindowSettings, WorldSettings, clamp_memory_mb, clean_args, clean_env_vars,
    clean_hook_command, clean_window_title,
};
