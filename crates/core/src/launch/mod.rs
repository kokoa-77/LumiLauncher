//! 启动：参数拼装、启动进程、读日志、跟踪游戏状态。

mod args;
pub mod autocover;
pub mod diagnostics;
pub mod discord;
pub mod gpu;
pub mod hooks;
pub mod jvm;
pub mod log;
pub mod memory;
mod policy;
pub mod preflight;
pub mod prepare;
pub mod record;
pub mod session;
mod session_pipe;

pub use args::QuickPlay;
pub use log::{LogLevel, LogLine, LogParser, decode_line};
pub use memory::MemoryChoice;
pub use policy::LaunchParameterError;
pub use prepare::{
    LaunchError, LaunchOptions, PrepareContext, PrepareProgress, PreparedLaunch, prepare,
};
pub use record::{
    LaunchOutcome, LaunchRecord, QuickPlayRecord, RecordEnd, RecordLog, RecordWriter,
};
pub use session::{GameState, SessionTracker};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 游戏进程发出的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// 进程起来了（进程号，自动封面用它找游戏窗口）
    Spawned(u32),
    Log(LogLine),
    State(GameState),
}

/// 启动游戏并一直跟踪到它退出。每条日志和每次状态变化都通过 `on_event` 告诉调用方。
/// 只有通过准备的正版会话可创建游戏进程。取消后可以强制结束游戏。
pub async fn run_game(
    prepared: &PreparedLaunch,
    stop: &CancellationToken,
    mut on_event: impl FnMut(GameEvent),
) -> Result<GameState, LaunchError> {
    let prepare::SpawnedGame {
        mut child,
        input,
        bootstrap: _bootstrap,
    } = prepared.spawn(stop)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, tx.clone());
    }
    drop(tx);

    // JVM 可能在入口类读取 stdin 前产生大量启动日志，因此先开始排空输出。
    if let Some(input) = input
        && let Err(error) = session_pipe::send(&mut child, &input, stop).await
    {
        let _ = child.kill().await;
        if stop.is_cancelled() {
            let stopped = GameState::Exited { code: None };
            on_event(GameEvent::State(stopped.clone()));
            return Ok(stopped);
        }
        return Err(error.into());
    }
    tracing::info!(pid = child.id(), java = %prepared.java().display(), "游戏进程已启动");
    if let Some(pid) = child.id() {
        on_event(GameEvent::Spawned(pid));
    }
    on_event(GameEvent::State(GameState::Starting));

    let mut parser = LogParser::default();
    let mut tracker = SessionTracker::default();
    let mut killed = false;
    loop {
        let raw = tokio::select! {
            raw = rx.recv() => raw,
            () = stop.cancelled(), if !killed => {
                killed = true;
                tracing::info!("用户要求结束游戏");
                if let Err(error) = child.start_kill() {
                    tracing::warn!(%error, "结束游戏进程失败");
                }
                continue;
            }
        };
        let Some(raw) = raw else { break };
        let raw = prepared.redact_output(&raw);
        if let Some(line) = parser.push_line(&raw) {
            let change = tracker.observe(&line);
            on_event(GameEvent::Log(line));
            if let Some(state) = change {
                tracing::info!(state = state.label(), "游戏状态变化");
                on_event(GameEvent::State(state));
            }
        }
    }

    let status = child.wait().await?;
    let final_state = if killed {
        GameState::Exited {
            code: status.code(),
        }
    } else {
        tracker.on_exit(status.code())
    };
    tracing::info!(code = ?status.code(), state = final_state.label(), "游戏进程已结束");
    on_event(GameEvent::State(final_state.clone()));
    Ok(final_state)
}

fn spawn_reader(
    stream: impl AsyncRead + Unpin + Send + 'static,
    tx: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer).await {
                Ok(0) => break,
                Ok(_) => {
                    if tx.send(decode_line(&buffer)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "读取游戏输出失败");
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::auth::{LaunchAccount, LaunchAuthority, LaunchAuthorizationError};

    fn test_launch(account: LaunchAccount, dir: PathBuf) -> PreparedLaunch {
        PreparedLaunch::fixture(
            account,
            PathBuf::from("nonexistent-java-for-test"),
            vec![],
            dir,
            vec![],
        )
    }

    #[tokio::test]
    async fn revocation_blocks_an_already_prepared_launch_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("game");
        let authority = LaunchAuthority::default();
        let account = LaunchAccount::for_test(&authority, Instant::now() + Duration::from_secs(60));
        let prepared = test_launch(account, dir.clone());
        authority.revoke();
        let result = run_game(&prepared, &CancellationToken::new(), |_| {
            panic!("no game events")
        })
        .await;
        assert!(matches!(
            result,
            Err(LaunchError::Authorization(
                LaunchAuthorizationError::Revoked
            ))
        ));
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn expiry_blocks_an_already_prepared_launch_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("game");
        let account = LaunchAccount::for_test(&LaunchAuthority::default(), Instant::now());
        let prepared = test_launch(account, dir.clone());
        let result = run_game(&prepared, &CancellationToken::new(), |_| {
            panic!("no game events")
        })
        .await;
        assert!(matches!(
            result,
            Err(LaunchError::Authorization(
                LaunchAuthorizationError::Expired
            ))
        ));
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn cancellation_blocks_an_already_prepared_launch_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("game");
        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let prepared = test_launch(account, dir.clone());
        let stop = CancellationToken::new();
        stop.cancel();
        let result = run_game(&prepared, &stop, |_| panic!("no game events")).await;
        assert!(matches!(
            result,
            Err(LaunchError::Authorization(
                LaunchAuthorizationError::Cancelled
            ))
        ));
        assert!(!dir.exists());
    }

    /// 环境变量真的交给了子进程（用 Windows 自带的 cmd 代替 Java，让它把变量打印出来）
    #[tokio::test]
    async fn the_same_verified_account_cannot_start_two_processes() {
        let root = tempfile::tempdir().unwrap();
        let authority = LaunchAuthority::default();
        let account = LaunchAccount::for_test(&authority, Instant::now() + Duration::from_secs(60));
        let first = test_launch(account.clone(), root.path().join("first"));
        let retry_dir = root.path().join("retry");
        let retry = test_launch(account, retry_dir.clone());
        let stop = CancellationToken::new();
        let first_result = run_game(&first, &stop, |_| panic!("test programs do not start")).await;
        assert!(matches!(first_result, Err(LaunchError::Process(_))));
        let retry_result = run_game(&retry, &stop, |_| panic!("test programs do not start")).await;
        assert!(matches!(
            retry_result,
            Err(LaunchError::Authorization(
                LaunchAuthorizationError::Expired
            ))
        ));
        assert!(!retry_dir.exists());
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "需要本机真实 Java 和编译好的模拟入口"]
    async fn passes_game_arguments_without_exposing_the_session_to_process_audit() {
        let game = tempfile::tempdir().unwrap();
        let java = PathBuf::from(std::env::var_os("LUMI_TEST_JAVA").unwrap());
        let classpath = std::env::var("LUMI_TEST_MAIN_CLASSES").unwrap();
        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let mut jvm = vec!["-verbose:class".to_owned(), "-cp".to_owned(), classpath];
        let main = if let Ok(module) = std::env::var("LUMI_TEST_FORGE_MODULE") {
            jvm.extend([
                "-p".to_owned(),
                module,
                "--add-modules".to_owned(),
                "ALL-MODULE-PATH".to_owned(),
            ]);
            "cpw.mods.bootstraplauncher.BootstrapLauncher"
        } else {
            "SessionPipeProbe"
        };
        let mut game_args = [
            "--accessToken",
            "fixture-game-token",
            "--session",
            "token:fixture-game-token:fixture",
            "--gameDir",
            "C:\\验收 游戏\\实例",
            "--quickPlaySingleplayer",
            "雪地 \"试验\"",
            "--empty",
            "",
        ]
        .map(str::to_owned)
        .to_vec();
        game_args.extend(["--long-input".to_owned(), "x".repeat(128 * 1024)]);
        let arguments = args::LaunchArguments {
            jvm,
            main: main.to_owned(),
            game: crate::redact::Secret::new(game_args),
        };
        let prepared = PreparedLaunch::fixture_game(account, java, arguments, game.path().into());
        let mut audit = None;
        let mut lines = Vec::new();
        run_game(&prepared, &CancellationToken::new(), |event| match event {
            GameEvent::Spawned(pid) => {
                let command = format!(
                    concat!(
                        "$p=Get-CimInstance Win32_Process -Filter 'ProcessId={}' -ErrorAction Stop; ",
                        "if(!$p -or !$p.CommandLine){{exit 2}}; ",
                        "if($p.CommandLine.Contains(('fixture-'+'game-token'))){{'exposed'}}else{{'clean'}}"
                    ),
                    pid
                );
                audit = Some(std::process::Command::new(std::env::var_os("LUMI_TEST_PWSH").unwrap())
                    .args(["-NoProfile", "-NonInteractive", "-Command", &command])
                    .output().unwrap());
            }
            GameEvent::Log(line) => lines.push(line.message),
            _ => {}
        }).await.unwrap();
        let audit = audit.unwrap();
        assert!(
            lines.iter().any(|line| line == "PIPE_ARGS_OK"),
            "{}",
            lines
                .iter()
                .rev()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(
            lines.iter().any(|line| line == "JAVA_COMMAND_CLEAN"),
            "JVM 启动命令包含模拟令牌"
        );
        assert!(
            audit.status.success(),
            "Windows 未返回进程命令行: {}",
            String::from_utf8_lossy(&audit.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&audit.stdout).trim(), "clean");
        assert!(lines.iter().any(|line| line == "GAME_CLASSPATH_OK"));
        assert!(lines.iter().any(|line| line.contains("token=<redacted>")));
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("fixture-game-token"))
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn passes_safe_env_vars_to_the_process_and_redacts_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let shell = std::path::PathBuf::from(windows)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        // 明确输出 UTF-8，避免远端 Windows 代码页将中文替换为问号。
        let args = [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            concat!(
                "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); ",
                "[Console]::WriteLine('value=' + $env:MCL_ENV_TEST + '; env=' + ",
                "$env:JAVA_TOOL_OPTIONS + '; token=fixture-game-token')",
            ),
        ]
        .map(str::to_owned)
        .to_vec();
        let env = vec![
            ("MCL_ENV_TEST".to_owned(), "你好 world".to_owned()),
            ("JAVA_TOOL_OPTIONS".to_owned(), "INJECTED".to_owned()),
        ];
        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let prepared = PreparedLaunch::fixture(account, shell, args, dir.path().to_path_buf(), env);
        let mut lines = Vec::new();
        run_game(&prepared, &CancellationToken::new(), |event| {
            if let GameEvent::Log(line) = event {
                lines.push(line.message);
            }
        })
        .await
        .unwrap();
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("INJECTED") || l.contains("fixture-game-token"))
        );
        assert!(lines.iter().any(|l| l.contains("token=<redacted>")));
        assert!(
            lines
                .iter()
                .any(|l| l == "value=你好 world; env=; token=<redacted>"),
            "子进程看到的输出：{lines:?}"
        );
    }
}
