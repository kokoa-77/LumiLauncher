use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use launcher_core::config::{DownloadSource, ProxySettings, SettingsStore};
use launcher_core::content::cache::TtlCache;
use launcher_core::content::claim::ClaimSession;
use launcher_core::content::curseforge::CurseForge;
use launcher_core::content::detail::ProjectDetail;
use launcher_core::content::favorites::Favorites;
use launcher_core::content::modrinth::SearchPage;
use launcher_core::install::neoforge::NeoForgeVersion;
use launcher_core::java::{JavaLocator, JavaPrefsStore};
use launcher_core::jobs::JobRegistry;
use launcher_core::modpack::PackPlan;
use launcher_core::net::{HttpError, build_client_with_proxy};
use launcher_core::paths::GameDir;
use reqwest::Client;

use crate::sessions::Sessions;
use crate::window::Backdrop;

/// 资源中心缓存的有效期：同样的搜索、同一个项目的详情 10 分钟内直接用上次的结果
pub const DISCOVER_CACHE_TTL: Duration = Duration::from_secs(600);

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
    /// DoH 开关（`client` 用它解析域名；设置里改了在这里开关，D17l）
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
    /// 拖进窗口、还没装也没取消的文件（一次性编号 → 认出来的结果，D17g）
    pub drops: Mutex<HashMap<String, crate::drops::PendingDrop>>,
    /// 资源中心的搜索结果（键里带平台、类型、关键词、版本、加载器、第几页）
    pub search_cache: TtlCache<SearchPage>,
    /// 项目详情
    pub detail_cache: TtlCache<ProjectDetail>,
    /// 收藏夹（存在应用数据目录的 favorites.json）
    pub favorites: Favorites,
    /// 用户对 Java 的选择：手动添加的、每个大版本的默认（应用数据目录的 java.json，D17h）
    pub java_prefs: JavaPrefsStore,
    /// 我们改过哪些程序的 Windows 显卡设置（关掉「高性能显卡」时按它撤回，D17h）
    pub gpu_ledger: PathBuf,
    /// 导出完的整合包：任务 ID → 文件（「打开所在文件夹」用，界面只拿任务 ID，D17i）
    pub exports: Mutex<HashMap<String, PathBuf>>,
    /// 同一个实例同一时间只做一件改文件的事（D17l）
    pub turns: crate::turns::InstanceTurns,
    /// 启动时退回的没做完的改动，等界面来拿（D17m）
    pub recovery: crate::recovery::RecoveryNotes,
    /// 装过的文件教会的中文名、两个平台的关联（按游戏目录缓存，识别缓存变了才重读，D17e）
    pub names: launcher_core::content::names::NameBook,
    /// 下载源「自动」时测官方和 BMCLAPI 谁快（D18）
    pub sources: launcher_core::net::SourcePicker,
    /// 挂上的别家目录、导入候选（D19）
    pub linked: crate::linked::Linked,
    /// 账户与皮肤（D19b）
    pub accounts: crate::accounts::Accounts,
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

    /// 这次下载用哪个源：「自动」先测速（半小时内测过就直接用），别的照设置。
    /// 安装、新建实例、下载 Java 这类真要下很多东西的用它
    pub async fn source(&self) -> DownloadSource {
        self.sources
            .resolve(self.settings.get().download.source)
            .await
    }

    /// 不等测速的下载源：「自动」测过用测的，没测过先官方（失败还会换镜像）。
    /// 启动前的检查、打开版本列表这类要快的用它
    pub fn source_now(&self) -> DownloadSource {
        self.sources.cached(self.settings.get().download.source)
    }

    /// 「Minecraft 元数据」那行（版本清单、版本描述、加载器版本列表，D18b）：「自动」和文件那行共用一次测速
    pub async fn meta_source(&self) -> DownloadSource {
        self.sources
            .resolve(self.settings.get().download.meta_source)
            .await
    }

    /// 同上，不等测速
    pub fn meta_source_now(&self) -> DownloadSource {
        self.sources
            .cached(self.settings.get().download.meta_source)
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

    /// 代理设置变更后立即重建并热替换 HTTP 客户端（同步更新测速器与 CurseForge）。
    pub fn apply_proxy(&self, proxy: &ProxySettings) -> Result<(), HttpError> {
        let next = build_client_with_proxy(self.doh.clone(), proxy)?;
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        self.sources.set_client(next.clone());
        self.curseforge.set_client(next);
        Ok(())
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
    pub neoforge: Mutex<Option<Vec<NeoForgeVersion>>>,
}
