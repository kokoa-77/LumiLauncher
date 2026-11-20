//! 拖文件进窗口就能装（D17g）。
//!
//! 放下文件时（没有正在进行的补件）：在后台认每个文件是什么（核心库 `content::dropped`），
//! 登记成一次性的编号，发 [`FilesDropped`] 给界面。界面只拿编号和认出来的结果，**拿不到路径**；
//! 用户在弹窗里选好装进哪个实例，再用编号调 [`drops_install`]。整合包走原来的导入预览。
//!
//! 「一包东西」（一堆模组、光影、地图塞在一个 zip 里）放下时先拆到
//! `<游戏目录>/.mcl-store/tmp/drops/<随机名>/`，里面每一样单独登记。拆出来的文件夹由 [`Staging`]
//! 管：用到它的编号都装完或者丢掉以后自动删；启动器异常退出留下的，下次启动时删（[`clean_leftovers`]）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_core::content::bundle::{self, BundleError};
use launcher_core::content::dropped::{self, DropKind, DropProblem, Dropped, ModFit};
use launcher_core::content::maps;
use launcher_core::content::store::STORE_DIR;
use launcher_core::content::{ContentKind, InstanceContent, LocalAdd, LocalOutcome, modrinth};
use launcher_core::fs::safe_join;
use launcher_core::jobs::ProgressUnit;
use launcher_core::net::sha1_file;
use launcher_core::saves;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::error::AppError;
use crate::events::{FilesDropped, FilesReading, emit};
use crate::game::PackChoice;
use crate::linked::FolderFound;
use crate::state::AppState;

/// 资源管理器打开的整合包在拖放监听就绪后沿用同一个预览。
pub(crate) struct ShellPack(std::sync::Mutex<ShellPackInbox>);

struct ShellPackInbox {
    listening: bool,
    pending: Option<PathBuf>,
}

impl ShellPack {
    pub fn from_args() -> Self {
        let mut args = std::env::args_os().skip(1);
        let pending = args
            .next()
            .zip(args.next())
            .and_then(|(flag, path)| crate::installer::pack_argument(&flag, &path));
        Self(std::sync::Mutex::new(ShellPackInbox {
            listening: false,
            pending,
        }))
    }
}

/// 单实例转发的文件名来自 Windows 资源管理器，前端只拿到拖放登记的令牌。
pub(crate) fn shell_open(app: &AppHandle, args: &[String]) -> bool {
    let [_, flag, file, ..] = args else {
        return false;
    };
    let Some(path) = crate::installer::pack_argument(flag.as_ref(), file.as_ref()) else {
        return false;
    };
    let Some(state) = app.try_state::<ShellPack>() else {
        return false;
    };
    let mut inbox = state
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if inbox.listening {
        drop(inbox);
        received(app, vec![path]);
    } else {
        inbox.pending = Some(path);
    }
    true
}

pub(crate) fn pause_shell(app: &AppHandle) {
    app.state::<ShellPack>()
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .listening = false;
}

#[tauri::command]
#[specta::specta]
pub fn drops_shell_ready(app: AppHandle, state: State<'_, ShellPack>) {
    let pending = {
        let mut inbox = state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inbox.listening = true;
        inbox.pending.take()
    };
    if let Some(file) = pending {
        received(&app, vec![file]);
    }
}

/// 一次最多认多少个文件（再多多半是拖错了整个文件夹的内容）
const MAX_FILES: usize = 64;
/// 一包东西里最多拆出这么多样（几百个模组的懒人包也够）
const MAX_BUNDLE_ITEMS: usize = 1000;

/// 拆一包东西用的临时文件夹：最后一个用到它的编号放手时，在后台线程里删掉
#[derive(Debug)]
pub struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let dir = std::mem::take(&mut self.0);
        std::thread::spawn(move || {
            if let Err(error) = std::fs::remove_dir_all(&dir)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%error, dir = %dir.display(), "删拆包用的临时文件夹失败");
            }
        });
    }
}

/// 登记着、等用户选的一项
#[derive(Debug, Clone)]
pub struct PendingDrop {
    pub file: Dropped,
    /// 从一包东西里拆出来的：那个临时文件夹（装完、丢掉以后自动删）
    pub staging: Option<Arc<Staging>>,
}

fn temp_root(store_root: &Path) -> PathBuf {
    store_root.join("tmp")
}

/// 启动时删掉上次没删干净的拆包临时文件夹（启动器被强制结束时会留下）
pub fn clean_leftovers(game_root: &Path) {
    let root = temp_root(&game_root.join(STORE_DIR));
    std::thread::spawn(move || {
        for sub in ["drops", "packs"] {
            let dir = root.join(sub);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => tracing::info!(dir = %dir.display(), "删掉了上次留下的拆包临时文件"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(%error, "删上次留下的拆包临时文件失败"),
            }
        }
    });
}

/// 给界面看的一个拖进来的文件
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DropItem {
    /// 一次性编号，装的时候传回来
    pub token: String,
    pub name: String,
    pub kind: DropKind,
    /// 不装的原因（`kind` 是 `unknown` 时有）
    pub problem: Option<DropProblem>,
    /// 认出来的名字（模组的显示名、地图的文件夹名）
    pub title: Option<String>,
    pub version: Option<String>,
    /// 图标（data URL）
    pub icon: Option<String>,
    /// 字节数
    pub size: f64,
    /// 从哪一包东西里拆出来的（那个压缩包的文件名）
    pub from: Option<String>,
    /// 拖进来的是别家的游戏目录、别家启动器的实例：能直接挂上、导入（D19）
    pub folder: Option<FolderFound>,
}

/// 拖进来的文件夹是不是别家的游戏目录、实例（只认拖进来的这一层，不往上找；一包东西里拆出来的不认）
fn folder_of(state: &AppState, file: &Dropped) -> Option<FolderFound> {
    let maybe = file.from.is_none()
        && matches!(
            file.problem,
            Some(DropProblem::GameDir | DropProblem::Folder)
        );
    if !maybe {
        return None;
    }
    match crate::linked::recognize(state, &file.path, false) {
        Ok(found) => found,
        Err(error) => {
            tracing::info!(name = %file.name, code = %error.code, "拖进来的文件夹不能挂上");
            None
        }
    }
}

/// 一包东西里没拆的别的文件（配置文件、说明文档……）
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BundleRest {
    /// 那个压缩包的文件名
    pub name: String,
    pub count: u32,
}

fn mark_unknown(mut file: Dropped, problem: DropProblem) -> Dropped {
    file.kind = DropKind::Unknown;
    file.problem = Some(problem);
    file
}

/// 认出来的每一样（拆包来的带着临时文件夹）和每包没拆的文件
type Inspected = (Vec<(Dropped, Option<Arc<Staging>>)>, Vec<BundleRest>);

/// 认拖进来的每一样；一包东西拆开后里面每一样单独认
fn inspect_all(paths: &[PathBuf], store_root: &Path) -> Inspected {
    let mut found = Vec::new();
    let mut rests = Vec::new();
    for path in paths.iter().take(MAX_FILES) {
        let file = dropped::inspect(path);
        if file.kind != DropKind::Bundle {
            found.push((file, None));
            continue;
        }
        let dir = temp_root(store_root)
            .join("drops")
            .join(uuid::Uuid::new_v4().simple().to_string());
        let staging = Arc::new(Staging(dir.clone()));
        let mut out = Unpacking {
            staging: &staging,
            found: &mut found,
            rests: &mut rests,
        };
        if let Err(error) = out.expand(path, &file.name, &dir, 1) {
            let problem = problem_of(&error, &file.name);
            found.push((mark_unknown(file, problem), None));
        }
    }
    (found, rests)
}

/// 一包东西里再套一包时，最多拆这么多层（外面一层 + 里面一层）
const MAX_DEPTH: usize = 2;

fn problem_of(error: &BundleError, name: &str) -> DropProblem {
    match error {
        BundleError::TooLarge => DropProblem::TooLarge,
        error => {
            tracing::warn!(%error, name, "拆一包东西失败");
            DropProblem::Unreadable
        }
    }
}

/// 拆包时往外收东西
struct Unpacking<'a> {
    staging: &'a Arc<Staging>,
    found: &'a mut Vec<(Dropped, Option<Arc<Staging>>)>,
    rests: &'a mut Vec<BundleRest>,
}

impl Unpacking<'_> {
    /// 把 `path` 这包拆到 `dir`，里面每一样认一遍；里面又是一包的，没到层数上限就接着拆
    fn expand(
        &mut self,
        path: &Path,
        name: &str,
        dir: &Path,
        depth: usize,
    ) -> Result<(), BundleError> {
        let unpacked = bundle::extract(path, dir)?;
        tracing::info!(
            name,
            items = unpacked.items.len(),
            skipped = unpacked.skipped,
            "拆开了一包东西"
        );
        for (index, item) in unpacked.items.iter().enumerate() {
            if self.found.len() >= MAX_BUNDLE_ITEMS {
                tracing::warn!(name, "一包东西里太多了，后面的不认了");
                break;
            }
            let mut child = dropped::inspect(item);
            if child.kind == DropKind::Bundle {
                if depth < MAX_DEPTH {
                    // 拆到这一层自己的子文件夹（和 f<序号>、w<序号> 不重）
                    let sub = dir.join(format!("n{index}"));
                    match self.expand(item, &child.name, &sub, depth + 1) {
                        Ok(()) => continue,
                        Err(error) => {
                            let problem = problem_of(&error, &child.name);
                            child = mark_unknown(child, problem);
                        }
                    }
                } else {
                    child = mark_unknown(child, DropProblem::NestedBundle);
                }
            }
            child.from = Some(name.to_owned());
            self.found.push((child, Some(self.staging.clone())));
        }
        if unpacked.skipped > 0 {
            self.rests.push(BundleRest {
                name: name.to_owned(),
                count: u32::try_from(unpacked.skipped).unwrap_or(u32::MAX),
            });
        }
        Ok(())
    }
}

/// 放下文件（没有正在进行的补件时）：后台认完发给界面
pub fn received(app: &AppHandle, paths: Vec<PathBuf>) {
    let app = app.clone();
    emit(&app, FilesReading(true));
    tauri::async_runtime::spawn(async move {
        let count = paths.len();
        let store_root = app.state::<AppState>().game().root().join(STORE_DIR);
        let scan_root = store_root.clone();
        let found =
            tauri::async_runtime::spawn_blocking(move || inspect_all(&paths, &scan_root)).await;
        let (found, rests) = match found {
            Ok(found) => found,
            Err(error) => {
                tracing::error!(%error, "认拖进来的文件失败");
                emit(
                    &app,
                    FilesDropped {
                        items: Vec::new(),
                        rests: Vec::new(),
                    },
                );
                return;
            }
        };
        let state = app.state::<AppState>();
        let mut pending = state.drops.lock().unwrap_or_else(|e| e.into_inner());
        // 上一次拖进来、没装也没取消的，不再留着（拆包的临时文件夹跟着删）
        pending.clear();
        let items: Vec<DropItem> = found
            .into_iter()
            .map(|(file, staging)| {
                let token = uuid::Uuid::new_v4().simple().to_string();
                let icon = file.icon.as_deref().and_then(|bytes| {
                    let name = launcher_core::content::identify::save_icon(&store_root, bytes)?;
                    launcher_core::content::identify::icon_data_url(&store_root, &name)
                });
                let folder = folder_of(&state, &file);
                let item = DropItem {
                    token: token.clone(),
                    name: file.name.clone(),
                    kind: file.kind,
                    problem: if folder.is_some() {
                        Some(DropProblem::GameDir)
                    } else {
                        file.problem
                    },
                    title: file.title.clone(),
                    version: file.version.clone(),
                    icon,
                    size: file.size as f64,
                    from: file.from.clone(),
                    folder,
                };
                pending.insert(token, PendingDrop { file, staging });
                item
            })
            .collect();
        drop(pending);
        tracing::info!(
            count,
            kinds = ?items.iter().map(|i| i.kind).collect::<Vec<_>>(),
            "拖进了文件"
        );
        emit(&app, FilesDropped { items, rests });
    });
}

/// 拖进来的东西和这个实例对不对得上
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DropCheck {
    pub token: String,
    /// 模组和实例的加载器、游戏版本（不是模组时总是 `fits`）
    pub fit: ModFit,
    pub duplicate: Option<DropDuplicate>,
}

/// 实例里已经有了
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DropDuplicate {
    /// 一模一样的文件
    Same,
    /// 同一个模组的另一个版本（装的时候换掉它）
    OtherVersion {
        /// 已装的那个文件（启用时的名字）
        file: String,
        version: Option<String>,
    },
}

fn content_kind(kind: DropKind) -> Option<ContentKind> {
    match kind {
        DropKind::Mod => Some(ContentKind::Mod),
        DropKind::ResourcePack => Some(ContentKind::ResourcePack),
        DropKind::Shader => Some(ContentKind::Shader),
        _ => None,
    }
}

fn pending(state: &AppState, tokens: &[String]) -> Result<Vec<(String, PendingDrop)>, AppError> {
    let pending = state.drops.lock().unwrap_or_else(|e| e.into_inner());
    tokens
        .iter()
        .map(|t| {
            pending
                .get(t)
                .cloned()
                .map(|d| (t.clone(), d))
                .ok_or_else(|| AppError::new("drop.expired", "请重新把文件拖进来"))
        })
        .collect()
}

/// 拖进来的文件装进这个实例合不合适：模组先按哈希问 Modrinth 支持哪些加载器和游戏版本
/// （查不到、没联网就看 jar 里写的加载器）；再看实例里是不是已经有了。
#[tauri::command]
#[specta::specta]
pub async fn drops_check(
    state: State<'_, AppState>,
    tokens: Vec<String>,
    instance_id: String,
) -> Result<Vec<DropCheck>, AppError> {
    let files: Vec<(String, Dropped)> = pending(&state, &tokens)?
        .into_iter()
        .map(|(token, p)| (token, p.file))
        .collect();
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = instance.game_dir(&game)?;
    let loader = instance.config.loader.as_ref().map(|l| l.kind.slug());
    let game_version = instance.config.game_version.clone();

    // 模组的哈希；实例里有没有同名的文件、内容一不一样
    let mut hashes = HashMap::new();
    let mut same_name = HashMap::new();
    for (token, file) in &files {
        if file.kind == DropKind::Mod
            && let Ok(sha1) = sha1_file(&file.path).await
        {
            hashes.insert(token.clone(), sha1);
        }
        let Some(kind) = content_kind(file.kind) else {
            continue;
        };
        let folder = dir.join(kind.folder());
        // 文件名是用户磁盘上的，也按不可信处理
        let target = [file.name.clone(), format!("{}.disabled", file.name)]
            .iter()
            .filter_map(|name| safe_join(&folder, name).ok())
            .find(|p| p.is_file());
        if let Some(target) = target {
            let same = match (sha1_file(&target).await, sha1_file(&file.path).await) {
                (Ok(a), Ok(b)) => a.eq_ignore_ascii_case(&b),
                _ => false,
            };
            same_name.insert(token.clone(), same);
        }
    }
    // 实例里已装模组的 ID（每个 jar 都要打开看一眼，放后台）
    let mods_dir = dir.join(ContentKind::Mod.folder());
    let installed =
        tauri::async_runtime::spawn_blocking(move || dropped::installed_mods(&mods_dir))
            .await
            .map_err(|e| AppError::new("drop.check_failed", "检查文件失败").with_detail(e))?;

    let wanted: Vec<String> = hashes.values().cloned().collect();
    let compat = match modrinth::compat_by_sha1(&state.http_client(), &wanted).await {
        Ok(found) => found,
        Err(error) => {
            // 没联网也能装：只看 jar 里写的加载器
            tracing::warn!(%error, "去 Modrinth 查模组支持的版本失败");
            HashMap::new()
        }
    };

    // 实例里装了 Sinytra Connector（或者这一批里就有它）：Fabric 模组也能用
    let bridge = installed.contains_key(dropped::FABRIC_BRIDGE)
        || files
            .iter()
            .any(|(_, f)| f.mod_id.as_deref() == Some(dropped::FABRIC_BRIDGE));

    Ok(files
        .into_iter()
        .map(|(token, file)| {
            let fit = if file.kind == DropKind::Mod {
                let remote = hashes
                    .get(&token)
                    .and_then(|h| compat.get(&h.to_ascii_lowercase()));
                match remote {
                    Some(r) => dropped::mod_fit(
                        &game_version,
                        loader,
                        &r.loaders,
                        &r.game_versions,
                        bridge,
                    ),
                    None => dropped::mod_fit(&game_version, loader, &file.loaders, &[], bridge),
                }
            } else {
                ModFit::Fits
            };
            let duplicate = match same_name.get(&token) {
                Some(true) => Some(DropDuplicate::Same),
                _ => file
                    .mod_id
                    .as_ref()
                    .and_then(|id| installed.get(id))
                    .filter(|(name, _)| *name != file.name)
                    .map(|(name, version)| DropDuplicate::OtherVersion {
                        file: name.clone(),
                        version: version.clone(),
                    }),
            };
            DropCheck {
                token,
                fit,
                duplicate,
            }
        })
        .collect())
}

/// 用户在弹窗里勾好的一项
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DropChoice {
    pub token: String,
    /// 顺带换掉的旧版本（`drops_check` 给的 `otherVersion.file`）
    pub replace: Option<String>,
}

/// 装进实例：模组、资源包、光影包进内容仓库（能撤销），地图解压 / 复制进存档，
/// 数据包放进 `world` 那个世界。是一个后台任务，返回任务 ID；装完界面收到任务成功的事件。
#[tauri::command]
#[specta::specta]
pub fn drops_install(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    choices: Vec<DropChoice>,
    world: Option<String>,
) -> Result<String, AppError> {
    let tokens: Vec<String> = choices.iter().map(|c| c.token.clone()).collect();
    let files = pending(&state, &tokens)?;
    let game = state.game();
    let instance = launcher_core::instance::load(&game, &instance_id)?
        .ok_or_else(|| AppError::new("instance.not_found", "找不到这个实例，可能已经被删除"))?;
    let dir = instance.game_dir(&game)?;
    let world_dir = match &world {
        Some(folder) => Some(saves::world_dir(&dir, folder)?),
        None => None,
    };
    if files.iter().any(|(_, p)| p.file.kind == DropKind::DataPack)
        && !world_dir.as_ref().is_some_and(|d| d.is_dir())
    {
        return Err(AppError::new(
            "drop.no_world",
            "数据包要装进一个世界：这个实例还没有存档，先进游戏建一个世界",
        ));
    }
    {
        let mut pending = state.drops.lock().unwrap_or_else(|e| e.into_inner());
        for token in &tokens {
            pending.remove(token);
        }
    }
    let replace: HashMap<String, Option<String>> =
        choices.into_iter().map(|c| (c.token, c.replace)).collect();
    let job = state.jobs.start_with(
        format!("drop:{instance_id}:{}", uuid::Uuid::new_v4().simple()),
        format!("装进 {}：{} 个文件", instance.config.name, files.len()),
        ProgressUnit::Items,
        crate::downloads::meta(
            launcher_core::jobs::JobSource::Local,
            Some(&instance_id),
            None,
            None,
        ),
    )?;
    let job_id = job.id().to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let Some(_turn) = state.turns.wait(&dir, &job).await else {
            job.cancelled();
            return;
        };
        if crate::game::instance_running(&state, &instance_id) {
            job.fail("游戏正在运行，先退出游戏再装".to_owned());
            return;
        }
        let total = files.len() as f64;
        job.set_progress(0.0, Some(total));
        let content = InstanceContent::for_game(&state.game(), &dir);
        let mut adds = Vec::new();
        let mut done = 0.0;
        let mut failure: Option<String> = None;
        // 拆包的临时文件夹要留到全部装完（装完这个列表放手，文件夹自动删）
        let mut staged = Vec::new();
        for (token, PendingDrop { file, staging }) in files {
            staged.extend(staging);
            if let Some(kind) = content_kind(file.kind) {
                adds.push(LocalAdd {
                    kind,
                    source: file.path.clone(),
                    replace: replace.get(&token).cloned().flatten(),
                });
                continue;
            }
            let saves_dir = dir.join("saves");
            let instance_dir = dir.clone();
            let world_dir = world_dir.clone();
            let result = tauri::async_runtime::spawn_blocking(move || match file.kind {
                DropKind::World if file.path.is_dir() => {
                    maps::copy_world_dir(&file.path, &saves_dir)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                DropKind::World => {
                    let fallback = file.title.clone().unwrap_or_else(|| "地图".to_owned());
                    maps::extract_world(&file.path, &saves_dir, &fallback)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                DropKind::DataPack => match &world_dir {
                    Some(world) => dropped::install_datapack(&file.path, world)
                        .map(|_| ())
                        .map_err(|e| format!("复制数据包失败：{e}")),
                    None => Err("数据包要装进一个世界".to_owned()),
                },
                DropKind::Schematic => dropped::install_schematic(&file.path, &instance_dir)
                    .map(|_| ())
                    .map_err(|e| format!("复制投影失败：{e}")),
                _ => Err(format!("「{}」不能装进实例", file.name)),
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            if let Err(error) = result {
                tracing::warn!(%error, "装拖进来的文件失败");
                failure.get_or_insert(error);
            }
            done += 1.0;
            job.set_progress(done, Some(total));
        }
        if !adds.is_empty() {
            match content.add_local(&adds).await {
                Ok(outcomes) => {
                    let same = outcomes
                        .iter()
                        .filter(|o| **o == LocalOutcome::Same)
                        .count();
                    tracing::info!(added = outcomes.len() - same, same, "拖进来的内容装好了");
                }
                Err(error) => {
                    let error = AppError::from(error);
                    failure.get_or_insert(error.message);
                }
            }
            job.set_progress(total, Some(total));
        }
        match failure {
            Some(error) => job.fail(error),
            None => job.succeed(),
        }
        drop(staged);
    });
    Ok(job_id)
}

/// 拖进来的整合包：和「选择文件导入」一样先检查、给出预览，确认后用 `modpack_import` 导入
#[tauri::command]
#[specta::specta]
pub async fn drops_modpack(
    state: State<'_, AppState>,
    token: String,
) -> Result<PackChoice, AppError> {
    let PendingDrop { file, staging } = state
        .drops
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&token)
        .ok_or_else(|| AppError::new("drop.expired", "请重新把整合包拖进来"))?;
    // 从一包东西里拆出来的整合包：挪出拆包的临时文件夹（那个文件夹随时会删），
    // 放到 tmp/packs/ 下（导入要读它，下次启动时删）
    let path = match staging {
        None => file.path,
        Some(_) => {
            let dir = temp_root(&state.game().root().join(STORE_DIR))
                .join("packs")
                .join(uuid::Uuid::new_v4().simple().to_string());
            let target = safe_join(&dir, &file.name)
                .map_err(|e| AppError::new("drop.pack_failed", "读整合包失败").with_detail(e))?;
            let source = file.path.clone();
            let moved = target.clone();
            tauri::async_runtime::spawn_blocking(move || {
                std::fs::create_dir_all(&dir)?;
                std::fs::rename(&source, &moved)
            })
            .await
            .map_err(|e| AppError::new("drop.pack_failed", "读整合包失败").with_detail(e))?
            .map_err(|e| AppError::new("drop.pack_failed", "读整合包失败").with_detail(e))?;
            target
        }
    };
    crate::game::pack_choice(&state, path).await
}

/// 不装了：丢掉这些编号
#[tauri::command]
#[specta::specta]
pub fn drops_discard(state: State<'_, AppState>, tokens: Vec<String>) {
    let mut pending = state.drops.lock().unwrap_or_else(|e| e.into_inner());
    for token in tokens {
        pending.remove(&token);
    }
}
