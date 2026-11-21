//! 启动前准备：从「实例 ID」到「可以执行的 Java 命令」。
//!
//! 读实例 → 加载器装好了没有（回执检查，缺了自动重装）→ 补全文件（只比大小）→ 选 Java（没有就下载）→ 定内存 →
//! 拼参数（垃圾回收预设、用户自定义的参数也在这一步加上）。
//! 命令行和界面共用这一个流程。

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use reqwest::Client;
use tokio_util::sync::CancellationToken;

use super::args::{LaunchArguments, LaunchSpec, QuickPlay, build_args, validate_version};
use super::jvm::{extra_jvm_args, split_args};
use super::memory::MemoryChoice;
use super::policy::{
    JAVA_OPTION_ENV, LaunchParameterError, validate_custom_game, validate_custom_jvm, validate_env,
};
use super::session_pipe;
use crate::auth::{LaunchAccount, LaunchAuthorizationError};
use crate::config::{DownloadSource, EnvVar, GcPreset, WindowMode, WindowSettings};
use crate::fs::UnsafePath;
use crate::install::processors::LoaderContext;
use crate::install::{
    InstallError, InstallPhase, LoaderProgress, install_files, load_or_fetch_version, loaders,
    neoforge,
};
use crate::instance::{self, InstanceError, LoaderKind, LoaderRef, Resolved};
use crate::java::{self, JavaContext, JavaError, JavaInstall, JavaLocator, JavaRequest};
use crate::meta::Environment;
use crate::net::{DownloadProgress, Downloader};
use crate::paths::GameDir;
use crate::process::hidden_command;
use crate::redact::{MASK, Secret, redact};

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error(transparent)]
    Authorization(#[from] LaunchAuthorizationError),
    #[error(transparent)]
    Parameters(#[from] LaunchParameterError),
    #[error("启动或跟踪 Java 游戏进程失败")]
    Process(#[from] std::io::Error),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error(transparent)]
    Java(#[from] JavaError),
    /// 挂上的别家目录不见了（移动、删除了，或者是拔掉的移动硬盘）
    #[error("找不到挂上的目录：{0}")]
    LinkedRootMissing(String),
}

pub struct PrepareContext<'a> {
    pub game: &'a GameDir,
    pub client: &'a Client,
    pub downloader: &'a Downloader,
    /// 文件从哪下（设置「Minecraft 文件、加载器与 Java」）
    pub source: DownloadSource,
    /// 版本描述从哪拿（设置「Minecraft 元数据」）
    pub meta_source: DownloadSource,
    /// 放临时文件的地方（加载器重装时用），要和游戏目录在同一个盘
    pub temp_root: &'a Path,
    pub cancel: &'a CancellationToken,
    /// 启动用的身份（由联网验过 Java 游玩权的会话派生）
    pub account: &'a LaunchAccount,
    /// 去哪里找 Java、下载的 Java 放哪里
    pub java: &'a JavaLocator,
    pub memory: MemoryChoice,
    /// 垃圾回收预设和用户自定义的参数
    pub options: &'a LaunchOptions,
    pub launcher_name: &'a str,
    pub launcher_version: &'a str,
}

/// 用户在设置里调的启动参数（全局设置和实例单独设置合起来的结果）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    pub gc: GcPreset,
    /// 自定义 JVM 参数（用户输入的原文，这里按空白和引号拆成数组）
    pub jvm_args: String,
    /// 自定义游戏参数（同上）
    pub game_args: String,
    /// 实例固定用的 Java（ID）；找不到就按通常的规则挑
    pub java_id: Option<String>,
    pub window: WindowSettings,
    /// 启动时额外加的环境变量
    pub env_vars: Vec<EnvVar>,
    /// 实例首次启动时默认写入 `options.txt` 的语言代码（如 `zh_cn`）；`None` 表示不预设
    pub default_language: Option<String>,
}

impl LaunchOptions {
    pub fn from_resolved(resolved: &Resolved) -> Self {
        Self {
            gc: resolved.gc_preset,
            jvm_args: resolved.jvm_args.clone(),
            game_args: resolved.game_args.clone(),
            java_id: resolved.java_id.clone(),
            window: resolved.window,
            env_vars: resolved.env_vars.clone(),
            default_language: None,
        }
    }
}

impl MemoryChoice {
    pub fn from_resolved(resolved: &Resolved) -> Self {
        if resolved.memory_auto {
            MemoryChoice::Auto
        } else {
            MemoryChoice::Manual(resolved.max_memory_mb)
        }
    }
}

/// 窗口设置 → 启动参数里的宽高、要不要加 `--fullscreen`（用户自己在游戏参数里写了就不重复加）
fn window_args(window: &WindowSettings, extra_game: &mut Vec<String>) -> Option<(u32, u32)> {
    match window.mode {
        WindowMode::Default | WindowMode::Maximized => None,
        WindowMode::Custom => Some((window.width, window.height)),
        WindowMode::Fullscreen => {
            if !extra_game.iter().any(|a| a == "--fullscreen") {
                extra_game.push("--fullscreen".to_owned());
            }
            None
        }
    }
}

#[derive(Debug, Clone)]
pub enum PrepareProgress {
    Loader(LoaderProgress),
    Files(InstallPhase, DownloadProgress),
    /// 电脑上没有合适的 Java，正在下载
    Java(DownloadProgress),
}

#[derive(Debug, Clone)]
enum PreparedCommand {
    Game(LaunchArguments),
    #[cfg(test)]
    Fixture(Secret<Vec<String>>),
}

pub(super) struct SpawnedGame {
    pub child: tokio::process::Child,
    pub input: Option<Secret<Vec<u8>>>,
    /// 进程退出前保留 Java 入口类文件。
    pub bootstrap: Option<tempfile::TempDir>,
}

/// 准备好的启动命令。
#[derive(Debug, Clone)]
pub struct PreparedLaunch {
    java: PathBuf,
    java_version: String,
    memory_mb: u32,
    command: PreparedCommand,
    game_dir: PathBuf,
    env: Secret<Vec<(String, String)>>,
    account: LaunchAccount,
    default_language: Option<String>,
}

impl PreparedLaunch {
    pub fn java(&self) -> &Path {
        &self.java
    }

    pub fn java_version(&self) -> &str {
        &self.java_version
    }

    pub fn memory_mb(&self) -> u32 {
        self.memory_mb
    }

    pub fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    pub(super) fn spawn(&self, stop: &CancellationToken) -> Result<SpawnedGame, LaunchError> {
        self.account.ensure_current(stop)?;
        std::fs::create_dir_all(&self.game_dir)?;
        if let Some(lang) = self.default_language.as_deref()
            && let Err(error) = ensure_default_game_language(&self.game_dir, lang)
        {
            tracing::warn!(%error, lang, "写入默认游戏语言失败，继续启动");
        }
        let mut command = hidden_command(&self.java);
        let (bootstrap, payload) = match &self.command {
            PreparedCommand::Game(arguments) => {
                let payload = session_pipe::payload(arguments)?;
                let bootstrap = session_pipe::stage()?;
                command
                    .args(session_pipe::jvm_args(arguments, bootstrap.path()))
                    .stdin(Stdio::piped());
                (Some(bootstrap), Some(payload))
            }
            #[cfg(test)]
            PreparedCommand::Fixture(args) => {
                command.args(args.expose()).stdin(Stdio::null());
                (None, None)
            }
        };
        command
            .envs(self.env.expose().iter().map(|(k, v)| (k, v)))
            .current_dir(&self.game_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        for key in JAVA_OPTION_ENV {
            command.env_remove(key);
        }
        let child = self.account.use_for_spawn(stop, || command.spawn())??;
        Ok(SpawnedGame {
            child,
            input: payload,
            bootstrap,
        })
    }

    pub(super) fn redact_output<'a>(&self, text: &'a str) -> Cow<'a, str> {
        // JVM 崩溃和模组日志可能把令牌写在没有字段名的地方。
        let token = self.account.token();
        if !token.is_empty() && text.contains(token) {
            redact(&text.replace(token, MASK)).into_owned().into()
        } else {
            redact(text)
        }
    }

    #[cfg(test)]
    pub(super) fn fixture_game(
        account: LaunchAccount,
        program: PathBuf,
        arguments: LaunchArguments,
        game_dir: PathBuf,
    ) -> Self {
        let mut fixture = Self::fixture(account, program, Vec::new(), game_dir, Vec::new());
        fixture.command = PreparedCommand::Game(arguments);
        fixture
    }

    #[cfg(test)]
    pub(super) fn fixture(
        account: LaunchAccount,
        program: PathBuf,
        args: Vec<String>,
        game_dir: PathBuf,
        env: Vec<(String, String)>,
    ) -> Self {
        Self {
            java: program,
            java_version: "test".to_owned(),
            memory_mb: 1024,
            command: PreparedCommand::Fixture(Secret::new(args)),
            game_dir,
            env: Secret::new(env),
            account,
            default_language: None,
        }
    }
}
/// 准备启动。`world` 是要直接进入的存档文件夹名（Quick Play）。
pub async fn prepare<F>(
    ctx: &PrepareContext<'_>,
    id: &str,
    quick_play: Option<QuickPlay>,
    on_progress: F,
) -> Result<PreparedLaunch, LaunchError>
where
    F: Fn(PrepareProgress) + Send + Sync,
{
    ctx.account.ensure_current(ctx.cancel)?;
    let custom_jvm = split_args(&ctx.options.jvm_args);
    let mut extra_game = split_args(&ctx.options.game_args);
    validate_custom_jvm(&custom_jvm)?;
    validate_custom_game(&extra_game)?;
    validate_env(&ctx.options.env_vars)?;
    let resolution = window_args(&ctx.options.window, &mut extra_game);

    // `id` 是实例 ID；不是实例时当成 versions/ 里的版本号（命令行工具直接启动某个版本）
    let instance = instance::load(ctx.game, id)?;
    let version_id = instance
        .as_ref()
        .map_or_else(|| id.to_owned(), instance::Instance::version_id);
    // 版本、依赖库、资源在哪：挂上的别家版本在别家的 .minecraft 里原地读
    let files = instance
        .as_ref()
        .map_or_else(|| ctx.game.clone(), |i| i.files(ctx.game));
    let linked = instance.as_ref().is_some_and(instance::Instance::is_linked);
    if linked && !files.root().is_dir() {
        return Err(LaunchError::LinkedRootMissing(
            files.root().display().to_string(),
        ));
    }
    // 挂上的版本加载器已经由别家装好了，不再装
    let loader = (!linked)
        .then(|| {
            instance
                .as_ref()
                .and_then(|i| {
                    i.config
                        .loader
                        .clone()
                        .map(|loader| (i.config.game_version.clone(), loader))
                })
                .or_else(|| {
                    // 命令行允许直接启动已知的 NeoForge 版本 ID。
                    let version = neoforge::loader_version_of(&version_id)?;
                    let mc = neoforge::game_version_of(version)?;
                    Some((
                        mc,
                        LoaderRef {
                            kind: LoaderKind::NeoForge,
                            version: version.to_owned(),
                        },
                    ))
                })
        })
        .flatten();
    // 实例的设置（instance.json）在本启动器自己的游戏目录里
    let own_game = ctx.game;
    let ctx = &PrepareContext {
        game: &files,
        ..*ctx
    };

    if let Some((mc, loader)) = &loader {
        loaders::validate(mc, loader)?;
    }
    if let Some((mc, loader)) = &loader
        && (!loader.kind.uses_processor() || !loaders::is_installed(ctx.game, mc, loader))
    {
        let java = if loader.kind.uses_processor() {
            let vanilla = load_or_fetch_version(ctx.game, mc, ctx.client, ctx.meta_source).await?;
            Some(pick_java(ctx, &JavaRequest::of(&vanilla), &on_progress).await?)
        } else {
            None
        };
        let loader_ctx = LoaderContext {
            game: ctx.game,
            client: ctx.client,
            downloader: ctx.downloader,
            source: ctx.source,
            meta_source: ctx.meta_source,
            java: java
                .as_ref()
                .map_or_else(|| Path::new(""), |java| java.executable.as_path()),
            work_dir: ctx.temp_root,
            cancel: ctx.cancel,
        };
        loaders::install(&loader_ctx, mc, loader, |p| {
            on_progress(PrepareProgress::Loader(p))
        })
        .await?;
    }

    let version = load_or_fetch_version(ctx.game, &version_id, ctx.client, ctx.meta_source).await?;
    let env = Environment::current();
    ctx.account.ensure_selected(ctx.cancel)?;
    validate_version(&version, &env, resolution, quick_play.as_ref())?;
    let installed = install_files(
        version,
        ctx.game,
        &env,
        ctx.downloader,
        ctx.source,
        ctx.cancel,
        |phase, p| on_progress(PrepareProgress::Files(phase, p)),
    )
    .await?;
    let java = pick_game_java(ctx, &JavaRequest::of(&installed.version), &on_progress).await?;

    // 实例的游戏目录是实例文件夹本身；直接启动某个版本时用游戏目录根
    let game_dir = match &instance {
        Some(instance) => instance.game_dir(ctx.game)?,
        None => ctx.game.root().to_path_buf(),
    };
    let recommended = instance
        .as_ref()
        .and_then(|i| i.config.recommended_memory_mb);
    let memory_mb = ctx.memory.resolve(&game_dir, recommended);
    let extra_jvm = extra_jvm_args(ctx.options.gc, java.major, &ctx.options.jvm_args);
    ctx.account.ensure_current(ctx.cancel)?;
    let spec = LaunchSpec {
        installed: &installed,
        game: ctx.game,
        game_directory: game_dir.clone(),
        account: ctx.account,
        max_memory_mb: memory_mb,
        launcher_name: ctx.launcher_name,
        launcher_version: ctx.launcher_version,
        resolution,
        quick_play,
        extra_jvm_args: extra_jvm,
        extra_game_args: extra_game,
    };
    let args = build_args(&spec, &env)?;
    if instance.is_some()
        && let Err(error) = instance::touch_last_played(own_game, id)
    {
        tracing::warn!(%error, "记录上次游玩时间失败");
    }
    Ok(PreparedLaunch {
        java: java.executable,
        java_version: java.version,
        memory_mb,
        command: PreparedCommand::Game(args),
        game_dir,
        env: Secret::new(
            ctx.options
                .env_vars
                .iter()
                .map(|v| (v.name.clone(), v.value.clone()))
                .collect(),
        ),
        account: ctx.account.clone(),
        default_language: ctx.options.default_language.clone(),
    })
}

/// 游戏用的 Java：实例固定了一个就用它（找不到了记一行日志、照常挑）；没固定就照常挑
async fn pick_game_java<F>(
    ctx: &PrepareContext<'_>,
    request: &JavaRequest,
    on_progress: &F,
) -> Result<JavaInstall, LaunchError>
where
    F: Fn(PrepareProgress) + Send + Sync,
{
    if let Some(id) = &ctx.options.java_id {
        let locator = ctx.java.clone();
        let installs = match tokio::task::spawn_blocking(move || locator.scan()).await {
            Ok(installs) => installs,
            Err(error) => {
                tracing::warn!(%error, "扫描 Java 的任务异常结束");
                Vec::new()
            }
        };
        match installs.into_iter().find(|j| &j.id() == id) {
            Some(found) => {
                if found.major < request.major {
                    tracing::warn!(
                        chosen = found.major,
                        required = request.major,
                        "实例固定的 Java 比这个版本要求的旧，照用户选的启动"
                    );
                }
                tracing::info!(java = %found.version, "用实例固定的 Java");
                return Ok(found);
            }
            None => tracing::warn!(java_id = %id, "实例固定的 Java 找不到了，改为自动挑选"),
        }
    }
    pick_java(ctx, request, on_progress).await
}

async fn pick_java<F>(
    ctx: &PrepareContext<'_>,
    request: &JavaRequest,
    on_progress: &F,
) -> Result<JavaInstall, LaunchError>
where
    F: Fn(PrepareProgress) + Send + Sync,
{
    let java_ctx = JavaContext {
        locator: ctx.java,
        client: ctx.client,
        downloader: ctx.downloader,
        source: ctx.source,
        cancel: ctx.cancel,
    };
    Ok(java::find_or_install(&java_ctx, request, |p| {
        on_progress(PrepareProgress::Java(p))
    })
    .await?)
}

/// 首次启动时把默认游戏语言写入实例的 `options.txt`。
/// - `options.txt` 不存在：新建并写入 `lang:<lang>\n`；
/// - `options.txt` 已存在且包含 `lang:` 行：绝不覆写，尊重玩家游戏内已选语言；
/// - `options.txt` 已存在但没有 `lang:` 行：在末尾追加 `lang:<lang>\n`。
pub fn ensure_default_game_language(game_dir: &Path, lang: &str) -> std::io::Result<bool> {
    let lang = lang.trim();
    if lang.is_empty()
        || !lang
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Ok(false);
    }
    let path = game_dir.join("options.txt");
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        if content.lines().any(|line| {
            line.trim_start()
                .strip_prefix("lang:")
                .is_some_and(|v| !v.trim().is_empty())
        }) {
            return Ok(false);
        }
        let mut next = content;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&format!("lang:{lang}\n"));
        crate::fs::write_atomic(&path, next.as_bytes()).map_err(std::io::Error::other)?;
        return Ok(true);
    }
    crate::fs::write_atomic(&path, format!("lang:{lang}\n").as_bytes())
        .map_err(std::io::Error::other)?;
    Ok(true)
}

#[cfg(test)]
mod default_language_tests {
    use super::*;

    #[test]
    fn writes_options_txt_for_fresh_instance() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_default_game_language(dir.path(), "zh_cn").unwrap());
        let text = std::fs::read_to_string(dir.path().join("options.txt")).unwrap();
        assert_eq!(text, "lang:zh_cn\n");
    }

    #[test]
    fn preserves_existing_language_in_options_txt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("options.txt");
        std::fs::write(&path, "fov:0.5\nlang:en_us\nsoundCategory_master:0.8\n").unwrap();
        assert!(!ensure_default_game_language(dir.path(), "zh_cn").unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("lang:en_us"));
        assert!(!text.contains("zh_cn"));
    }

    #[test]
    fn appends_language_when_options_txt_has_no_lang_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("options.txt");
        std::fs::write(&path, "fov:0.5\nrenderDistance:12").unwrap();
        assert!(ensure_default_game_language(dir.path(), "zh_cn").unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "fov:0.5\nrenderDistance:12\nlang:zh_cn\n");
    }
}
