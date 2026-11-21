//! 实例自己的设置：内存、Java、启动参数、游戏窗口。
//!
//! 每一组都可以为空：为空就跟着全局设置走（全局怎么改，这个实例跟着变）；
//! 填了就只用这个实例自己的。合在一起的结果见 [`Resolved`]。

use serde::{Deserialize, Serialize};

use crate::config::{
    EnvVar, GameSettings, GcPreset, LaunchHooks, WindowSettings, clamp_memory_mb, clean_args,
    clean_env_vars, clean_window_title,
};

/// 单实例跨实例同步细粒度排除开关（某项为 true 表示仅本实例跳过该项同步）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct InstanceSyncExclusions {
    #[serde(default)]
    pub options: bool,
    #[serde(default)]
    pub servers: bool,
    #[serde(default)]
    pub resource_packs: bool,
    #[serde(default)]
    pub shader_packs: bool,
    #[serde(default)]
    pub command_history: bool,
    #[serde(default)]
    pub hotbar: bool,
}

/// 实例单独的设置。字段为空 = 这一组跟全局。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct InstanceOverrides {
    pub memory: Option<MemoryOverride>,
    /// 这个实例固定用哪个 Java（Java 的 ID，见 [`crate::java::java_id`]）；
    /// 找不到了（被删、换了电脑）就按全局的规则挑
    pub java_id: Option<String>,
    pub launch: Option<LaunchOverride>,
    pub window: Option<WindowSettings>,
    /// 环境变量：单独设置时整组替换全局的（老的 instance.json 没有这一项，读出来是空 = 跟全局）
    pub env_vars: Option<Vec<EnvVar>>,
    /// 为 true 时该实例脱离本机跨实例同步，保持完全独立
    #[serde(default)]
    pub sync_disabled: bool,
    /// 实例同步细粒度排除（在参与同步时单独排除部分资产）
    #[serde(default)]
    pub sync_exclusions: InstanceSyncExclusions,
    /// 实例单独的启动钩子（为空表示跟随全局）
    #[serde(default)]
    pub hooks: Option<LaunchHooks>,
    /// 实例单独的自定义游戏窗口标题（为空表示跟随全局；设为 `Some("")` 表示该实例显式关闭自定义标题）
    #[serde(default)]
    pub custom_window_title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MemoryOverride {
    /// 自动（按这台电脑和这个实例的模组数量）
    pub auto: bool,
    /// 手动时给多少（MB）
    pub max_memory_mb: u32,
}

/// 启动参数：单独设置时整组替换全局的（不是叠在全局后面），一眼能看出这个实例到底用了什么
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LaunchOverride {
    pub gc_preset: GcPreset,
    pub jvm_args: String,
    pub game_args: String,
}

impl InstanceOverrides {
    /// 把越界的值拉回合法范围，空的 Java ID 当成没选
    pub fn normalized(mut self) -> Self {
        if let Some(memory) = &mut self.memory {
            memory.max_memory_mb = clamp_memory_mb(memory.max_memory_mb);
        }
        self.java_id = self
            .java_id
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        if let Some(launch) = &mut self.launch {
            launch.jvm_args = clean_args(&launch.jvm_args);
            launch.game_args = clean_args(&launch.game_args);
        }
        self.window = self.window.map(WindowSettings::normalized);
        self.env_vars = self.env_vars.map(|vars| clean_env_vars(&vars));
        self.hooks = self.hooks.map(LaunchHooks::normalized);
        self.custom_window_title = self
            .custom_window_title
            .map(|title| clean_window_title(&title));
        self
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 全局设置 + 实例设置合起来，这一次启动真正用的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// `true` 自动；`false` 用 `max_memory_mb`
    pub memory_auto: bool,
    pub max_memory_mb: u32,
    pub java_id: Option<String>,
    pub gc_preset: GcPreset,
    pub jvm_args: String,
    pub game_args: String,
    pub window: WindowSettings,
    pub env_vars: Vec<EnvVar>,
    pub hooks: LaunchHooks,
    pub custom_window_title: String,
}

/// 合起来：实例填了的用实例的，没填的用全局的。
pub fn resolve(global: &GameSettings, overrides: &InstanceOverrides) -> Resolved {
    let (memory_auto, max_memory_mb) = match overrides.memory {
        Some(memory) => (memory.auto, memory.max_memory_mb),
        None => (global.memory_auto, global.max_memory_mb),
    };
    let (gc_preset, jvm_args, game_args) = match &overrides.launch {
        Some(launch) => (
            launch.gc_preset,
            launch.jvm_args.clone(),
            launch.game_args.clone(),
        ),
        None => (
            global.gc_preset,
            global.jvm_args.clone(),
            global.game_args.clone(),
        ),
    };
    Resolved {
        memory_auto,
        max_memory_mb,
        java_id: overrides.java_id.clone(),
        gc_preset,
        jvm_args,
        game_args,
        window: overrides.window.unwrap_or(global.window),
        env_vars: overrides
            .env_vars
            .clone()
            .unwrap_or_else(|| global.env_vars.clone()),
        hooks: overrides
            .hooks
            .clone()
            .unwrap_or_else(|| global.hooks.clone()),
        custom_window_title: overrides
            .custom_window_title
            .clone()
            .unwrap_or_else(|| global.custom_window_title.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WindowMode;

    #[test]
    fn empty_overrides_follow_global() {
        let global = GameSettings {
            memory_auto: false,
            max_memory_mb: 6144,
            jvm_args: "-Dglobal=1".to_owned(),
            env_vars: vec![EnvVar {
                name: "A".to_owned(),
                value: "1".to_owned(),
            }],
            ..GameSettings::default()
        };
        let resolved = resolve(&global, &InstanceOverrides::default());
        assert!(!resolved.memory_auto);
        assert_eq!(resolved.max_memory_mb, 6144);
        assert_eq!(resolved.jvm_args, "-Dglobal=1");
        assert_eq!(resolved.java_id, None);
        assert_eq!(resolved.window, global.window);
        assert_eq!(resolved.env_vars, global.env_vars);
    }

    #[test]
    fn filled_groups_replace_global() {
        let global = GameSettings {
            jvm_args: "-Dglobal=1".to_owned(),
            game_args: "--demo".to_owned(),
            ..GameSettings::default()
        };
        let overrides = InstanceOverrides {
            memory: Some(MemoryOverride {
                auto: false,
                max_memory_mb: 10240,
            }),
            java_id: Some("abc123".to_owned()),
            launch: Some(LaunchOverride {
                gc_preset: GcPreset::Zgc,
                jvm_args: "-Dmine=1".to_owned(),
                game_args: String::new(),
            }),
            window: Some(WindowSettings {
                mode: WindowMode::Fullscreen,
                ..WindowSettings::default()
            }),
            env_vars: Some(Vec::new()),
            sync_disabled: false,
            sync_exclusions: InstanceSyncExclusions::default(),
            hooks: None,
            custom_window_title: None,
        };
        let resolved = resolve(&global, &overrides);
        assert!(!resolved.memory_auto);
        assert_eq!(resolved.max_memory_mb, 10240);
        assert_eq!(resolved.java_id.as_deref(), Some("abc123"));
        assert_eq!(resolved.gc_preset, GcPreset::Zgc);
        assert_eq!(resolved.jvm_args, "-Dmine=1", "整组替换，不叠全局的");
        assert!(resolved.game_args.is_empty());
        assert_eq!(resolved.window.mode, WindowMode::Fullscreen);
        assert!(resolved.env_vars.is_empty(), "单独设成空，就是不加全局的");
    }

    #[test]
    fn normalizes_values() {
        let overrides = InstanceOverrides {
            memory: Some(MemoryOverride {
                auto: false,
                max_memory_mb: 1,
            }),
            java_id: Some("  ".to_owned()),
            launch: Some(LaunchOverride {
                gc_preset: GcPreset::Auto,
                jvm_args: " -Xss4m\r\n".to_owned(),
                game_args: String::new(),
            }),
            window: None,
            env_vars: None,
            sync_disabled: false,
            sync_exclusions: InstanceSyncExclusions::default(),
            hooks: None,
            custom_window_title: None,
        }
        .normalized();
        assert_eq!(overrides.memory.unwrap().max_memory_mb, 512);
        assert_eq!(overrides.java_id, None);
        assert_eq!(overrides.launch.unwrap().jvm_args, "-Xss4m");
    }
}
