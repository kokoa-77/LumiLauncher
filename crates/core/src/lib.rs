//! 启动器核心库。
//!
//! 分层原则（详见 `ARCHITECTURE.md`）：
//! - 不依赖 Tauri，任何功能都能在单元测试和命令行工具里跑通；
//! - 模块从底到顶：`fs` / `config` ← `net` ← `meta` ← `java` / `auth`
//!   ← `install` / `content` / `modpack` ← `launch`，下层不许引用上层。

pub mod auth;
pub mod capture;
pub mod config;
pub mod content;
pub mod external;
pub mod fs;
pub mod install;
pub mod instance;
pub mod java;
pub mod jobs;
pub mod launch;
pub mod meta;
pub mod modpack;
pub mod net;
pub mod paths;
mod process;
pub mod redact;
pub mod saves;
pub mod shortcut;
pub mod storage;
pub mod sync;

/// 当前毫秒级 Unix 时间戳。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
