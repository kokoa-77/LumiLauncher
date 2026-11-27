//! 「永不玩坏」可视化时间线（V1.1-B1）。
//!
//! 职责：
//! 1. 在 `Lumi/timeline.json` 中维护实例内容的多节点快照链（最多保留 [`MAX_TIMELINE_NODES`] 个，
//!    文件本体全部去重保存在 `.mcl-store`，几乎零额外磁盘占用）；
//! 2. 融合 `Lumi/launches/` 历史启动记录，在时间轴上精准标出：
//!    - **上次已验证可玩**（绿色徽章）：进过世界且正常退出对应的快照节点；
//!    - **改动后崩溃**（红标）：未验证改动后紧接着发生崩溃的启动记录及对应的改动节点；
//! 3. 计算相邻快照之间的详细 diff（新增 / 移除 / 更新 / 启用 / 停用，涵盖模组、光影、资源包）；
//! 4. 支持一键恢复到**任意历史节点**，恢复前自动将当前状态存为节点（永不丢状态，支持来回跳转与前滚）。

use serde::{Deserialize, Serialize};

use super::identify::IdentityCache;
use super::snapshot::{OfferChange, Snapshot, describe_changes};
use super::{ContentError, ContentKind, InstanceContent};
use crate::fs::{read_json, write_json_atomic};
use crate::launch::record::{self as launch_record, LaunchOutcome};

const TIMELINE_FILE: &str = "timeline.json";
/// 兼容尚无 `timeline.json` 的旧实例时合成基线节点的固定 ID
const LEGACY_SNAPSHOT_ID: &str = "legacy-snapshot";
/// 每个实例最多保留多少个时间线快照节点（超限时优先保留最近一次已验证可玩的节点）
const MAX_TIMELINE_NODES: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum TimelineTrigger {
    /// 首次改动前自动记录的基线状态
    Initial,
    /// 安装 / 更新 / 切换版本 / 删除 / 启停 / 撤销等改动后的状态
    Change,
    /// 游戏进过世界并正常退出验证可玩的状态
    VerifiedPlay,
    /// 恢复任意历史节点前自动为当前态保存的保护节点（永不丢状态）
    BeforeRestore,
    /// 恢复到某个历史节点后形成的状态
    Restored,
}

/// 改动前在内存中暂存的快照令牌（操作真正成功后才落盘，避免失败操作污染时间线）。
pub struct TimelineGuard(Snapshot);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineNode {
    pub id: String,
    pub at: f64,
    pub trigger: TimelineTrigger,
    pub summary: String,
    pub verified: bool,
    #[serde(default)]
    pub verified_at: Option<f64>,
    #[serde(default)]
    pub restored_from: Option<String>,
    /// 记录该节点时的 Minecraft 版本（用于跨 MC 大版本升级后的防呆提示）
    #[serde(default)]
    pub game_version: Option<String>,
    pub snapshot: Snapshot,
}

/// 给界面渲染的完整时间线视图（快照节点 + 启动记录按时间倒序融合）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct TimelineView {
    /// 融合后的时间线条目（最新的在前）
    pub entries: Vec<TimelineEntry>,
    /// 最近一次「已验证可玩」的快照节点 ID（供顶部一键回退横幅使用）
    pub latest_verified_id: Option<String>,
    /// 当前磁盘状态与最新快照不一致（例如用户在文件夹里手动增删了模组）
    pub has_unsaved_changes: bool,
    /// 相对最近一次「已验证可玩」节点是否存在未验证的改动
    pub changed_since_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum TimelineEntry {
    Snapshot {
        id: String,
        at: f64,
        trigger: TimelineTrigger,
        summary: String,
        /// 该状态经过进世界正常退出验证
        verified: bool,
        /// 是否是最近一次「上次已验证可玩」节点（绿色主徽章）
        is_latest_verified: bool,
        /// 是否为当前实例所处的状态（时间轴最新且内容一致）
        is_current: bool,
        /// 内容是否与当前磁盘状态完全一致（用于历史同态节点防呆禁点）
        same_as_current: bool,
        /// 该节点记录时的 Minecraft 版本（老节点可能为空）
        game_version: Option<String>,
        /// 该节点所属 Minecraft 版本与当前实例不同（恢复时仅动模组/光影/资源包，不改游戏版本）
        version_mismatch: bool,
        /// 该节点之后紧接着发生过崩溃或启动失败（红色警告标记）
        crashed_after: bool,
        /// 该节点包含的内容文件总数（模组 + 光影 + 资源包）
        file_count: u32,
        /// 相对上一个时间线节点的详细改动列表
        changes: Vec<OfferChange>,
        /// 该节点是否可以直接一键恢复（所需文件均在本地或内容仓库中）
        restorable: bool,
    },
    Launch {
        record_id: String,
        at: f64,
        ended_at: Option<f64>,
        outcome: LaunchOutcome,
        reached_world: bool,
        exit_code: Option<i32>,
        game_version: String,
        loader: Option<String>,
        mod_count: u32,
        has_crash_report: bool,
        error: Option<String>,
        /// 该次启动属于「改动后崩溃」（红色徽章）
        crashed_after_change: bool,
        /// 该次启动属于「已验证可玩」（绿色徽章）
        verified_play: bool,
    },
}

impl TimelineEntry {
    fn sort_key(&self) -> (f64, u8) {
        match self {
            // 同一毫秒内先显示启动结局，再显示同期落盘的快照
            TimelineEntry::Launch { at, .. } => (*at, 1),
            TimelineEntry::Snapshot { at, .. } => (*at, 0),
        }
    }
}

fn new_node(
    trigger: TimelineTrigger,
    summary: String,
    verified: bool,
    restored_from: Option<String>,
    game_version: Option<String>,
    snapshot: Snapshot,
) -> TimelineNode {
    let at = snapshot.at;
    TimelineNode {
        id: uuid::Uuid::new_v4().simple().to_string(),
        at,
        trigger,
        summary,
        verified,
        verified_at: verified.then_some(at),
        restored_from,
        game_version,
        snapshot,
    }
}

fn legacy_node(legacy: Snapshot, game_version: Option<String>) -> TimelineNode {
    let trigger = if legacy.verified {
        TimelineTrigger::VerifiedPlay
    } else {
        TimelineTrigger::Initial
    };
    let label = if legacy.verified {
        "上次已验证可玩".to_owned()
    } else {
        "初始状态".to_owned()
    };
    let mut node = new_node(trigger, label, legacy.verified, None, game_version, legacy);
    node.id = LEGACY_SNAPSHOT_ID.to_owned();
    node
}

fn prune_nodes(nodes: &mut Vec<TimelineNode>) {
    if nodes.len() <= MAX_TIMELINE_NODES {
        return;
    }
    let mut latest_verified = nodes.iter().rposition(|n| n.verified);
    let mut excess = nodes.len() - MAX_TIMELINE_NODES;
    let mut idx = 0;
    while excess > 0 && idx < nodes.len() {
        if latest_verified == Some(idx) {
            idx += 1;
            continue;
        }
        nodes.remove(idx);
        excess -= 1;
        if let Some(v) = latest_verified
            && idx < v
        {
            latest_verified = Some(v - 1);
        }
    }
}

impl InstanceContent {
    fn current_game_version(&self) -> Option<String> {
        let value: serde_json::Value = read_json(&self.meta_path("instance.json")).ok()??;
        value
            .get("gameVersion")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    fn load_timeline_nodes(&self) -> Vec<TimelineNode> {
        match read_json(&self.meta_path(TIMELINE_FILE)) {
            Ok(Some(nodes)) => nodes,
            Ok(None) => Vec::new(),
            Err(error) => {
                tracing::warn!(%error, "时间线快照读不了，按空处理");
                Vec::new()
            }
        }
    }

    fn save_timeline_nodes(&self, nodes: &mut Vec<TimelineNode>) {
        prune_nodes(nodes);
        if let Err(error) = write_json_atomic(&self.meta_path(TIMELINE_FILE), nodes) {
            tracing::warn!(%error, "保存时间线快照失败");
        }
    }

    /// 改动前确保时间线存在基线节点：
    /// - 时间线为空时，若已有旧版 `snapshot.json` 先迁移为首个节点，再补齐当前状态；
    /// - 时间线非空但磁盘被外部手动改过时，补记一个「外部改动」节点，防止丢状态。
    pub(super) fn ensure_timeline_baseline(&self, current: &Snapshot) {
        let mut nodes = self.load_timeline_nodes();
        let mut changed = false;
        let mc = self.current_game_version();
        if nodes.is_empty() {
            if let Some(legacy) = self.load_snapshot() {
                let same = legacy.same_state(current);
                nodes.push(legacy_node(legacy, mc.clone()));
                if !same {
                    nodes.push(new_node(
                        TimelineTrigger::Change,
                        "外部改动内容".to_owned(),
                        false,
                        None,
                        mc,
                        current.clone(),
                    ));
                }
            } else {
                nodes.push(new_node(
                    TimelineTrigger::Initial,
                    "初始状态".to_owned(),
                    false,
                    None,
                    mc,
                    current.clone(),
                ));
            }
            changed = true;
        } else if let Some(last) = nodes.last()
            && !last.snapshot.same_state(current)
        {
            nodes.push(new_node(
                TimelineTrigger::Change,
                "外部改动内容".to_owned(),
                false,
                None,
                mc,
                current.clone(),
            ));
            changed = true;
        }
        if changed {
            self.save_timeline_nodes(&mut nodes);
        }
    }

    /// 供外部操作（如单文件启停、整合包无损更新、版本切换）在改动前抓取内存态基线，
    /// 改动成功后配合 [`Self::finish_timeline_change`] 一起落盘，确保操作失败时不留下脏节点。
    pub async fn begin_timeline_change(&self) -> Option<TimelineGuard> {
        self.capture(false).await.ok().map(TimelineGuard)
    }

    /// 配合 [`Self::begin_timeline_change`] 在操作成功后落盘基线与变动节点。
    pub async fn finish_timeline_change(&self, guard: Option<TimelineGuard>, summary: String) {
        if let Some(TimelineGuard(before)) = &guard {
            self.ensure_timeline_baseline(before);
        }
        self.record_timeline_change(summary).await;
    }

    /// 单项启用/停用成功后的快速时间线落盘：
    /// 对模组启停直接从改动前快照内存派生新状态，免去第二次全目录扫描与重复读缓存。
    pub async fn finish_timeline_toggle(
        &self,
        guard: Option<TimelineGuard>,
        kind: ContentKind,
        file_name: &str,
        enabled: bool,
        summary: String,
    ) {
        if kind == ContentKind::Mod
            && let Some(TimelineGuard(before)) = guard
        {
            self.ensure_timeline_baseline(&before);
            let mut after = before.clone();
            after.at = crate::now_millis() as f64;
            after.verified = false;
            if let Some(entry) = after
                .files
                .iter_mut()
                .find(|f| f.kind == ContentKind::Mod && f.file_name.eq_ignore_ascii_case(file_name))
            {
                entry.disabled = !enabled;
                self.push_timeline_change_snapshot(summary, after);
                return;
            }
            self.record_timeline_change(summary).await;
            return;
        }
        self.finish_timeline_change(guard, summary).await;
    }

    fn push_timeline_change_snapshot(&self, summary: String, current: Snapshot) {
        let mut nodes = self.load_timeline_nodes();
        if let Some(last) = nodes.last()
            && last.snapshot.same_state(&current)
        {
            return;
        }
        nodes.push(new_node(
            TimelineTrigger::Change,
            summary,
            false,
            None,
            self.current_game_version(),
            current,
        ));
        self.save_timeline_nodes(&mut nodes);
    }

    /// 一次内容改动成功后调用：把改动后的最新状态记入时间线。失败仅记日志，不阻塞主操作。
    pub async fn record_timeline_change(&self, summary: String) {
        let current = match self.capture(false).await {
            Ok(snap) => snap,
            Err(error) => {
                tracing::warn!(%error, "记录时间线变动节点失败");
                return;
            }
        };
        self.push_timeline_change_snapshot(summary, current);
    }

    /// 游戏进世界并正常退出时调用：若最新节点与当前态一致则直接点亮「已验证可玩」，否则新增验证节点。
    pub(super) fn record_timeline_verified(&self, current: &Snapshot) {
        let mut nodes = self.load_timeline_nodes();
        if let Some(last) = nodes.last_mut()
            && last.snapshot.same_state(current)
        {
            last.verified = true;
            last.verified_at = Some(current.at);
            self.save_timeline_nodes(&mut nodes);
            return;
        }
        nodes.push(new_node(
            TimelineTrigger::VerifiedPlay,
            "已验证可玩".to_owned(),
            true,
            None,
            self.current_game_version(),
            current.clone(),
        ));
        self.save_timeline_nodes(&mut nodes);
    }

    /// 恢复操作完成后写入「旧基线（如需要）+ 恢复前自动保存（如需要）+ 已恢复」节点，保证随时可再次前滚回来。
    pub(super) fn record_timeline_restore(
        &self,
        before_restore: &Snapshot,
        restored_target: &Snapshot,
        label: &str,
        restored_from: Option<String>,
    ) {
        let mut nodes = self.load_timeline_nodes();
        let mc = self.current_game_version();
        if nodes.is_empty()
            && let Some(legacy) = self.load_snapshot()
        {
            nodes.push(legacy_node(legacy, mc.clone()));
        }
        let needs_guard = nodes
            .last()
            .is_none_or(|last| !last.snapshot.same_state(before_restore));
        if needs_guard {
            nodes.push(new_node(
                TimelineTrigger::BeforeRestore,
                "恢复前自动保存".to_owned(),
                false,
                None,
                mc.clone(),
                before_restore.clone(),
            ));
        }
        let mut restored_snap = restored_target.clone();
        restored_snap.at = crate::now_millis() as f64;
        nodes.push(new_node(
            TimelineTrigger::Restored,
            label.to_owned(),
            restored_target.verified,
            restored_from,
            mc,
            restored_snap,
        ));
        self.save_timeline_nodes(&mut nodes);
    }

    /// 构建「永不玩坏」融合时间线（快照节点 + 历史启动记录，按时间倒序排列）。
    pub async fn timeline(
        &self,
        running_record_id: Option<&str>,
    ) -> Result<TimelineView, ContentError> {
        let current = self.capture(false).await?;
        let current_mc = self.current_game_version();
        let cache = IdentityCache::load(self.store.root());

        // 若还没有 timeline.json 但有旧版 snapshot.json，自动以只读方式合成基线节点展示
        let mut nodes = self.load_timeline_nodes();
        if nodes.is_empty()
            && let Some(legacy) = self.load_snapshot()
        {
            nodes.push(legacy_node(legacy, current_mc.clone()));
        }

        // 启动记录按从旧到新排，用来与快照节点按时间对齐
        let mut launches = launch_record::list(&self.dir, running_record_id);
        launches.sort_by(|a, b| a.started_at.total_cmp(&b.started_at));

        // 统计每个快照节点是否已验证（兼容早于 timeline 落盘的成功游玩记录）以及之后是否紧接着崩溃
        let mut verified_flags: Vec<bool> = nodes.iter().map(|n| n.verified).collect();
        let mut crashed_after_flags: Vec<bool> = vec![false; nodes.len()];
        let mut launch_entries: Vec<TimelineEntry> = Vec::with_capacity(launches.len());

        for launch in &launches {
            let active_idx = nodes.iter().rposition(|n| n.at <= launch.started_at);
            let verified_play = launch.outcome == LaunchOutcome::Exited && launch.reached_world;
            if verified_play && let Some(idx) = active_idx {
                verified_flags[idx] = true;
            }
            let is_crash = matches!(
                launch.outcome,
                LaunchOutcome::Crashed | LaunchOutcome::Failed
            );
            let crashed_after_change = if is_crash {
                if let Some(idx) = active_idx {
                    let node_verified = verified_flags[idx];
                    let has_change_after_verified = !node_verified && idx > 0;
                    let is_unverified_change = !node_verified
                        && matches!(
                            nodes[idx].trigger,
                            TimelineTrigger::Change | TimelineTrigger::Restored
                        );
                    let flag = has_change_after_verified || is_unverified_change;
                    if flag {
                        crashed_after_flags[idx] = true;
                    }
                    flag
                } else {
                    false
                }
            } else {
                false
            };

            launch_entries.push(TimelineEntry::Launch {
                record_id: launch.id.clone(),
                at: launch.started_at,
                ended_at: launch.ended_at,
                outcome: launch.outcome,
                reached_world: launch.reached_world,
                exit_code: launch.exit_code,
                game_version: launch.game_version.clone(),
                loader: launch.loader.clone(),
                mod_count: u32::try_from(launch.mods.len()).unwrap_or(u32::MAX),
                has_crash_report: launch.has_crash_report,
                error: launch.error.clone(),
                crashed_after_change,
                verified_play,
            });
        }

        let latest_verified_idx = verified_flags.iter().rposition(|&v| v);
        let latest_verified_id = latest_verified_idx.map(|i| nodes[i].id.clone());
        let has_unsaved_changes = nodes
            .last()
            .is_some_and(|last| !last.snapshot.same_state(&current));
        let changed_since_verified =
            latest_verified_idx.is_some_and(|idx| !nodes[idx].snapshot.same_state(&current));

        let mut entries: Vec<TimelineEntry> =
            Vec::with_capacity(nodes.len() + launch_entries.len());

        for (idx, node) in nodes.iter().enumerate() {
            let changes = if idx == 0 {
                Vec::new()
            } else {
                describe_changes(&nodes[idx - 1].snapshot, &node.snapshot, &cache)
            };
            let same_as_current = node.snapshot.same_state(&current);
            let is_current = idx + 1 == nodes.len() && same_as_current;
            let version_mismatch = current_mc
                .as_deref()
                .is_some_and(|cur| node.game_version.as_deref().is_some_and(|v| v != cur));
            let restorable = same_as_current
                || self
                    .missing_for_restore(&node.snapshot, &current)
                    .is_empty();
            entries.push(TimelineEntry::Snapshot {
                id: node.id.clone(),
                at: node.at,
                trigger: node.trigger,
                summary: node.summary.clone(),
                verified: verified_flags[idx],
                is_latest_verified: latest_verified_idx == Some(idx),
                is_current,
                same_as_current,
                game_version: node.game_version.clone(),
                version_mismatch,
                crashed_after: crashed_after_flags[idx],
                file_count: u32::try_from(node.snapshot.files.len()).unwrap_or(u32::MAX),
                changes,
                restorable,
            });
        }
        entries.extend(launch_entries);
        entries.sort_by(|a, b| {
            let (ta, pa) = a.sort_key();
            let (tb, pb) = b.sort_key();
            tb.total_cmp(&ta).then_with(|| pb.cmp(&pa))
        });

        Ok(TimelineView {
            entries,
            latest_verified_id,
            has_unsaved_changes,
            changed_since_verified,
        })
    }

    /// 一键恢复到时间线上的任意历史快照节点：
    /// 恢复前先校验完整性，仅在日志事务彻底成功后才更新 `timeline.json`（失败零副作用），
    /// 并自动保留当前态节点、把被替换/移走的文件全量收进 `.mcl-store`，随时可再次前滚回来。
    pub async fn restore_timeline_node(&self, node_id: &str) -> Result<u32, ContentError> {
        self.settle().await?;
        let nodes = self.load_timeline_nodes();
        let target = if let Some(found) = nodes.iter().find(|n| n.id == node_id).cloned() {
            found
        } else if nodes.is_empty()
            && node_id == LEGACY_SNAPSHOT_ID
            && let Some(legacy) = self.load_snapshot()
        {
            legacy_node(legacy, self.current_game_version())
        } else {
            return Err(ContentError::Message(
                "找不到对应的时间线快照节点，可能已被清理".to_owned(),
            ));
        };

        let current = self.capture(false).await?;
        if target.snapshot.same_state(&current) {
            return Ok(0);
        }

        let clean_summary = target
            .summary
            .strip_prefix("恢复到「")
            .and_then(|s| s.strip_suffix('」'))
            .unwrap_or(&target.summary);
        let label = format!("恢复到「{clean_summary}」");
        let count = self
            .apply_restore_to(&target.snapshot, &current, &label)
            .await?;
        if count > 0 {
            self.record_timeline_restore(&current, &target.snapshot, &label, Some(target.id));
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::store::STORE_DIR;
    use crate::content::{ContentKind, LocalAdd};
    use crate::launch::record::{LaunchRecord, RecordEnd, RecordWriter};

    fn setup() -> (tempfile::TempDir, InstanceContent) {
        let dir = tempfile::tempdir().unwrap();
        let content = InstanceContent::new(dir.path(), dir.path().join(STORE_DIR));
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        (dir, content)
    }

    fn write_launch(dir: &std::path::Path, started_at: f64, outcome: LaunchOutcome, reached: bool) {
        let record = LaunchRecord::start(started_at, "1.21.1".to_owned(), None, None, &[]);
        let writer = RecordWriter::begin(dir, record).unwrap();
        let _ = writer.finish(RecordEnd {
            ended_at: started_at + 10_000.0,
            outcome,
            exit_code: Some(if outcome == LaunchOutcome::Crashed {
                -1
            } else {
                0
            }),
            reached_world: reached,
            error: None,
            crash_report: None,
        });
    }

    #[tokio::test]
    async fn timeline_tracks_changes_launches_jump_back_and_roll_forward() {
        let (dir, content) = setup();
        let mods = dir.path().join("mods");
        let src = dir.path().join("incoming");
        std::fs::create_dir_all(&src).unwrap();

        // 初始状态：只有 base.jar
        std::fs::write(mods.join("base.jar"), b"base-v1").unwrap();
        // 改动 1：安装 mod-a.jar
        std::fs::write(src.join("mod-a.jar"), b"mod-a-bytes").unwrap();
        content
            .add_local(&[LocalAdd {
                kind: ContentKind::Mod,
                source: src.join("mod-a.jar"),
                replace: None,
            }])
            .await
            .unwrap();

        // 验证可玩：记一次成功进世界的启动 + mark_good
        let t_good = crate::now_millis() as f64 + 10.0;
        write_launch(dir.path(), t_good, LaunchOutcome::Exited, true);
        content.mark_good().await.unwrap();

        // 改动 2：安装 mod-b.jar
        std::fs::write(src.join("mod-b.jar"), b"mod-b-bytes").unwrap();
        content
            .add_local(&[LocalAdd {
                kind: ContentKind::Mod,
                source: src.join("mod-b.jar"),
                replace: None,
            }])
            .await
            .unwrap();

        // 改动 3：停用 mod-a.jar
        content
            .set_many_enabled(ContentKind::Mod, &["mod-a.jar".to_owned()], false)
            .await
            .unwrap();

        // 改动后崩溃：记一次崩溃启动（时间晚于改动 3）
        let t_crash = crate::now_millis() as f64 + 50.0;
        write_launch(dir.path(), t_crash, LaunchOutcome::Crashed, false);

        // 校验时间线融合状态
        let view = content.timeline(None).await.unwrap();
        assert!(
            view.latest_verified_id.is_some(),
            "应标出上次已验证可玩节点"
        );
        assert!(view.changed_since_verified, "崩溃前有未验证改动");

        let snapshots: Vec<_> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                TimelineEntry::Snapshot {
                    id,
                    trigger,
                    verified,
                    is_latest_verified,
                    crashed_after,
                    changes,
                    restorable,
                    ..
                } => Some((
                    id.clone(),
                    *trigger,
                    *verified,
                    *is_latest_verified,
                    *crashed_after,
                    changes.clone(),
                    *restorable,
                )),
                TimelineEntry::Launch { .. } => None,
            })
            .collect();
        // 至少包含：Initial -> 改动1(已验证可玩) -> 改动2 -> 改动3(改动后崩溃)
        assert_eq!(snapshots.len(), 4);
        assert!(
            snapshots.iter().any(|s| s.2 && s.3),
            "应标出绿色已验证可玩节点"
        );
        assert!(snapshots[0].4, "最新改动节点紧接崩溃，应标出 crashed_after");
        assert!(!snapshots[0].5.is_empty(), "最新改动节点应有详细 diff");
        assert!(snapshots.iter().all(|s| s.6), "所有历史节点均应可一键恢复");

        let launches: Vec<_> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                TimelineEntry::Launch {
                    crashed_after_change,
                    verified_play,
                    ..
                } => Some((*crashed_after_change, *verified_play)),
                TimelineEntry::Snapshot { .. } => None,
            })
            .collect();
        assert_eq!(launches.len(), 2);
        assert!(launches.iter().any(|(c, _)| *c), "崩溃启动应标出改动后崩溃");
        assert!(launches.iter().any(|(_, v)| *v), "正常游玩应标出已验证可玩");

        // 跳转到第 1 个节点（初始状态，仅剩 base.jar）
        let initial_id = snapshots.last().unwrap().0.clone();
        let tip_before_jump_id = snapshots[0].0.clone();
        let moved = content.restore_timeline_node(&initial_id).await.unwrap();
        assert!(moved >= 2);
        assert!(mods.join("base.jar").is_file());
        assert!(!mods.join("mod-a.jar").exists());
        assert!(!mods.join("mod-a.jar.disabled").exists());
        assert!(!mods.join("mod-b.jar").exists());

        // 再次前滚回到跳回之前的第 3 次改动状态（mod-a 停用 + mod-b 启用 + base）
        let forward_moved = content
            .restore_timeline_node(&tip_before_jump_id)
            .await
            .unwrap();
        assert!(forward_moved >= 2);
        assert!(mods.join("base.jar").is_file());
        assert!(mods.join("mod-a.jar.disabled").is_file());
        assert!(mods.join("mod-b.jar").is_file());
    }

    #[tokio::test]
    async fn timeline_never_touches_saves_or_configs_and_preserves_manual_files_on_restore() {
        let (dir, content) = setup();
        let mods = dir.path().join("mods");
        let saves = dir.path().join("saves/MyWorld");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let world_bytes = b"precious-world-nbt-data";
        let cfg_bytes = b"difficulty = \"hard\"\n";
        std::fs::write(saves.join("level.dat"), world_bytes).unwrap();
        std::fs::write(config.join("custom.toml"), cfg_bytes).unwrap();

        std::fs::write(mods.join("core.jar"), b"core-v1").unwrap();
        content.mark_good().await.unwrap();

        // 玩家在文件夹里手动放入一个新模组（未通过启动器安装）
        std::fs::write(mods.join("manual-drop.jar"), b"manual-mod-bytes").unwrap();
        let before_view = content.timeline(None).await.unwrap();
        assert!(before_view.has_unsaved_changes);
        let verified_id = before_view.latest_verified_id.clone().unwrap();

        // 一键回到已验证可玩状态：manual-drop.jar 应移入仓库并自动生成 BeforeRestore 保护节点
        let moved = content.restore_timeline_node(&verified_id).await.unwrap();
        assert_eq!(moved, 1);
        assert!(!mods.join("manual-drop.jar").exists());
        assert_eq!(std::fs::read(saves.join("level.dat")).unwrap(), world_bytes);
        assert_eq!(
            std::fs::read(config.join("custom.toml")).unwrap(),
            cfg_bytes
        );

        // 通过自动生成的 BeforeRestore 保护节点一键前滚找回手动放入的文件
        let after_view = content.timeline(None).await.unwrap();
        let guard_id = after_view
            .entries
            .iter()
            .find_map(|e| match e {
                TimelineEntry::Snapshot {
                    id,
                    trigger: TimelineTrigger::BeforeRestore,
                    restorable,
                    ..
                } => {
                    assert!(*restorable, "恢复前保护节点必须可完整前滚");
                    Some(id.clone())
                }
                _ => None,
            })
            .expect("存在外部改动时必须自动保存 BeforeRestore 节点");

        let restored = content.restore_timeline_node(&guard_id).await.unwrap();
        assert_eq!(restored, 1);
        assert_eq!(
            std::fs::read(mods.join("manual-drop.jar")).unwrap(),
            b"manual-mod-bytes"
        );
        // 反复跳回与前滚后，世界存档与配置文件依然 100% 字节无损
        assert_eq!(std::fs::read(saves.join("level.dat")).unwrap(), world_bytes);
        assert_eq!(
            std::fs::read(config.join("custom.toml")).unwrap(),
            cfg_bytes
        );
    }

    #[tokio::test]
    async fn timeline_aborts_cleanly_when_backup_missing_and_flags_mc_version_mismatch() {
        let (dir, content) = setup();
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(dir.path().join("Lumi")).unwrap();
        std::fs::write(
            dir.path().join("Lumi/instance.json"),
            r#"{"format":1,"name":"Test","gameVersion":"1.20.1","versionId":"1.20.1","createdAt":1}"#,
        )
        .unwrap();
        std::fs::write(mods.join("old.jar"), b"old-bytes").unwrap();
        content.mark_good().await.unwrap();

        // 模拟玩家在外部手动删除 old.jar（没进内容仓库），再把实例升级到 1.21.1
        std::fs::remove_file(mods.join("old.jar")).unwrap();
        std::fs::write(mods.join("new.jar"), b"new-bytes").unwrap();
        std::fs::write(
            dir.path().join("Lumi/instance.json"),
            r#"{"format":1,"name":"Test","gameVersion":"1.21.1","versionId":"1.21.1","createdAt":1}"#,
        )
        .unwrap();

        let view = content.timeline(None).await.unwrap();
        let old_snap = view
            .entries
            .iter()
            .find_map(|e| match e {
                TimelineEntry::Snapshot {
                    id,
                    restorable,
                    version_mismatch,
                    game_version,
                    ..
                } => Some((
                    id.clone(),
                    *restorable,
                    *version_mismatch,
                    game_version.clone(),
                )),
                _ => None,
            })
            .unwrap();
        assert!(!old_snap.1, "缺失仓库备份的节点应标为不可恢复");
        assert!(old_snap.2, "跨 MC 版本节点应标出 version_mismatch");
        assert_eq!(old_snap.3.as_deref(), Some("1.20.1"));

        // 强行请求恢复：必须在动手前报错，磁盘上的 new.jar 纹丝不动
        assert!(content.restore_timeline_node(&old_snap.0).await.is_err());
        assert_eq!(std::fs::read(mods.join("new.jar")).unwrap(), b"new-bytes");
    }
}
