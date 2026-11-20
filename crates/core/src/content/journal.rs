//! 改到一半能恢复（D17m，来自外部审阅）。
//!
//! 一次改好几个文件的操作（安装放进实例、拖进来装、更新、删除、批量删除、撤销、回到快照、批量启停）
//! 都先在内存里排成一份计划（[`Plan`]），动手前把每一步和要改的记录文件的**原样**写进
//! `Lumi/journal.json`，全部做完再删掉它：
//! - 中途出错（文件被占用、磁盘满、记录写不进去）：当场按相反的顺序退回，实例保持原样；
//! - 启动器被关掉：下次打开（或者下次改这个实例之前、启动这个实例之前）看到日志还在，同样退回。
//!
//! 没做完的一律退回、不接着做完：接着做很可能又碰到同一个问题；退回只用仓库里现成的文件，
//! 结果一定是「改之前」，用户再点一次就行。
//!
//! 每一步都能从磁盘上看出做没做（文件在不在、哈希对不对），所以不用每做一步写一次日志；
//! 退回本身被打断了，下次再退一次也不会出错。
//!
//! 步骤的顺序固定：先把要走的文件收进仓库，再改名（启用 / 停用），再放进新文件，最后写记录文件
//! （来源记录、光影设置、`options.txt`，改动历史放最后）。

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{ContentError, InstanceContent, io_error, json_bytes, store::copy_atomic};
use crate::fs::{AtomicWriteError, read_json, safe_join, write_atomic};
use crate::net::sha1_file;

const JOURNAL_FILE: &str = "journal.json";
const JOURNAL_VERSION: u32 = 1;

/// 日志里的一步（路径都相对实例目录，`/` 分隔）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Step {
    /// 实例里的文件收进仓库
    Take { path: String, sha1: String },
    /// 放进一个文件（新下载的，或者从仓库拿的）
    Place { path: String, sha1: String },
    /// 改名（启用 / 停用模组）
    Rename { from: String, to: String },
    /// 改一个记录文件；`original` 是改之前的内容，`None` 表示原来没有这个文件
    Write {
        path: String,
        original: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    version: u32,
    /// 这次在做什么（「更新 Sodium 等 3 项」），恢复时告诉用户
    summary: String,
    at: f64,
    steps: Vec<Step>,
}

/// 上次没做完、这次退回了的改动
#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    pub summary: String,
    pub at: f64,
}

/// 放进来的文件从哪来
#[derive(Debug)]
enum Source {
    /// 下载到临时文件夹的，挪进来
    Staged(PathBuf),
    /// 从内容仓库复制一份
    Store,
}

/// 计划里的一步（执行用，比日志里多了「从哪来」「写什么」）
#[derive(Debug)]
enum Action {
    Take {
        rel: String,
        sha1: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Place {
        rel: String,
        sha1: String,
        source: Source,
    },
    Write {
        rel: String,
        bytes: Vec<u8>,
    },
}

/// 一次改动要做的事，先在内存里排好，交给 [`InstanceContent::apply`] 执行
#[derive(Debug, Default)]
pub(super) struct Plan {
    takes: Vec<(String, String)>,
    renames: Vec<(String, String)>,
    places: Vec<(String, String, Source)>,
    writes: Vec<(String, Vec<u8>)>,
    /// 已经排进计划的路径（Windows 的文件名不分大小写，存小写）
    taken: HashSet<String>,
    placed: HashSet<String>,
}

impl Plan {
    /// 收进仓库（同一个文件只排一次）
    pub(super) fn take(&mut self, rel: String, sha1: String) {
        if self.taken.insert(rel.to_lowercase()) {
            self.takes.push((rel, sha1));
        }
    }

    pub(super) fn is_taken(&self, rel: &str) -> bool {
        self.taken.contains(&rel.to_lowercase())
    }

    /// 放进下载到临时文件夹的文件
    pub(super) fn place_staged(&mut self, rel: String, sha1: String, from: PathBuf) {
        self.placed.insert(rel.to_lowercase());
        self.places.push((rel, sha1, Source::Staged(from)));
    }

    /// 从仓库放回一份
    pub(super) fn place_stored(&mut self, rel: String, sha1: String) {
        self.placed.insert(rel.to_lowercase());
        self.places.push((rel, sha1, Source::Store));
    }

    pub(super) fn is_placed(&self, rel: &str) -> bool {
        self.placed.contains(&rel.to_lowercase())
    }

    pub(super) fn rename(&mut self, from: String, to: String) {
        self.renames.push((from, to));
    }

    /// 写一个记录文件；同一个文件写两次，后一次为准
    pub(super) fn write(&mut self, rel: String, bytes: Vec<u8>) {
        self.writes.retain(|(r, _)| !r.eq_ignore_ascii_case(&rel));
        self.writes.push((rel, bytes));
    }

    pub(super) fn has_file_changes(&self) -> bool {
        !(self.takes.is_empty() && self.renames.is_empty() && self.places.is_empty())
    }

    fn is_empty(&self) -> bool {
        !self.has_file_changes() && self.writes.is_empty()
    }

    /// 按固定顺序排好：收走 → 改名 → 放进 → 写记录
    fn into_actions(self) -> Vec<Action> {
        let mut actions = Vec::new();
        actions.extend(
            self.takes
                .into_iter()
                .map(|(rel, sha1)| Action::Take { rel, sha1 }),
        );
        actions.extend(
            self.renames
                .into_iter()
                .map(|(from, to)| Action::Rename { from, to }),
        );
        actions.extend(
            self.places
                .into_iter()
                .map(|(rel, sha1, source)| Action::Place { rel, sha1, source }),
        );
        actions.extend(
            self.writes
                .into_iter()
                .map(|(rel, bytes)| Action::Write { rel, bytes }),
        );
        actions
    }
}

/// 测试用：在某一步模拟出事
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(super) enum Fault {
    /// 做第 n 步（从 0 数）之前启动器被关掉：不退回，日志留着
    CrashAt(usize),
    /// 第 n 步出错
    FailAt(usize, io::ErrorKind),
    /// 改动日志写不进去
    NoJournal,
}

/// 读一个记录文件（文本）。没有这个文件是 `None`；不是 UTF-8 的不敢改，报错。
pub(super) fn read_text(path: &Path) -> Result<Option<String>, ContentError> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
            ContentError::Message(format!(
                "「{}」里有认不出的字符，没法安全地改",
                file_label(path)
            ))
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// 出错的原因，说成人话（文件被占用、磁盘满、没有权限……）
pub(super) fn describe(error: &ContentError) -> String {
    let io = match error {
        ContentError::Io { path, source } => Some((path.as_path(), source)),
        ContentError::Write(AtomicWriteError::Io { path, source }) => {
            Some((path.as_path(), source))
        }
        _ => None,
    };
    let Some((path, source)) = io else {
        return error.to_string();
    };
    let name = file_label(path);
    // Windows：32 被别的程序打开着、33 被锁住（游戏开着时 Java 就是这样占着模组的）
    if cfg!(windows) && matches!(source.raw_os_error(), Some(32 | 33)) {
        return format!("「{name}」被别的程序占用（可能是游戏或杀毒软件）");
    }
    match source.kind() {
        io::ErrorKind::StorageFull => "磁盘空间不够".to_owned(),
        io::ErrorKind::PermissionDenied => format!("没有权限改「{name}」（可能是只读文件）"),
        io::ErrorKind::AlreadyExists => format!("实例里已经有「{name}」"),
        io::ErrorKind::NotFound => format!("找不到「{name}」"),
        io::ErrorKind::InvalidData => format!("「{name}」：{source}"),
        _ => format!("改「{name}」时出错：{source}"),
    }
}

fn already_exists(path: &Path) -> ContentError {
    io_error(
        path,
        io::Error::new(io::ErrorKind::AlreadyExists, "已经有同名文件"),
    )
}

/// 下载好的文件挪进实例：同一个盘直接改名；跨盘就先复制成临时文件再改名（中途断了不会留下半个文件）
async fn move_in(from: &Path, to: &Path) -> io::Result<()> {
    if tokio::fs::rename(from, to).await.is_ok() {
        return Ok(());
    }
    copy_atomic(from, to).await?;
    if let Err(error) = tokio::fs::remove_file(from).await {
        tracing::debug!(%error, file = %from.display(), "删除下载临时文件失败（文件夹最后整个删）");
    }
    Ok(())
}

/// 复制、原子写留下的临时文件：`<名字>.part-<32 位十六进制>`、`.<名字>.<32 位十六进制>.tmp`
fn is_leftover_temp(name: &str) -> bool {
    let id = |s: &str| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some((_, ext)) = name.rsplit_once('.')
        && let Some(rest) = ext.strip_prefix("part-")
    {
        return id(rest);
    }
    if name.starts_with('.')
        && let Some(rest) = name.strip_suffix(".tmp")
        && let Some((_, last)) = rest.rsplit_once('.')
    {
        return id(last);
    }
    false
}

/// 开发版的验收开关（正式版里没有）：
/// - `MCL_DEBUG_CRASH_AT_STEP=3`：做第 3 步之前直接退出，模拟启动器被关掉
/// - `MCL_DEBUG_STEP_DELAY_MS=1500`：每步之前等一会儿，来得及手动关掉启动器
#[cfg(debug_assertions)]
async fn debug_hooks(index: usize, total: usize) {
    let number = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    if let Some(ms) = number("MCL_DEBUG_STEP_DELAY_MS") {
        tracing::warn!(
            step = index + 1,
            total,
            ms,
            "调试开关：这一步之前先等一会儿"
        );
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
    if number("MCL_DEBUG_CRASH_AT_STEP") == Some(index as u64 + 1) {
        tracing::error!(
            step = index + 1,
            total,
            "调试开关 MCL_DEBUG_CRASH_AT_STEP：故意在这一步之前退出"
        );
        std::process::exit(3);
    }
}

impl InstanceContent {
    pub(super) fn journal_path(&self) -> PathBuf {
        self.meta_path(JOURNAL_FILE)
    }

    /// 有没有上次没做完的改动（启动游戏前看一眼）
    pub fn has_pending(&self) -> bool {
        self.journal_path().is_file()
    }

    pub(super) fn resolve(&self, rel: &str) -> Result<PathBuf, ContentError> {
        Ok(safe_join(&self.dir, rel)?)
    }

    /// 按计划做完前面几步之后，这个位置上有没有文件
    pub(super) fn occupied(&self, plan: &Plan, rel: &str) -> bool {
        plan.is_placed(rel) || (!plan.is_taken(rel) && self.resolve(rel).is_ok_and(|p| p.exists()))
    }

    /// 改之前先看上次有没有没做完的：有就退回；退不回去就不让改（免得在一半旧一半新上接着改）
    pub(super) async fn settle(&self) -> Result<(), ContentError> {
        self.recover().await.map(|_| ())
    }

    /// 上次没做完的改动：退回改之前。没有就是 `None`。
    /// 退回失败时日志留着（下次再退），返回错误。
    pub async fn recover(&self) -> Result<Option<Recovered>, ContentError> {
        let path = self.journal_path();
        let journal: Journal = match read_json(&path) {
            Ok(None) => return Ok(None),
            Ok(Some(journal)) => journal,
            Err(error) => {
                self.set_aside(&path, &error.to_string());
                return Ok(None);
            }
        };
        if journal.version != JOURNAL_VERSION {
            self.set_aside(&path, &format!("版本 {} 认不出", journal.version));
            return Ok(None);
        }
        tracing::warn!(
            summary = %journal.summary,
            steps = journal.steps.len(),
            "上次的改动没做完，退回改之前"
        );
        if let Err(error) = self.undo(&journal).await {
            return Err(ContentError::Pending {
                summary: journal.summary,
                reason: describe(&error),
            });
        }
        self.finish_journal();
        tracing::info!(summary = %journal.summary, "已退回改之前");
        Ok(Some(Recovered {
            summary: journal.summary,
            at: journal.at,
        }))
    }

    /// 读不懂的日志（不该发生：日志是原子写的）挪到一边，不挡以后的操作
    fn set_aside(&self, path: &Path, why: &str) {
        let aside = path.with_file_name(format!("journal.broken-{}.json", crate::now_millis()));
        tracing::error!(why, aside = %aside.display(), "改动日志读不懂，挪到一边");
        if let Err(error) = std::fs::rename(path, &aside) {
            tracing::error!(%error, "挪开读不懂的改动日志失败");
        }
    }

    fn finish_journal(&self) {
        if let Err(error) = std::fs::remove_file(self.journal_path())
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::error!(%error, "删除改动日志失败：下次打开会把这次改动当成没做完退回去");
        }
    }

    /// 执行计划：先写日志，再一步步做，做完删日志；中途出错就退回改之前。
    /// `summary` 写进日志，恢复时给用户看。
    pub(super) async fn apply(&self, plan: Plan, summary: &str) -> Result<(), ContentError> {
        if plan.is_empty() {
            return Ok(());
        }
        let actions = plan.into_actions();
        let mut steps = Vec::with_capacity(actions.len());
        for action in &actions {
            steps.push(self.step_of(action)?);
        }
        let journal = Journal {
            version: JOURNAL_VERSION,
            summary: summary.to_owned(),
            at: crate::now_millis() as f64,
            steps,
        };
        #[cfg(test)]
        let journal_path = match self.fault_at(usize::MAX) {
            // 父文件夹的位置是个文件：写不进去
            Some(Fault::NoJournal) => self.meta_path("content.json").join(JOURNAL_FILE),
            _ => self.journal_path(),
        };
        #[cfg(not(test))]
        let journal_path = self.journal_path();
        // 日志都写不进去：什么都还没动，直接报错
        write_atomic(&journal_path, &json_bytes(&journal)?).map_err(|error| {
            ContentError::Reverted {
                summary: summary.to_owned(),
                reason: format!(
                    "记不下要做的事（{}）",
                    describe(&ContentError::Write(error))
                ),
            }
        })?;

        let total = actions.len();
        for (index, action) in actions.into_iter().enumerate() {
            #[cfg(debug_assertions)]
            debug_hooks(index, total).await;
            #[cfg(test)]
            {
                match self.fault_at(index) {
                    Some(Fault::CrashAt(_)) => {
                        return Err(ContentError::Message("模拟启动器被关掉".to_owned()));
                    }
                    Some(Fault::FailAt(_, kind)) => {
                        let error = io_error(Path::new("测试"), io::Error::from(kind));
                        return Err(self.revert_now(&journal, &error).await);
                    }
                    Some(Fault::NoJournal) | None => {}
                }
            }
            if let Err(error) = self.run(action).await {
                tracing::warn!(%error, step = index + 1, total, summary, "改到一半出错，退回改之前");
                return Err(self.revert_now(&journal, &error).await);
            }
        }
        self.finish_journal();
        Ok(())
    }

    #[cfg(test)]
    fn fault_at(&self, index: usize) -> Option<Fault> {
        let fault = *self.fault.lock().unwrap_or_else(|e| e.into_inner());
        fault.filter(|f| match f {
            Fault::CrashAt(n) | Fault::FailAt(n, _) => *n == index,
            Fault::NoJournal => index == usize::MAX,
        })
    }

    /// 当场退回；退不回去就把日志留着，下次打开再退
    async fn revert_now(&self, journal: &Journal, error: &ContentError) -> ContentError {
        let reason = describe(error);
        match self.undo(journal).await {
            Ok(()) => {
                self.finish_journal();
                ContentError::Reverted {
                    summary: journal.summary.clone(),
                    reason,
                }
            }
            Err(undo_error) => {
                tracing::error!(%undo_error, "退回改之前失败，日志留着，下次再退");
                ContentError::NotReverted { reason }
            }
        }
    }

    fn step_of(&self, action: &Action) -> Result<Step, ContentError> {
        Ok(match action {
            Action::Take { rel, sha1 } => Step::Take {
                path: rel.clone(),
                sha1: sha1.clone(),
            },
            Action::Rename { from, to } => Step::Rename {
                from: from.clone(),
                to: to.clone(),
            },
            Action::Place { rel, sha1, .. } => Step::Place {
                path: rel.clone(),
                sha1: sha1.clone(),
            },
            Action::Write { rel, .. } => Step::Write {
                path: rel.clone(),
                original: read_text(&self.resolve(rel)?)?,
            },
        })
    }

    async fn run(&self, action: Action) -> Result<(), ContentError> {
        match action {
            Action::Take { rel, sha1 } => {
                let path = self.resolve(&rel)?;
                self.store
                    .take_expect(&path, &sha1)
                    .await
                    .map_err(|source| io_error(&path, source))
            }
            Action::Rename { from, to } => {
                let (from, to) = (self.resolve(&from)?, self.resolve(&to)?);
                if to.exists() {
                    return Err(already_exists(&to));
                }
                tokio::fs::rename(&from, &to)
                    .await
                    .map_err(|source| io_error(&from, source))
            }
            Action::Place { rel, sha1, source } => {
                let dest = self.resolve(&rel)?;
                // 改名会盖掉同名文件：计划里该收走的都先收走了，这里还有就是意外，不盖
                if dest.exists() {
                    return Err(already_exists(&dest));
                }
                if let Some(parent) = dest.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|source| io_error(parent, source))?;
                }
                match source {
                    Source::Staged(from) => move_in(&from, &dest).await,
                    Source::Store => self.store.restore(&sha1, &dest).await,
                }
                .map_err(|source| io_error(&dest, source))
            }
            Action::Write { rel, bytes } => {
                write_atomic(&self.resolve(&rel)?, &bytes)?;
                Ok(())
            }
        }
    }

    /// 按相反的顺序退回。一步退不了就先退别的（每一步都看磁盘上的样子决定，重做不会出错），
    /// 最后报第一个错误
    async fn undo(&self, journal: &Journal) -> Result<(), ContentError> {
        let mut failed = None;
        for step in journal.steps.iter().rev() {
            if let Err(error) = self.undo_step(step).await {
                tracing::warn!(%error, ?step, "退回这一步失败，先退别的");
                failed.get_or_insert(error);
            }
        }
        self.sweep_temp_files(&journal.steps);
        failed.map_or(Ok(()), Err)
    }

    async fn undo_step(&self, step: &Step) -> Result<(), ContentError> {
        match step {
            Step::Take { path, sha1 } => {
                let target = self.resolve(path)?;
                // 文件还在：这步没做
                if target.exists() {
                    return Ok(());
                }
                if !self.store.contains(sha1) {
                    tracing::warn!(
                        path,
                        "仓库里没有这个文件，放不回去（可能收走之前就被手动删了）"
                    );
                    return Ok(());
                }
                self.store
                    .restore(sha1, &target)
                    .await
                    .map_err(|source| io_error(&target, source))
            }
            Step::Place { path, sha1 } => {
                let target = self.resolve(path)?;
                if !target.is_file() {
                    return Ok(());
                }
                let actual = sha1_file(&target)
                    .await
                    .map_err(|source| io_error(&target, source))?;
                // 内容对不上：不是这次放进来的，不动
                if !actual.eq_ignore_ascii_case(sha1) {
                    return Ok(());
                }
                tokio::fs::remove_file(&target)
                    .await
                    .map_err(|source| io_error(&target, source))
            }
            Step::Rename { from, to } => {
                let (from, to) = (self.resolve(from)?, self.resolve(to)?);
                if to.exists() && !from.exists() {
                    tokio::fs::rename(&to, &from)
                        .await
                        .map_err(|source| io_error(&to, source))?;
                }
                Ok(())
            }
            Step::Write { path, original } => {
                let target = self.resolve(path)?;
                if read_text(&target).ok().as_ref() == Some(original) {
                    return Ok(());
                }
                match original {
                    Some(text) => write_atomic(&target, text.as_bytes())?,
                    None => match std::fs::remove_file(&target) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => {
                            return Err(io_error(&target, error));
                        }
                        _ => {}
                    },
                }
                Ok(())
            }
        }
    }

    /// 被打断时复制、写文件留下的临时文件，在这次动过的文件夹里清掉
    fn sweep_temp_files(&self, steps: &[Step]) {
        let mut folders = HashSet::new();
        for step in steps {
            let rels: Vec<&str> = match step {
                Step::Take { path, .. } | Step::Place { path, .. } | Step::Write { path, .. } => {
                    vec![path]
                }
                Step::Rename { from, to } => vec![from, to],
            };
            for rel in rels {
                if let Ok(path) = self.resolve(rel)
                    && let Some(parent) = path.parent()
                {
                    folders.insert(parent.to_path_buf());
                }
            }
        }
        for folder in folders {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_leftover_temp(&name) && entry.path().is_file() {
                    match std::fs::remove_file(entry.path()) {
                        Ok(()) => tracing::info!(file = %name, "清掉了上次留下的临时文件"),
                        Err(error) => tracing::warn!(%error, file = %name, "清临时文件失败"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_leftover_temp_files_only() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(is_leftover_temp(&format!("sodium.part-{id}")));
        assert!(is_leftover_temp(&format!("a.jar.part-{id}")));
        assert!(is_leftover_temp(&format!(".content.json.{id}.tmp")));
        assert!(!is_leftover_temp("sodium.jar"));
        assert!(!is_leftover_temp("my.part-file.jar"));
        assert!(!is_leftover_temp("notes.tmp"));
        assert!(!is_leftover_temp(&format!("a.part-{}", &id[..31])));
    }

    #[test]
    fn steps_run_in_a_fixed_order() {
        let mut plan = Plan::default();
        plan.write("options.txt".to_owned(), b"x".to_vec());
        plan.place_stored("mods/b.jar".to_owned(), "b".repeat(40));
        plan.rename("mods/c.jar".to_owned(), "mods/c.jar.disabled".to_owned());
        plan.take("mods/a.jar".to_owned(), "a".repeat(40));
        plan.take("MODS/A.JAR".to_owned(), "a".repeat(40));
        plan.write("options.txt".to_owned(), b"y".to_vec());
        assert!(plan.is_taken("mods/A.jar"), "不分大小写");
        let order: Vec<&str> = plan
            .into_actions()
            .iter()
            .map(|a| match a {
                Action::Take { .. } => "take",
                Action::Rename { .. } => "rename",
                Action::Place { .. } => "place",
                Action::Write { .. } => "write",
            })
            .collect();
        assert_eq!(order, ["take", "rename", "place", "write"]);
    }

    #[test]
    fn explains_common_failures() {
        let full = ContentError::Io {
            path: PathBuf::from("mods/a.jar"),
            source: io::Error::from(io::ErrorKind::StorageFull),
        };
        assert_eq!(describe(&full), "磁盘空间不够");
        #[cfg(windows)]
        {
            let locked = ContentError::Io {
                path: PathBuf::from("mods/a.jar"),
                source: io::Error::from_raw_os_error(32),
            };
            assert_eq!(
                describe(&locked),
                "「a.jar」被别的程序占用（可能是游戏或杀毒软件）"
            );
        }
        assert_eq!(describe(&ContentError::Message("别的".to_owned())), "别的");
    }
}
