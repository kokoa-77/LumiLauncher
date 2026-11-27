use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use launcher_core::config::{DownloadSettings, ProxySettings, SettingsStore};
use launcher_core::content::cache::TtlCache;
use launcher_core::content::claim::ClaimSession;
use launcher_core::content::curseforge::CurseForge;
use launcher_core::content::detail::ProjectDetail;
use launcher_core::content::favorites::Favorites;
use launcher_core::content::modrinth::SearchPage;
use launcher_core::install::neoforge::NeoForgeVersion;
use launcher_core::java::{JavaLocator, JavaPrefsStore};
use launcher_core::jobs::{JobHandle, JobRegistry};
use launcher_core::meta::GameVersionEntry;
use launcher_core::modpack::PackPlan;
use launcher_core::modpack::export::ExportPlan;
use launcher_core::modpack::update::PackUpdatePlan;
use launcher_core::net::{
    DownloadEngine, FailedDownloadEntry, FailureDiagnosis, FileFailure, HttpError,
    PartialFailureReceipt, ProxyWatcher, RootCauseKind,
};

use crate::error::AppError;
use launcher_core::paths::GameDir;
use launcher_core::upgrade::UpgradePlan;
use reqwest::Client;

use crate::sessions::Sessions;
use crate::window::Backdrop;

/// 资源中心缓存的有效期：同样的搜索、同一个项目的详情 10 分钟内直接用上次的结果
pub const DISCOVER_CACHE_TTL: Duration = Duration::from_secs(600);

/// 失败回执缓存条目：部分失败文件列表、根因诊断和重试计数
pub type FailureReceiptEntry = (PartialFailureReceipt, Option<FailureDiagnosis>, u32);

/// 托管在 Tauri 里的全局状态。
pub struct AppState {
    pub data_dir: PathBuf,
    pub settings: SettingsStore,
    pub jobs: JobRegistry,
    /// 前端是否已经报告第一帧画好
    pub ui_ready: AtomicBool,
    /// 前端当前界面语言（`zh-CN` / `en`，供「默认游戏语言：跟随启动器」解析）
    pub ui_language: Mutex<String>,
    pub backdrop: Mutex<Backdrop>,
    /// 当前游戏目录（.minecraft）。界面只拿实例 ID，路径都在这里解析；用 [`AppState::game`] 读
    pub game_dir: Mutex<GameDir>,
    /// 游戏目录是不是由环境变量 `MCL_GAME_DIR` 指定的（开发时用，界面里不能改）
    pub game_dir_from_env: bool,
    /// 假装电脑上没有 Java，只用启动器自己下载的（开发时用环境变量 `MCL_IGNORE_SYSTEM_JAVA` 打开，
    /// 用来验收「没装 Java 也能一键启动」）
    pub ignore_system_java: bool,
    pub client: RwLock<Client>,
    /// 专用于 Microsoft / Xbox / Mojang 认证与官方材质请求的客户端（启用 DoH + 代理 + 禁重定向）
    pub auth_client: RwLock<Client>,
    /// 全局下载引擎单例（托管三池 HTTP 客户端、双优先级网络信号量与磁盘信号量）
    pub engine: std::sync::Arc<DownloadEngine>,
    /// 3 秒系统代理热感知与休眠唤醒断连守卫
    pub proxy_watcher: std::sync::Arc<ProxyWatcher>,
    /// DoH 开关（`client` 用它解析域名；设置里改了在这里开关）
    pub doh: std::sync::Arc<launcher_core::net::Doh>,
    /// CurseForge 客户端（Key 在里面，界面拿不到）
    pub curseforge: CurseForge,
    /// 系统的「下载」文件夹（补件时盯着这里）
    pub downloads_dir: Option<PathBuf>,
    /// 正在进行的补件，按开始顺序
    pub claims: Mutex<Vec<Arc<ClaimSession>>>,
    /// 交给界面显示的本地图片（编号 ↔ 路径）
    pub images: crate::images::Images,
    pub sessions: Sessions,
    /// 版本列表缓存（启动器运行期间有效）
    pub catalog: Catalog,
    /// 本地导入：已经检查过、等用户确认的整合包（键是一次性的令牌）
    pub pending_imports: Mutex<HashMap<String, PackPlan>>,
    /// 实例版本切换/升级向导：已经预检完毕、等用户确认的升级计划（一次性令牌）
    pub pending_upgrades: Mutex<HashMap<String, UpgradePlan>>,
    /// 实例版本切换/升级向导：进行中的预检请求取消令牌（按前端 request_id 索引）
    pub upgrade_preview_cancels: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
    /// 整合包无损更新：已完成三方比对、等用户确认的更新计划（一次性令牌）
    pub pending_pack_updates: Mutex<HashMap<String, PackUpdatePlan>>,
    /// 整合包导出：已本地扫描完毕、可选已匹配外链的导出计划（按弹窗令牌缓存）
    pub pending_exports: Mutex<HashMap<String, PendingExport>>,
    /// 拖进窗口、还没装也没取消的文件（一次性编号 → 认出来的结果）
    pub drops: Mutex<HashMap<String, crate::drops::PendingDrop>>,
    /// 资源中心的搜索结果（键里带平台、类型、关键词、版本、加载器、第几页）
    pub search_cache: TtlCache<SearchPage>,
    /// 项目详情
    pub detail_cache: TtlCache<ProjectDetail>,
    /// 收藏夹（存在应用数据目录的 favorites.json）
    pub favorites: Favorites,
    /// 用户对 Java 的选择：手动添加的、每个大版本的默认（应用数据目录的 java.json）
    pub java_prefs: JavaPrefsStore,
    /// 我们改过哪些程序的 Windows 显卡设置（关掉「高性能显卡」时按它撤回）
    pub gpu_ledger: PathBuf,
    /// 导出完的整合包：任务 ID → 文件（「打开所在文件夹」用，界面只拿任务 ID）
    pub exports: Mutex<HashMap<String, PathBuf>>,
    /// 同一个实例同一时间只做一件改文件的事
    pub turns: crate::turns::InstanceTurns,
    /// 启动时退回的没做完的改动，等界面来拿
    pub recovery: crate::recovery::RecoveryNotes,
    /// 装过的文件教会的中文名、两个平台的关联（按游戏目录缓存，识别缓存变了才重读）
    pub names: launcher_core::content::names::NameBook,
    /// 挂上的别家目录、导入候选
    pub linked: crate::linked::Linked,
    /// 账户与皮肤
    pub accounts: crate::accounts::Accounts,
    /// 部分失败回执（仅留后端内存，保留 `DownloadItem` 以支持「仅重试失败 (N)」）
    pub failure_receipts: Mutex<HashMap<String, FailureReceiptEntry>>,
}

impl AppState {
    /// 当前游戏目录（复制一份，改目录不影响正在进行的任务）
    /// 现在这个游戏目录的中文名、两个平台关联（搜索结果、收藏夹、详情页补中文名用）
    pub fn learned_names(&self) -> std::sync::Arc<launcher_core::content::names::Learned> {
        let store = self
            .game()
            .root()
            .join(launcher_core::content::store::STORE_DIR);
        self.names.learned(&store)
    }

    pub fn game(&self) -> GameDir {
        self.game_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 去哪里找 Java：电脑上已有的 + 启动器下载的（放在游戏目录里）+ 用户手动添加的，
    /// 每个大版本按用户选的默认来
    pub fn java_locator(&self) -> JavaLocator {
        JavaLocator {
            use_system: !self.ignore_system_java,
            ..JavaLocator::for_game(self.game().root())
        }
        .with_prefs(&self.java_prefs.get())
    }

    pub fn set_game(&self, game: GameDir) {
        *self.game_dir.lock().unwrap_or_else(|e| e.into_inner()) = game;
    }

    /// 取当前生效的 HTTP 客户端（克隆极轻，底层共享连接池）。
    pub fn http_client(&self) -> Client {
        self.client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 取当前生效的官方认证 HTTP 客户端。
    pub fn auth_http_client(&self) -> Client {
        self.auth_client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 代理设置变更后立即重建并热替换 HTTP 客户端（同步更新 DoH 客户端、认证客户端与 CurseForge 客户端）。
    pub fn apply_proxy(&self, proxy: &ProxySettings) -> Result<(), HttpError> {
        let snap = self.proxy_watcher.snapshot();
        self.doh.sync_proxy_with_snapshot(proxy, Some(&snap))?;
        let next = self.engine.rebuild_clients(proxy, Some(&snap))?;
        let next_auth =
            launcher_core::net::build_auth_client(self.doh.clone(), proxy, Some(&snap))?;
        self.proxy_watcher.bump_epoch();
        self.doh.on_network_epoch_change();
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        *self.auth_client.write().unwrap_or_else(|e| e.into_inner()) = next_auth;
        self.curseforge.set_client(next);
        Ok(())
    }

    /// 供短接口建连出错时 0ms 即时复查一次系统代理，若代理刚切换同步刷新全局客户端并返回新 `Client`。
    pub fn refresh_proxy_if_changed(&self) -> Option<Client> {
        let next = self.engine.refresh_proxy_if_changed()?;
        let proxy = self.settings.get().download.proxy;
        let snap = self.proxy_watcher.snapshot();
        if let Ok(next_auth) =
            launcher_core::net::build_auth_client(self.doh.clone(), &proxy, Some(&snap))
        {
            *self.auth_client.write().unwrap_or_else(|e| e.into_inner()) = next_auth;
        }
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        self.curseforge.set_client(next.clone());
        Some(next)
    }

    /// 热同步设置页修改的下载调优参数到全局下载引擎。
    pub fn apply_download_settings(&self, cfg: &DownloadSettings) {
        self.engine.apply_settings(cfg);
    }

    /// 将失败文件清单收敛为诊断卡片与后端回执，并结束任务。
    pub fn record_download_failures(
        &self,
        job_id: &str,
        step_label: &str,
        failures: &[FileFailure],
    ) -> (u32, Option<FailureDiagnosis>) {
        let failed_entries: Vec<FailedDownloadEntry> = failures
            .iter()
            .filter_map(|f| {
                let item = f.item.clone()?;
                Some(FailedDownloadEntry {
                    item,
                    cause: f.cause,
                    reason: f.reason.clone(),
                    attempts: f.attempts,
                    last_http_status: f.last_http_status,
                    has_resumable_part: f.has_resumable_part,
                })
            })
            .collect();
        let top_diag = failures.iter().find_map(|f| f.diagnosis.clone());
        if !failed_entries.is_empty() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let count = u32::try_from(failures.len()).unwrap_or(u32::MAX);
            let receipt = PartialFailureReceipt {
                job_id: job_id.to_owned(),
                step_label: step_label.to_owned(),
                created_at_ms: now_ms,
                total_files: failed_entries.len() as u64,
                succeeded_files: 0,
                reused_files: 0,
                failed_entries,
            };
            let mut guard = self
                .failure_receipts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if guard.len() >= 32
                && let Some(oldest) = guard
                    .iter()
                    .min_by_key(|(_, (r, _, _))| r.created_at_ms)
                    .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest);
            }
            guard.insert(job_id.to_owned(), (receipt, top_diag.clone(), count));
        }
        if let Some(ref d) = top_diag {
            self.engine.metrics().set_diagnosis(Some(d.clone()));
        } else if let Some(first) = failures.first()
            && first.cause != RootCauseKind::Unknown
        {
            let fallback_diag = self.engine.runtime_metrics().active_diagnosis;
            return (
                u32::try_from(failures.len()).unwrap_or(u32::MAX),
                fallback_diag,
            );
        }
        (u32::try_from(failures.len()).unwrap_or(u32::MAX), top_diag)
    }

    /// 捕获领域错误链中的 `DownloadError::Failed`，登记部分失败回执并返回 `AppError`。
    pub fn capture_app_error<E>(&self, job: &JobHandle, step_label: &str, err: E) -> AppError
    where
        E: std::error::Error + 'static,
        crate::error::AppError: From<E>,
    {
        let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(&err);
        while let Some(node) = cur {
            if let Some(launcher_core::net::DownloadError::Failed(failures)) =
                node.downcast_ref::<launcher_core::net::DownloadError>()
            {
                let _ = self.record_download_failures(job.id(), step_label, failures);
                break;
            }
            cur = node.source();
        }
        crate::error::AppError::from(err)
    }

    /// 结合此前记录的失败回执/诊断以结构化状态结束失败任务。
    pub fn finish_failed_job(&self, job: JobHandle, message: impl Into<String>) {
        let meta = self
            .failure_receipts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(job.id())
            .map(|(_, diag, count)| (*count, diag.clone()));
        if let Some((count, diag)) = meta {
            job.fail_with_diagnosis(message, count, diag);
        } else {
            job.fail(message);
        }
    }

    pub fn fail_job_with_receipt(
        &self,
        job: JobHandle,
        step_label: &str,
        message: impl Into<String>,
        failures: Option<&[FileFailure]>,
    ) {
        let msg = message.into();
        if let Some(list) = failures
            && !list.is_empty()
        {
            let (count, diag) = self.record_download_failures(job.id(), step_label, list);
            job.fail_with_diagnosis(msg, count, diag);
        } else {
            job.fail(msg);
        }
    }

    pub fn ui_language(&self) -> String {
        self.ui_language
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Default)]
pub struct Catalog {
    pub game_versions: Mutex<Option<Vec<String>>>,
    pub game_version_entries: Mutex<Option<Vec<GameVersionEntry>>>,
    pub neoforge: Mutex<Option<Vec<NeoForgeVersion>>>,
}

/// 缓存在后端的待确认导出计划，避免导出时重复扫描与重复联网查外链。
pub struct PendingExport {
    pub instance_id: String,
    pub plan: ExportPlan,
    pub matched: bool,
}
