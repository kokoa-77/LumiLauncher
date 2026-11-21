//! 自动封面：玩的时候每隔几分钟悄悄截一张游戏画面，退出后首页就是刚才玩的样子。
//!
//! 只在「人在单人世界里 + 游戏窗口在最前面」时截，截到黑屏、纯色画面不要；
//! 存在实例自己的文件夹里，怎么存、怎么挑见 [`crate::saves::cover`]。
//! 多人游戏暂时不截（没有存档可以挂）。设置里关掉以后马上不再截。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

use super::GameState;
use crate::saves::cover;

/// 进世界多久以后截第一张（等地形加载完、人站稳）
const FIRST: Duration = Duration::from_secs(60);
/// 之后每隔多久截一张
const EVERY: Duration = Duration::from_secs(180);
/// 这次没截成（窗口不在最前面、截到黑屏）多久以后再试
const RETRY: Duration = Duration::from_secs(20);
/// 存档的 `session.lock` 比「进世界」早多久以内，都算这次打开的（服务器先锁存档、再让玩家进来）
const LOCK_SLACK: Duration = Duration::from_secs(300);

/// 这一次截得怎么样
#[derive(Debug, PartialEq, Eq)]
enum Tick {
    Saved,
    /// 现在不合适：不在单人世界里、窗口不在最前面
    NotNow,
    /// 截到黑屏或纯色画面（全屏独占模式下系统可能给不出画面），按正常间隔再试，不刷日志
    Blank,
    Failed,
}

/// 跟着游戏状态截图，直到 `state` 的发送端没了（游戏结束）。
/// `enabled` 每次截之前都看一眼（设置里的「自动封面」开关）。
pub async fn run(
    instance_dir: PathBuf,
    pid: u32,
    mut state: watch::Receiver<GameState>,
    enabled: impl Fn() -> bool + Send + 'static,
) {
    loop {
        if state.wait_for(|s| *s == GameState::InWorld).await.is_err() {
            return;
        }
        let entered = SystemTime::now();
        let mut wait = FIRST;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                changed = state.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    if *state.borrow_and_update() != GameState::InWorld {
                        break;
                    }
                    continue;
                }
            }
            if !enabled() {
                wait = EVERY;
                continue;
            }
            let dir = instance_dir.clone();
            wait = match tokio::task::spawn_blocking(move || tick(&dir, pid, entered)).await {
                Ok(Tick::Saved) => EVERY,
                Ok(Tick::NotNow) => RETRY,
                Ok(Tick::Blank | Tick::Failed) => EVERY,
                Err(error) => {
                    tracing::warn!(%error, "自动封面的后台任务出错");
                    EVERY
                }
            };
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn tick(instance_dir: &Path, pid: u32, entered: SystemTime) -> Tick {
    let since = entered.checked_sub(LOCK_SLACK).unwrap_or(UNIX_EPOCH);
    let Some(world) = cover::open_world(instance_dir, since) else {
        return Tick::NotNow;
    };
    let image = match crate::capture::focused_window(pid) {
        Ok(Some(image)) => image,
        Ok(None) => return Tick::NotNow,
        Err(error) => {
            tracing::warn!(%error, "截游戏画面失败");
            return Tick::Failed;
        }
    };
    match cover::save_auto(instance_dir, &world, &image, now_ms()) {
        Ok(Some(path)) => {
            tracing::info!(%world, file = %path.display(), "存了一张自动封面");
            Tick::Saved
        }
        Ok(None) => {
            tracing::info!(%world, "截到的是黑屏或纯色画面，不要（全屏时可能截不到）");
            Tick::Blank
        }
        Err(error) => {
            tracing::warn!(%error, "保存自动封面失败");
            Tick::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_nothing_outside_a_singleplayer_world() {
        let dir = tempfile::tempdir().unwrap();
        // 没有打开的存档（多人游戏、还没进世界）：不截图，过一会儿再看
        assert_eq!(tick(dir.path(), u32::MAX, SystemTime::now()), Tick::NotNow);
    }

    #[test]
    fn world_open_but_no_window_in_front_is_not_now() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("saves/w");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("session.lock"), "☃").unwrap();
        // 这个进程号没有窗口：当作窗口不在最前面
        assert_eq!(tick(dir.path(), u32::MAX, SystemTime::now()), Tick::NotNow);
        assert!(!cover::covers_dir(dir.path()).exists(), "什么都没存");
    }

    #[tokio::test]
    async fn stops_when_the_game_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = watch::channel(GameState::Starting);
        let task = tokio::spawn(run(dir.path().to_path_buf(), u32::MAX, rx, || true));
        tx.send_replace(GameState::InWorld);
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send_replace(GameState::MainMenu);
        drop(tx);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("游戏结束后马上停")
            .unwrap();
    }
}
