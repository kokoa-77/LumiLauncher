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
struct Snapshot {
    at: f64,
    /// 游戏在这个状态下正常玩过；否则只是「第一次改动之前」的样子
    verified: bool,
    files: Vec<SnapFile>,
    #[serde(default)]
    records: Records,
    #[serde(default)]
    shader: Option<String>,
    #[serde(default)]
    resource_packs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapFile {
    kind: ContentKind,
    /// 启用时的文件名
    file_name: String,
    sha1: String,
    /// 模组是不是停用状态
    disabled: bool,
}

impl Snapshot {
    fn enabled(&self, file: &SnapFile) -> bool {
        match file.kind {
            ContentKind::Mod => !file.disabled,
            ContentKind::Shader => self.shader.as_deref() == Some(file.file_name.as_str()),
            ContentKind::ResourcePack => self
                .resource_packs
                .contains(&format!("file/{}", file.file_name)),
        }
    }

    fn file_ref(&self, file: &SnapFile) -> FileRef {
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
}

/// 同一个文件名在快照里和现在的差别。`before` 是快照里的，`after` 是现在的。
#[derive(Debug)]
struct Op {
    kind: ContentKind,
    before: Option<SnapFile>,
    after: Option<SnapFile>,
}

impl Op {
    /// 文件内容没变，只是启用 / 停用变了
    fn toggle_only(&self) -> bool {
        matches!((&self.before, &self.after), (Some(b), Some(a)) if b.sha1 == a.sha1)
    }
}

fn diff(snapshot: &Snapshot, current: &Snapshot) -> Vec<Op> {
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

impl InstanceContent {
    fn load_snapshot(&self) -> Option<Snapshot> {
        match read_json(&self.meta_path(SNAPSHOT_FILE)) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "快照读不了，当作没有");
                None
            }
        }
    }

    /// 把实例现在的样子记成清单（算哈希用缓存，没改过的文件不重算）。
    async fn capture(&self, verified: bool) -> Result<Snapshot, ContentError> {
        let mut cache = IdentityCache::load(self.store.root());
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
        if let Err(error) = cache.save(self.store.root()) {
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

    /// 游戏在现在这个状态下正常玩过：记成「上次能玩的状态」。
    pub async fn mark_good(&self) -> Result<(), ContentError> {
        let snapshot = self.capture(true).await?;
        write_json_atomic(&self.meta_path(SNAPSHOT_FILE), &snapshot)?;
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
        let mut changes = Vec::new();
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for op in &ops {
            match (&op.before, &op.after) {
                (Some(before), Some(after)) => {
                    let now = describe(after, &current.records, &cache);
                    let change = if before.sha1 != after.sha1 {
                        ChangeType::Updated
                    } else if after.disabled {
                        ChangeType::Disabled
                    } else {
                        ChangeType::Enabled
                    };
                    let was = describe(before, &snapshot.records, &cache);
                    let updated = change == ChangeType::Updated;
                    changes.push(OfferChange {
                        kind: op.kind,
                        change,
                        title: now.title,
                        from_version: was.version.filter(|_| updated),
                        to_version: now.version.filter(|_| updated),
                    });
                }
                (None, Some(after)) => {
                    added.push((op.kind, describe(after, &current.records, &cache)))
                }
                (Some(before), None) => {
                    removed.push((op.kind, describe(before, &snapshot.records, &cache)))
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
        changes.sort_by_key(|c| (c.kind, c.title.to_lowercase()));
        Ok(Some(RollbackOffer {
            verified: snapshot.verified,
            at: snapshot.at,
            changes,
        }))
    }

    /// 回到快照那时的样子：多出来的文件进仓库，少了的 / 换掉的从仓库放回来，启用状态和安装记录一并恢复。
    /// 记一条改动历史（可以再撤销）。返回动了几个文件。要么全回去、要么一点不动。
    pub async fn restore_snapshot(&self) -> Result<u32, ContentError> {
        self.settle().await?;
        let snapshot = self
            .load_snapshot()
            .ok_or_else(|| ContentError::Message("这个实例还没有记下可以回去的状态".to_owned()))?;
        let current = self.capture(false).await?;
        let ops = diff(&snapshot, &current);
        if ops.is_empty() {
            return Ok(0);
        }

        // 先确认要放回的文件都拿得到（仓库里有，或者就是这次要收走的文件），缺一个就整个不做
        let leaving: HashSet<&str> = ops
            .iter()
            .filter(|op| !op.toggle_only())
            .filter_map(|op| op.after.as_ref())
            .map(|f| f.sha1.as_str())
            .collect();
        let missing: Vec<&str> = ops
            .iter()
            .filter(|op| !op.toggle_only())
            .filter_map(|op| op.before.as_ref())
            .filter(|f| !self.store.contains(&f.sha1) && !leaving.contains(f.sha1.as_str()))
            .map(|f| f.file_name.as_str())
            .collect();
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
                added: op.before.as_ref().map(|f| snapshot.file_ref(f)),
            })
            .collect();

        let mut draft = self.draft();
        // 现在多出来的、被换过的文件收进仓库
        for op in ops.iter().filter(|op| !op.toggle_only()) {
            let Some(after) = &op.after else { continue };
            draft.plan.take(
                rel_path(op.kind, &after.file_name, after.disabled)?,
                after.sha1.clone(),
            );
        }
        // 快照里的文件放回来；只是启停变了的改个名
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
        draft.records = snapshot.records.clone();
        // 动过光影 / 材质包文件的，把「用哪个」也恢复成当时的
        if ops.iter().any(|op| op.kind == ContentKind::Shader) {
            draft.active.shader = snapshot.shader.clone();
        }
        if ops.iter().any(|op| op.kind == ContentKind::ResourcePack)
            && !snapshot.resource_packs.is_empty()
        {
            draft.active.packs = snapshot.resource_packs.clone();
        }

        let count = changes.len();
        let summary = if snapshot.verified {
            format!("回到上次能玩的状态（{count} 项改动）")
        } else {
            format!("回到改动前（{count} 项改动）")
        };
        let history = self.pushed_history(HistoryAction::Restore, summary.clone(), changes);
        self.commit(draft, &summary, Some(history)).await?;
        tracing::info!(count, verified = snapshot.verified, "已回到快照的状态");
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
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
