//! 启动参数拼装（纯函数，方便测试）。
//!
//! 顺序：内存 → 编码 → version json 的 JVM 参数（或旧版本的默认参数）→ 日志配置 →
//! 用户自定义 JVM 参数 → 主类 → 游戏参数（新格式或旧格式）→ 用户自定义游戏参数。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::policy::{
    LaunchParameterError, validate_custom_game, validate_custom_jvm, validate_version_game,
    validate_version_jvm,
};
use crate::auth::LaunchAccount;
use crate::install::InstalledVersion;
use crate::meta::{ArgValue, Argument, Environment, VersionJson, rules_allow};
use crate::paths::GameDir;
use crate::redact::Secret;

/// 拼参数需要的全部输入。
#[derive(Debug, Clone)]
pub(super) struct LaunchSpec<'a> {
    pub installed: &'a InstalledVersion,
    pub game: &'a GameDir,
    /// 游戏工作目录：不隔离时是游戏根目录，版本隔离时是 `versions/<版本号>`
    pub game_directory: PathBuf,
    /// 通过官方在线核验的游戏身份
    pub account: &'a LaunchAccount,
    pub max_memory_mb: u32,
    pub launcher_name: &'a str,
    pub launcher_version: &'a str,
    /// 自定义窗口大小，`None` 用游戏默认
    pub resolution: Option<(u32, u32)>,
    /// 启动后直接进到哪里（Quick Play，1.20 起支持）：单人存档或者多人服务器
    pub quick_play: Option<QuickPlay>,
    pub extra_jvm_args: Vec<String>,
    pub extra_game_args: Vec<String>,
}

/// 启动后直接进到哪里（Quick Play，1.20 起支持）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickPlay {
    /// 单人存档（存档文件夹名）
    World(String),
    /// 多人服务器（地址，可以带端口）
    Server(String),
}

impl QuickPlay {
    fn world(&self) -> Option<&str> {
        match self {
            QuickPlay::World(folder) => Some(folder),
            QuickPlay::Server(_) => None,
        }
    }

    fn server(&self) -> Option<&str> {
        match self {
            QuickPlay::Server(address) => Some(address),
            QuickPlay::World(_) => None,
        }
    }
}

/// JVM 选项与游戏入口参数分开。只有 JVM 选项进入进程命令行。
#[derive(Debug, Clone)]
pub(super) struct LaunchArguments {
    pub jvm: Vec<String>,
    pub main: String,
    pub game: Secret<Vec<String>>,
}

pub(super) fn build_args(
    spec: &LaunchSpec<'_>,
    base_env: &Environment,
) -> Result<LaunchArguments, LaunchParameterError> {
    let version = &spec.installed.version;
    validate_custom_game(&spec.extra_game_args)?;
    validate_custom_jvm(&spec.extra_jvm_args)?;
    validate_version(version, base_env, spec.resolution, spec.quick_play.as_ref())?;
    let env = launch_environment(base_env, spec.resolution, spec.quick_play.as_ref());
    let values = placeholder_values(spec);

    let mut args = vec![format!("-Xmx{}M", spec.max_memory_mb)];
    // 让游戏日志用 UTF-8 输出，避免中文 Windows 上乱码（Java 18 以前会忽略这几个参数）
    args.extend(
        [
            "-Dfile.encoding=UTF-8",
            "-Dstdout.encoding=UTF-8",
            "-Dstderr.encoding=UTF-8",
        ]
        .map(str::to_owned),
    );

    match version.arguments.as_ref().filter(|a| !a.jvm.is_empty()) {
        Some(arguments) => {
            let jar_name = spec
                .installed
                .client_jar
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            args.extend(
                expand(&arguments.jvm, &env, &values)
                    .into_iter()
                    .map(|arg| ensure_jar_ignored(arg, &jar_name)),
            );
        }
        None => {
            // 1.12.2 及以前的版本没有 JVM 参数列表，用官方启动器的默认值
            for token in [
                "-Djava.library.path=${natives_directory}",
                "-cp",
                "${classpath}",
            ] {
                args.push(substitute(token, &values));
            }
        }
    }

    if let (Some(logging), Some(path)) = (
        version.logging.as_ref().and_then(|l| l.client.as_ref()),
        spec.installed.log_config.as_ref(),
    ) {
        let mut log_values = values.clone();
        log_values.insert("path", path.to_string_lossy().into_owned());
        args.push(substitute(&logging.argument, &log_values));
    }

    args.extend(spec.extra_jvm_args.iter().cloned());
    let token = spec.account.token();
    if token.is_empty() || args.iter().any(|arg| arg.contains(token)) {
        return Err(LaunchParameterError::Version);
    }

    let mut game = Vec::new();
    for raw in version_game_args(version, &env) {
        let expanded = substitute(raw, &values);
        if super::policy::protected_game_arg(&expanded) && !super::policy::protected_game_arg(raw) {
            return Err(LaunchParameterError::Version);
        }
        game.push(expanded);
    }
    let legacy = version.arguments.as_ref().is_none_or(|a| a.game.is_empty());
    if legacy && let Some((width, height)) = spec.resolution {
        game.extend([
            "--width".into(),
            width.to_string(),
            "--height".into(),
            height.to_string(),
        ]);
    }

    game.extend(spec.extra_game_args.iter().cloned());
    Ok(LaunchArguments {
        jvm: args,
        main: version.main_class.clone().unwrap_or_default(),
        game: Secret::new(game),
    })
}

/// 在安装文件前检查实际启用的版本参数，防止挂入版本带入自定义认证配置。
pub(super) fn validate_version(
    version: &VersionJson,
    base_env: &Environment,
    resolution: Option<(u32, u32)>,
    quick_play: Option<&QuickPlay>,
) -> Result<(), LaunchParameterError> {
    let valid_main_class = version.main_class.as_deref().is_some_and(|name| {
        name.split('.').all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        })
    });
    if !valid_main_class {
        return Err(LaunchParameterError::Version);
    }
    let env = launch_environment(base_env, resolution, quick_play);
    if let Some(arguments) = &version.arguments {
        validate_version_jvm(active_args(&arguments.jvm, &env).into_iter())?;
    }
    if let Some(logging) = version.logging.as_ref().and_then(|l| l.client.as_ref()) {
        validate_version_jvm(std::iter::once(logging.argument.as_str()))?;
    }
    validate_version_game(&version_game_args(version, &env))
}

fn version_game_args<'a>(version: &'a VersionJson, env: &Environment) -> Vec<&'a str> {
    match version.arguments.as_ref().filter(|a| !a.game.is_empty()) {
        Some(arguments) => active_args(&arguments.game, env),
        None => version
            .minecraft_arguments
            .as_deref()
            .map(|legacy| legacy.split_whitespace().collect())
            .unwrap_or_default(),
    }
}

/// NeoForge / 新版 Forge 用 `-DignoreList=` 让引导器跳过原版 jar（按文件名匹配）。
/// 它写的是 `${version_name}.jar`，前提是原版 jar 被复制成了 `versions/<加载器版本>/<加载器版本>.jar`；
/// 我们直接用 `versions/1.21.1/1.21.1.jar`，不复制，所以要把真实文件名补进去，否则原版类会被重复加载。
fn ensure_jar_ignored(arg: String, jar_name: &str) -> String {
    match arg.strip_prefix("-DignoreList=") {
        Some(list) if !jar_name.is_empty() && !list.split(',').any(|item| item == jar_name) => {
            format!("{arg},{jar_name}")
        }
        _ => arg,
    }
}

/// 规则判定用的环境：在系统信息上加上启动器开启的功能。
fn launch_environment(
    base: &Environment,
    resolution: Option<(u32, u32)>,
    quick_play: Option<&QuickPlay>,
) -> Environment {
    base.clone()
        .with_feature("is_demo_user", false)
        .with_feature("has_custom_resolution", resolution.is_some())
        .with_feature("has_quick_plays_support", false)
        .with_feature(
            "is_quick_play_singleplayer",
            quick_play.and_then(QuickPlay::world).is_some(),
        )
        .with_feature(
            "is_quick_play_multiplayer",
            quick_play.and_then(QuickPlay::server).is_some(),
        )
        .with_feature("is_quick_play_realms", false)
}

fn expand(
    arguments: &[Argument],
    env: &Environment,
    values: &HashMap<&str, String>,
) -> Vec<String> {
    active_args(arguments, env)
        .into_iter()
        .map(|token| substitute(token, values))
        .collect()
}

fn active_args<'a>(arguments: &'a [Argument], env: &Environment) -> Vec<&'a str> {
    let mut out = Vec::new();
    for argument in arguments {
        match argument {
            Argument::Plain(token) => out.push(token.as_str()),
            Argument::Conditional { rules, value } => {
                if rules_allow(Some(rules), env) {
                    out.extend(value_tokens(value).map(String::as_str));
                }
            }
        }
    }
    out
}

fn value_tokens(value: &ArgValue) -> impl Iterator<Item = &String> {
    value.values().iter()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn placeholder_values(spec: &LaunchSpec<'_>) -> HashMap<&'static str, String> {
    let installed = spec.installed;
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = installed
        .classpath
        .iter()
        .map(|p| path_string(p))
        .collect::<Vec<_>>()
        .join(separator);
    let uuid = spec.account.uuid().simple().to_string();
    let assets_root = path_string(&spec.game.assets());
    let game_assets = installed
        .legacy_assets_dir
        .as_ref()
        .map_or_else(|| assets_root.clone(), |p| path_string(p));
    let (width, height) = spec.resolution.unwrap_or((854, 480));

    HashMap::from([
        ("auth_player_name", spec.account.name().to_owned()),
        ("auth_uuid", uuid.clone()),
        ("auth_access_token", spec.account.token().to_owned()),
        (
            "auth_session",
            format!("token:{}:{uuid}", spec.account.token()),
        ),
        ("auth_xuid", "0".to_owned()),
        ("clientid", String::new()),
        ("user_type", "msa".to_owned()),
        ("user_properties", "{}".to_owned()),
        ("profile_properties", "{}".to_owned()),
        ("version_name", installed.version.id.clone()),
        ("version_type", spec.launcher_name.to_owned()),
        ("game_directory", path_string(&spec.game_directory)),
        ("assets_root", assets_root),
        (
            "assets_index_name",
            installed.asset_index_id.clone().unwrap_or_default(),
        ),
        ("game_assets", game_assets),
        ("natives_directory", path_string(&installed.natives_dir)),
        ("library_directory", path_string(&spec.game.libraries())),
        ("classpath_separator", separator.to_owned()),
        ("classpath", classpath),
        ("primary_jar", path_string(&installed.client_jar)),
        ("launcher_name", spec.launcher_name.to_owned()),
        ("launcher_version", spec.launcher_version.to_owned()),
        ("resolution_width", width.to_string()),
        ("resolution_height", height.to_string()),
        ("quickPlayPath", String::new()),
        (
            "quickPlaySingleplayer",
            spec.quick_play
                .as_ref()
                .and_then(QuickPlay::world)
                .unwrap_or_default()
                .to_owned(),
        ),
        (
            "quickPlayMultiplayer",
            spec.quick_play
                .as_ref()
                .and_then(QuickPlay::server)
                .unwrap_or_default()
                .to_owned(),
        ),
        ("quickPlayRealms", String::new()),
    ])
}

/// 把 `${名字}` 替换成对应的值；不认识的占位符原样保留并记日志。
fn substitute(token: &str, values: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match values.get(key) {
                    Some(value) => out.push_str(value),
                    None => {
                        tracing::warn!(placeholder = key, "启动参数里有不认识的占位符");
                        out.push_str(&rest[start..start + 2 + end + 1]);
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use std::time::{Duration, Instant};

    use crate::auth::LaunchAuthority;
    use crate::meta::VersionJson;

    fn windows() -> Environment {
        Environment {
            os_name: "windows".into(),
            os_arch: "x86_64".into(),
            os_version: "10.0".into(),
            features: BTreeMap::new(),
        }
    }

    fn installed() -> InstalledVersion {
        let version =
            VersionJson::parse("1.21.1", include_bytes!("../../tests/fixtures/1.21.1.json"))
                .unwrap();
        InstalledVersion {
            asset_index_id: Some("17".into()),
            version,
            classpath: vec![
                PathBuf::from("D:/mc/libraries/a.jar"),
                PathBuf::from("D:/mc/versions/1.21.1/1.21.1.jar"),
            ],
            client_jar: PathBuf::from("D:/mc/versions/1.21.1/1.21.1.jar"),
            natives_dir: PathBuf::from("D:/mc/versions/1.21.1/natives-windows-x86_64"),
            legacy_assets_dir: None,
            map_to_resources: false,
            log_config: Some(PathBuf::from("D:/mc/assets/log_configs/client-1.12.xml")),
        }
    }

    fn build(world: Option<&str>, resolution: Option<(u32, u32)>) -> Vec<String> {
        build_quick(world.map(|w| QuickPlay::World(w.to_owned())), resolution)
    }

    fn build_quick(quick_play: Option<QuickPlay>, resolution: Option<(u32, u32)>) -> Vec<String> {
        let installed = installed();
        let game = GameDir::new("D:/mc");
        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let spec = LaunchSpec {
            installed: &installed,
            game: &game,
            game_directory: PathBuf::from("D:/mc"),
            account: &account,
            max_memory_mb: 2048,
            launcher_name: "Lumi",
            launcher_version: "0.1.0",
            resolution,
            quick_play,
            extra_jvm_args: vec![],
            extra_game_args: vec![],
        };
        let LaunchArguments {
            mut jvm,
            main,
            game,
        } = build_args(&spec, &windows()).unwrap();
        jvm.push(main);
        jvm.extend(game.expose().iter().cloned());
        jvm
    }

    fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }

    #[test]
    fn real_1_21_1_arguments_are_complete() {
        let args = build(None, None);
        let unresolved: Vec<&String> = args.iter().filter(|a| a.contains("${")).collect();
        assert!(
            unresolved.is_empty(),
            "不能留下没替换的占位符：{unresolved:?}"
        );

        assert_eq!(args[0], "-Xmx2048M");
        let main = args
            .iter()
            .position(|a| a == "net.minecraft.client.main.Main")
            .expect("要有主类");
        let cp = args.iter().position(|a| a == "-cp").expect("要有 -cp");
        assert!(cp < main, "classpath 必须在主类之前");
        assert!(args[cp + 1].contains("a.jar;") && args[cp + 1].ends_with("1.21.1.jar"));

        assert_eq!(value_after(&args, "--username"), Some("Steve"));
        assert_eq!(value_after(&args, "--version"), Some("1.21.1"));
        assert_eq!(value_after(&args, "--assetIndex"), Some("17"));
        assert_eq!(
            value_after(&args, "--uuid"),
            Some("00000000000000000000000000000007")
        );
        assert_eq!(value_after(&args, "--userType"), Some("msa"));
        assert!(
            args.iter()
                .any(|a| a == "-Dlog4j.configurationFile=D:/mc/assets/log_configs/client-1.12.xml")
        );
        assert!(
            args.iter()
                .any(|a| a.starts_with("-Djava.library.path=")
                    && a.contains("natives-windows-x86_64"))
        );
    }

    #[test]
    fn rejects_custom_duplicate_identity_and_replacement_jvm() {
        let installed = installed();
        let game = GameDir::new("D:/mc");
        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let mut spec = LaunchSpec {
            installed: &installed,
            game: &game,
            game_directory: PathBuf::from("D:/mc"),
            account: &account,
            max_memory_mb: 2048,
            launcher_name: "Lumi",
            launcher_version: "0.1.0",
            resolution: None,
            quick_play: None,
            extra_jvm_args: vec![],
            extra_game_args: vec!["--username=Other".to_owned()],
        };
        assert_eq!(
            build_args(&spec, &windows()).unwrap_err(),
            LaunchParameterError::CustomGame
        );
        spec.extra_game_args.clear();
        spec.extra_jvm_args
            .push("-javaagent:alternate.jar".to_owned());
        assert_eq!(
            build_args(&spec, &windows()).unwrap_err(),
            LaunchParameterError::CustomJvm
        );
    }

    #[test]
    fn actual_game_args_use_the_verified_token() {
        let args = build(None, None);
        assert_eq!(
            value_after(&args, "--accessToken"),
            Some("fixture-game-token")
        );
        assert_eq!(value_after(&args, "--userType"), Some("msa"));
    }
    #[test]
    fn older_game_uses_a_legacy_session_derived_from_the_verified_token() {
        let mut installed = installed();
        installed.version.arguments = None;
        installed.version.minecraft_arguments = Some(
            "--username ${auth_player_name} --session ${auth_session} --version ${version_name}"
                .into(),
        );
        let game = GameDir::new("D:/mc");
        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let spec = LaunchSpec {
            installed: &installed,
            game: &game,
            game_directory: game.root().into(),
            account: &account,
            max_memory_mb: 2048,
            launcher_name: "Lumi",
            launcher_version: "0.1.0",
            resolution: None,
            quick_play: None,
            extra_jvm_args: vec![],
            extra_game_args: vec![],
        };
        let launch = build_args(&spec, &windows()).unwrap();
        let args = launch.game.expose();
        assert_eq!(value_after(args, "--username"), Some("Steve"));
        assert_eq!(
            value_after(args, "--session"),
            Some("token:fixture-game-token:00000000000000000000000000000007"),
        );
    }
    #[test]
    fn os_specific_and_optional_arguments_are_filtered() {
        let args = build(None, None);
        assert!(
            !args.iter().any(|a| a == "-XstartOnFirstThread"),
            "这是 macOS 专用的"
        );
        assert!(
            args.iter().any(|a| a.starts_with("-XX:HeapDumpPath=")),
            "这是 Windows 专用的，应该有"
        );
        assert!(!args.iter().any(|a| a == "--demo"));
        assert!(!args.iter().any(|a| a == "--width"), "没设窗口大小就不加");
        assert!(!args.iter().any(|a| a.starts_with("--quickPlay")));
    }

    #[test]
    fn quick_play_and_resolution_are_added_when_requested() {
        let args = build(Some("新的世界"), Some((1280, 720)));
        assert_eq!(
            value_after(&args, "--quickPlaySingleplayer"),
            Some("新的世界")
        );
        assert_eq!(value_after(&args, "--width"), Some("1280"));
        assert_eq!(value_after(&args, "--height"), Some("720"));
    }

    #[test]
    fn quick_play_can_join_a_server_instead() {
        let args = build_quick(
            Some(QuickPlay::Server("play.example.com:25566".into())),
            None,
        );
        assert_eq!(
            value_after(&args, "--quickPlayMultiplayer"),
            Some("play.example.com:25566")
        );
        assert!(!args.iter().any(|a| a == "--quickPlaySingleplayer"));
        // 不直达时两个都不带
        let plain = build(None, None);
        assert!(!plain.iter().any(|a| a.starts_with("--quickPlay")));
    }

    #[test]
    fn neoforge_ignore_list_gets_the_real_client_jar_name() {
        let arg = "-DignoreList=client-extra,neoforge-21.1.265.jar".to_owned();
        assert_eq!(
            ensure_jar_ignored(arg, "1.21.1.jar"),
            "-DignoreList=client-extra,neoforge-21.1.265.jar,1.21.1.jar"
        );
        let already = "-DignoreList=client-extra,1.21.1.jar".to_owned();
        assert_eq!(ensure_jar_ignored(already.clone(), "1.21.1.jar"), already);
        assert_eq!(
            ensure_jar_ignored("-Xss2M".to_owned(), "1.21.1.jar"),
            "-Xss2M"
        );
    }

    #[test]
    fn substitutes_multiple_and_unknown_placeholders() {
        let values = HashMap::from([("a", "1".to_owned()), ("b", "2".to_owned())]);
        assert_eq!(substitute("x${a}-${b}y", &values), "x1-2y");
        assert_eq!(substitute("${unknown}", &values), "${unknown}");
        assert_eq!(substitute("broken ${a", &values), "broken ${a");
        assert_eq!(substitute("plain", &values), "plain");
    }
}
