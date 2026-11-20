//! 任务注册表：所有耗时操作（安装、下载、导入整合包、更新模组）都是一个任务。
//!
//! - 同一个 `key`（例如 `install:<实例 ID>`）同时只能有一个任务在跑；
//! - 每个任务可取消（`CancellationToken`）；
//! - 进度推送自动节流（默认每 100 毫秒最多一次），状态变化和完成时立即推送；
//! - `JobHandle` 被丢弃时如果还没结束，会自动标记为「已取消」或「意外中断」，不会留下卡住的任务。
//!
//! 下载中心（D18）：
//! - 每个任务记下属于哪个实例、从哪下载（[`JobSource`]）、做过的每一步（[`JobStep`]），
//!   按最近 5 秒的进度统一算速度和预计剩余时间；
//! - 用 [`JobRegistry::with_history`] 打开时，任务写进历史文件（开始、换步骤、结束时写，后台线程写，
//!   不卡调用的人），关掉启动器再打开还在；那时还没做完的记成「已中断」（[`JobState::Interrupted`]）；
//! - 开任务时可以带一份「重试配方」（应用壳自己定格式），失败或取消后凭它再来一次。
//!   配方只存在历史文件里，不发给界面（里面可能有文件路径）。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::DownloadSource;
use crate::fs::{read_json, write_json_atomic};
use crate::now_millis;

/// 下载历史最多留多少条（这次打开后结束的 + 以前的）
pub const MAX_HISTORY: usize = 200;
/// 每个任务最多记多少步（只留最后这些）
const MAX_STEPS: usize = 60;
const DEFAULT_THROTTLE: Duration = Duration::from_millis(100);
const EVENT_CHANNEL_CAPACITY: usize = 256;
/// 算速度用最近多久的进度
const RATE_WINDOW: Duration = Duration::from_secs(5);
/// 至少攒够这么久的进度才报速度（刚开始那一下不准）
const RATE_MIN_SPAN: Duration = Duration::from_millis(800);
const HISTORY_FORMAT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// 启动器关掉时还没做完（只出现在以前的历史里，D18）
    Interrupted,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        !matches!(self, JobState::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    /// 已完成量（字节或个数，看 `unit`）。用 f64 是为了前端不丢精度又能直接显示。
    pub done: f64,
    /// 总量；未知时为空（显示为不确定进度条）。
    pub total: Option<f64>,
    pub unit: ProgressUnit,
    /// 当前步骤的说明，例如「正在下载资源文件」。
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ProgressUnit {
    Bytes,
    Items,
}

/// 任务从哪下载东西（下载历史按它筛选）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum JobSource {
    /// 游戏本体、加载器
    Game,
    Java,
    Modrinth,
    CurseForge,
    /// 本地文件：拖进来的、导入的整合包、世界备份、导出、复制实例
    Local,
    #[default]
    Other,
}

/// 做过的一步
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct JobStep {
    pub label: String,
    /// 开始这一步的时间（毫秒级 Unix 时间戳）
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub key: String,
    pub title: String,
    pub state: JobState,
    pub progress: JobProgress,
    /// 给用户看的失败原因（中文人话）。
    pub error: Option<String>,
    /// 毫秒级 Unix 时间戳。
    pub started_at: f64,
    pub finished_at: Option<f64>,
    pub source: JobSource,
    /// 属于哪个实例（新建实例、导入整合包的做完才知道）
    pub instance_id: Option<String>,
    /// 显示用的图标（模组、整合包的图标地址）
    pub icon_url: Option<String>,
    /// 做过的每一步，最早的在前
    pub steps: Vec<JobStep>,
    /// 每秒完成多少（单位同 `progress.unit`）；刚开始、换步骤、结束后为空
    pub speed: Option<f64>,
    /// 预计还要多少秒；不知道总量或还没速度时为空
    pub eta: Option<f64>,
    /// 失败或取消后能不能「重试」
    pub retryable: bool,
    /// 游戏文件这次用的下载源（「自动」测速后选的那个）；只下模组、整合包这类平台文件的任务为空
    pub download_source: Option<DownloadSource>,
    /// 有几个文件第一个地址下不下来、换了地址才下好（换源次数）
    pub fallbacks: u32,
}

/// 开任务时说明它是什么
#[derive(Debug, Clone, Default)]
pub struct JobMeta {
    pub source: JobSource,
    pub instance_id: Option<String>,
    pub icon_url: Option<String>,
    /// 「重试」配方（应用壳定格式）
    pub retry: Option<serde_json::Value>,
    /// 不记进下载历史（启动游戏前的准备）
    pub transient: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("同一个任务正在进行中（{key}）")]
    AlreadyRunning { key: String, id: String },
}

/// 按最近一段时间的进度算速度
#[derive(Default)]
struct Rate {
    samples: VecDeque<(Instant, f64)>,
}

impl Rate {
    fn reset(&mut self) {
        self.samples.clear();
    }

    /// 记一次计数（下载时是真从网络收到的字节，见 [`JobHandle::set_download_progress`]；
    /// 别的任务就是完成量），`remaining` 是还剩多少。返回（速度，预计剩余秒数）；还算不出或者没在动时都为空
    fn sample(
        &mut self,
        now: Instant,
        counter: f64,
        remaining: Option<f64>,
    ) -> (Option<f64>, Option<f64>) {
        // 计数往回走了（重试时退回已计的字节）：重新算
        if self.samples.back().is_some_and(|&(_, last)| counter < last) {
            self.samples.clear();
        }
        let done = counter;
        self.samples.push_back((now, done));
        // 只留窗口里的（至少留两次，进度报得很稀时也算得出来）
        while self.samples.len() > 2
            && self
                .samples
                .front()
                .is_some_and(|&(at, _)| now.duration_since(at) > RATE_WINDOW)
        {
            self.samples.pop_front();
        }
        let Some(&(first_at, first_done)) = self.samples.front() else {
            return (None, None);
        };
        let span = now.duration_since(first_at);
        if span < RATE_MIN_SPAN {
            return (None, None);
        }
        let speed = (done - first_done) / span.as_secs_f64();
        if speed <= 0.0 {
            return (None, None);
        }
        (Some(speed), remaining.map(|left| left.max(0.0) / speed))
    }
}

struct Record {
    snapshot: JobSnapshot,
    token: CancellationToken,
    last_emit: Option<Instant>,
    retry: Option<serde_json::Value>,
    transient: bool,
    /// 完成量涨得多快（算剩余时间：已有文件跳过得快，也算进度）
    rate: Rate,
    /// 网络收到的字节涨得多快（算显示的速度，D18 实测）
    net: Rate,
    /// 以前几步加起来的换源次数（每一步的下载各自从 0 数）
    fallback_base: u32,
    /// 这一步的换源次数
    fallback_step: u32,
}

/// 历史文件里的一条
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    job: JobSnapshot,
    #[serde(default)]
    retry: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct HistoryFile {
    format: u32,
    jobs: Vec<Stored>,
}

/// 读的时候每条单独解析：哪一条坏了（老版本少字段、手改坏了）只跳过那一条
#[derive(Deserialize)]
struct RawHistory {
    format: u32,
    jobs: Vec<serde_json::Value>,
}

#[derive(Default)]
struct State {
    records: HashMap<String, Record>,
    /// 运行中任务：key → id
    running_keys: HashMap<String, String>,
    /// 已结束任务的 id，按结束顺序排列，用于淘汰旧记录
    finished_order: Vec<String>,
    /// 以前（这次打开之前）的任务，新的在前
    past: Vec<Stored>,
}

struct History {
    path: PathBuf,
    /// 写文件时拿着：先拿它再取快照，后写的一定是更新的状态
    write_lock: Mutex<()>,
    /// 叫后台线程写一次；没有后台线程（测试）时当场写
    wake: Option<mpsc::Sender<()>>,
}

struct Inner {
    state: Mutex<State>,
    events: broadcast::Sender<JobSnapshot>,
    throttle: Duration,
    history: Option<History>,
}

/// 任务注册表。克隆是廉价的（内部共享同一份状态）。
#[derive(Clone)]
pub struct JobRegistry {
    inner: Arc<Inner>,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::with_throttle(DEFAULT_THROTTLE)
    }

    /// 自定义进度节流间隔（测试里用 0 关闭节流）。
    pub fn with_throttle(throttle: Duration) -> Self {
        Self::build(throttle, None, Vec::new())
    }

    /// 带历史文件：读出以前的任务，以后的变化都写回去。
    pub fn with_history(path: PathBuf) -> Self {
        let (wake, rx) = mpsc::channel();
        let past = load_history(&path);
        let registry = Self::build(
            DEFAULT_THROTTLE,
            Some(History {
                path,
                write_lock: Mutex::new(()),
                wake: Some(wake),
            }),
            past,
        );
        let weak = Arc::downgrade(&registry.inner);
        // 线程没开起来时 `rx` 跟着丢掉，`persist` 发不出去就当场写
        if let Err(error) = std::thread::Builder::new()
            .name("job-history".to_owned())
            .spawn(move || write_loop(&weak, &rx))
        {
            tracing::warn!(%error, "下载历史的后台线程没开起来，改成当场写");
        }
        registry
    }

    /// 测试用：不开后台线程，每次当场写（不会有上一个注册表的线程晚写一步）
    #[cfg(test)]
    fn with_history_sync(path: PathBuf) -> Self {
        let past = load_history(&path);
        Self::build(
            Duration::ZERO,
            Some(History {
                path,
                write_lock: Mutex::new(()),
                wake: None,
            }),
            past,
        )
    }

    fn build(throttle: Duration, history: Option<History>, past: Vec<Stored>) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    past,
                    ..State::default()
                }),
                events,
                throttle,
                history,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // 某个线程持锁时崩溃会让锁「中毒」；任务表只是展示用状态，继续使用里面的数据即可。
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, snapshot: JobSnapshot) {
        // 没有订阅者时发送会失败，这是正常情况（比如还没打开界面）。
        let _ = self.inner.events.send(snapshot);
    }

    /// 叫后台线程把历史写一次（线程没开起来就当场写）
    fn persist(&self) {
        let Some(history) = &self.inner.history else {
            return;
        };
        let sent = history.wake.as_ref().is_some_and(|w| w.send(()).is_ok());
        if !sent {
            self.save_now();
        }
    }

    /// 马上把历史写进文件（后台线程用；退出前、测试里也可以直接调）
    pub fn save_now(&self) {
        let Some(history) = &self.inner.history else {
            return;
        };
        let _writing = history.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let file = HistoryFile {
            format: HISTORY_FORMAT,
            jobs: self.stored(),
        };
        if let Err(error) = write_json_atomic(&history.path, &file) {
            tracing::warn!(%error, "保存下载历史失败");
        }
    }

    /// 要记进历史的全部任务，新的在前
    fn stored(&self) -> Vec<Stored> {
        let state = self.lock();
        let mut jobs: Vec<Stored> = state
            .records
            .values()
            .filter(|r| !r.transient)
            .map(|r| Stored {
                job: r.snapshot.clone(),
                retry: r.retry.clone(),
            })
            .collect();
        jobs.sort_by(|a, b| b.job.started_at.total_cmp(&a.job.started_at));
        jobs.extend(state.past.iter().cloned());
        jobs
    }

    /// 开始一个任务。同一个 `key` 已有任务在跑时返回 `AlreadyRunning`。
    pub fn start(
        &self,
        key: impl Into<String>,
        title: impl Into<String>,
        unit: ProgressUnit,
    ) -> Result<JobHandle, JobError> {
        self.start_with(key, title, unit, JobMeta::default())
    }

    /// 开始一个任务，带上它属于哪个实例、从哪下载、怎么重试。
    pub fn start_with(
        &self,
        key: impl Into<String>,
        title: impl Into<String>,
        unit: ProgressUnit,
        meta: JobMeta,
    ) -> Result<JobHandle, JobError> {
        let key = key.into();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let token = CancellationToken::new();
        let snapshot = JobSnapshot {
            id: id.clone(),
            key: key.clone(),
            title: title.into(),
            state: JobState::Running,
            progress: JobProgress {
                done: 0.0,
                total: None,
                unit,
                message: None,
            },
            error: None,
            started_at: now_millis() as f64,
            finished_at: None,
            source: meta.source,
            instance_id: meta.instance_id,
            icon_url: meta.icon_url,
            steps: Vec::new(),
            speed: None,
            eta: None,
            retryable: meta.retry.is_some(),
            download_source: None,
            fallbacks: 0,
        };

        {
            let mut state = self.lock();
            if let Some(existing) = state.running_keys.get(&key) {
                return Err(JobError::AlreadyRunning {
                    key,
                    id: existing.clone(),
                });
            }
            state.running_keys.insert(key, id.clone());
            state.records.insert(
                id.clone(),
                Record {
                    snapshot: snapshot.clone(),
                    token: token.clone(),
                    last_emit: None,
                    retry: meta.retry,
                    transient: meta.transient,
                    rate: Rate::default(),
                    net: Rate::default(),
                    fallback_base: 0,
                    fallback_step: 0,
                },
            );
        }
        tracing::info!(job = %id, key = %snapshot.key, title = %snapshot.title, "任务开始");
        self.emit(snapshot);
        if !meta.transient {
            self.persist();
        }

        Ok(JobHandle {
            id,
            registry: self.clone(),
            token,
            finished: false,
        })
    }

    /// 请求取消。返回任务是否存在且仍在运行。
    pub fn cancel(&self, id: &str) -> bool {
        let state = self.lock();
        match state.records.get(id) {
            Some(record) if !record.snapshot.state.is_finished() => {
                record.token.cancel();
                tracing::info!(job = %id, "已请求取消任务");
                true
            }
            _ => false,
        }
    }

    /// 在没有进行中任务时执行一个短操作。执行期间不接受新任务登记。
    pub fn when_idle<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let state = self.lock();
        if state
            .records
            .values()
            .any(|record| !record.snapshot.state.is_finished())
        {
            return None;
        }
        let result = action();
        drop(state);
        Some(result)
    }

    /// 这次打开后的任务快照：运行中的在前，然后是最近结束的。
    pub fn list(&self) -> Vec<JobSnapshot> {
        let state = self.lock();
        let mut list: Vec<JobSnapshot> =
            state.records.values().map(|r| r.snapshot.clone()).collect();
        sort_running_first(&mut list);
        list
    }

    /// 下载历史：这次打开后的（不含启动前的准备）+ 以前的，运行中的在前，然后新的在前。
    pub fn history(&self) -> Vec<JobSnapshot> {
        let mut list: Vec<JobSnapshot> = self.stored().into_iter().map(|s| s.job).collect();
        sort_running_first(&mut list);
        list
    }

    /// 找一个任务（这次的或以前的），带上它的重试配方。
    pub fn find(&self, id: &str) -> Option<(JobSnapshot, Option<serde_json::Value>)> {
        let state = self.lock();
        if let Some(record) = state.records.get(id) {
            return Some((record.snapshot.clone(), record.retry.clone()));
        }
        state
            .past
            .iter()
            .find(|s| s.job.id == id)
            .map(|s| (s.job.clone(), s.retry.clone()))
    }

    /// 从历史里删掉一个已结束的任务（重试时新任务接替它）。运行中的删不掉。
    pub fn remove(&self, id: &str) -> bool {
        let removed = {
            let mut state = self.lock();
            let finished = state
                .records
                .get(id)
                .is_some_and(|r| r.snapshot.state.is_finished());
            if finished {
                state.records.remove(id);
                state.finished_order.retain(|f| f != id);
                true
            } else {
                let before = state.past.len();
                state.past.retain(|s| s.job.id != id);
                state.past.len() != before
            }
        };
        if removed {
            self.persist();
        }
        removed
    }

    /// 订阅任务变化。
    pub fn subscribe(&self) -> broadcast::Receiver<JobSnapshot> {
        self.inner.events.subscribe()
    }

    /// 清掉所有已结束的任务记录（包括以前的历史）。
    pub fn clear_finished(&self) {
        {
            let mut state = self.lock();
            let finished = std::mem::take(&mut state.finished_order);
            for id in finished {
                state.records.remove(&id);
            }
            state.past.clear();
        }
        self.persist();
    }

    /// 改一条运行中的记录，按需推送和存盘
    fn modify(&self, id: &str, change: impl FnOnce(&mut Record) -> Change) {
        let (snapshot, persist) = {
            let mut state = self.lock();
            let Some(record) = state.records.get_mut(id) else {
                return;
            };
            if record.snapshot.state.is_finished() {
                return;
            }
            let change = change(record);
            if change == Change::None {
                return;
            }
            record.last_emit = Some(Instant::now());
            (
                record.snapshot.clone(),
                change == Change::Saved && !record.transient,
            )
        };
        self.emit(snapshot);
        if persist {
            self.persist();
        }
    }

    /// 改进度。`received`：真从网络收到的字节（下载时给，算速度只看它；不给就按完成量算）
    fn update_progress(
        &self,
        id: &str,
        update: impl FnOnce(&mut JobProgress),
        received: Option<f64>,
        force: bool,
    ) {
        let throttle = self.inner.throttle;
        self.modify(id, |record| {
            update(&mut record.snapshot.progress);
            let progress = &record.snapshot.progress;
            let now = Instant::now();
            let remaining = progress.total.map(|total| total - progress.done);
            // 剩余时间按完成量的快慢算（本地已有、直接跳过的文件也是进度，不然开头会报出「还剩 9 小时」）；
            // 显示的速度按网络真收到的字节算（不然会报出几百 MB/s）
            let (done_speed, eta) = record.rate.sample(now, progress.done, remaining);
            let speed = match received {
                Some(bytes) => record.net.sample(now, bytes, None).0,
                None => done_speed,
            };
            record.snapshot.speed = speed;
            record.snapshot.eta = eta;
            let reached_total = progress.total.is_some_and(|t| progress.done >= t);
            let due = record.last_emit.is_none_or(|t| t.elapsed() >= throttle);
            if force || reached_total || due {
                Change::Shown
            } else {
                Change::None
            }
        });
    }

    fn finish(&self, id: &str, final_state: JobState, error: Option<String>) {
        let snapshot = {
            let mut state = self.lock();
            let Some(record) = state.records.get_mut(id) else {
                return;
            };
            if record.snapshot.state.is_finished() {
                return;
            }
            record.snapshot.state = final_state;
            record.snapshot.error = error;
            record.snapshot.finished_at = Some(now_millis() as f64);
            record.snapshot.speed = None;
            record.snapshot.eta = None;
            let snapshot = record.snapshot.clone();

            state.running_keys.remove(&snapshot.key);
            state.finished_order.push(id.to_owned());
            // 历史满了：先丢以前的，再丢这次最早结束的
            while state.finished_order.len() + state.past.len() > MAX_HISTORY {
                if state.past.pop().is_none() {
                    let oldest = state.finished_order.remove(0);
                    state.records.remove(&oldest);
                }
            }
            snapshot
        };
        match final_state {
            JobState::Failed => tracing::warn!(
                job = %id,
                error = snapshot.error.as_deref().unwrap_or(""),
                "任务失败"
            ),
            _ => tracing::info!(job = %id, state = ?final_state, "任务结束"),
        }
        self.emit(snapshot);
        self.persist();
    }
}

/// 改一条记录的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// 没变 / 节流期内，不推送
    None,
    /// 推送给界面
    Shown,
    /// 推送并存进历史（换了步骤、知道了实例）
    Saved,
}

/// 记一步（和上一步一样就不记）。返回是不是新的一步
fn push_step(record: &mut Record, label: &str) -> bool {
    if record
        .snapshot
        .steps
        .last()
        .is_some_and(|s| s.label == label)
    {
        return false;
    }
    record.snapshot.steps.push(JobStep {
        label: label.to_owned(),
        at: now_millis() as f64,
    });
    let steps = &mut record.snapshot.steps;
    if steps.len() > MAX_STEPS {
        let extra = steps.len() - MAX_STEPS;
        steps.drain(..extra);
    }
    // 换步骤了，速度从头算；这一步的换源次数并进总数
    record.rate.reset();
    record.net.reset();
    record.fallback_base = record.fallback_base.saturating_add(record.fallback_step);
    record.fallback_step = 0;
    record.snapshot.speed = None;
    record.snapshot.eta = None;
    true
}

fn sort_running_first(list: &mut [JobSnapshot]) {
    list.sort_by(|a, b| {
        a.state
            .is_finished()
            .cmp(&b.state.is_finished())
            .then(b.started_at.total_cmp(&a.started_at))
    });
}

/// 读历史文件；那时还在进行中的记成「已中断」
fn load_history(path: &std::path::Path) -> Vec<Stored> {
    let file = match read_json::<RawHistory>(path) {
        Ok(Some(file)) => file,
        Ok(None) => return Vec::new(),
        Err(error) => {
            tracing::warn!(%error, "下载历史读不出来，从空的开始");
            return Vec::new();
        }
    };
    if file.format > HISTORY_FORMAT {
        tracing::warn!(format = file.format, "下载历史是更新版本的启动器写的，不读");
        return Vec::new();
    }
    let total = file.jobs.len();
    let mut jobs: Vec<Stored> = file
        .jobs
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    if jobs.len() < total {
        tracing::warn!(
            skipped = total - jobs.len(),
            "下载历史里有几条读不出来，跳过了"
        );
    }
    jobs.truncate(MAX_HISTORY);
    for stored in &mut jobs {
        let job = &mut stored.job;
        job.speed = None;
        job.eta = None;
        if !job.state.is_finished() {
            job.state = JobState::Interrupted;
            job.error = None;
            job.finished_at = Some(job.steps.last().map_or(job.started_at, |s| s.at));
        }
    }
    jobs
}

/// 后台写历史：有人叫就写一次，连着叫好几次只写最后一次
fn write_loop(inner: &Weak<Inner>, rx: &mpsc::Receiver<()>) {
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        let Some(inner) = inner.upgrade() else {
            break;
        };
        JobRegistry { inner }.save_now();
    }
}

/// 正在运行的任务。必须调用 `succeed` / `fail` / `cancelled` 之一结束它；
/// 直接丢弃时会自动收尾。
pub struct JobHandle {
    id: String,
    registry: JobRegistry,
    token: CancellationToken,
    finished: bool,
}

impl std::fmt::Debug for JobHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobHandle")
            .field("id", &self.id)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl JobHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 取消令牌，传给下载等子操作。
    pub fn cancel_token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// 设置进度（会被节流）。
    pub fn set_progress(&self, done: f64, total: Option<f64>) {
        self.registry.update_progress(
            &self.id,
            |p| {
                p.done = done;
                p.total = total;
            },
            None,
            false,
        );
    }

    /// 更新进度和随进度变化的说明（例如「补全文件 12/3998」），和进度一起节流。
    /// 不要用 [`Self::set_message`] 报这类说明：它不节流，几千个文件会刷出几千个事件。
    pub fn set_progress_detail(&self, done: f64, total: Option<f64>, detail: impl Into<String>) {
        let detail = detail.into();
        self.registry.update_progress(
            &self.id,
            |p| {
                p.done = done;
                p.total = total;
                p.message = Some(detail);
            },
            None,
            false,
        );
    }

    /// 下载进度（D18）：和 [`Self::set_progress_detail`] 一样，另外给出这次真从网络收到的字节，
    /// 速度只按它算（已经有、直接跳过的文件不会把速度撑到几百 MB/s）
    /// `fallbacks` 是这一步里换了地址才下好的文件数。
    pub fn set_download_progress(
        &self,
        done: f64,
        total: Option<f64>,
        received: f64,
        fallbacks: u32,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        self.registry.update_progress(
            &self.id,
            |p| {
                p.done = done;
                p.total = total;
                p.message = Some(detail);
            },
            Some(received),
            false,
        );
        self.registry.modify(&self.id, |record| {
            if fallbacks <= record.fallback_step {
                return Change::None;
            }
            record.fallback_step = fallbacks;
            record.snapshot.fallbacks = record.fallback_base.saturating_add(fallbacks);
            Change::Shown
        });
    }

    /// 记下这次游戏文件用的下载源（「自动」测速后选的那个，D18 下载历史里显示）
    pub fn set_download_source(&self, source: DownloadSource) {
        self.registry.modify(&self.id, |record| {
            record.snapshot.download_source = Some(source);
            Change::Saved
        });
    }

    /// 切换到新的步骤说明（立即推送，不节流）。只在步骤真的变了时用。也会记成一步。
    pub fn set_message(&self, message: impl Into<String>) {
        let message = message.into();
        self.registry.modify(&self.id, |record| {
            let new_step = push_step(record, &message);
            record.snapshot.progress.message = Some(message);
            if new_step {
                Change::Saved
            } else {
                Change::Shown
            }
        });
    }

    /// 进入某一步（「下载整合包文件」）；和现在这步一样时什么也不做，可以每次报进度前都调。
    pub fn set_step(&self, label: &str) {
        self.registry.modify(&self.id, |record| {
            if push_step(record, label) {
                Change::Saved
            } else {
                Change::None
            }
        });
    }

    /// 知道了属于哪个实例（新建实例、导入整合包时）
    pub fn set_instance(&self, instance_id: impl Into<String>) {
        let instance_id = instance_id.into();
        self.registry.modify(&self.id, |record| {
            record.snapshot.instance_id = Some(instance_id);
            Change::Saved
        });
    }

    /// 换一份重试配方（比如知道了更多能接着做的信息）
    pub fn set_retry(&self, retry: serde_json::Value) {
        self.registry.modify(&self.id, |record| {
            record.retry = Some(retry);
            record.snapshot.retryable = true;
            Change::Saved
        });
    }

    pub fn succeed(mut self) {
        self.finished = true;
        self.registry.finish(&self.id, JobState::Succeeded, None);
    }

    /// 以失败结束。`error` 是给用户看的中文说明。
    pub fn fail(mut self, error: impl Into<String>) {
        self.finished = true;
        self.registry
            .finish(&self.id, JobState::Failed, Some(error.into()));
    }

    pub fn cancelled(mut self) {
        self.finished = true;
        self.registry.finish(&self.id, JobState::Cancelled, None);
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.token.is_cancelled() {
            self.registry.finish(&self.id, JobState::Cancelled, None);
        } else {
            self.registry.finish(
                &self.id,
                JobState::Failed,
                Some("任务意外中断，请重试；如果反复出现，请导出诊断包反馈".to_owned()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> JobRegistry {
        JobRegistry::with_throttle(Duration::ZERO)
    }

    #[test]
    fn same_key_cannot_run_twice_but_can_restart_after_finishing() {
        let jobs = registry();
        let first = jobs
            .start("install:a", "安装 A", ProgressUnit::Bytes)
            .unwrap();

        let err = jobs
            .start("install:a", "再装一次", ProgressUnit::Bytes)
            .unwrap_err();
        assert_eq!(
            err,
            JobError::AlreadyRunning {
                key: "install:a".into(),
                id: first.id().into()
            }
        );

        first.succeed();
        assert!(
            jobs.start("install:a", "重新安装", ProgressUnit::Bytes)
                .is_ok()
        );
    }

    #[test]
    fn cancel_flips_token_and_reports_running_jobs_only() {
        let jobs = registry();
        let job = jobs.start("k", "t", ProgressUnit::Items).unwrap();
        let id = job.id().to_owned();

        assert!(jobs.cancel(&id));
        assert!(job.is_cancelled());
        job.cancelled();

        assert!(!jobs.cancel(&id), "已结束的任务不能再取消");
        assert!(!jobs.cancel("no-such-job"));
        assert_eq!(jobs.list()[0].state, JobState::Cancelled);
    }

    #[test]
    fn dropping_an_unfinished_handle_marks_it_failed_and_frees_the_key() {
        let jobs = registry();
        drop(jobs.start("k", "t", ProgressUnit::Items).unwrap());

        let snapshot = &jobs.list()[0];
        assert_eq!(snapshot.state, JobState::Failed);
        assert!(snapshot.error.is_some());
        assert!(jobs.start("k", "t", ProgressUnit::Items).is_ok());
    }

    #[test]
    fn dropping_after_cancel_request_counts_as_cancelled() {
        let jobs = registry();
        let job = jobs.start("k", "t", ProgressUnit::Items).unwrap();
        jobs.cancel(job.id());
        drop(job);
        assert_eq!(jobs.list()[0].state, JobState::Cancelled);
    }

    #[tokio::test]
    async fn emits_start_progress_and_finish_events() {
        let jobs = registry();
        let mut rx = jobs.subscribe();

        let job = jobs.start("k", "下载", ProgressUnit::Bytes).unwrap();
        job.set_progress(50.0, Some(100.0));
        job.set_message("正在校验");
        job.fail("网络断开");

        let states: Vec<_> = (0..4).map(|_| rx.try_recv().unwrap()).collect();
        assert_eq!(states[0].state, JobState::Running);
        assert_eq!(states[1].progress.done, 50.0);
        assert_eq!(states[2].progress.message.as_deref(), Some("正在校验"));
        assert_eq!(states[3].state, JobState::Failed);
        assert_eq!(states[3].error.as_deref(), Some("网络断开"));
    }

    #[test]
    fn progress_is_throttled_but_completion_always_emits() {
        let jobs = JobRegistry::with_throttle(Duration::from_secs(60));
        let mut rx = jobs.subscribe();
        let job = jobs.start("k", "t", ProgressUnit::Bytes).unwrap();
        let _start = rx.try_recv().unwrap();

        job.set_progress(1.0, Some(10.0)); // 第一次进度：推送
        job.set_progress(2.0, Some(10.0)); // 节流期内：不推送
        job.set_progress(3.0, Some(10.0)); // 节流期内：不推送
        job.set_progress(10.0, Some(10.0)); // 达到总量：立即推送

        assert_eq!(rx.try_recv().unwrap().progress.done, 1.0);
        assert_eq!(rx.try_recv().unwrap().progress.done, 10.0);
        assert!(rx.try_recv().is_err());
        job.succeed();
    }

    /// D6 实测：检查 3998 个文件时每个文件都报一次「x/3998」，不能每次都推送
    #[test]
    fn progress_details_are_throttled_with_the_progress() {
        let jobs = JobRegistry::with_throttle(Duration::from_secs(60));
        let mut rx = jobs.subscribe();
        let job = jobs.start("k", "t", ProgressUnit::Bytes).unwrap();
        let _start = rx.try_recv().unwrap();

        for i in 1..=500 {
            job.set_progress_detail(f64::from(i), Some(1000.0), format!("检查游戏文件 {i}/1000"));
        }
        assert_eq!(
            rx.try_recv().unwrap().progress.message.as_deref(),
            Some("检查游戏文件 1/1000")
        );
        assert!(rx.try_recv().is_err(), "节流期内的 499 次更新都不推送");
        let latest = jobs.list().into_iter().find(|j| j.key == "k").unwrap();
        assert_eq!(
            latest.progress.message.as_deref(),
            Some("检查游戏文件 500/1000"),
            "但最新值已记下"
        );
        job.succeed();
    }

    #[test]
    fn running_jobs_are_listed_before_finished_ones_and_history_is_bounded() {
        let jobs = registry();
        for i in 0..(MAX_HISTORY + 5) {
            jobs.start(format!("done:{i}"), "t", ProgressUnit::Items)
                .unwrap()
                .succeed();
        }
        let _running = jobs.start("live", "进行中", ProgressUnit::Items).unwrap();

        let list = jobs.list();
        assert_eq!(list.len(), MAX_HISTORY + 1);
        assert_eq!(list[0].key, "live");

        jobs.clear_finished();
        assert_eq!(jobs.list().len(), 1);
    }

    #[test]
    fn speed_and_eta_come_from_the_last_few_seconds() {
        let mut rate = Rate::default();
        let t0 = Instant::now();
        assert_eq!(rate.sample(t0, 0.0, Some(1000.0)), (None, None));
        assert_eq!(
            rate.sample(t0 + Duration::from_millis(500), 50.0, Some(950.0)),
            (None, None),
            "不到 0.8 秒先不报"
        );
        let (speed, eta) = rate.sample(t0 + Duration::from_secs(2), 200.0, Some(800.0));
        assert_eq!(speed, Some(100.0));
        assert_eq!(eta, Some(8.0));

        // 很久以后速度变快：只看最近 5 秒
        rate.sample(t0 + Duration::from_secs(10), 300.0, Some(700.0));
        let (speed, _) = rate.sample(t0 + Duration::from_secs(12), 700.0, Some(300.0));
        assert_eq!(speed, Some(200.0), "起点是 10 秒那次");

        // 进度往回走（重试退回已计的字节）：重新开始算
        assert_eq!(
            rate.sample(t0 + Duration::from_secs(13), 600.0, Some(400.0)),
            (None, None)
        );
        // 没在动：不报 0 B/s，也不报剩余时间
        rate.sample(t0 + Duration::from_secs(20), 600.0, Some(400.0));
        assert_eq!(
            rate.sample(t0 + Duration::from_secs(22), 600.0, Some(400.0)),
            (None, None)
        );
    }

    /// D18 实测：本地已经有的文件直接跳过，完成量一下涨很多，速度不能跟着算（跑出过 163 MB/s）；
    /// 剩余时间反过来要按完成量算（只按网络速度算时，开头报过「还剩 9 小时」）
    #[test]
    fn download_speed_counts_only_network_bytes() {
        let jobs = registry();
        let job = jobs.start("k", "t", ProgressUnit::Bytes).unwrap();
        job.set_download_progress(0.0, Some(1000.0), 0.0, 0, "下载 0/10");
        std::thread::sleep(Duration::from_millis(900));
        // 一下跳过了 900 字节的已有文件，网络只收了 9 字节
        job.set_download_progress(909.0, Some(1000.0), 9.0, 0, "下载 9/10");
        let snapshot = &jobs.list()[0];
        let speed = snapshot.speed.unwrap();
        assert!(speed < 20.0, "速度只算网络收到的：{speed}");
        assert!(
            snapshot.eta.unwrap() < 1.0,
            "剩余时间按完成量算：一秒涨了 909，剩 91 很快就好"
        );
        job.succeed();
    }

    /// 换源次数：每一步的下载各自从 0 数，任务上记的是加起来的
    #[test]
    fn fallbacks_add_up_across_steps() {
        let jobs = registry();
        let job = jobs.start("k", "t", ProgressUnit::Bytes).unwrap();
        job.set_download_source(DownloadSource::Mirror);
        job.set_step("下载加载器依赖");
        job.set_download_progress(1.0, Some(9.0), 1.0, 2, "1/9");
        job.set_download_progress(2.0, Some(9.0), 2.0, 3, "2/9");
        job.set_step("补全游戏文件");
        job.set_download_progress(1.0, Some(9.0), 1.0, 4, "1/9");
        let snapshot = &jobs.list()[0];
        assert_eq!(snapshot.fallbacks, 7);
        assert_eq!(snapshot.download_source, Some(DownloadSource::Mirror));
        job.succeed();
    }

    #[test]
    fn steps_are_recorded_once_and_reset_the_speed() {
        let jobs = registry();
        let job = jobs.start("k", "t", ProgressUnit::Bytes).unwrap();
        job.set_step("下载整合包");
        job.set_step("下载整合包");
        job.set_message("检查整合包");
        job.set_step("下载整合包文件");
        let steps: Vec<String> = jobs.list()[0]
            .steps
            .iter()
            .map(|s| s.label.clone())
            .collect();
        assert_eq!(steps, ["下载整合包", "检查整合包", "下载整合包文件"]);

        for i in 0..(MAX_STEPS + 10) {
            job.set_message(format!("安装加载器 {i}"));
        }
        let steps = &jobs.list()[0].steps;
        assert_eq!(steps.len(), MAX_STEPS, "只留最后这些步");
        assert_eq!(
            steps.last().map(|s| s.label.as_str()),
            Some(format!("安装加载器 {}", MAX_STEPS + 9).as_str())
        );
        job.succeed();
    }

    fn history_registry(path: &std::path::Path) -> JobRegistry {
        JobRegistry::with_history_sync(path.to_path_buf())
    }

    /// 真的开后台线程写：过一会儿文件里就有
    #[test]
    fn background_writer_saves_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        let jobs = JobRegistry::with_history(path.clone());
        jobs.start("k", "安装", ProgressUnit::Items)
            .unwrap()
            .succeed();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let saved = read_json::<RawHistory>(&path).ok().flatten();
            if saved.is_some_and(|f| {
                f.jobs
                    .first()
                    .is_some_and(|s| s["job"]["state"] == "succeeded")
            }) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("5 秒内后台线程没写历史");
    }

    #[test]
    fn history_survives_a_restart_with_retry_recipes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        let failed_id = {
            let jobs = history_registry(&path);
            let job = jobs
                .start_with(
                    "content:a:x",
                    "安装 X",
                    ProgressUnit::Bytes,
                    JobMeta {
                        source: JobSource::Modrinth,
                        instance_id: Some("a".to_owned()),
                        retry: Some(serde_json::json!({ "kind": "content" })),
                        ..JobMeta::default()
                    },
                )
                .unwrap();
            job.set_step("下载文件");
            let id = job.id().to_owned();
            job.fail("网络断开");
            jobs.start_with(
                "launch:a",
                "准备启动",
                ProgressUnit::Bytes,
                JobMeta {
                    transient: true,
                    ..JobMeta::default()
                },
            )
            .unwrap()
            .succeed();
            jobs.save_now();
            id
        };

        let jobs = history_registry(&path);
        assert!(jobs.list().is_empty(), "以前的不算这次的任务");
        let history = jobs.history();
        assert_eq!(history.len(), 1, "启动前的准备不进历史");
        let job = &history[0];
        assert_eq!(job.id, failed_id);
        assert_eq!(job.source, JobSource::Modrinth);
        assert_eq!(job.instance_id.as_deref(), Some("a"));
        assert_eq!(job.steps[0].label, "下载文件");
        assert!(job.retryable);
        let (_, retry) = jobs.find(&failed_id).unwrap();
        assert_eq!(retry, Some(serde_json::json!({ "kind": "content" })));

        assert!(jobs.remove(&failed_id));
        assert!(jobs.history().is_empty());
    }

    #[test]
    fn jobs_still_running_at_exit_come_back_as_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        {
            let jobs = history_registry(&path);
            let job = jobs.start("k", "新建实例", ProgressUnit::Bytes).unwrap();
            job.set_progress(10.0, Some(100.0));
            jobs.save_now();
            // 启动器被关掉：任务没机会收尾
            std::mem::forget(job);
        }
        let jobs = history_registry(&path);
        let job = &jobs.history()[0];
        assert_eq!(job.state, JobState::Interrupted);
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn clearing_forgets_old_history_but_keeps_running_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        {
            let jobs = history_registry(&path);
            jobs.start("old", "以前的", ProgressUnit::Items)
                .unwrap()
                .succeed();
            jobs.save_now();
        }
        let jobs = history_registry(&path);
        let _running = jobs.start("live", "进行中", ProgressUnit::Items).unwrap();
        jobs.clear_finished();
        jobs.save_now();
        let history = jobs.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].key, "live");
        drop(_running);

        let reopened = history_registry(&path);
        assert_eq!(reopened.history().len(), 1, "清掉的不会再从文件里回来");
    }

    #[test]
    fn one_bad_entry_does_not_lose_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        {
            let jobs = history_registry(&path);
            jobs.start("ok", "好的那条", ProgressUnit::Items)
                .unwrap()
                .succeed();
            jobs.save_now();
        }
        let mut raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        raw["jobs"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "job": { "id": "坏的", "title": "少字段" } }));
        std::fs::write(&path, raw.to_string()).unwrap();
        let history = history_registry(&path).history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "好的那条");
    }

    #[test]
    fn broken_history_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs-history.json");
        std::fs::write(&path, "{ 坏了").unwrap();
        assert!(history_registry(&path).history().is_empty());
    }
}
