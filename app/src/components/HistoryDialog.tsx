import {
  AlertOctagon,
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  GitCommitHorizontal,
  History,
  Info,
  Loader2,
  Lock,
  Play,
  RotateCcw,
  ScrollText,
  ShieldCheck,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  type ChangeType,
  commands,
  type OfferChange,
  type TimelineEntry,
  type TimelineView,
} from "@/lib/bindings";
import { formatDateTime } from "@/lib/format";
import { reportError } from "@/lib/report";
import { playLumiSound } from "@/lib/sound";
import { isActive, useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { EmptyState } from "./ui/EmptyState";
import { HoverHint } from "./ui/HoverCard";
import { Segmented } from "./ui/Segmented";
import { Tag } from "./ui/Tag";

type TimelineFilter = "all" | "snapshots" | "launches";

const CHANGE_TONE: Record<ChangeType, "success" | "danger" | "accent" | "warning"> = {
  added: "success",
  removed: "danger",
  updated: "accent",
  enabled: "success",
  disabled: "warning",
};

function durationMinutes(startedAt: number | null, endedAt: number | null): number | null {
  if (startedAt === null || endedAt === null || endedAt <= startedAt) return null;
  return Math.round((endedAt - startedAt) / 60_000);
}

function ChangeRows({ changes }: { changes: OfferChange[] }) {
  const { t } = useTranslation();
  return (
    <ul className="divide-y divide-line border-line border-t bg-surface-2/60">
      {changes.map((item) => (
        <li
          key={`${item.kind}-${item.change}-${item.title}-${item.toVersion ?? ""}`}
          className="flex items-center gap-2.5 px-3.5 py-2 text-xs"
        >
          <Tag tone={CHANGE_TONE[item.change]}>{t(`safety.change.${item.change}`)}</Tag>
          <span className="min-w-0 flex-1 truncate text-fg">
            {item.title}
            {item.kind !== "mod" && (
              <span className="text-fg-muted"> · {t(`kind.${item.kind}`)}</span>
            )}
          </span>
          {(item.fromVersion || item.toVersion) && (
            <span className="shrink-0 font-mono text-2xs text-fg-muted tabular-nums">
              {item.fromVersion && item.toVersion
                ? `${item.fromVersion} → ${item.toVersion}`
                : (item.toVersion ?? item.fromVersion)}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

/**
 * 「永不玩坏」可视化时间线（V1.1-B1）：
 * 将改动快照与历史启动/崩溃记录按时间轴融合展示，突出「上次已验证可玩（绿）」与「改动后崩溃（红）」，
 * 支持展开查看每步详细 diff，并可一键恢复到任意历史节点（恢复前自动保留当前态，随时可前滚回来）。
 */
export function HistoryDialog({
  instanceId,
  open,
  onClose,
}: {
  instanceId: string;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const bumpContent = useGameStore((s) => s.bumpContent);
  const launch = useGameStore((s) => s.launch);
  const running = useGameStore((s) => isActive(s.sessions[instanceId]));

  const [view, setView] = useState<TimelineView | null>(null);
  const [filter, setFilter] = useState<TimelineFilter>("all");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const next = unwrap(await commands.contentTimeline(instanceId));
      setView(next);
      // 默认展开最新 2 个有改动的快照，以及所有「改动后崩溃」节点，方便一眼定位问题
      const autoOpen = new Set<string>();
      let shownRecent = 0;
      for (const entry of next.entries) {
        if (entry.kind !== "snapshot" || entry.changes.length === 0) continue;
        if (entry.crashedAfter || shownRecent < 2) {
          autoOpen.add(entry.id);
          shownRecent += 1;
        }
      }
      setExpanded(autoOpen);
    } catch (error) {
      reportError(error, "读取时间线");
      setView({
        entries: [],
        latestVerifiedId: null,
        hasUnsavedChanges: false,
        changedSinceVerified: false,
      });
    }
  }, [instanceId]);

  useEffect(() => {
    if (!open) return;
    setView(null);
    setFilter("all");
    void load();
  }, [open, load]);

  const restoreNode = async (nodeId: string, summary: string) => {
    setBusyId(nodeId);
    try {
      const count = unwrap(await commands.contentRestoreTimelineNode(instanceId, nodeId));
      bumpContent();
      if (count > 0) {
        playLumiSound("rollback");
        toast.success(t("history.restoredToast", { summary, n: count }), {
          action: { label: t("safety.play"), onClick: () => void launch(instanceId) },
        });
      } else {
        toast.info(t("history.alreadySameToast"));
      }
      await load();
    } catch (error) {
      reportError(error, "恢复时间线节点");
    } finally {
      setBusyId(null);
    }
  };

  const filteredEntries = useMemo(() => {
    const entries = view?.entries ?? [];
    if (filter === "snapshots") return entries.filter((e) => e.kind === "snapshot");
    if (filter === "launches") return entries.filter((e) => e.kind === "launch");
    return entries;
  }, [view, filter]);

  const latestVerifiedSummary = useMemo(() => {
    if (!view?.latestVerifiedId) return t("history.badge.latestVerified");
    const found = view.entries.find(
      (e): e is Extract<TimelineEntry, { kind: "snapshot" }> =>
        e.kind === "snapshot" && e.id === view.latestVerifiedId,
    );
    return found?.summary ?? t("history.badge.latestVerified");
  }, [view, t]);

  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const openLog = (recordId: string) => {
    onClose();
    navigate(
      `/instances/${encodeURIComponent(instanceId)}?tab=log&record=${encodeURIComponent(recordId)}`,
    );
  };

  return (
    <Dialog open={open} onClose={onClose} className="max-w-2xl" title={t("history.title")}>
      <div className="flex flex-col gap-4">
        <p className="text-fg-muted text-sm">{t("history.intro")}</p>

        {view === null ? (
          <div className="flex items-center justify-center gap-2.5 py-10 text-fg-muted text-sm">
            <Loader2 className="size-4 animate-spin text-accent" aria-hidden />
            <span>{t("history.loading")}</span>
          </div>
        ) : view.entries.length === 0 ? (
          <EmptyState icon={History} title={t("history.empty")} hint={t("history.emptyHint")} />
        ) : (
          <>
            {/* 顶部快捷回退横幅：有已验证可玩节点且当前已偏离时显示 */}
            {view.changedSinceVerified && view.latestVerifiedId && (
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-panel border border-line bg-success-soft px-4 py-3">
                <div className="flex min-w-0 flex-1 items-start gap-2.5">
                  <ShieldCheck className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
                  <div className="min-w-0">
                    <p className="font-medium text-fg text-sm">
                      {t("history.banner.unverifiedTitle")}
                    </p>
                    <p className="text-fg-muted text-xs">{t("history.banner.unverifiedHint")}</p>
                  </div>
                </div>
                <Button
                  variant="primary"
                  size="sm"
                  disabled={busyId !== null || running}
                  onClick={() => {
                    if (view.latestVerifiedId) {
                      void restoreNode(view.latestVerifiedId, latestVerifiedSummary);
                    }
                  }}
                >
                  <RotateCcw className="size-3.5" aria-hidden />
                  {t("history.banner.restoreVerified")}
                </Button>
              </div>
            )}

            {running && (
              <div className="flex items-center gap-2 rounded-panel border border-line bg-surface-2 px-3.5 py-2 text-fg-muted text-xs">
                <Lock className="size-3.5 shrink-0" aria-hidden />
                <span>{t("history.runningLocked")}</span>
              </div>
            )}

            {view.hasUnsavedChanges && (
              <div className="flex items-start gap-2 rounded-panel border border-line bg-surface-2 px-3.5 py-2 text-fg-muted text-xs">
                <Info className="mt-0.5 size-3.5 shrink-0 text-accent" aria-hidden />
                <span>{t("history.banner.unsavedHint")}</span>
              </div>
            )}

            <Segmented
              value={filter}
              onChange={setFilter}
              label={t("history.filter.label")}
              fullWidth
              options={[
                { value: "all", label: t("history.filter.all") },
                { value: "snapshots", label: t("history.filter.snapshots") },
                { value: "launches", label: t("history.filter.launches") },
              ]}
            />

            {filteredEntries.length === 0 ? (
              <p className="py-8 text-center text-fg-muted text-sm">{t("history.emptyFiltered")}</p>
            ) : (
              <ol className="relative flex select-text flex-col gap-3 pl-7">
                {/* 纵向时间轴轨道 */}
                <span
                  aria-hidden
                  className="pointer-events-none absolute top-3 bottom-3 left-2.5 w-px bg-line"
                />

                {filteredEntries.map((entry) => {
                  if (entry.kind === "snapshot") {
                    const isOpen = expanded.has(entry.id);
                    const isBusy = busyId === entry.id;
                    const dotClass = entry.verified
                      ? "bg-success-soft text-success ring-1 ring-line"
                      : entry.crashedAfter
                        ? "bg-danger-soft text-danger ring-1 ring-line"
                        : entry.isCurrent
                          ? "bg-accent-soft text-accent ring-1 ring-line"
                          : "bg-surface-2 text-fg-muted ring-1 ring-line";
                    const DotIcon = entry.verified
                      ? CheckCircle2
                      : entry.crashedAfter
                        ? AlertOctagon
                        : GitCommitHorizontal;

                    return (
                      <li key={`snap:${entry.id}`} className="relative">
                        <span
                          aria-hidden
                          className={`absolute top-3.5 -left-7 flex size-5 items-center justify-center rounded-icon ${dotClass}`}
                        >
                          <DotIcon className="size-3.5" />
                        </span>

                        <div className="overflow-hidden rounded-panel border border-line bg-surface">
                          <div className="flex items-start justify-between gap-3 px-3.5 py-3">
                            <div className="min-w-0 flex-1">
                              <div className="flex flex-wrap items-center gap-1.5">
                                <span className="font-medium text-fg text-sm">{entry.summary}</span>
                                {entry.isLatestVerified && (
                                  <Tag tone="success">{t("history.badge.latestVerified")}</Tag>
                                )}
                                {entry.verified && !entry.isLatestVerified && (
                                  <Tag tone="success">{t("history.badge.verified")}</Tag>
                                )}
                                {entry.crashedAfter && (
                                  <Tag tone="danger">{t("history.badge.crashedAfter")}</Tag>
                                )}
                                {entry.isCurrent && (
                                  <Tag tone="accent">{t("history.badge.current")}</Tag>
                                )}
                                {entry.trigger === "beforeRestore" && (
                                  <Tag tone="warning">{t("history.badge.beforeRestore")}</Tag>
                                )}
                                {entry.trigger === "initial" && (
                                  <Tag tone="neutral">{t("history.badge.initial")}</Tag>
                                )}
                                {entry.trigger === "restored" && !entry.isCurrent && (
                                  <Tag tone="neutral">{t("history.badge.restored")}</Tag>
                                )}
                                {entry.versionMismatch && entry.gameVersion && (
                                  <HoverHint
                                    hint={t("history.versionMismatchHint", {
                                      version: entry.gameVersion,
                                    })}
                                  >
                                    <Tag tone="warning">MC {entry.gameVersion}</Tag>
                                  </HoverHint>
                                )}
                              </div>

                              <div className="mt-1 flex flex-wrap items-center gap-2 text-fg-muted text-xs tabular-nums">
                                <span>{formatDateTime(entry.at ?? 0)}</span>
                                <span>·</span>
                                <span>{t("history.fileCount", { n: entry.fileCount })}</span>
                                {entry.changes.length === 0 && entry.trigger === "initial" && (
                                  <>
                                    <span>·</span>
                                    <span>{t("history.diffBaseline")}</span>
                                  </>
                                )}
                              </div>
                            </div>

                            <div className="shrink-0">
                              {entry.isCurrent ? (
                                <span className="inline-flex h-8 items-center px-2 text-fg-muted text-xs">
                                  {t("history.alreadyCurrent")}
                                </span>
                              ) : entry.sameAsCurrent ? (
                                <span className="inline-flex h-8 items-center px-2 text-fg-muted text-xs">
                                  {t("history.sameAsCurrent")}
                                </span>
                              ) : !entry.restorable ? (
                                <HoverHint hint={t("history.missingBackupHint")}>
                                  <Button variant="secondary" size="sm" disabled>
                                    {t("history.missingBackup")}
                                  </Button>
                                </HoverHint>
                              ) : (
                                <Button
                                  variant="secondary"
                                  size="sm"
                                  disabled={busyId !== null || running}
                                  onClick={() => void restoreNode(entry.id, entry.summary)}
                                >
                                  {isBusy ? (
                                    <Loader2 className="size-3.5 animate-spin" aria-hidden />
                                  ) : (
                                    <RotateCcw className="size-3.5" aria-hidden />
                                  )}
                                  {isBusy ? t("history.restoring") : t("history.restoreNode")}
                                </Button>
                              )}
                            </div>
                          </div>

                          {entry.changes.length > 0 && (
                            <>
                              <button
                                type="button"
                                onClick={() => toggleExpand(entry.id)}
                                className="flex w-full items-center gap-1.5 border-line border-t bg-surface-2/40 px-3.5 py-1.5 text-left text-fg-muted text-xs transition-colors hover:bg-surface-hover hover:text-fg"
                              >
                                {isOpen ? (
                                  <ChevronDown className="size-3.5 shrink-0" aria-hidden />
                                ) : (
                                  <ChevronRight className="size-3.5 shrink-0" aria-hidden />
                                )}
                                <span>{t("history.diffToggle", { n: entry.changes.length })}</span>
                              </button>
                              {isOpen && <ChangeRows changes={entry.changes} />}
                            </>
                          )}
                        </div>
                      </li>
                    );
                  }

                  // 启动记录节点
                  const isCrash = entry.outcome === "crashed" || entry.outcome === "failed";
                  const mins = durationMinutes(entry.at, entry.endedAt);
                  const durationText =
                    mins === null
                      ? null
                      : mins < 1
                        ? t("history.launch.durationShort")
                        : t("history.launch.durationMin", { n: mins });
                  const dotClass = entry.verifiedPlay
                    ? "bg-success-soft text-success ring-1 ring-line"
                    : isCrash
                      ? "bg-danger-soft text-danger ring-1 ring-line"
                      : entry.outcome === "running"
                        ? "bg-accent-soft text-accent ring-1 ring-line"
                        : "bg-surface-2 text-fg-muted ring-1 ring-line";
                  const DotIcon = entry.verifiedPlay
                    ? CheckCircle2
                    : isCrash
                      ? AlertTriangle
                      : Play;

                  return (
                    <li key={`launch:${entry.recordId}`} className="relative">
                      <span
                        aria-hidden
                        className={`absolute top-3 -left-7 flex size-5 items-center justify-center rounded-icon ${dotClass}`}
                      >
                        <DotIcon className="size-3" />
                      </span>

                      <div
                        className={`flex flex-wrap items-center justify-between gap-2.5 rounded-panel border px-3.5 py-2.5 ${
                          entry.crashedAfterChange
                            ? "border-danger/35 bg-danger-soft/50"
                            : entry.verifiedPlay
                              ? "border-line bg-surface-2/70"
                              : "border-line bg-surface-2/45"
                        }`}
                      >
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-1.5">
                            <span className="font-medium text-fg text-xs">
                              {t(`history.launch.${entry.outcome}`)}
                            </span>
                            {entry.verifiedPlay && (
                              <Tag tone="success">{t("history.badge.verified")}</Tag>
                            )}
                            {entry.crashedAfterChange && (
                              <Tag tone="danger">{t("history.badge.crashedAfter")}</Tag>
                            )}
                            {entry.exitCode !== null && isCrash && (
                              <span className="font-mono text-2xs text-danger tabular-nums">
                                {t("history.launch.exitCode", { code: entry.exitCode })}
                              </span>
                            )}
                          </div>
                          <div className="mt-0.5 flex flex-wrap items-center gap-2 text-2xs text-fg-muted tabular-nums">
                            <span>{formatDateTime(entry.at ?? 0)}</span>
                            <span>·</span>
                            <span>
                              {entry.gameVersion}
                              {entry.loader ? ` · ${entry.loader}` : ""}
                            </span>
                            <span>·</span>
                            <span>{t("history.modCount", { n: entry.modCount })}</span>
                            {durationText && (
                              <>
                                <span>·</span>
                                <span>{durationText}</span>
                              </>
                            )}
                            {entry.reachedWorld && !entry.verifiedPlay && (
                              <>
                                <span>·</span>
                                <span>{t("history.launch.reachedWorld")}</span>
                              </>
                            )}
                          </div>
                          {entry.error && (
                            <p className="mt-1 truncate text-2xs text-danger" title={entry.error}>
                              {entry.error}
                            </p>
                          )}
                        </div>

                        <Button variant="ghost" size="sm" onClick={() => openLog(entry.recordId)}>
                          <ScrollText className="size-3.5" aria-hidden />
                          {t("history.viewLog")}
                        </Button>
                      </div>
                    </li>
                  );
                })}
              </ol>
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}

export default HistoryDialog;
