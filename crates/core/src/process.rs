//! 子进程的公共设置（启动游戏、运行安装处理器都用它）。

use std::path::Path;

use tokio::process::Command;

/// Windows：启动子进程时不弹出黑色控制台窗口
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 创建一个不弹窗口的子进程命令。参数一律用 `.arg()` / `.args()` 按数组传，绝不拼成一条字符串。
pub(crate) fn hidden_command(program: &Path) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
