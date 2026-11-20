//! 主窗口销毁、重建期间的状态。只由窗口模块驱动，不持有窗口或游戏进程。

#[derive(Default)]
pub struct Lifecycle {
    release: Option<Release>,
    pub generation: u64,
}

struct Release {
    game: u64,
    phase: Phase,
}

enum Phase {
    Destroying { reopen: bool },
    Hidden,
    Rebuilding,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenAction {
    Show,
    Rebuild,
    Wait,
}

impl Lifecycle {
    pub fn keep_running(&self) -> bool {
        self.release.is_some()
    }

    pub fn begin_release(&mut self, game: u64) -> bool {
        if self.keep_running() {
            return false;
        }
        self.release = Some(Release {
            game,
            phase: Phase::Destroying { reopen: false },
        });
        true
    }

    pub fn request_open(&mut self, game: Option<u64>) -> OpenAction {
        let Some(release) = &mut self.release else {
            return if game.is_some() {
                OpenAction::Wait
            } else {
                OpenAction::Show
            };
        };
        if game.is_some_and(|game| game != release.game) {
            return OpenAction::Wait;
        }
        match &mut release.phase {
            Phase::Destroying { reopen } => *reopen = true,
            Phase::Hidden => {
                release.phase = Phase::Rebuilding;
                self.generation += 1;
                return OpenAction::Rebuild;
            }
            Phase::Rebuilding => {}
        }
        OpenAction::Wait
    }

    pub fn window_destroyed(&mut self) -> OpenAction {
        let Some(release) = &mut self.release else {
            return OpenAction::Wait;
        };
        let Phase::Destroying { reopen } = release.phase else {
            return OpenAction::Wait;
        };
        release.phase = Phase::Hidden;
        if reopen {
            self.request_open(None)
        } else {
            OpenAction::Wait
        }
    }

    pub fn finish_rebuild(&mut self, success: bool) {
        if success {
            self.release = None;
        } else if let Some(release) = &mut self.release {
            release.phase = Phase::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Lifecycle, OpenAction};

    #[test]
    fn game_exit_while_window_is_closing_reopens_after_destruction() {
        let mut lifecycle = Lifecycle::default();
        assert!(lifecycle.begin_release(1));
        assert_eq!(lifecycle.request_open(Some(1)), OpenAction::Wait);
        assert!(lifecycle.keep_running());
        assert_eq!(lifecycle.window_destroyed(), OpenAction::Rebuild);
        assert!(lifecycle.keep_running());
        lifecycle.finish_rebuild(true);
        assert!(!lifecycle.keep_running());
    }

    #[test]
    fn unrelated_exit_does_not_reopen_and_failed_rebuild_can_be_retried() {
        let mut lifecycle = Lifecycle::default();
        assert!(lifecycle.begin_release(1));
        assert_eq!(lifecycle.window_destroyed(), OpenAction::Wait);
        assert_eq!(lifecycle.request_open(Some(2)), OpenAction::Wait);
        assert_eq!(lifecycle.request_open(None), OpenAction::Rebuild);
        assert_eq!(lifecycle.request_open(Some(1)), OpenAction::Wait);
        lifecycle.finish_rebuild(false);
        assert!(lifecycle.keep_running());
        assert_eq!(lifecycle.request_open(None), OpenAction::Rebuild);
        lifecycle.finish_rebuild(true);
        assert_eq!(lifecycle.request_open(Some(1)), OpenAction::Wait);
        assert_eq!(lifecycle.request_open(None), OpenAction::Show);
    }
}
