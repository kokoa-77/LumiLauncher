//! 正在运行的游戏会话：状态、最近的日志、结束开关。只做登记，不含业务逻辑。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use launcher_core::launch::{GameState, LogLine};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// 每个会话保留的日志行数（日志窗口用）
const MAX_LOG_LINES: usize = 5000;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub instance_id: String,
    pub state: GameState,
    pub started_at: f64,
    /// 启动前准备失败时的提示（中文人话）
    pub error: Option<String>,
    /// 这一次的启动记录编号（「日志」页签据此知道哪一条是正在运行的）
    pub record_id: Option<String>,
}

struct Session {
    directory: PathBuf,
    snapshot: SessionSnapshot,
    stop: CancellationToken,
    log: VecDeque<LogLine>,
    running: bool,
}

#[derive(Default)]
pub struct Sessions {
    inner: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Session>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 开始一个会话；任一入口正使用这个实际目录时返回 `None`。
    pub fn begin(
        &self,
        instance_id: &str,
        directory: PathBuf,
        now: f64,
    ) -> Option<CancellationToken> {
        let mut sessions = self.lock();
        if sessions.values().any(|s| {
            s.running && (s.directory == directory || s.snapshot.instance_id == instance_id)
        }) {
            return None;
        }
        let stop = CancellationToken::new();
        sessions.insert(
            instance_id.to_owned(),
            Session {
                directory,
                snapshot: SessionSnapshot {
                    instance_id: instance_id.to_owned(),
                    state: GameState::Starting,
                    started_at: now,
                    error: None,
                    record_id: None,
                },
                stop: stop.clone(),
                log: VecDeque::new(),
                running: true,
            },
        );
        Some(stop)
    }

    /// 更新安装期间禁止建立新游戏会话。
    pub fn when_idle<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let sessions = self.lock();
        if sessions.values().any(|session| session.running) {
            return None;
        }
        let result = action();
        drop(sessions);
        Some(result)
    }

    pub fn running_in(&self, directory: &Path) -> bool {
        self.lock()
            .values()
            .any(|session| session.running && session.directory == directory)
    }

    pub fn set_state(&self, instance_id: &str, state: GameState) -> Option<SessionSnapshot> {
        let mut sessions = self.lock();
        let session = sessions.get_mut(instance_id)?;
        if matches!(state, GameState::Exited { .. } | GameState::Crashed { .. }) {
            session.running = false;
        }
        session.snapshot.state = state;
        Some(session.snapshot.clone())
    }

    /// 启动前准备失败：会话结束并记下原因
    pub fn fail(&self, instance_id: &str, message: String) -> Option<SessionSnapshot> {
        let mut sessions = self.lock();
        let session = sessions.get_mut(instance_id)?;
        session.running = false;
        session.snapshot.state = GameState::Exited { code: None };
        session.snapshot.error = Some(message);
        Some(session.snapshot.clone())
    }

    /// 记下这一次的启动记录编号
    pub fn set_record(&self, instance_id: &str, record_id: &str) -> Option<SessionSnapshot> {
        let mut sessions = self.lock();
        let session = sessions.get_mut(instance_id)?;
        session.snapshot.record_id = Some(record_id.to_owned());
        Some(session.snapshot.clone())
    }

    /// 正在运行的那一次的启动记录编号
    pub fn running_record(&self, instance_id: &str) -> Option<String> {
        self.lock()
            .get(instance_id)
            .filter(|s| s.running)
            .and_then(|s| s.snapshot.record_id.clone())
    }

    pub fn push_logs(&self, instance_id: &str, lines: &[LogLine]) {
        let mut sessions = self.lock();
        if let Some(session) = sessions.get_mut(instance_id) {
            session.log.extend(lines.iter().cloned());
            let overflow = session.log.len().saturating_sub(MAX_LOG_LINES);
            session.log.drain(..overflow);
        }
    }

    pub fn stop(&self, instance_id: &str) -> bool {
        match self.lock().get(instance_id) {
            Some(session) if session.running => {
                session.stop.cancel();
                true
            }
            _ => false,
        }
    }

    pub fn list(&self) -> Vec<SessionSnapshot> {
        self.lock().values().map(|s| s.snapshot.clone()).collect()
    }

    pub fn log(&self, instance_id: &str) -> Vec<LogLine> {
        self.lock()
            .get(instance_id)
            .map(|s| s.log.iter().cloned().collect())
            .unwrap_or_default()
    }
}
