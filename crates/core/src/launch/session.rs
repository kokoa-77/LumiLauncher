//! 游戏会话状态机：从日志判断游戏现在在干什么。
//!
//! 「直达你的世界」「永不玩坏」「冲突模组自动定位」「启动胶囊」都靠它。
//! 判断依据是各版本都稳定出现的日志句子；以后遇到新版本改了措辞，只改这里的关键字。

use serde::Serialize;

use super::log::LogLine;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GameState {
    /// 进程已启动，窗口还没出来
    Starting,
    /// 窗口已出来，正在加载资源
    Loading,
    /// 主菜单
    MainMenu,
    /// 正在连接服务器（还没确认进去了：连不上、被拒绝都会回到主菜单）
    Connecting,
    /// 已进入世界（单人或多人）
    InWorld,
    /// 正常退出
    Exited { code: Option<i32> },
    /// 崩溃退出
    Crashed {
        code: Option<i32>,
        crash_report: Option<String>,
    },
}

impl GameState {
    /// 给用户看的中文名
    pub fn label(&self) -> &'static str {
        match self {
            GameState::Starting => "启动中",
            GameState::Loading => "加载中",
            GameState::MainMenu => "主菜单",
            GameState::Connecting => "正在连接服务器",
            GameState::InWorld => "已进入世界",
            GameState::Exited { .. } => "已退出",
            GameState::Crashed { .. } => "已崩溃",
        }
    }
}

const LOADING_MARKERS: &[&str] = &["Setting user:", "Backend library:", "LWJGL Version"];
const MENU_MARKERS: &[&str] = &["Sound engine started"];
/// 注意：只能用不随游戏语言翻译的句子。「xxx joined the game」在中文界面下会变成「xxx加入了游戏」，
/// 服务器线程的「logged in with entity id」不翻译（实测），单人世界看它。
const IN_WORLD_MARKERS: &[&str] = &["logged in with entity id"];
/// 多人游戏（收紧，外部审阅指出）：客户端「Connecting to 地址, 端口」只说明开始连了，连不上也会打这一句，
/// 不能当成进了世界。原版客户端在服务器里没有「进去了」的专门日志，但收到的每条聊天消息都会记成
/// `[System] [CHAT] …`（不翻译，1.21.1 客户端 ChatComponent），能收到聊天就是真进去了。
const CONNECTING_MARKERS: &[&str] = &["Connecting to "];
const SERVER_JOINED_MARKERS: &[&str] = &["[CHAT]"];
/// 连不上（ConnectScreen）、被踢或断开（ClientCommonPacketListenerImpl）：回到主菜单
const SERVER_LEFT_MARKERS: &[&str] = &[
    "Couldn't connect to server",
    "Client disconnected with reason",
];
const LEFT_WORLD_MARKERS: &[&str] = &["Stopping server", "Stopping singleplayer server"];
const CRASH_MARKERS: &[&str] = &["---- Minecraft Crash Report ----", "#@!@# Game crashed!"];
const CRASH_REPORT_PREFIXES: &[&str] = &[
    "This crash report has been saved to:",
    "Crash report saved to:",
    "#@!@# Game crashed! Crash report saved to:",
];

#[derive(Debug)]
pub struct SessionTracker {
    state: GameState,
    crashed: bool,
    crash_report: Option<String>,
}

impl Default for SessionTracker {
    fn default() -> Self {
        Self {
            state: GameState::Starting,
            crashed: false,
            crash_report: None,
        }
    }
}

impl SessionTracker {
    pub fn state(&self) -> &GameState {
        &self.state
    }

    /// 看一条日志；状态变化时返回新状态。
    pub fn observe(&mut self, line: &LogLine) -> Option<GameState> {
        let message = line.message.as_str();
        if CRASH_MARKERS.iter().any(|m| message.contains(m)) {
            self.crashed = true;
        }
        if let Some(path) = CRASH_REPORT_PREFIXES.iter().find_map(|p| {
            // 游戏在路径前面加了 `#@!@#` 标记，要去掉（实测）
            message
                .split_once(p)
                .map(|(_, rest)| rest.trim().trim_start_matches("#@!@#").trim().to_owned())
        }) {
            self.crashed = true;
            self.crash_report = Some(path);
        }

        let next = match self.state {
            GameState::Starting if contains_any(message, LOADING_MARKERS) => {
                Some(GameState::Loading)
            }
            GameState::Starting | GameState::Loading if contains_any(message, MENU_MARKERS) => {
                Some(GameState::MainMenu)
            }
            GameState::Starting
            | GameState::Loading
            | GameState::MainMenu
            | GameState::Connecting
                if contains_any(message, IN_WORLD_MARKERS) =>
            {
                Some(GameState::InWorld)
            }
            GameState::Starting | GameState::Loading | GameState::MainMenu
                if contains_any(message, CONNECTING_MARKERS) =>
            {
                Some(GameState::Connecting)
            }
            GameState::Connecting if contains_any(message, SERVER_JOINED_MARKERS) => {
                Some(GameState::InWorld)
            }
            GameState::Connecting if contains_any(message, SERVER_LEFT_MARKERS) => {
                Some(GameState::MainMenu)
            }
            GameState::InWorld
                if contains_any(message, LEFT_WORLD_MARKERS)
                    || contains_any(message, SERVER_LEFT_MARKERS) =>
            {
                Some(GameState::MainMenu)
            }
            _ => None,
        };
        if let Some(state) = &next {
            self.state = state.clone();
        }
        next
    }

    /// 进程退出时调用，返回最终状态。
    pub fn on_exit(&mut self, code: Option<i32>) -> GameState {
        let crashed = self.crashed || code.is_some_and(|c| c != 0) || code.is_none();
        self.state = if crashed {
            GameState::Crashed {
                code,
                crash_report: self.crash_report.clone(),
            }
        } else {
            GameState::Exited { code }
        };
        self.state.clone()
    }
}

fn contains_any(message: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| message.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::log::LogLevel;

    fn line(message: &str) -> LogLine {
        LogLine {
            level: LogLevel::Info,
            thread: None,
            message: message.to_owned(),
        }
    }

    #[test]
    fn follows_a_normal_session() {
        let mut tracker = SessionTracker::default();
        assert_eq!(
            tracker.observe(&line("Setting user: Steve")),
            Some(GameState::Loading)
        );
        assert_eq!(
            tracker.observe(&line("Backend library: LWJGL version 3.3.3")),
            None
        );
        assert_eq!(
            tracker.observe(&line("Sound engine started")),
            Some(GameState::MainMenu)
        );
        assert_eq!(
            tracker.observe(&line(
                "Steve[local:E:4588e335] logged in with entity id 15 at (41.5, 64.0, -13.5)"
            )),
            Some(GameState::InWorld)
        );
        assert_eq!(
            tracker.observe(&line("Stopping server")),
            Some(GameState::MainMenu)
        );
        assert_eq!(
            tracker.on_exit(Some(0)),
            GameState::Exited { code: Some(0) }
        );
    }

    /// 真实验收时的日志（游戏语言切成了简体中文）：进世界的提示被翻译了，状态机不能依赖它
    #[test]
    fn works_when_the_game_language_is_chinese() {
        let real_log = [
            "Setting user: Player",
            "Backend library: LWJGL version 3.3.3+5",
            "OpenAL initialized on device OpenAL Soft on 扬声器 (Realtek(R) Audio)",
            "Sound engine started",
            "Reloading ResourceManager: vanilla",
            "Sound engine started",
            "Starting integrated minecraft server version 1.21.1",
            "准备生成区域中：51%",
            "Player[local:E:4588e335] logged in with entity id 15 at (41.5, 64.0, -13.5)",
            "Player加入了游戏",
            "Saving and pausing game...",
            "Player lost connection: 连接中断",
            "Player退出了游戏",
            "Stopping singleplayer server as player logged out",
            "Stopping server",
            "Stopping!",
        ];
        let mut tracker = SessionTracker::default();
        let changes: Vec<GameState> = real_log
            .iter()
            .filter_map(|m| tracker.observe(&line(m)))
            .collect();
        assert_eq!(
            changes,
            [
                GameState::Loading,
                GameState::MainMenu,
                GameState::InWorld,
                GameState::MainMenu
            ]
        );
        assert_eq!(
            tracker.on_exit(Some(0)),
            GameState::Exited { code: Some(0) }
        );
    }

    /// 2026-11-12 真实日志（验收测试实例点「加入」Vanilla Valley SMP）：服务器连不上，又回到了标题画面。
    /// 以前一看到「Connecting to」就算进了世界，正常退出后会被记成「能玩的状态」
    #[test]
    fn a_failed_server_connection_is_not_in_world() {
        let real_log = [
            "Sound engine started",
            "Connecting to vanillavale.duckdns.org, 25565",
            "Unable to start LAN server detection: No route to host: setsockopt",
            "Couldn't connect to server",
            "Can't ping vanillavale.duckdns.org: Internal Exception: java.net.SocketException",
            "Creating pipeline for dimension minecraft:overworld",
            "Stopping!",
        ];
        let mut tracker = SessionTracker::default();
        let changes: Vec<GameState> = real_log
            .iter()
            .filter_map(|m| tracker.observe(&line(m)))
            .collect();
        assert_eq!(
            changes,
            [
                GameState::MainMenu,
                GameState::Connecting,
                GameState::MainMenu
            ]
        );
        assert!(!changes.contains(&GameState::InWorld));
    }

    #[test]
    fn a_server_counts_once_chat_arrives() {
        let mut tracker = SessionTracker::default();
        tracker.observe(&line("Sound engine started"));
        assert_eq!(
            tracker.observe(&line("Connecting to play.example.com, 25565")),
            Some(GameState::Connecting)
        );
        assert_eq!(
            tracker.observe(&line("[System] [CHAT] 欢迎来到服务器")),
            Some(GameState::InWorld)
        );
        assert_eq!(
            tracker.observe(&line("Client disconnected with reason: 服务器已关闭")),
            Some(GameState::MainMenu),
            "被踢、服务器关了：回到主菜单"
        );
    }

    #[test]
    fn crash_report_marks_the_session_crashed_even_with_exit_code_0() {
        let mut tracker = SessionTracker::default();
        tracker.observe(&line("---- Minecraft Crash Report ----"));
        tracker.observe(&line(
            "#@!@# Game crashed! Crash report saved to: #@!@# D:\\mc\\crash-reports\\crash.txt",
        ));
        let state = tracker.on_exit(Some(0));
        assert_eq!(
            state,
            GameState::Crashed {
                code: Some(0),
                crash_report: Some("D:\\mc\\crash-reports\\crash.txt".to_owned()),
            }
        );
    }

    #[test]
    fn non_zero_exit_is_a_crash() {
        let mut tracker = SessionTracker::default();
        assert!(matches!(
            tracker.on_exit(Some(1)),
            GameState::Crashed { code: Some(1), .. }
        ));
    }

    #[test]
    fn states_only_move_forward_except_leaving_a_world() {
        let mut tracker = SessionTracker::default();
        tracker.observe(&line("Sound engine started"));
        assert_eq!(
            tracker.observe(&line("Setting user: Steve")),
            None,
            "不会从主菜单退回加载中"
        );
    }
}
