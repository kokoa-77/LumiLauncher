//! 别家启动器：挂上别家的 `.minecraft`（原地读、不挪文件），从 Prism、CurseForge、ATLauncher、
//! Modrinth App 导入实例（复制一份游戏文件）。
//!
//! 挂上的目录记在应用数据目录的 `linked-roots.json`；里面的版本同步成实例（`instance::Link`），
//! 启动时、挂上时、换游戏目录后重新扫一遍（别家新装、删掉的版本跟着变）。
//! 界面只拿编号：目录、导入候选的编号都是路径算出来的短哈希；路径只给界面显示，界面不拼路径。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use launcher_core::external::foreign::{self, ForeignInstance};
use launcher_core::external::{Launcher, root};
use launcher_core::fs::{read_json, write_json_atomic};
use launcher_core::install::LoaderContext;
use launcher_core::install::setup::{SetupProgress, install_version};
use launcher_core::instance::{self, CopyError, path_key, same_path};
use launcher_core::jobs::{JobSource, ProgressUnit};
use launcher_core::paths::GameDir;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::events::{InstancesChanged, emit};
use crate::game::{Pick, phase_label, pick, pick_java_for, report, report_loader};
use crate::state::AppState;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootsFile {
    roots: Vec<StoredRoot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRoot {
    path: PathBuf,
    added_at: f64,
    /// 用户自己从 Lumi 移除的版本（`versions/` 里的文件夹名）：重新扫的时候不加回来，「加回来」才清掉
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<String>,
}

/// 挂上的目录清单，和这次运行里列给界面的候选（编号 → 路径 / 实例）
pub struct Linked {
    file: PathBuf,
    /// 读改写清单时一个一个来
    write: Mutex<()>,
    suggestions: Mutex<HashMap<String, PathBuf>>,
    foreign: Mutex<HashMap<String, ForeignInstance>>,
}

impl Linked {
    pub fn open(file: PathBuf) -> Self {
        Self {
            file,
            write: Mutex::new(()),
            suggestions: Mutex::default(),
            foreign: Mutex::default(),
        }
    }

    fn roots(&self) -> Vec<StoredRoot> {
        match read_json::<RootsFile>(&self.file) {
            Ok(found) => found.unwrap_or_default().roots,
            Err(error) => {
                tracing::warn!(%error, "挂上的目录清单读不了，按空的处理");
                Vec::new()
            }
        }
    }

    pub(crate) fn root_paths(&self) -> Vec<PathBuf> {
        self.roots().into_iter().map(|r| r.path).collect()
    }

    /// 改清单：读 → 改 → 原子写回
    fn change(&self, f: impl FnOnce(&mut Vec<StoredRoot>)) -> Result<(), AppError> {
        let _guard = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut roots = self.roots();
        f(&mut roots);
        write_json_atomic(&self.file, &RootsFile { roots })
            .map_err(|e| AppError::new("linked.save_failed", "保存挂上的目录失败").with_detail(e))
    }

    /// 用户从 Lumi 移除了挂上的一个版本：记下来，重新扫时不加回来
    pub fn hide(&self, root: &Path, version: &str) -> Result<(), AppError> {
        self.change(|roots| {
            for r in roots.iter_mut().filter(|r| same_path(&r.path, root)) {
                if !r.hidden.iter().any(|h| h == version) {
                    r.hidden.push(version.to_owned());
                }
            }
        })
    }
}

/// 路径 → 给界面的编号（同一次运行里稳定就够了）
fn id_of(path: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path_key(path).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LinkedRootView {
    pub id: String,
    /// 只给界面显示
    pub path: String,
    /// 分组名（`PCL2`、`官方启动器`）
    pub name: String,
    pub launcher: Launcher,
    /// 有几个版本
    pub versions: u32,
    /// 找不到了（移走、删掉了，或者是拔掉的移动硬盘）
    pub missing: bool,
    /// 从 Lumi 移除过、那边还在的版本有几个（可以「加回来」）
    pub hidden: u32,
}

fn view_of(path: &Path, hidden: &[String]) -> LinkedRootView {
    let missing = !root::is_game_root(path);
    let hidden = hidden
        .iter()
        .filter(|v| path.join("versions").join(v).is_dir())
        .count();
    let launcher = if missing {
        Launcher::Other
    } else {
        root::detect_launcher(path)
    };
    LinkedRootView {
        id: id_of(path),
        path: path.display().to_string(),
        name: root::group_name(path, launcher),
        launcher,
        versions: u32::try_from(root::count_versions(path)).unwrap_or(u32::MAX),
        missing,
        hidden: u32::try_from(hidden).unwrap_or(u32::MAX),
    }
}

/// 扫一个挂上的目录、同步成实例（`hidden` 里的版本不加回来）。返回有没有变化
fn sync_one(game: &GameDir, path: &Path, hidden: &[String]) -> bool {
    if !root::is_game_root(path) {
        // 目录不见了：实例留着（可能只是移动硬盘没插），启动时会说找不到
        tracing::warn!(root = %path.display(), "挂上的目录找不到了，先不动");
        return false;
    }
    match root::scan(path) {
        Ok(scanned) => {
            let group = root::group_name(path, scanned.launcher);
            root::sync(game, &scanned, &group, hidden) != root::SyncReport::default()
        }
        Err(error) => {
            tracing::warn!(root = %path.display(), %error, "挂上的目录扫不了");
            false
        }
    }
}

/// 把全部挂上的目录重新扫一遍（启动时、换游戏目录后，在后台跑）。有变化就通知界面刷新实例列表
pub fn spawn_rescan(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let game = state.game();
        let started = std::time::Instant::now();
        let roots = state.linked.roots();
        // 每个都要扫，不能遇到第一个有变化的就停
        let mut changed = false;
        for r in &roots {
            changed |= sync_one(&game, &r.path, &r.hidden);
        }
        tracing::info!(
            roots = roots.len(),
            changed,
            ms = started.elapsed().as_millis() as u64,
            "重新扫了挂上的目录"
        );
        if changed {
            emit(&app, InstancesChanged);
        }
    });
}

/// 挂上的目录
#[tauri::command]
#[specta::specta]
pub fn linked_roots_list(state: State<'_, AppState>) -> Vec<LinkedRootView> {
    state
        .linked
        .roots()
        .iter()
        .map(|r| view_of(&r.path, &r.hidden))
        .collect()
}

/// 在电脑上找别家的 `.minecraft`（自己的游戏目录、已经挂上的不列）
#[tauri::command]
#[specta::specta]
pub async fn linked_roots_discover(
    state: State<'_, AppState>,
) -> Result<Vec<LinkedRootView>, AppError> {
    let mut exclude: Vec<PathBuf> = state.linked.roots().into_iter().map(|r| r.path).collect();
    exclude.push(state.game().root().to_path_buf());
    let found = tauri::async_runtime::spawn_blocking(move || root::discover(&exclude))
        .await
        .map_err(|e| AppError::new("linked.discover_failed", "找目录失败").with_detail(e))?;
    let mut suggestions = state
        .linked
        .suggestions
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    Ok(found
        .iter()
        .map(|path| {
            suggestions.insert(id_of(path), path.clone());
            view_of(path, &[])
        })
        .collect())
}

/// 选到、拖进来的文件夹认出来是什么（界面只拿编号，路径只用来显示）
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FolderFound {
    /// 别家的游戏目录（`.minecraft`）：`root.id` 交给 `linked_roots_add` 挂上；`linked` 是已经挂上了
    GameRoot { root: LinkedRootView, linked: bool },
    /// 别家启动器的实例：编号交给 `foreign_import`
    Foreign { instances: Vec<ForeignView> },
}

/// 认一个文件夹：别家的游戏目录，还是别家启动器的实例。`up`：选文件夹时选到里面一两层也往上找
/// （系统选文件夹的窗口里点中了 `versions`、`.minecraft` 再确认），拖进窗口的不往上找。
/// 是 Lumi 自己的游戏目录时报错；什么都不是时返回空
pub(crate) fn recognize(
    state: &AppState,
    folder: &Path,
    up: bool,
) -> Result<Option<FolderFound>, AppError> {
    // 选的就是游戏目录（或者它外面那层）的先认；别家实例的描述文件很明确，其次；最后才往上找游戏目录
    let mut found = root::game_root_in(folder);
    if found.is_none() {
        let instances = if up {
            foreign::inspect_any(folder)
        } else {
            foreign::inspect_here(folder)
        };
        if !instances.is_empty() {
            return Ok(Some(FolderFound::Foreign {
                instances: remember(state, instances),
            }));
        }
        if up {
            found = root::game_root_near(folder);
        }
    }
    let Some(path) = found else {
        return Ok(None);
    };
    check_not_ours(state, &path)?;
    let stored = state
        .linked
        .roots()
        .into_iter()
        .find(|r| same_path(&r.path, &path));
    state
        .linked
        .suggestions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id_of(&path), path.clone());
    let hidden = stored
        .as_ref()
        .map(|r| r.hidden.as_slice())
        .unwrap_or_default();
    Ok(Some(FolderFound::GameRoot {
        root: view_of(&path, hidden),
        linked: stored.is_some(),
    }))
}

/// Lumi 自己的游戏目录不挂
fn check_not_ours(state: &AppState, path: &Path) -> Result<(), AppError> {
    if same_path(path, state.game().root()) {
        return Err(AppError::new(
            "linked.own_dir",
            "这就是 Lumi 现在用的游戏目录，里面的实例已经在了",
        ));
    }
    if path.join(launcher_core::content::store::STORE_DIR).exists() {
        return Err(AppError::new(
            "linked.lumi_dir",
            "这是 Lumi 自己的游戏目录：想用它的话在上面的「游戏目录」里换过去",
        ));
    }
    Ok(())
}

/// 「添加文件夹」：弹出系统窗口选一个文件夹，认出来是什么（还没挂上）。用户关了窗口返回空
#[tauri::command]
#[specta::specta]
pub async fn linked_roots_pick(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<FolderFound>, AppError> {
    let Some(folder) = pick(&app, Pick::GameRoot).await? else {
        return Ok(None);
    };
    recognize(&state, &folder, true)?.map(Some).ok_or_else(|| {
        AppError::new(
            "linked.not_game_dir",
            "这个文件夹里没有游戏：选 .minecraft 这一层（里面有 versions 文件夹）",
        )
    })
}

/// 挂上一个目录（找到的、选到的、拖进来的，都是先认出来再拿编号挂）。已经挂上的再挂一次，移除过的版本也都加回来
#[tauri::command]
#[specta::specta]
pub async fn linked_roots_add(
    state: State<'_, AppState>,
    suggestion_id: String,
) -> Result<LinkedRootView, AppError> {
    let path = state
        .linked
        .suggestions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&suggestion_id)
        .cloned()
        .ok_or_else(|| AppError::new("linked.not_found", "找不到这个目录，请重新找一下"))?;
    check_not_ours(&state, &path)?;
    let game = state.game();
    let known = state
        .linked
        .roots()
        .iter()
        .any(|r| same_path(&r.path, &path));
    if known {
        return restore(&state, &path).await;
    }
    let stored = StoredRoot {
        path: path.clone(),
        added_at: launcher_core::now_millis() as f64,
        hidden: Vec::new(),
    };
    state.linked.change(|roots| roots.push(stored))?;
    state.engine.set_external_roots(state.linked.root_paths());
    tracing::info!(root = %path.display(), "挂上了别家的目录");
    let sync_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || sync_one(&game, &sync_path, &[]))
        .await
        .map_err(|e| AppError::new("linked.scan_failed", "扫描目录失败").with_detail(e))?;
    Ok(view_of(&path, &[]))
}

/// 把从 Lumi 移除过的版本加回来（只在 Lumi 这边，别家的文件本来就没动）
#[tauri::command]
#[specta::specta]
pub async fn linked_roots_restore(
    state: State<'_, AppState>,
    id: String,
) -> Result<LinkedRootView, AppError> {
    let path = state
        .linked
        .roots()
        .into_iter()
        .find(|r| id_of(&r.path) == id)
        .map(|r| r.path)
        .ok_or_else(|| AppError::new("linked.not_found", "找不到这个目录，可能已经移除了"))?;
    restore(&state, &path).await
}

/// 清掉移除过的版本，重新扫一遍
async fn restore(state: &AppState, path: &Path) -> Result<LinkedRootView, AppError> {
    let key = path_key(path);
    state.linked.change(|roots| {
        for r in roots.iter_mut().filter(|r| path_key(&r.path) == key) {
            r.hidden.clear();
        }
    })?;
    let game = state.game();
    let sync_path = path.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || sync_one(&game, &sync_path, &[]))
        .await
        .map_err(|e| AppError::new("linked.scan_failed", "扫描目录失败").with_detail(e))?;
    tracing::info!(root = %path.display(), "把移除过的版本加回来了");
    Ok(view_of(path, &[]))
}

/// 移除一个挂上的目录：它的实例都从 Lumi 里移除（只删 Lumi 这边的设置，别家的文件不动）。返回移除了几个实例
#[tauri::command]
#[specta::specta]
pub fn linked_roots_remove(state: State<'_, AppState>, id: String) -> Result<u32, AppError> {
    let roots = state.linked.roots();
    let target = roots
        .iter()
        .find(|r| id_of(&r.path) == id)
        .ok_or_else(|| AppError::new("linked.not_found", "找不到这个目录，可能已经移除了"))?;
    let game = state.game();
    for linked in instance::linked_to(&game, &target.path) {
        crate::instances::ensure_idle(&state, &linked.id, &linked.config.name, "移除")?;
    }
    let removed = instance::unlink_root(&game, &target.path);
    let key = path_key(&target.path);
    state
        .linked
        .change(|roots| roots.retain(|r| path_key(&r.path) != key))?;
    state.engine.set_external_roots(state.linked.root_paths());
    tracing::info!(root = %target.path.display(), removed, "移除了挂上的目录");
    Ok(u32::try_from(removed).unwrap_or(u32::MAX))
}

// ---------- 从别家导入 ----------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ForeignView {
    pub id: String,
    pub launcher: Launcher,
    pub name: String,
    pub game_version: String,
    /// 例如「NeoForge 21.1.77」；原版为空
    pub loader: Option<String>,
    /// 本启动器现在能不能装它的加载器
    pub supported: bool,
}

fn remember(state: &AppState, found: Vec<ForeignInstance>) -> Vec<ForeignView> {
    let mut map = state
        .linked
        .foreign
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    found
        .into_iter()
        .map(|f| {
            let id = id_of(&f.folder);
            let view = ForeignView {
                id: id.clone(),
                launcher: f.launcher,
                name: f.name.clone(),
                game_version: f.game_version.clone(),
                loader: f
                    .loader
                    .as_ref()
                    .map(|l| format!("{} {}", l.kind.label(), l.version)),
                supported: f
                    .loader
                    .as_ref()
                    .is_none_or(|l| l.kind.installable_for(&f.game_version)),
            };
            map.insert(id, f);
            view
        })
        .collect()
}

/// 电脑上各家启动器默认位置里的实例（Prism、CurseForge、ATLauncher、Modrinth App）
#[tauri::command]
#[specta::specta]
pub async fn foreign_list(state: State<'_, AppState>) -> Result<Vec<ForeignView>, AppError> {
    let found = tauri::async_runtime::spawn_blocking(foreign::known_instances)
        .await
        .map_err(|e| {
            AppError::new("foreign.list_failed", "找别的启动器的实例失败").with_detail(e)
        })?;
    Ok(remember(&state, found))
}

/// 弹出系统窗口选一个文件夹（一个实例，或者放实例的文件夹），认出来是什么：别家的实例列出来导入；
/// PCL、HMCL、官方启动器的游戏目录不用导入，界面问一句直接挂上。用户关了窗口返回空
#[tauri::command]
#[specta::specta]
pub async fn foreign_pick(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<FolderFound>, AppError> {
    let Some(folder) = pick(&app, Pick::ForeignInstance).await? else {
        return Ok(None);
    };
    recognize(&state, &folder, true)?.map(Some).ok_or_else(|| {
        AppError::new(
            "foreign.not_found",
            "这个文件夹里没认出别的启动器的实例（支持 Prism、MultiMC、CurseForge、ATLauncher、Modrinth App）",
        )
    })
}

/// 导入一个别家的实例（后台任务，返回任务 ID）：复制游戏文件 → 装好游戏本体和加载器
#[tauri::command]
#[specta::specta]
pub fn foreign_import(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<String, AppError> {
    let found = state
        .linked
        .foreign
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
        .ok_or_else(|| AppError::new("foreign.not_found", "找不到这个实例，请重新打开导入窗口"))?;
    if let Some(loader) = &found.loader
        && !loader.kind.installable_for(&found.game_version)
    {
        return Err(AppError::new(
            "foreign.unsupported_loader",
            format!(
                "暂时无法导入 Minecraft {} 的 {} 实例",
                found.game_version,
                loader.kind.label()
            ),
        ));
    }
    let job = state.jobs.start_with(
        // 用 instance-create 开头：做完界面会刷新实例列表
        format!("instance-create:import:{id}"),
        format!("导入实例 {}", found.name),
        ProgressUnit::Bytes,
        crate::downloads::meta(JobSource::Local, None, None, None),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let game = state.game();
        let copy_game = game.clone();
        let copied = tauri::async_runtime::spawn_blocking(move || {
            let cancel = job.cancel_token();
            let result = foreign::import(&copy_game, &found, &cancel, |p| {
                job.set_progress_detail(
                    p.done_bytes as f64,
                    Some(p.total_bytes as f64),
                    format!("复制文件 {}/{}", p.done_files, p.total_files),
                );
            });
            (job, result)
        })
        .await;
        let (job, imported) = match copied {
            Ok((job, Ok(imported))) => (job, imported),
            Ok((job, Err(CopyError::Cancelled))) => return job.cancelled(),
            Ok((job, Err(error))) => return job.fail(AppError::from(error).message),
            Err(error) => {
                // 复制的线程崩了：任务句柄跟着丢掉，会自动收尾成失败
                tracing::error!(%error, "导入实例的任务异常结束");
                return;
            }
        };
        job.set_instance(imported.id.clone());
        // 游戏本体和加载器：现在就装好，导完直接能玩（装失败也不要紧，启动时还会补）
        let installed = async {
            let downloader = crate::game::downloader_for_job(
                &state,
                launcher_core::net::DownloadPriority::Batch,
                &job,
            );
            let java =
                pick_java_for(&state, &job, &downloader, &imported.config.game_version).await?;
            let work_dir = game.root().join(".mcl-tmp").join("create");
            let cancel = job.cancel_token();
            let ctx = LoaderContext {
                game: &game,
                client: &state.http_client(),
                downloader: &downloader,
                java: &java,
                work_dir: &work_dir,
                cancel: &cancel,
            };
            install_version(
                &ctx,
                &imported.config.game_version,
                imported.config.loader.as_ref(),
                |p| match &p {
                    SetupProgress::Loader(l) => report_loader(&job, l),
                    SetupProgress::Files(phase, d) => report(&job, phase_label(*phase), d),
                },
            )
            .await
            .map_err(|e| state.capture_app_error(&job, "补全游戏文件", e))
        }
        .await;
        match installed {
            Ok(_) => job.succeed(),
            Err(_) if job.is_cancelled() => job.cancelled(),
            Err(error) => {
                tracing::warn!(instance = %imported.id, error = %error.message, "导入后装游戏本体失败，启动时再补");
                state.finish_failed_job(
                    job,
                    format!(
                        "文件已经导入，游戏本体没装完（启动时会再装）：{}",
                        error.message
                    ),
                );
            }
        }
    });
    Ok(job_id)
}
