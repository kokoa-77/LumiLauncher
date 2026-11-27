//! 「永不玩坏」（招牌 3，基础版）：记住实例**能玩的时候**长什么样，改动后崩溃了一键放回去。
//!
//! 快照只存清单（哪个文件夹里有哪个文件、哈希是多少、是不是停用），不存文件本体：
//! 启动器换掉或删掉的文件都先进了内容仓库（`store`），放回时按哈希从仓库取。
//!
//! 每个实例只留一份快照（`Lumi/snapshot.json`），两个时机会写：
//! - 游戏进过世界并且正常退出：把现在的样子记成「上次能玩的状态」（[`InstanceContent::mark_good`]）
//! - 还没有快照时，第一次改动之前：记成「改动前」（[`InstanceContent::snapshot_before_change`]）
//!
//! 崩溃后比较「快照 ↔ 现在」：有差别就能提示「回到改动前？」（[`InstanceContent::crash_offer`]），
//! 手动放进文件夹的模组也算在内。放回（[`InstanceContent::restore_snapshot`]）本身记一条改动历史，可以再撤销。

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use super::identify::IdentityCache;
use super::{
    Change, ContentError, ContentKind, FileRef, HistoryAction, InstanceContent, Records,
    current_shader, display_name, enabled_resource_packs, record_key, rel_path,
};
use crate::fs::{read_json, write_json_atomic};

const SNAPSHOT_FILE: &str = "snapshot.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Snapshot {
    pub(super) at: f64,
    /// 游戏在这个状态下正常玩过；否则只是「第一次改动之前」的样子
    pub(super) verified: bool,
    pub(super) files: Vec<SnapFile>,
    #[serde(default)]
    pub(super) records: Records,
    #[serde(default)]
    pub(super) shader: Option<String>,
    #[serde(default)]
    pub(super) resource_packs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SnapFile {
    pub(super) kind: ContentKind,
    /// 启用时的文件名
    pub(super) file_name: String,
    pub(super) sha1: String,
    /// 模组是不是停用状态
    pub(super) disabled: bool,
}

impl Snapshot {
    pub(super) fn enabled(&self, file: &SnapFile) -> bool {
        match file.kind {
            ContentKind::Mod => !file.disabled,
            ContentKind::Shader => self.shader.as_deref() == Some(file.file_name.as_str()),
            ContentKind::ResourcePack => self
                .resource_packs
                .contains(&format!("file/{}", file.file_name)),
        }
    }

    pub(super) fn file_ref(&self, file: &SnapFile) -> FileRef {
        FileRef {
            file_name: file.file_name.clone(),
            sha1: file.sha1.clone(),
            disabled: file.disabled,
            enabled: self.enabled(file),
            record: self
                .records
                .get(&record_key(file.kind, &file.file_name))
                .cloned(),
        }
    }

    pub(super) fn same_state(&self, other: &Self) -> bool {
        let packs_same = self.resource_packs.is_empty()
            || other.resource_packs.is_empty()
            || self.resource_packs == other.resource_packs;
        diff(self, other).is_empty() && self.shader == other.shader && packs_same
    }
}

/// 同一个文件名在快照里和现在的差别。`before` 是快照里的，`after` 是现在的。
#[derive(Debug)]
pub(super) struct Op {
    pub(super) kind: ContentKind,
    pub(super) before: Option<SnapFile>,
    pub(super) after: Option<SnapFile>,
}

impl Op {
    /// 文件内容没变，只是启用 / 停用变了
    pub(super) fn toggle_only(&self) -> bool {
        matches!((&self.before, &self.after), (Some(b), Some(a)) if b.sha1 == a.sha1)
    }
}

pub(super) fn diff(snapshot: &Snapshot, current: &Snapshot) -> Vec<Op> {
    let index = |s: &Snapshot| -> BTreeMap<(ContentKind, String), SnapFile> {
        s.files
            .iter()
            .map(|f| ((f.kind, f.file_name.clone()), f.clone()))
            .collect()
    };
    let before = index(snapshot);
    let after = index(current);
    let keys: BTreeSet<&(ContentKind, String)> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter_map(|key| {
            let (b, a) = (before.get(key), after.get(key));
            (b != a).then(|| Op {
                kind: key.0,
                before: b.cloned(),
                after: a.cloned(),
            })
        })
        .collect()
}

/// 崩溃后给用户的提议：回到快照那时的样子。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RollbackOffer {
    /// 快照是「正常玩过」的状态；否则只是「第一次改动之前」
    pub verified: bool,
    /// 快照是什么时候记的
    pub at: f64,
    /// 从那以后改了什么
    pub changes: Vec<OfferChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct OfferChange {
    pub kind: ContentKind,
    pub change: ChangeType,
    pub title: String,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ChangeType {
    Added,
    Removed,
    Updated,
    Enabled,
    Disabled,
}

/// 一个文件给人看的信息：标题、版本、属于哪个项目（用来把「删旧文件 + 加新文件」认成一次更新）
struct Described {
    title: String,
    version: Option<String>,
    project: Option<String>,
}

fn describe(file: &SnapFile, records: &Records, cache: &IdentityCache) -> Described {
    let record = records.get(&record_key(file.kind, &file.file_name));
    let identity = cache.identity(&file.sha1);
    Described {
        title: record
            .map(|r| r.title.clone())
            .or_else(|| identity.map(|i| i.title.clone()))
            .unwrap_or_else(|| display_name(&file.file_name)),
        version: record
            .map(|r| r.version_number.clone())
            .or_else(|| identity.and_then(|i| i.version.clone())),
        project: record
            .map(|r| format!("{:?}:{}", r.source, r.project_id))
            .or_else(|| {
                identity
                    .and_then(|i| i.project_id.as_ref())
                    .map(|p| format!("Modrinth:{p}"))
            }),
    }
}

pub(super) fn describe_changes(
    before: &Snapshot,
    after: &Snapshot,
    cache: &IdentityCache,
) -> Vec<OfferChange> {
    let ops = diff(before, after);
    let mut changes = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut touched_files: HashSet<(ContentKind, String)> = HashSet::new();

    for op in &ops {
        match (&op.before, &op.after) {
            (Some(b), Some(a)) => {
                touched_files.insert((op.kind, a.file_name.clone()));
                let now = describe(a, &after.records, cache);
                let change = if b.sha1 != a.sha1 {
                    ChangeType::Updated
                } else if a.disabled {
                    ChangeType::Disabled
                } else {
                    ChangeType::Enabled
                };
                let was = describe(b, &before.records, cache);
                let updated = change == ChangeType::Updated;
                changes.push(OfferChange {
                    kind: op.kind,
                    change,
                    title: now.title,
                    from_version: was.version.filter(|_| updated),
                    to_version: now.version.filter(|_| updated),
                });
            }
            (None, Some(a)) => {
                touched_files.insert((op.kind, a.file_name.clone()));
                added.push((op.kind, describe(a, &after.records, cache)));
            }
            (Some(b), None) => {
                touched_files.insert((op.kind, b.file_name.clone()));
                removed.push((op.kind, describe(b, &before.records, cache)));
            }
            (None, None) => {}
        }
    }
    // 同一个项目「旧文件没了 + 新文件来了」是一次更新（新版本文件名不一样）
    for (kind, now) in added {
        let pair = now.project.as_ref().and_then(|project| {
            removed
                .iter()
                .position(|(k, was)| *k == kind && was.project.as_ref() == Some(project))
        });
        let change = match pair {
            Some(index) => {
                let (_, was) = removed.swap_remove(index);
                OfferChange {
                    kind,
                    change: ChangeType::Updated,
                    title: now.title,
                    from_version: was.version,
                    to_version: now.version,
                }
            }
            None => OfferChange {
                kind,
                change: ChangeType::Added,
                title: now.title,
                from_version: None,
                to_version: now.version,
            },
        };
        changes.push(change);
    }
    for (kind, was) in removed {
        changes.push(OfferChange {
            kind,
            change: ChangeType::Removed,
            title: was.title,
            from_version: was.version,
            to_version: None,
        });
    }

    // 光影启用/关闭状态变化（文件本身未增删）
    if before.shader != after.shader {
        if let Some(prev) = &before.shader
            && !touched_files.contains(&(ContentKind::Shader, prev.clone()))
        {
            let title = before
                .records
                .get(&record_key(ContentKind::Shader, prev))
                .map(|r| r.title.clone())
                .unwrap_or_else(|| display_name(prev));
            changes.push(OfferChange {
                kind: ContentKind::Shader,
                change: ChangeType::Disabled,
                title,
                from_version: None,
                to_version: None,
            });
        }
        if let Some(next) = &after.shader
            && !touched_files.contains(&(ContentKind::Shader, next.clone()))
        {
            let title = after
                .records
                .get(&record_key(ContentKind::Shader, next))
                .map(|r| r.title.clone())
                .unwrap_or_else(|| display_name(next));
            changes.push(OfferChange {
                kind: ContentKind::Shader,
                change: ChangeType::Enabled,
                title,
                from_version: None,
                to_version: None,
            });
        }
    }

    // 材质包启用/停用变化（文件本身未增删）
    let before_packs: HashSet<&str> = before
        .resource_packs
        .iter()
        .filter_map(|p| p.strip_prefix("file/"))
        .collect();
    let after_packs: HashSet<&str> = after
        .resource_packs
        .iter()
        .filter_map(|p| p.strip_prefix("file/"))
        .collect();
    for enabled_pack in after_packs.difference(&before_packs) {
        if !touched_files.contains(&(ContentKind::ResourcePack, (*enabled_pack).to_owned())) {
            let title = after
                .records
                .get(&record_key(ContentKind::ResourcePack, enabled_pack))
                .map(|r| r.title.clone())
                .unwrap_or_else(|| display_name(enabled_pack));
            changes.push(OfferChange {
                kind: ContentKind::ResourcePack,
                change: ChangeType::Enabled,
                title,
                from_version: None,
                to_version: None,
            });
        }
    }
    for disabled_pack in before_packs.difference(&after_packs) {
        if !touched_files.contains(&(ContentKind::ResourcePack, (*disabled_pack).to_owned())) {
            let title = before
                .records
                .get(&record_key(ContentKind::ResourcePack, disabled_pack))
                .map(|r| r.title.clone())
                .unwrap_or_else(|| display_name(disabled_pack));
            changes.push(OfferChange {
                kind: ContentKind::ResourcePack,
                change: ChangeType::Disabled,
                title,
                from_version: None,
                to_version: None,
            });
        }
    }

    changes.sort_by_key(|c| (c.kind, c.title.to_lowercase()));
    changes
}

impl InstanceContent {
    pub(super) fn load_snapshot(&self) -> Option<Snapshot> {
        match read_json(&self.meta_path(SNAPSHOT_FILE)) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "快照读不了，当作没有");
                None
            }
        }
    }

    /// 把实例现在的样子记成清单（算哈希用缓存，没改过的文件不重算、无新增哈希不重写缓存）。
    pub(super) async fn capture(&self, verified: bool) -> Result<Snapshot, ContentError> {
        let mut cache = IdentityCache::load(self.store.root());
        let hashes_before = cache.hash_count();
        let mut files = Vec::new();
        for kind in ContentKind::ALL {
            for (scanned, sha1) in self.hash_files(kind, &mut cache).await? {
                files.push(SnapFile {
                    kind,
                    file_name: scanned.file_name,
                    sha1,
                    disabled: scanned.disabled,
                });
            }
        }
        if cache.hash_count() != hashes_before
            && let Err(error) = cache.save(self.store.root())
        {
            tracing::warn!(%error, "保存识别缓存失败");
        }
        Ok(Snapshot {
            at: crate::now_millis() as f64,
            verified,
            files,
            records: self.load_records(),
            shader: current_shader(&self.dir),
            resource_packs: enabled_resource_packs(&self.dir),
        })
    }

    /// 游戏在现在这个状态下正常玩过：记成「上次能玩的状态」，同时同步标记时间线节点为已验证可玩。
    pub async fn mark_good(&self) -> Result<(), ContentError> {
        let snapshot = self.capture(true).await?;
        write_json_atomic(&self.meta_path(SNAPSHOT_FILE), &snapshot)?;
        self.record_timeline_verified(&snapshot);
        tracing::info!(files = snapshot.files.len(), "已记下能玩的状态");
        Ok(())
    }

    /// 改动之前调用：还没有快照就记一份「改动前」。已经有快照（上次能玩的状态，或者更早的改动前）就不动。
    /// 记快照失败不挡改动，只写日志。
    pub async fn snapshot_before_change(&self) {
        if self.meta_path(SNAPSHOT_FILE).exists() {
            return;
        }
        let saved = match self.capture(false).await {
            Ok(snapshot) => write_json_atomic(&self.meta_path(SNAPSHOT_FILE), &snapshot)
                .map_err(ContentError::from),
            Err(error) => Err(error),
        };
        match saved {
            Ok(()) => tracing::info!("已记下改动前的状态"),
            Err(error) => tracing::warn!(%error, "记改动前的状态失败，这次改动不能一键回去"),
        }
    }

    /// 崩溃后调用：和快照比有没有改动。有就返回提议，没有（或者没有快照）返回 `None`。
    pub async fn crash_offer(&self) -> Result<Option<RollbackOffer>, ContentError> {
        let Some(snapshot) = self.load_snapshot() else {
            return Ok(None);
        };
        let current = self.capture(false).await?;
        let ops = diff(&snapshot, &current);
        if ops.is_empty() {
            return Ok(None);
        }
        let cache = IdentityCache::load(self.store.root());
        let changes = describe_changes(&snapshot, &current, &cache);
        if changes.is_empty() {
            return Ok(None);
        }
        Ok(Some(RollbackOffer {
            verified: snapshot.verified,
            at: snapshot.at,
            changes,
        }))
    }

    /// 检查从 `current` 恢复到 `target` 所需的所有文件是否都能拿到（已在仓库中或将由本次操作收进仓库）。
    pub(super) fn missing_for_restore(&self, target: &Snapshot, current: &Snapshot) -> Vec<String> {
        let ops = diff(target, current);
        let leaving: HashSet<&str> = ops
            .iter()
            .filter(|op| !op.toggle_only())
            .filter_map(|op| op.after.as_ref())
            .map(|f| f.sha1.as_str())
            .collect();
        ops.iter()
            .filter(|op| !op.toggle_only())
            .filter_map(|op| op.before.as_ref())
            .filter(|f| !self.store.contains(&f.sha1) && !leaving.contains(f.sha1.as_str()))
            .map(|f| f.file_name.clone())
            .collect()
    }

    /// 核心原子恢复原语：把实例恢复到指定 `target` 快照的状态，写入改动历史（返回改动项数）。
    pub(super) async fn apply_restore_to(
        &self,
        target: &Snapshot,
        current: &Snapshot,
        summary_label: &str,
    ) -> Result<u32, ContentError> {
        let ops = diff(target, current);
        let packs_changed =
            !target.resource_packs.is_empty() && target.resource_packs != current.resource_packs;
        let active_changed = target.shader != current.shader || packs_changed;
        if ops.is_empty() && !active_changed {
            return Ok(0);
        }

        let missing = self.missing_for_restore(target, current);
        if !missing.is_empty() {
            return Err(ContentError::Message(format!(
                "仓库里找不到 {} 的备份（可能是在文件夹里手动删掉的），没法一键回去。可以在「改动历史」里逐条撤销",
                missing.join("、")
            )));
        }

        let changes: Vec<Change> = ops
            .iter()
            .map(|op| Change {
                kind: op.kind,
                removed: op.after.as_ref().map(|f| current.file_ref(f)),
                added: op.before.as_ref().map(|f| target.file_ref(f)),
            })
            .collect();

        let mut draft = self.draft();
        // 现在多出来的、被换过的文件收进仓库（保证后续随时可前滚回来）
        for op in ops.iter().filter(|op| !op.toggle_only()) {
            let Some(after) = &op.after else { continue };
            draft.plan.take(
                rel_path(op.kind, &after.file_name, after.disabled)?,
                after.sha1.clone(),
            );
        }
        // 目标快照里的文件放回来；只是启停变了的改个名
        for op in &ops {
            let Some(before) = &op.before else { continue };
            let dest = rel_path(op.kind, &before.file_name, before.disabled)?;
            if op.toggle_only() {
                draft.plan.rename(
                    rel_path(op.kind, &before.file_name, !before.disabled)?,
                    dest,
                );
            } else {
                draft.plan.place_stored(dest, before.sha1.clone());
            }
        }
        draft.records = target.records.clone();
        if ops.iter().any(|op| op.kind == ContentKind::Shader) || target.shader != current.shader {
            draft.active.shader = target.shader.clone();
        }
        if (ops.iter().any(|op| op.kind == ContentKind::ResourcePack)
            || target.resource_packs != current.resource_packs)
            && !target.resource_packs.is_empty()
        {
            draft.active.packs = target.resource_packs.clone();
        }

        let count = changes.len().max(usize::from(active_changed));
        let summary = format!("{summary_label}（{count} 项改动）");
        let history = self.pushed_history(HistoryAction::Restore, summary.clone(), changes);
        self.commit_inner(draft, &summary, Some(history), false)
            .await?;
        tracing::info!(count, verified = target.verified, "已回到快照的状态");
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// 回到快照那时的样子：多出来的文件进仓库，少了的 / 换掉的从仓库放回来，启用状态和安装记录一并恢复。
    /// 恢复前自动将当前态留存到时间线，记一条改动历史（可以再撤销）。返回动了几个文件。要么全回去、要么一点不动。
    pub async fn restore_snapshot(&self) -> Result<u32, ContentError> {
        self.settle().await?;
        let snapshot = self
            .load_snapshot()
            .ok_or_else(|| ContentError::Message("这个实例还没有记下可以回去的状态".to_owned()))?;
        let current = self.capture(false).await?;
        let label = if snapshot.verified {
            "回到上次能玩的状态"
        } else {
            "回到改动前"
        };
        let count = self.apply_restore_to(&snapshot, &current, label).await?;
        if count > 0 {
            self.record_timeline_restore(&current, &snapshot, label, None);
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::store::STORE_DIR;

    fn instance() -> (tempfile::TempDir, InstanceContent) {
        let dir = tempfile::tempdir().unwrap();
        let content = InstanceContent::new(dir.path(), dir.path().join(STORE_DIR));
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        (dir, content)
    }

    fn change<'a>(offer: &'a RollbackOffer, title: &str) -> &'a OfferChange {
        offer.changes.iter().find(|c| c.title == title).unwrap()
    }

    #[tokio::test]
    async fn crash_after_changes_can_go_back_in_one_step() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::write(mods.join("a.jar"), b"a v1").unwrap();
        std::fs::write(mods.join("b.jar"), b"b").unwrap();
        std::fs::write(mods.join("d.jar"), b"d").unwrap();

        content.mark_good().await.unwrap();
        assert_eq!(content.crash_offer().await.unwrap(), None, "没改动就不提示");

        // 之后：a 被换成新内容（旧文件进仓库）、b 被删、d 被停用、c 是手动放进来的
        content.store.take(&mods.join("a.jar")).await.unwrap();
        std::fs::write(mods.join("a.jar"), b"a v2 broken").unwrap();
        content.remove(ContentKind::Mod, "b.jar").await.unwrap();
        content
            .set_enabled(ContentKind::Mod, "d.jar", false)
            .unwrap();
        std::fs::write(mods.join("c.jar"), b"c crashes the game").unwrap();

        let offer = content.crash_offer().await.unwrap().unwrap();
        assert!(offer.verified);
        assert_eq!(offer.changes.len(), 4);
        assert_eq!(change(&offer, "a").change, ChangeType::Updated);
        assert_eq!(change(&offer, "b").change, ChangeType::Removed);
        assert_eq!(change(&offer, "c").change, ChangeType::Added);
        assert_eq!(change(&offer, "d").change, ChangeType::Disabled);

        assert_eq!(content.restore_snapshot().await.unwrap(), 4);
        assert_eq!(std::fs::read(mods.join("a.jar")).unwrap(), b"a v1");
        assert_eq!(std::fs::read(mods.join("b.jar")).unwrap(), b"b");
        assert!(!mods.join("c.jar").exists());
        assert!(mods.join("d.jar").exists() && !mods.join("d.jar.disabled").exists());
        assert_eq!(content.crash_offer().await.unwrap(), None, "回去后没差别了");

        // 放回本身是一条历史，撤销后又是崩溃前的样子
        let history = content.history();
        assert_eq!(history[0].action, HistoryAction::Restore);
        assert_eq!(history[0].count, 4);
        content.rollback(&history[0].id).await.unwrap();
        assert_eq!(std::fs::read(mods.join("a.jar")).unwrap(), b"a v2 broken");
        assert!(!mods.join("b.jar").exists());
        assert!(mods.join("c.jar").exists());
        assert!(mods.join("d.jar.disabled").exists());
    }

    #[tokio::test]
    async fn first_change_records_the_state_before_it() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::write(mods.join("a.jar"), b"a").unwrap();

        // 从没正常玩过：第一次改动（删除）之前自动记一份
        content.remove(ContentKind::Mod, "a.jar").await.unwrap();
        std::fs::write(mods.join("b.jar"), b"b").unwrap();
        // 第二次改动不会盖掉第一份
        content.snapshot_before_change().await;

        let offer = content.crash_offer().await.unwrap().unwrap();
        assert!(!offer.verified);
        assert_eq!(offer.changes.len(), 2);

        content.restore_snapshot().await.unwrap();
        assert!(mods.join("a.jar").exists());
        assert!(!mods.join("b.jar").exists());

        // 正常玩过之后，快照换成新的
        std::fs::write(mods.join("e.jar"), b"e").unwrap();
        content.mark_good().await.unwrap();
        assert_eq!(content.crash_offer().await.unwrap(), None);
    }

    #[tokio::test]
    async fn refuses_when_a_backup_is_missing_and_changes_nothing() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::write(mods.join("a.jar"), b"a").unwrap();
        std::fs::write(mods.join("b.jar"), b"b").unwrap();
        content.mark_good().await.unwrap();

        // a 是在文件夹里手动删的（没进仓库），c 是新加的
        std::fs::remove_file(mods.join("a.jar")).unwrap();
        std::fs::write(mods.join("c.jar"), b"c").unwrap();

        let error = content.restore_snapshot().await.unwrap_err();
        assert!(error.to_string().contains("a.jar"), "{error}");
        assert!(mods.join("c.jar").exists(), "失败时什么都不动");
        assert!(content.history().is_empty());
    }

    #[tokio::test]
    async fn a_renamed_file_of_the_same_content_needs_no_backup() {
        let (dir, content) = instance();
        let mods = dir.path().join("mods");
        std::fs::write(mods.join("a.jar"), b"a").unwrap();
        content.mark_good().await.unwrap();
        std::fs::rename(mods.join("a.jar"), mods.join("a-renamed.jar")).unwrap();

        content.restore_snapshot().await.unwrap();
        assert_eq!(std::fs::read(mods.join("a.jar")).unwrap(), b"a");
        assert!(!mods.join("a-renamed.jar").exists());
    }

    #[tokio::test]
    async fn shader_choice_comes_back_with_the_file() {
        let (dir, content) = instance();
        std::fs::create_dir_all(dir.path().join("shaderpacks")).unwrap();
        std::fs::write(dir.path().join("shaderpacks/BSL.zip"), b"bsl").unwrap();
        content
            .set_enabled(ContentKind::Shader, "BSL.zip", true)
            .unwrap();
        content.mark_good().await.unwrap();

        content
            .remove(ContentKind::Shader, "BSL.zip")
            .await
            .unwrap();
        assert_eq!(current_shader(dir.path()), None);

        content.restore_snapshot().await.unwrap();
        assert_eq!(current_shader(dir.path()).as_deref(), Some("BSL.zip"));
    }
}
