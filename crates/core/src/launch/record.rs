//! 每次启动的完整记录。
//!
//! 每个实例在 `instances/<实例>/Lumi/launches/` 下最多留 [`KEEP`] 次：
//!
//! ```text
//! launches/
//! ├─ <编号>.json        概要：什么时候、什么版本、用的 Java 和内存、当时的模组、怎么结束的
//! ├─ <编号>.log         游戏的完整输出（脱敏），一行一条，和游戏自己的 latest.log 长得一样
//! └─ <编号>.crash.txt   崩溃报告的副本（崩溃了才有）
//! ```
//!
//! 为什么要自己存：游戏的 `latest.log` 每次启动都会被覆盖，启动器内存里也只留最近 5000 行。
//! 诊断包从这里读取指定的一次启动。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::log::{LogLevel, LogLine};
use crate::content::{ContentItem, IdentitySource};
use crate::fs::{UnsafePath, read_json, safe_join, write_atomic, write_json_atomic};
use crate::instance::META_DIR;
use crate::redact::redact;

/// 每个实例保留最近几次
pub const KEEP: usize = 20;
const RECORDS_DIR: &str = "launches";
/// 一次的输出超过这么大就不再往文件里写，只留最后 [`TAIL_ENTRIES`] 条，结束时补在后面
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
const TAIL_ENTRIES: usize = 5000;
/// 崩溃报告最多复制这么多（正常的崩溃报告几十 KB）
const MAX_CRASH_REPORT: u64 = 4 * 1024 * 1024;
/// 游戏把崩溃报告写在实例的这个文件夹里
const CRASH_REPORTS: &str = "crash-reports";

/// 记录放在哪：`<实例游戏目录>/Lumi/launches`
pub fn records_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join(META_DIR).join(RECORDS_DIR)
}

/// 这一次是怎么结束的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum LaunchOutcome {
    /// 还在运行
    Running,
    /// 正常退出
    Exited,
    /// 崩溃了
    Crashed,
    /// 玩家点了「结束游戏」
    Stopped,
    /// 没启动起来（准备文件、下载 Java、启动 Java 时出错）
    Failed,
    /// 记着「还在运行」，但启动器中途被关掉了，不知道结局
    Interrupted,
}

/// 直接进的世界或服务器
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum QuickPlayRecord {
    World { folder: String },
    Server { address: String },
}

impl From<&super::args::QuickPlay> for QuickPlayRecord {
    fn from(quick_play: &super::args::QuickPlay) -> Self {
        match quick_play {
            super::args::QuickPlay::World(folder) => QuickPlayRecord::World {
                folder: folder.clone(),
            },
            super::args::QuickPlay::Server(address) => QuickPlayRecord::Server {
                address: address.clone(),
            },
        }
    }
}

/// 启动那一刻装着的一个模组
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RecordedMod {
    pub title: String,
    pub version: Option<String>,
    pub file_name: String,
    pub enabled: bool,
    pub source: Option<IdentitySource>,
}

impl From<&ContentItem> for RecordedMod {
    fn from(item: &ContentItem) -> Self {
        Self {
            title: item.title.clone(),
            version: item.version_number.clone(),
            file_name: item.file_name.clone(),
            enabled: item.enabled,
            source: item.source,
        }
    }
}

/// 一次启动的概要（`<编号>.json`，也原样给界面）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecord {
    /// 编号：开始时间（毫秒）+ 短随机串，按字符串排就是按时间排
    pub id: String,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub game_version: String,
    /// 例如「NeoForge 21.1.265」；原版为空
    pub loader: Option<String>,
    /// 用的哪个 Java（只用来显示，界面不会传回来）
    pub java_path: Option<String>,
    pub java_version: Option<String>,
    /// 给游戏的最大内存（MB）
    pub memory_mb: Option<u32>,
    pub quick_play: Option<QuickPlayRecord>,
    /// 启动那一刻装着的模组（按名字排好）
    pub mods: Vec<RecordedMod>,
    pub outcome: LaunchOutcome,
    pub exit_code: Option<i32>,
    /// 这一次进没进过世界
    pub reached_world: bool,
    /// 没启动起来时的原因（中文人话）
    pub error: Option<String>,
    /// 准备失败时的稳定错误编号。旧记录没有这一项。
    #[serde(default)]
    pub error_code: Option<String>,
    /// 有没有复制到崩溃报告
    pub has_crash_report: bool,
    /// 游戏一共输出了几条
    pub log_lines: u32,
    /// 输出太长，中间省略了几条
    pub omitted_lines: u32,
}

impl LaunchRecord {
    /// 刚开始启动时的记录（Java、内存在准备好以后补上）
    pub fn start(
        started_at: f64,
        game_version: String,
        loader: Option<String>,
        quick_play: Option<QuickPlayRecord>,
        mods: &[ContentItem],
    ) -> Self {
        let mut mods: Vec<RecordedMod> = mods.iter().map(RecordedMod::from).collect();
        mods.sort_by_key(|m| m.title.to_lowercase());
        let random = uuid::Uuid::new_v4().simple().to_string();
        Self {
            id: format!("{:013}-{}", started_at.max(0.0) as u64, &random[..6]),
            started_at,
            ended_at: None,
            game_version,
            loader,
            java_path: None,
            java_version: None,
            memory_mb: None,
            quick_play,
            mods,
            outcome: LaunchOutcome::Running,
            exit_code: None,
            reached_world: false,
            error: None,
            error_code: None,
            has_crash_report: false,
            log_lines: 0,
            omitted_lines: 0,
        }
    }
}

/// 结束时补上的信息
#[derive(Debug, Clone)]
pub struct RecordEnd {
    pub ended_at: f64,
    pub outcome: LaunchOutcome,
    pub exit_code: Option<i32>,
    pub reached_world: bool,
    pub error: Option<String>,
    /// 游戏输出里写的崩溃报告位置（只认实例自己 `crash-reports` 里的文件）
    pub crash_report: Option<String>,
}

/// 一次启动的记录正在写。
pub struct RecordWriter {
    dir: PathBuf,
    instance_dir: PathBuf,
    record: LaunchRecord,
    log: Option<BufWriter<File>>,
    written: u64,
    /// 写满以后的最后几条（结束时补在文件末尾）
    tail: VecDeque<String>,
    started: Instant,
    max_bytes: u64,
    tail_cap: usize,
}

impl RecordWriter {
    /// 开始记一次启动：先删掉太旧的记录（只留最近 [`KEEP`] 次，包括这一次），再写概要、开日志文件。
    pub fn begin(instance_dir: &Path, record: LaunchRecord) -> std::io::Result<Self> {
        let dir = records_dir(instance_dir);
        std::fs::create_dir_all(&dir)?;
        prune(&dir, KEEP.saturating_sub(1));
        let log = File::create(dir.join(format!("{}.log", record.id)))?;
        let writer = Self {
            dir,
            instance_dir: instance_dir.to_path_buf(),
            record,
            log: Some(BufWriter::with_capacity(64 * 1024, log)),
            written: 0,
            tail: VecDeque::new(),
            started: Instant::now(),
            max_bytes: MAX_LOG_BYTES,
            tail_cap: TAIL_ENTRIES,
        };
        writer.save();
        Ok(writer)
    }

    pub fn id(&self) -> &str {
        &self.record.id
    }

    /// 准备好了：记下用的 Java 和内存
    pub fn set_launch(&mut self, java_path: &Path, java_version: &str, memory_mb: u32) {
        self.record.java_path = Some(java_path.display().to_string());
        self.record.java_version = Some(java_version.to_owned());
        self.record.memory_mb = Some(memory_mb);
        self.save();
    }

    pub fn set_failure_code(&mut self, code: &str) {
        self.record.error_code = Some(code.to_owned());
    }

    /// 这一次进过世界（立刻写进概要：启动器中途被关掉也知道进过）
    pub fn mark_reached_world(&mut self) {
        if !self.record.reached_world {
            self.record.reached_world = true;
            self.save();
        }
    }

    /// 追加一批游戏输出（脱敏后写入）。写文件失败只记一次日志，之后不再写，不影响游戏。
    pub fn append(&mut self, lines: &[LogLine]) {
        if lines.is_empty() {
            return;
        }
        let elapsed = self.started.elapsed().as_secs();
        for line in lines {
            let entry = format_entry(elapsed, line);
            self.record.log_lines = self.record.log_lines.saturating_add(1);
            self.written += entry.len() as u64 + 1;
            if self.written <= self.max_bytes {
                self.write_entry(&entry);
            } else {
                self.tail.push_back(entry);
                if self.tail.len() > self.tail_cap {
                    self.tail.pop_front();
                    self.record.omitted_lines = self.record.omitted_lines.saturating_add(1);
                }
            }
        }
        // 每批都落盘：启动器中途被关掉，已经写的也还在
        if let Some(log) = &mut self.log
            && let Err(error) = log.flush()
        {
            tracing::warn!(%error, "写启动记录失败，这一次后面的输出不再记录");
            self.log = None;
        }
    }

    fn write_entry(&mut self, entry: &str) {
        let Some(log) = &mut self.log else { return };
        if let Err(error) = writeln!(log, "{entry}") {
            tracing::warn!(%error, "写启动记录失败，这一次后面的输出不再记录");
            self.log = None;
        }
    }

    /// 结束：补上省略后的最后几条、复制崩溃报告、写好概要。返回最终的概要。
    pub fn finish(self, end: RecordEnd) -> LaunchRecord {
        self.finish_with_redactor(end, |text| redact(text).into_owned())
    }

    /// 保存带游戏进程的记录，同时从崩溃报告中移除这次启动的完整游戏令牌。
    pub fn finish_with_launch(
        self,
        end: RecordEnd,
        launch: &super::PreparedLaunch,
    ) -> LaunchRecord {
        self.finish_with_redactor(end, |text| launch.redact_output(text).into_owned())
    }

    fn finish_with_redactor(
        mut self,
        end: RecordEnd,
        redact_report: impl FnOnce(&str) -> String,
    ) -> LaunchRecord {
        if !self.tail.is_empty() {
            let note = LogLine {
                level: LogLevel::Warn,
                thread: Some("启动器".to_owned()),
                message: format!(
                    "输出太长，中间省略了 {} 条（只留开头和最后 {} 条）",
                    self.record.omitted_lines,
                    self.tail.len()
                ),
            };
            let note = format_entry(self.started.elapsed().as_secs(), &note);
            self.write_entry(&note);
            let tail = std::mem::take(&mut self.tail);
            for entry in &tail {
                self.write_entry(entry);
            }
        }
        if let Some(mut log) = self.log.take()
            && let Err(error) = log.flush()
        {
            tracing::warn!(%error, "写启动记录失败");
        }

        if let Some(printed) = end.crash_report.as_deref() {
            self.record.has_crash_report = self.copy_crash_report(printed, redact_report);
        }
        self.record.ended_at = Some(end.ended_at);
        self.record.outcome = end.outcome;
        self.record.exit_code = end.exit_code;
        self.record.reached_world |= end.reached_world;
        self.record.error = end.error;
        self.save();
        self.record
    }

    fn copy_crash_report(&self, printed: &str, redact_report: impl FnOnce(&str) -> String) -> bool {
        let Some(source) = crash_report_in(&self.instance_dir, printed) else {
            tracing::warn!(report = %printed, "崩溃报告不在实例的 crash-reports 里，不复制");
            return false;
        };
        let text = match read_limited(&source, MAX_CRASH_REPORT) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(%error, report = %source.display(), "读取崩溃报告失败");
                return false;
            }
        };
        let target = self.dir.join(format!("{}.crash.txt", self.record.id));
        match write_atomic(&target, redact_report(&text).as_bytes()) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "复制崩溃报告失败");
                false
            }
        }
    }

    fn save(&self) {
        let path = self.dir.join(format!("{}.json", self.record.id));
        if let Err(error) = write_json_atomic(&path, &self.record) {
            tracing::warn!(%error, "保存启动记录失败");
        }
    }
}

/// 游戏输出里写的崩溃报告位置 → 实例 `crash-reports` 里同名的文件（只取文件名，不信任前面的路径）
fn crash_report_in(instance_dir: &Path, printed: &str) -> Option<PathBuf> {
    let name = printed
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty())?;
    let path = safe_join(&instance_dir.join(CRASH_REPORTS), name).ok()?;
    path.is_file().then_some(path)
}

fn read_limited(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path)?.take(max).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 一条输出写成一行：`[时:分:秒] [线程/级别]: 内容`。时间从启动开始算。
/// 内容里的换行（报错的调用栈）原样写，读回来时没有这个开头的行算上一条的续行。
fn format_entry(elapsed_secs: u64, line: &LogLine) -> String {
    let (h, m, s) = (
        elapsed_secs / 3600,
        elapsed_secs / 60 % 60,
        elapsed_secs % 60,
    );
    let thread = line.thread.as_deref().unwrap_or("");
    let level = level_name(line.level);
    let text = format!(
        "[{h:02}:{m:02}:{s:02}] [{thread}/{level}]: {}",
        line.message.trim_end()
    );
    redact(&text).into_owned()
}

fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
        LogLevel::Fatal => "FATAL",
    }
}

fn parse_level(text: &str) -> Option<LogLevel> {
    Some(match text {
        "TRACE" => LogLevel::Trace,
        "DEBUG" => LogLevel::Debug,
        "INFO" => LogLevel::Info,
        "WARN" => LogLevel::Warn,
        "ERROR" => LogLevel::Error,
        "FATAL" => LogLevel::Fatal,
        _ => return None,
    })
}

/// 读回 [`format_entry`] 写的一行；不是这个格式时返回 `None`（续行）
fn parse_entry(line: &str) -> Option<LogLine> {
    let rest = line.strip_prefix('[')?;
    let (time, rest) = rest.split_once("] [")?;
    if time.len() < 8 || !time.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return None;
    }
    let (tag, message) = rest.split_once("]: ")?;
    let (thread, level) = tag.rsplit_once('/')?;
    Some(LogLine {
        level: parse_level(level)?,
        thread: (!thread.is_empty()).then(|| thread.to_owned()),
        message: message.to_owned(),
    })
}

/// 界面看一次启动的输出
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RecordLog {
    pub lines: Vec<LogLine>,
    /// 文件里一共几条
    pub total: u32,
    /// 太多了只给最后的这些，前面跳过了几条（完整的在文件里）
    pub skipped: u32,
}

/// 编号只认 [`LaunchRecord::start`] 生成的样子（数字、小写字母、`-`），再经过 `safe_join`
pub(super) fn record_file(dir: &Path, id: &str, suffix: &str) -> Result<PathBuf, UnsafePath> {
    let valid = !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'-');
    if !valid {
        return Err(UnsafePath::InvalidName(id.to_owned()));
    }
    safe_join(dir, &format!("{id}{suffix}"))
}

/// 一次启动的游戏输出文件（「在文件夹中显示」用）
pub fn log_file(instance_dir: &Path, id: &str) -> Result<PathBuf, UnsafePath> {
    record_file(&records_dir(instance_dir), id, ".log")
}

/// 读一次启动的输出，最多给最后 `max` 条。
pub fn read_log(instance_dir: &Path, id: &str, max: usize) -> std::io::Result<RecordLog> {
    let path = log_file(instance_dir, id).map_err(std::io::Error::other)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<LogLine> = Vec::new();
    for raw in text.lines() {
        match parse_entry(raw) {
            Some(line) => lines.push(line),
            None => match lines.last_mut() {
                Some(last) => {
                    last.message.push('\n');
                    last.message.push_str(raw);
                }
                None => lines.push(LogLine {
                    level: LogLevel::Info,
                    thread: None,
                    message: raw.to_owned(),
                }),
            },
        }
    }
    let total = lines.len();
    let skipped = total.saturating_sub(max);
    lines.drain(..skipped);
    Ok(RecordLog {
        lines,
        total: u32::try_from(total).unwrap_or(u32::MAX),
        skipped: u32::try_from(skipped).unwrap_or(u32::MAX),
    })
}

/// 一次启动复制下来的崩溃报告；没有时返回 `None`。
pub fn read_crash_report(instance_dir: &Path, id: &str) -> std::io::Result<Option<String>> {
    let path =
        record_file(&records_dir(instance_dir), id, ".crash.txt").map_err(std::io::Error::other)?;
    match read_limited(&path, MAX_CRASH_REPORT) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// 最近的记录，新的在前。`running` 是现在正在运行的那一次的编号：
/// 其他还写着「运行中」的，是启动器中途被关掉了，改成「不知道结局」。读不了的跳过并记日志。
pub fn list(instance_dir: &Path, running: Option<&str>) -> Vec<LaunchRecord> {
    let dir = records_dir(instance_dir);
    let mut records: Vec<LaunchRecord> = record_ids(&dir)
        .into_iter()
        .filter_map(|id| {
            let path = dir.join(format!("{id}.json"));
            match read_json::<LaunchRecord>(&path) {
                Ok(record) => record,
                Err(error) => {
                    tracing::warn!(%error, file = %path.display(), "读取启动记录失败，跳过");
                    None
                }
            }
        })
        .collect();
    for record in &mut records {
        if record.outcome == LaunchOutcome::Running && running != Some(record.id.as_str()) {
            record.outcome = LaunchOutcome::Interrupted;
        }
    }
    records.sort_by(|a, b| b.started_at.total_cmp(&a.started_at));
    records
}

/// 文件夹里所有记录的编号（从小到大，也就是从旧到新）
fn record_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_owned)
        })
        .filter(|id| record_file(dir, id, ".json").is_ok())
        .collect();
    ids.sort();
    ids
}

/// 只留最新的 `keep` 次，旧的连同输出、崩溃报告一起删
fn prune(dir: &Path, keep: usize) {
    let ids = record_ids(dir);
    let extra = ids.len().saturating_sub(keep);
    for id in &ids[..extra] {
        for suffix in [".json", ".log", ".crash.txt"] {
            let path = dir.join(format!("{id}{suffix}"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(%error, file = %path.display(), "删除旧的启动记录失败")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: LogLevel, thread: Option<&str>, message: &str) -> LogLine {
        LogLine {
            level,
            thread: thread.map(str::to_owned),
            message: message.to_owned(),
        }
    }

    fn record(at: f64) -> LaunchRecord {
        LaunchRecord::start(
            at,
            "1.21.1".into(),
            Some("NeoForge 21.1.265".into()),
            Some(QuickPlayRecord::World {
                folder: "新的世界".into(),
            }),
            &[],
        )
    }

    fn end(outcome: LaunchOutcome) -> RecordEnd {
        RecordEnd {
            ended_at: 2.0,
            outcome,
            exit_code: Some(0),
            reached_world: true,
            error: None,
            crash_report: None,
        }
    }

    #[test]
    fn output_round_trips_with_threads_levels_and_stack_traces() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        let lines = [
            line(LogLevel::Info, Some("Render thread"), "Setting user: Steve"),
            line(
                LogLevel::Error,
                Some("Worker-Main-1"),
                "boom\njava.lang.RuntimeException: x\n\tat a.b(C.java:1)",
            ),
            line(LogLevel::Warn, None, "WARN StatusConsoleListener 没有线程"),
            line(
                LogLevel::Info,
                Some("Render thread"),
                "带 [方括号] 和 a/b: 的内容",
            ),
        ];
        writer.append(&lines);
        let id = writer.id().to_owned();
        let done = writer.finish(end(LaunchOutcome::Exited));
        assert_eq!(done.log_lines, 4);
        assert_eq!(done.outcome, LaunchOutcome::Exited);

        let log = read_log(dir.path(), &id, 100).unwrap();
        assert_eq!(log.total, 4);
        assert_eq!(log.skipped, 0);
        assert_eq!(log.lines, lines);
    }

    #[test]
    fn tokens_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        writer.append(&[line(
            LogLevel::Info,
            Some("main"),
            "args: --accessToken abc123 --username Steve",
        )]);
        let id = writer.id().to_owned();
        writer.finish(end(LaunchOutcome::Exited));
        let text = std::fs::read_to_string(log_file(dir.path(), &id).unwrap()).unwrap();
        assert!(!text.contains("abc123"), "{text}");
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn long_output_keeps_the_head_and_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        writer.max_bytes = 200;
        writer.tail_cap = 3;
        let lines: Vec<LogLine> = (0..20)
            .map(|i| line(LogLevel::Info, Some("t"), &format!("第 {i} 条")))
            .collect();
        writer.append(&lines);
        let id = writer.id().to_owned();
        let done = writer.finish(end(LaunchOutcome::Crashed));
        assert_eq!(done.log_lines, 20);

        let log = read_log(dir.path(), &id, 1000).unwrap();
        let messages: Vec<&str> = log.lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(messages.first(), Some(&"第 0 条"));
        assert_eq!(
            &messages[messages.len() - 3..],
            ["第 17 条", "第 18 条", "第 19 条"]
        );
        let note = messages[messages.len() - 4];
        assert!(
            note.contains(&format!("省略了 {} 条", done.omitted_lines)),
            "{note}"
        );
        let head = messages.len() - 4;
        assert_eq!(head as u32 + done.omitted_lines + 3, 20);
    }

    #[test]
    fn read_log_gives_only_the_last_lines_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        let lines: Vec<LogLine> = (0..10)
            .map(|i| line(LogLevel::Info, None, &i.to_string()))
            .collect();
        writer.append(&lines);
        let id = writer.id().to_owned();
        writer.finish(end(LaunchOutcome::Exited));
        let log = read_log(dir.path(), &id, 4).unwrap();
        assert_eq!((log.total, log.skipped), (10, 6));
        assert_eq!(log.lines[0].message, "6");
    }

    #[test]
    fn keeps_only_the_latest_twenty() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..25 {
            let writer = RecordWriter::begin(dir.path(), record(1_000.0 + f64::from(i))).unwrap();
            writer.finish(end(LaunchOutcome::Exited));
        }
        let records = list(dir.path(), None);
        assert_eq!(records.len(), KEEP);
        assert_eq!(records[0].started_at, 1_024.0, "新的在前");
        assert_eq!(records[KEEP - 1].started_at, 1_005.0);
        let logs = std::fs::read_dir(records_dir(dir.path()))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".log"))
            .count();
        assert_eq!(logs, KEEP, "旧记录的输出也删了");
    }

    #[test]
    fn stale_running_records_become_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let old = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        let old_id = old.id().to_owned();
        drop(old); // 启动器中途被关掉：没有 finish
        let now = RecordWriter::begin(dir.path(), record(2.0)).unwrap();
        let now_id = now.id().to_owned();

        let records = list(dir.path(), Some(&now_id));
        let outcome = |id: &str| records.iter().find(|r| r.id == id).unwrap().outcome;
        assert_eq!(outcome(&now_id), LaunchOutcome::Running);
        assert_eq!(outcome(&old_id), LaunchOutcome::Interrupted);
    }

    #[test]
    fn copies_crash_reports_only_from_the_instance() {
        let dir = tempfile::tempdir().unwrap();
        let reports = dir.path().join(CRASH_REPORTS);
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("crash-2026-11-12_15.00.00-client.txt"),
            "---- Minecraft Crash Report ----\n--accessToken secret",
        )
        .unwrap();

        let writer = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        let id = writer.id().to_owned();
        let mut crashed = end(LaunchOutcome::Crashed);
        crashed.crash_report =
            Some(r"D:\somewhere\else\crash-reports\crash-2026-11-12_15.00.00-client.txt".into());
        let done = writer.finish(crashed);
        assert!(
            done.has_crash_report,
            "按文件名在实例自己的 crash-reports 里找"
        );
        let text = read_crash_report(dir.path(), &id).unwrap().unwrap();
        assert!(text.starts_with("---- Minecraft Crash Report ----"));
        assert!(!text.contains("secret"));

        let writer = RecordWriter::begin(dir.path(), record(2.0)).unwrap();
        let mut outside = end(LaunchOutcome::Crashed);
        outside.crash_report = Some(r"C:\Windows\win.ini".into());
        assert!(!writer.finish(outside).has_crash_report);
        assert_eq!(crash_report_in(dir.path(), "..\\..\\secret.txt"), None);
    }

    #[test]
    fn masks_the_game_token_in_the_saved_report_and_export() {
        use std::io::{Cursor, Read};
        use std::time::Duration;

        use crate::auth::{LaunchAccount, LaunchAuthority};
        use crate::launch::{PreparedLaunch, diagnostics};

        let dir = tempfile::tempdir().unwrap();
        let reports = dir.path().join(CRASH_REPORTS);
        std::fs::create_dir_all(&reports).unwrap();
        let report_name = "crash-2026-11-12_15.00.00-client.txt";
        std::fs::write(
            reports.join(report_name),
            "---- Minecraft Crash Report ----\nplain game token fixture-game-token",
        )
        .unwrap();

        let account = LaunchAccount::for_test(
            &LaunchAuthority::default(),
            Instant::now() + Duration::from_secs(60),
        );
        let launch = PreparedLaunch::fixture(
            account,
            PathBuf::from("not-a-test-executable"),
            vec![],
            dir.path().to_path_buf(),
            vec![],
        );
        let writer = RecordWriter::begin(dir.path(), record(1.0)).unwrap();
        let id = writer.id().to_owned();
        let mut crashed = end(LaunchOutcome::Crashed);
        crashed.crash_report = Some(report_name.to_owned());
        assert!(writer.finish_with_launch(crashed, &launch).has_crash_report);

        let report = read_crash_report(dir.path(), &id).unwrap().unwrap();
        assert!(report.contains("plain game token <redacted>"));
        assert!(!report.contains("fixture-game-token"));

        let archive = diagnostics::export(dir.path(), &id, None).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut report = String::new();
        zip.by_name("crash-report.txt")
            .unwrap()
            .read_to_string(&mut report)
            .unwrap();
        assert!(report.contains("plain game token <redacted>"));
        assert!(!report.contains("fixture-game-token"));
    }
    #[test]
    fn rejects_record_ids_that_are_not_ours() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "../x", "..\\x", "C:x", "abc.json", "ABC"] {
            assert!(log_file(dir.path(), bad).is_err(), "{bad}");
        }
        assert!(log_file(dir.path(), "1731400000000-ab12cd").is_ok());
    }
}
