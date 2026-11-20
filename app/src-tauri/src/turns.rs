//! 同一个实际游戏目录同一时间只允许一个写入操作或游戏会话。
//!
//! 以前安装 A、安装 B、更新是各自独立的任务，两个共用前置的模组一起装、或者更新下载期间又点了删除，
//! 会同时改同一批文件和同一份来源记录。现在每个实际目录一把锁，挂载入口共享：
//! - 后台任务（安装、更新、拖进来装）排队等，等的时候任务中心写「排队中」，可以取消；
//! - 点一下就做完的操作（启用停用、删除、撤销、回到快照）遇到正在装的，直接告诉用户等一下；
//! - 启动、复制、删除实例、导出遇到正在装的也先拦住。
//!
//! 只管写文件的阶段之前排队，下载和写入一起算在任务里（任务本身不长，排队比并发简单可靠）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use launcher_core::jobs::JobHandle;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::error::AppError;

/// 拿到的「这一轮」：放手（丢掉）就轮到下一个
pub type Turn = OwnedMutexGuard<()>;

#[derive(Default)]
pub struct InstanceTurns(Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>);

/// 解析目录别名（包括 Windows 联接）。目录暂时不可访问时由调用者继续处理原始路径。
pub fn directory_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

impl InstanceTurns {
    fn lock_of(&self, directory: &Path) -> Arc<AsyncMutex<()>> {
        let key = directory_key(directory);
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(key).or_default().clone()
    }

    /// 现在就要（点一下就做完的操作）：有别的事在做时返回 `None`
    pub fn try_take(&self, directory: &Path) -> Option<Turn> {
        self.lock_of(directory).try_lock_owned().ok()
    }

    /// 等到轮到为止（启动时恢复、启动前恢复用：没有任务可以显示「排队中」，而且等的时间很短）
    pub async fn take(&self, directory: &Path) -> Turn {
        self.lock_of(directory).lock_owned().await
    }

    /// 这个实例上有没有事在做
    pub fn is_busy(&self, directory: &Path) -> bool {
        self.lock_of(directory).try_lock().is_err()
    }

    /// 后台任务排队等：等的时候任务上写「排队中」；任务被取消时返回 `None`
    pub async fn wait(&self, directory: &Path, job: &JobHandle) -> Option<Turn> {
        let cancel = job.cancel_token();
        if cancel.is_cancelled() {
            return None;
        }
        let lock = self.lock_of(directory);
        if let Ok(turn) = lock.clone().try_lock_owned() {
            return Some(turn);
        }
        tracing::info!(job = %job.id(), "游戏目录上有别的事在做，排队");
        job.set_message("排队中：等同一个游戏目录前面的事做完");
        tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            turn = lock.lock_owned() => Some(turn),
        }
    }
}

/// 点一下就做完的操作遇到正在装东西时的提示
pub fn busy_error() -> AppError {
    AppError::new(
        "instance.busy",
        "这个实例正在装东西，等装完再改（任务中心里能看到进度）",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::jobs::{JobRegistry, ProgressUnit};

    #[test]
    fn aliases_share_the_same_turn() {
        let dir = tempfile::tempdir().unwrap();
        let saves = dir.path().join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let alias = saves.join("..");
        let turns = InstanceTurns::default();
        let _first = turns.try_take(dir.path()).unwrap();
        assert!(turns.try_take(&alias).is_none(), "共享目录不能同时写");
    }

    #[tokio::test]
    async fn second_task_waits_for_the_first() {
        let turns = Arc::new(InstanceTurns::default());
        let jobs = JobRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path();
        std::fs::create_dir_all(original.join("saves")).unwrap();
        let alias = original.join("saves").join("..");
        let first = turns.try_take(original).unwrap();
        assert!(turns.is_busy(&alias), "指向同一个目录的入口要排队");
        assert!(turns.try_take(&alias).is_none());

        let job = jobs.start("b", "装 B", ProgressUnit::Bytes).unwrap();
        let shared = turns.clone();
        let waiting = tokio::spawn(async move {
            let turn = shared.wait(&alias, &job).await;
            let got = turn.is_some();
            drop(turn);
            job.succeed();
            got
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "前面的还没做完，后面的在排队");
        drop(first);
        assert!(waiting.await.unwrap(), "前面的做完就轮到它");
        assert!(!turns.is_busy(original));
    }

    #[tokio::test]
    async fn concurrent_backups_for_different_worlds_share_one_index() {
        use launcher_core::saves::backup::{BackupReason, Backups};

        let dir = tempfile::tempdir().unwrap();
        let saves = dir.path().join("saves");
        for name in ["first", "second"] {
            std::fs::create_dir_all(saves.join(name)).unwrap();
            std::fs::write(saves.join(name).join("level.dat"), name).unwrap();
        }

        let turns = Arc::new(InstanceTurns::default());
        let jobs = JobRegistry::new();
        let mut tasks = Vec::new();
        for (dir, name) in [
            (dir.path().to_owned(), "first"),
            (saves.join(".."), "second"),
        ] {
            let turns = turns.clone();
            let job = jobs.start(name, name, ProgressUnit::Bytes).unwrap();
            tasks.push(tokio::spawn(async move {
                let turn = turns.wait(&dir, &job).await.unwrap();
                let cancel = job.cancel_token();
                tokio::task::spawn_blocking(move || {
                    let _turn = turn;
                    Backups::new(dir)
                        .create(name, BackupReason::Manual, 3, &cancel, |_, _| {})
                        .unwrap();
                })
                .await
                .unwrap();
                job.succeed();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let backups = Backups::new(dir.path());
        assert_eq!(backups.list("first").len(), 1);
        assert_eq!(backups.list("second").len(), 1);
    }

    #[tokio::test]
    async fn waiting_can_be_cancelled() {
        let turns = InstanceTurns::default();
        let jobs = JobRegistry::new();
        let _first = turns.try_take(Path::new("Pack")).unwrap();
        let job = jobs.start("b", "装 B", ProgressUnit::Bytes).unwrap();
        job.cancel_token().cancel();
        assert!(turns.wait(Path::new("Pack"), &job).await.is_none());
        job.cancelled();
        assert!(
            turns.try_take(Path::new("Other")).is_some(),
            "别的实例不受影响"
        );
    }
}
