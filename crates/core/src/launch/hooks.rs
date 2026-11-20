//! 启动钩子与自定义游戏窗口标题模板（D26c）。
//!
//! - 启动前钩子非零退出或超时会阻断本次启动，并把命令输出返回给用户排查；
//! - 退出后钩子在游戏结束后执行，失败只记日志不影响退出清理；
//! - 统一通过无黑窗进程执行，并注入只读环境变量 `LUMI_INSTANCE_*` / `LUMI_GAME_*`。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::clean_window_title;
use crate::process::hidden_command;

/// 启动前钩子超时上限（防止挂死阻塞启动）
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct HookContext {
    pub instance_id: String,
    pub instance_name: String,
    pub game_version: String,
    pub loader: Option<String>,
    pub game_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("无法启动钩子命令：{0}")]
    Spawn(#[source] std::io::Error),
    #[error("钩子命令执行超时（超过 30 秒）")]
    Timeout,
    #[error("钩子命令执行失败（退出码 {code:?}）{detail}")]
    Failed { code: Option<i32>, detail: String },
}

/// 渲染自定义游戏窗口标题模板；模板为空时返回 `None`。
/// 支持占位符：`{instance}`、`{version}`、`{loader}`、`{player}`。
pub fn render_window_title(template: &str, ctx: &HookContext, player_name: &str) -> Option<String> {
    let trimmed = template.trim();
    if trimmed.is_empty() {
        return None;
    }
    let loader = ctx.loader.as_deref().unwrap_or("Vanilla");
    let rendered = trimmed
        .replace("{instance}", &ctx.instance_name)
        .replace("{version}", &ctx.game_version)
        .replace("{loader}", loader)
        .replace("{player}", player_name);
    let cleaned = clean_window_title(&rendered);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// 在实例目录中执行一条启动前或退出后钩子命令。命令为空时直接返回 `Ok(())`。
pub async fn run_hook(command: &str, ctx: &HookContext) -> Result<(), HookError> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return Ok(());
    }
    let mut child_cmd = if cfg!(windows) {
        let mut c = hidden_command(Path::new("cmd"));
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = hidden_command(Path::new("sh"));
        c.args(["-c", cmd]);
        c
    };
    child_cmd
        .current_dir(&ctx.game_dir)
        .env("LUMI_INSTANCE_ID", &ctx.instance_id)
        .env("LUMI_INSTANCE_NAME", &ctx.instance_name)
        .env("LUMI_GAME_VERSION", &ctx.game_version)
        .env("LUMI_LOADER", ctx.loader.as_deref().unwrap_or(""))
        .env("LUMI_GAME_DIR", &ctx.game_dir)
        .kill_on_drop(true);

    let output = tokio::time::timeout(HOOK_TIMEOUT, child_cmd.output())
        .await
        .map_err(|_| HookError::Timeout)?
        .map_err(HookError::Spawn)?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snippet = stderr.trim().if_empty_then(stdout.trim());
    let detail = if snippet.is_empty() {
        String::new()
    } else {
        format!("：{}", snippet.chars().take(200).collect::<String>())
    };
    Err(HookError::Failed {
        code: output.status.code(),
        detail,
    })
}

trait StrExt {
    fn if_empty_then<'a>(&'a self, fallback: &'a str) -> &'a str;
}

impl StrExt for str {
    fn if_empty_then<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.is_empty() { fallback } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_window_title_placeholders() {
        let ctx = HookContext {
            instance_id: "survival".into(),
            instance_name: "星夜生存".into(),
            game_version: "1.21.1".into(),
            loader: Some("NeoForge 21.1".into()),
            game_dir: PathBuf::from("."),
        };
        assert_eq!(render_window_title("   ", &ctx, "Steve"), None);
        assert_eq!(
            render_window_title(
                "Lumi · {instance} ({version} {loader}) - {player}",
                &ctx,
                "Steve"
            )
            .as_deref(),
            Some("Lumi · 星夜生存 (1.21.1 NeoForge 21.1) - Steve")
        );
    }

    #[tokio::test]
    async fn runs_hook_with_environment_variables() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = HookContext {
            instance_id: "test-id".into(),
            instance_name: "MyWorld".into(),
            game_version: "1.21.1".into(),
            loader: None,
            game_dir: dir.path().to_path_buf(),
        };
        let cmd = if cfg!(windows) {
            "echo %LUMI_INSTANCE_NAME%> hook_ran.txt"
        } else {
            "echo $LUMI_INSTANCE_NAME > hook_ran.txt"
        };
        run_hook(cmd, &ctx).await.unwrap();
        let content = std::fs::read_to_string(dir.path().join("hook_ran.txt")).unwrap();
        assert!(content.contains("MyWorld"));
    }
}
