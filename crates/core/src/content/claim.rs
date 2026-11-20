//! 补件（招牌特色 7「尊重作者的补件流程」）：作者禁止第三方启动器下载的文件，由用户在浏览器里下载，
//! 启动器自动认领。
//!
//! - 每个文件给出**精确下载页**（那个文件、那个版本），不是项目首页，不会下错
//! - 盯着下载文件夹：下载好就自动认领（定时扫描，不用额外依赖）；以前下过的也能认出来
//! - 也可以把文件拖进窗口、或者在系统窗口里选
//! - **按 SHA-1 核对**，对上了才算；拖错了用 CurseForge 指纹查出是哪个版本，告诉用户要的是哪个
//! - 认领的文件进内容仓库（按哈希存），下次再装同一个文件不用再下。默认复制，下载文件夹里的原文件不动；
//!   设置里打开「移走」后，在下载文件夹里认领的文件会被移走（拖进来、手动选的始终复制，
//!   免得把别的实例里的模组搬走）
//! - 必需的文件齐了就自动继续；可选的可以跳过

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::curseforge::CurseForge;
use super::fingerprint::fingerprint_file_async;
use super::store::ContentStore;
use super::{ContentError, ContentKind};
use crate::net::sha1_file;

/// 多久扫一次下载文件夹
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// 比这大的新文件不拿去查指纹（补件的文件是模组和材质包，不会这么大）
const MAX_IDENTIFY_BYTES: u64 = 512 * 1024 * 1024;

/// 一个需要用户自己下载的文件。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ManualFile {
    pub project_id: u32,
    pub file_id: u32,
    pub project_name: String,
    /// 这个版本在 CurseForge 上的显示名
    pub display_name: String,
    pub file_name: String,
    pub size: f64,
    /// 必需（不是可选）
    pub required: bool,
    pub kind: ContentKind,
    /// 精确下载页；没有时界面只能显示文件编号
    pub page_url: Option<String>,
    pub icon_url: Option<String>,
    /// 期望的 SHA-1（核对用，不发给界面）
    #[serde(skip)]
    pub sha1: String,
    /// 放进实例的相对位置，例如 `mods/xxx.jar`（不发给界面）
    #[serde(skip)]
    pub path: String,
}

/// 认领好的文件：在内容仓库里（按 SHA-1），导入时复制到实例的 `path`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    pub path: String,
    pub sha1: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ClaimState {
    /// 等用户下载
    Waiting,
    /// 已认领（哈希对上了）
    Claimed,
    /// 用户跳过了（只有可选文件能跳过）
    Skipped,
    /// 用户给的是同一个项目的别的版本
    Wrong { got: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ClaimEntry {
    pub file: ManualFile,
    pub state: ClaimState,
}

/// 给界面看的补件进度。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ClaimSnapshot {
    pub id: String,
    /// 等着这些文件的后台任务（取消安装就是取消它）
    pub job_id: String,
    pub title: String,
    pub entries: Vec<ClaimEntry>,
    /// 正在盯着的文件夹（只用来显示）
    pub watching: Vec<String>,
    /// 最近一次给用户的提示（拖错了、不是要的文件……）
    pub notice: Option<String>,
    /// 补件结束了（补齐、取消或失败），界面可以收起
    pub closed: bool,
    /// 在下载文件夹里认领的文件会被移走（不是复制）
    pub moves_claimed: bool,
}

/// 创建补件会话需要的东西。
pub struct ClaimParams {
    pub job_id: String,
    pub title: String,
    pub files: Vec<ManualFile>,
    /// 盯着的文件夹（通常是系统的「下载」文件夹）
    pub watch: Vec<PathBuf>,
    pub store: ContentStore,
    /// 用来识别拖错的文件（没有 Key 时只能说「不是要的文件」）
    pub curseforge: CurseForge,
    /// 在下载文件夹里认领的文件移走而不是复制（设置里的开关）
    pub move_claimed: bool,
}

struct State {
    entries: Vec<ClaimEntry>,
    /// 看过的文件 → （大小, 修改时间）；没变就不再看
    seen: HashMap<PathBuf, (u64, SystemTime)>,
    notice: Option<String>,
    closed: bool,
}

type Listener = Box<dyn Fn(ClaimSnapshot) + Send + Sync>;

/// 一次补件。用 `Arc` 共享：后台任务在 [`ClaimSession::wait`] 里等，界面的命令调 `offer`、`set_skipped`。
pub struct ClaimSession {
    id: String,
    job_id: String,
    title: String,
    watch: Vec<PathBuf>,
    store: ContentStore,
    curseforge: CurseForge,
    move_claimed: bool,
    started: SystemTime,
    state: Mutex<State>,
    /// 立刻再扫一次（用户拖了文件、跳过了文件）
    wake: Notify,
    /// 状态变了（等补齐的一方据此再看一次）
    changed: Notify,
    on_update: Listener,
}

/// 一个文件看下来的结果
enum Verdict {
    Claimed,
    AlreadyClaimed,
    /// 同一个项目的别的版本：（第几项, 拿到的版本名）
    WrongVersion(usize, String),
    /// 认出来了，但不是要补的：项目名 + 版本名
    NotNeeded(String),
    Unknown,
}

impl ClaimSession {
    pub fn new(
        params: ClaimParams,
        on_update: impl Fn(ClaimSnapshot) + Send + Sync + 'static,
    ) -> Arc<Self> {
        // 内容仓库里已经有的（以前补过）直接算认领
        let entries = params
            .files
            .into_iter()
            .map(|file| {
                let state = if params.store.contains(&file.sha1) {
                    ClaimState::Claimed
                } else {
                    ClaimState::Waiting
                };
                ClaimEntry { file, state }
            })
            .collect();
        Arc::new(Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            job_id: params.job_id,
            title: params.title,
            watch: params.watch,
            store: params.store,
            curseforge: params.curseforge,
            move_claimed: params.move_claimed,
            started: SystemTime::now(),
            state: Mutex::new(State {
                entries,
                seen: HashMap::new(),
                notice: None,
                closed: false,
            }),
            wake: Notify::new(),
            changed: Notify::new(),
            on_update: Box::new(on_update),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> ClaimSnapshot {
        let state = self.lock();
        ClaimSnapshot {
            id: self.id.clone(),
            job_id: self.job_id.clone(),
            title: self.title.clone(),
            entries: state.entries.clone(),
            watching: self.watch.iter().map(|d| d.display().to_string()).collect(),
            notice: state.notice.clone(),
            closed: state.closed,
            moves_claimed: self.move_claimed,
        }
    }

    fn publish(&self) {
        self.changed.notify_waiters();
        (self.on_update)(self.snapshot());
    }

    /// 结束补件（补齐、取消、失败都调用），界面收到后收起面板。
    pub fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_waiters();
        self.publish();
    }

    /// 全部处理完时（必需的都认领了，可选的认领或跳过了）返回要放进实例的文件。
    fn ready_files(&self) -> Option<Vec<LocalFile>> {
        let state = self.lock();
        let ready = state
            .entries
            .iter()
            .all(|e| matches!(e.state, ClaimState::Claimed | ClaimState::Skipped));
        ready.then(|| {
            state
                .entries
                .iter()
                .filter(|e| e.state == ClaimState::Claimed)
                .map(|e| LocalFile {
                    path: e.file.path.clone(),
                    sha1: e.file.sha1.clone(),
                })
                .collect()
        })
    }

    /// 盯着下载文件夹，直到补件结束或取消。会话一建好就该在后台跑起来：
    /// 用户可以在启动器下载其他文件的同时去浏览器里下载。
    pub async fn watch(&self, cancel: &CancellationToken) {
        loop {
            for dir in &self.watch {
                self.scan(dir).await;
            }
            if self.lock().closed {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(POLL_INTERVAL) => {}
                () = self.wake.notified() => {}
                () = cancel.cancelled() => return,
            }
        }
    }

    /// 等用户补齐，返回要放进实例的文件。
    pub async fn wait(&self, cancel: &CancellationToken) -> Result<Vec<LocalFile>, ContentError> {
        loop {
            if let Some(files) = self.ready_files() {
                return Ok(files);
            }
            if self.lock().closed {
                return Err(ContentError::Message("补件已结束".to_owned()));
            }
            // 万一错过了通知，最多晚这么一会儿再看
            tokio::select! {
                () = self.changed.notified() => {}
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
                () = cancel.cancelled() => return Err(ContentError::Message("已取消".to_owned())),
            }
        }
    }

    /// 跳过 / 取消跳过一个可选文件。必需的文件不能跳过，返回 `false`。
    pub fn set_skipped(&self, file_id: u32, skipped: bool) -> bool {
        let changed = {
            let mut state = self.lock();
            let Some(entry) = state
                .entries
                .iter_mut()
                .find(|e| e.file.file_id == file_id && !e.file.required)
            else {
                return false;
            };
            match (&entry.state, skipped) {
                (ClaimState::Waiting | ClaimState::Wrong { .. }, true) => {
                    entry.state = ClaimState::Skipped;
                    true
                }
                (ClaimState::Skipped, false) => {
                    entry.state = ClaimState::Waiting;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.wake.notify_waiters();
            self.publish();
        }
        true
    }

    /// 用户拖进来或选中的文件（文件夹会看里面一层）。每个文件都给一句结果。
    pub async fn offer(&self, paths: Vec<PathBuf>) {
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(list_candidates(&path).await.into_iter().map(|(p, _, _)| p));
            } else {
                files.push(path);
            }
        }
        let mut notices = Vec::new();
        for path in files {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !is_candidate_name(&name) {
                notices.push(format!("「{name}」不是 .jar 或 .zip 文件"));
                continue;
            }
            match self.check(&path, true, false).await {
                Verdict::Claimed => {}
                Verdict::AlreadyClaimed => notices.push(format!("「{name}」已经认领过了")),
                Verdict::WrongVersion(index, got) => {
                    let want = self.lock().entries[index].file.display_name.clone();
                    notices.push(format!("「{name}」是 {got}，要的是 {want}"));
                }
                Verdict::NotNeeded(what) => {
                    notices.push(format!("「{name}」是 {what}，这次不需要它"));
                }
                Verdict::Unknown => notices.push(format!("「{name}」不是要补的文件")),
            }
        }
        self.lock().notice = (!notices.is_empty()).then(|| notices.join("；"));
        self.wake.notify_waiters();
        self.publish();
    }

    /// 扫一遍下载文件夹：大小对得上的才算哈希；会话开始后新出现的文件拿去查是不是下错了版本。
    async fn scan(&self, dir: &Path) {
        for (path, len, modified) in list_candidates(dir).await {
            let unchanged = {
                let mut state = self.lock();
                let previous = state.seen.insert(path.clone(), (len, modified));
                previous == Some((len, modified))
            };
            if unchanged {
                continue;
            }
            let size_matches = {
                let state = self.lock();
                state
                    .entries
                    .iter()
                    .any(|e| e.state != ClaimState::Claimed && e.file.size as u64 == len)
            };
            let is_new = modified >= self.started;
            if !size_matches && !(is_new && len > 0 && len <= MAX_IDENTIFY_BYTES) {
                continue;
            }
            match self.check(&path, is_new, true).await {
                Verdict::WrongVersion(index, got) => {
                    let want = self.lock().entries[index].file.display_name.clone();
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.lock().notice =
                        Some(format!("下载文件夹里的「{name}」是 {got}，要的是 {want}"));
                    self.publish();
                }
                // 下载文件夹里本来就有各种文件，认不出来的不提示
                Verdict::Claimed
                | Verdict::AlreadyClaimed
                | Verdict::NotNeeded(_)
                | Verdict::Unknown => {}
            }
        }
    }

    /// 看一个文件：哈希对上了就认领；`identify` 为真时再用指纹查它是什么。
    /// `from_watched`：文件是在盯着的下载文件夹里找到的（只有这种才可能按设置移走）。
    async fn check(&self, path: &Path, identify: bool, from_watched: bool) -> Verdict {
        let sha1 = match sha1_file(path).await {
            Ok(sha1) => sha1,
            Err(error) => {
                tracing::warn!(%error, file = %path.display(), "读不了这个文件，跳过");
                return Verdict::Unknown;
            }
        };
        let index = {
            let state = self.lock();
            state
                .entries
                .iter()
                .position(|e| e.file.sha1.eq_ignore_ascii_case(&sha1))
        };
        if let Some(index) = index {
            if self.lock().entries[index].state == ClaimState::Claimed {
                return Verdict::AlreadyClaimed;
            }
            let moving = from_watched && self.move_claimed;
            let stored = if moving {
                self.store.take(path).await.and_then(|taken| {
                    if taken.eq_ignore_ascii_case(&sha1) {
                        Ok(())
                    } else {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "移动途中文件内容变了，请重新下载",
                        ))
                    }
                })
            } else {
                self.store.add_copy(path, &sha1).await
            };
            return match stored {
                Ok(()) => {
                    {
                        let mut state = self.lock();
                        state.entries[index].state = ClaimState::Claimed;
                        state.notice = None;
                    }
                    tracing::info!(file = %path.display(), moved = moving, "补件：认领了一个文件");
                    self.publish();
                    Verdict::Claimed
                }
                Err(error) => {
                    tracing::warn!(%error, file = %path.display(), "补件：复制进内容仓库失败");
                    self.lock().notice = Some(format!("复制文件失败：{error}"));
                    Verdict::Unknown
                }
            };
        }
        if !identify || !self.curseforge.usable() {
            return Verdict::Unknown;
        }
        self.identify(path).await
    }

    /// 用 CurseForge 指纹查一个对不上哈希的文件是什么
    async fn identify(&self, path: &Path) -> Verdict {
        let fingerprint = match fingerprint_file_async(path).await {
            Ok(fp) => fp,
            Err(error) => {
                tracing::warn!(%error, file = %path.display(), "算指纹失败");
                return Verdict::Unknown;
            }
        };
        let found = match self.curseforge.identify(&[fingerprint]).await {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "按指纹识别失败（可能断网）");
                return Verdict::Unknown;
            }
        };
        let Some(file) = found.get(&fingerprint) else {
            return Verdict::Unknown;
        };
        let wrong = {
            let mut state = self.lock();
            let index = state
                .entries
                .iter()
                .position(|e| e.file.project_id == file.mod_id && e.state != ClaimState::Claimed);
            if let Some(i) = index {
                let got = format!(
                    "{} 的 {}",
                    state.entries[i].file.project_name, file.display_name
                );
                if state.entries[i].state == ClaimState::Waiting {
                    state.entries[i].state = ClaimState::Wrong {
                        got: file.display_name.clone(),
                    };
                }
                Some((i, got))
            } else {
                None
            }
        };
        if let Some((index, got)) = wrong {
            return Verdict::WrongVersion(index, got);
        }
        let project = match self.curseforge.mods(&[file.mod_id]).await {
            Ok(mods) => mods.get(&file.mod_id).map(|m| m.name.clone()),
            Err(error) => {
                tracing::warn!(%error, "查项目名失败");
                None
            }
        };
        Verdict::NotNeeded(match project {
            Some(name) => format!("{name} 的 {}", file.display_name),
            None => file.display_name.clone(),
        })
    }
}

/// 补件只看 .jar 和 .zip（浏览器下载中的临时文件是 .crdownload、.part，不会被当成好文件）
fn is_candidate_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".jar") || lower.ends_with(".zip")
}

/// 文件夹里（只看一层）的候选文件：路径、大小、修改时间。读不了的文件夹当成空的。
async fn list_candidates(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let dir = dir.to_path_buf();
    let listed = tokio::task::spawn_blocking(move || {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| is_candidate_name(&e.file_name().to_string_lossy()))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some((e.path(), meta.len(), meta.modified().ok()?))
            })
            .collect()
    })
    .await;
    match listed {
        Ok(files) => files,
        Err(error) => {
            tracing::warn!(%error, "扫描文件夹的任务异常结束");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use reqwest::Client;

    use super::*;
    use crate::content::curseforge::LOADER_NEOFORGE;
    use crate::net::{DownloadItem, Downloader, build_client, sha1_file};

    async fn manual(dir: &Path, name: &str, body: &[u8], required: bool) -> ManualFile {
        let source = dir.join(format!("source-{name}"));
        std::fs::write(&source, body).unwrap();
        let sha1 = sha1_file(&source).await.unwrap();
        std::fs::remove_file(&source).unwrap();
        ManualFile {
            project_id: 1,
            file_id: body.len() as u32,
            project_name: "Mod".to_owned(),
            display_name: name.to_owned(),
            file_name: name.to_owned(),
            size: body.len() as f64,
            required,
            kind: ContentKind::Mod,
            page_url: None,
            icon_url: None,
            sha1,
            path: format!("mods/{name}"),
        }
    }

    fn session(
        root: &Path,
        watch: &Path,
        files: Vec<ManualFile>,
    ) -> (Arc<ClaimSession>, Arc<AtomicUsize>) {
        session_with(root, watch, files, false)
    }

    fn session_with(
        root: &Path,
        watch: &Path,
        files: Vec<ManualFile>,
        move_claimed: bool,
    ) -> (Arc<ClaimSession>, Arc<AtomicUsize>) {
        let updates = Arc::new(AtomicUsize::new(0));
        let counter = updates.clone();
        let session = ClaimSession::new(
            ClaimParams {
                job_id: "job".to_owned(),
                title: "Pack".to_owned(),
                files,
                watch: vec![watch.to_path_buf()],
                store: ContentStore::new(root.join("store")),
                curseforge: CurseForge::new(Client::new(), None),
                move_claimed,
            },
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        (session, updates)
    }

    #[tokio::test]
    async fn claims_files_from_the_watched_folder_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let a = manual(dir.path(), "a.jar", b"mod a bytes", true).await;
        let b = manual(dir.path(), "b.zip", b"pack b", false).await;
        let (session, updates) = session(dir.path(), &downloads, vec![a, b]);

        // 用户在浏览器里下载了 a（名字被浏览器改了也没关系），还下了个无关文件
        std::fs::write(downloads.join("a (1).jar"), b"mod a bytes").unwrap();
        std::fs::write(downloads.join("other.jar"), b"something else").unwrap();
        session.scan(&downloads).await;
        let snapshot = session.snapshot();
        assert_eq!(snapshot.entries[0].state, ClaimState::Claimed);
        assert_eq!(snapshot.entries[1].state, ClaimState::Waiting);
        assert!(updates.load(Ordering::SeqCst) > 0);
        assert!(downloads.join("a (1).jar").exists(), "用户的原文件不动");

        // 可选的 b 跳过 → 补齐了，只带上 a
        assert!(session.set_skipped(snapshot.entries[1].file.file_id, true));
        let cancel = CancellationToken::new();
        let files = session.wait(&cancel).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "mods/a.jar");
        assert!(ContentStore::new(dir.path().join("store")).contains(&files[0].sha1));
    }

    #[tokio::test]
    async fn required_files_cannot_be_skipped_and_wrong_files_get_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let a = manual(dir.path(), "a.jar", b"mod a bytes", true).await;
        let file_id = a.file_id;
        let (session, _) = session(dir.path(), dir.path(), vec![a]);
        assert!(!session.set_skipped(file_id, true));

        let wrong = dir.path().join("wrong.jar");
        std::fs::write(&wrong, b"not it").unwrap();
        let text = dir.path().join("readme.txt");
        std::fs::write(&text, b"x").unwrap();
        session.offer(vec![wrong, text]).await;
        let notice = session.snapshot().notice.unwrap();
        assert!(notice.contains("「wrong.jar」不是要补的文件"), "{notice}");
        assert!(notice.contains("不是 .jar 或 .zip"), "{notice}");

        let right = dir.path().join("right.jar");
        std::fs::write(&right, b"mod a bytes").unwrap();
        session.offer(vec![right.clone()]).await;
        assert_eq!(session.snapshot().entries[0].state, ClaimState::Claimed);
        assert_eq!(session.snapshot().notice, None);
        session.offer(vec![right]).await;
        assert!(session.snapshot().notice.unwrap().contains("已经认领过了"));
    }

    #[tokio::test]
    async fn moving_only_applies_to_the_watched_folder() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        let elsewhere = dir.path().join("other-instance/mods");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let a = manual(dir.path(), "a.jar", b"mod a bytes", true).await;
        let b = manual(dir.path(), "b.jar", b"mod b", true).await;
        let (session, _) = session_with(dir.path(), &downloads, vec![a, b], true);
        assert!(session.snapshot().moves_claimed);

        std::fs::write(downloads.join("a.jar"), b"mod a bytes").unwrap();
        session.scan(&downloads).await;
        assert_eq!(session.snapshot().entries[0].state, ClaimState::Claimed);
        assert!(
            !downloads.join("a.jar").exists(),
            "下载文件夹里认领的被移走"
        );

        let dragged = elsewhere.join("b.jar");
        std::fs::write(&dragged, b"mod b").unwrap();
        session.offer(vec![dragged.clone()]).await;
        assert_eq!(session.snapshot().entries[1].state, ClaimState::Claimed);
        assert!(dragged.exists(), "拖进来的始终复制，原文件不动");

        let cancel = CancellationToken::new();
        let store = ContentStore::new(dir.path().join("store"));
        for file in session.wait(&cancel).await.unwrap() {
            assert!(store.contains(&file.sha1));
        }
    }

    #[tokio::test]
    async fn files_already_in_the_store_count_as_claimed() {
        let dir = tempfile::tempdir().unwrap();
        let a = manual(dir.path(), "a.jar", b"mod a bytes", true).await;
        let store = ContentStore::new(dir.path().join("store"));
        let copy = dir.path().join("copy.jar");
        std::fs::write(&copy, b"mod a bytes").unwrap();
        store.add_copy(&copy, &a.sha1).await.unwrap();
        let (session, _) = session(dir.path(), dir.path(), vec![a]);
        assert_eq!(session.snapshot().entries[0].state, ClaimState::Claimed);
    }

    #[tokio::test]
    async fn cancelling_stops_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let a = manual(dir.path(), "a.jar", b"mod a bytes", true).await;
        let (session, _) = session(dir.path(), dir.path(), vec![a]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(session.wait(&cancel).await.is_err());
    }

    /// 真实环境检查（要联网和 `MCL_CURSEFORGE_KEY`，默认不跑）：
    /// `cargo test -p launcher-core real_wrong_version -- --ignored --nocapture`
    /// 拿 JEI（允许第三方下载）的两个相邻版本：要新的、给旧的，应该认出「同一个项目的别的版本」。
    #[tokio::test]
    #[ignore = "要联网和 MCL_CURSEFORGE_KEY"]
    async fn real_wrong_version_is_recognized() {
        let client = build_client().unwrap();
        let cf = CurseForge::from_env(client.clone());
        assert!(cf.has_key(), "没有设置 MCL_CURSEFORGE_KEY");
        let files = cf
            .mod_files(238_222, Some("1.21.1"), Some(LOADER_NEOFORGE))
            .await
            .unwrap();
        let (want, got) = (&files[0], &files[1]);
        let dir = tempfile::tempdir().unwrap();
        let wrong_path = dir.path().join(&got.file_name);
        Downloader::new(client, 2)
            .with_host_header(cf.download_header())
            .download_all(
                &[DownloadItem {
                    urls: vec![got.usable_download_url().unwrap().to_owned()],
                    path: wrong_path.clone(),
                    sha1: got.sha1(),
                    size: Some(got.file_length),
                }],
                &CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();
        let wanted = ManualFile {
            project_id: want.mod_id,
            file_id: want.id,
            project_name: "Just Enough Items".to_owned(),
            display_name: want.display_name.clone(),
            file_name: want.file_name.clone(),
            size: want.file_length as f64,
            required: true,
            kind: ContentKind::Mod,
            page_url: None,
            icon_url: None,
            sha1: want.sha1().unwrap(),
            path: format!("mods/{}", want.file_name),
        };
        let session = ClaimSession::new(
            ClaimParams {
                job_id: "job".to_owned(),
                title: "JEI".to_owned(),
                files: vec![wanted],
                watch: Vec::new(),
                store: ContentStore::new(dir.path().join("store")),
                curseforge: cf,
                move_claimed: false,
            },
            |_| {},
        );
        session.offer(vec![wrong_path]).await;
        let snapshot = session.snapshot();
        let notice = snapshot.notice.clone().unwrap();
        println!("给的：{}；要的：{}", got.display_name, want.display_name);
        println!("提示：{notice}");
        assert_eq!(
            snapshot.entries[0].state,
            ClaimState::Wrong {
                got: got.display_name.clone()
            }
        );
        assert!(notice.contains(&want.display_name), "{notice}");
    }
}
