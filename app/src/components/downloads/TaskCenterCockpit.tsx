import {
  Activity,
  ChevronDown,
  CircleAlert,
  CircleCheck,
  CircleSlash,
  FileDown,
  Gauge,
  HardDrive,
  History,
  Pause,
  Play,
  RotateCw,
  ShieldAlert,
  Sparkles,
  Wifi,
  X,
} from "lucide-react";
import { memo, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { unwrap } from "@/lib/api";
import {
  commands,
  type FailureDiagnosis,
  type JobSnapshot,
  type QuickFixAction,
} from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { jobProgressLabel, jobTitle } from "@/lib/jobLabels";
import {
  canRetry,
  instanceOf,
  jobAmount,
  jobEta,
  jobFraction,
  jobSpeed,
  playableAfter,
  stateTone,
} from "@/lib/jobs";
import { logError, reportError } from "@/lib/report";
import { type SettingsSection, useAppStore } from "@/stores/app";
import { isComplete, settledCount, useClaimStore } from "@/stores/claims";
import { seedDownloadMetrics, useDownloadMetricsStore } from "@/stores/downloadMetrics";
import { isActive, useGameStore } from "@/stores/game";
import { DownloadRestArt } from "../art/DownloadRestArt";
import { Button } from "../ui/Button";
import { IconButton } from "../ui/IconButton";
import { ProgressBar } from "../ui/ProgressBar";
import { JobIcon } from "./JobIcon";

const W = 384;
const H = 56;
const PAD_Y = 6;

/** 纯 SVG 60 点实时速率折线图（固定仅 4 个 SVG 节点，React.memo 隔离 60fps 主线程零抖动）。 */
const SpeedSparkline = memo(function SpeedSparkline({
  samples,
  limitBps,
}: {
  samples: number[];
  limitBps: number;
}) {
  const maxSample = Math.max(0, ...samples);
  const ceiling = Math.max(maxSample * 1.15, limitBps > 0 ? limitBps * 1.1 : 0, 512 * 1024);
  const innerH = H - PAD_Y * 2;
  const n = Math.max(2, samples.length);
  const dx = W / (n - 1);

  const coords = samples.map((val, idx) => {
    const x = +(idx * dx).toFixed(1);
    const ratio = Math.min(1, Math.max(0, val / ceiling));
    const y = +(H - PAD_Y - ratio * innerH).toFixed(1);
    return `${x},${y}`;
  });
  const linePath = `M${coords.join(" L")}`;
  const areaPath = `${linePath} L${W},${H - PAD_Y} L0,${H - PAD_Y} Z`;
  const limitY =
    limitBps > 0 ? +(H - PAD_Y - Math.min(1, limitBps / ceiling) * innerH).toFixed(1) : null;

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      className="h-14 w-full overflow-hidden rounded-control bg-surface-2/80"
      aria-hidden
    >
      <defs>
        <linearGradient id="lumi-speed-grad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--color-accent)" stopOpacity="0.28" />
          <stop offset="100%" stopColor="var(--color-accent)" stopOpacity="0.02" />
        </linearGradient>
      </defs>
      <path d={areaPath} fill="url(#lumi-speed-grad)" />
      <path
        d={linePath}
        fill="none"
        stroke="var(--color-accent)"
        strokeWidth="1.75"
        strokeLinejoin="round"
        strokeLinecap="round"
      />
      {limitY !== null && (
        <line
          x1="0"
          y1={limitY}
          x2={W}
          y2={limitY}
          stroke="var(--color-warning)"
          strokeWidth="1"
          strokeDasharray="4 3"
        />
      )}
    </svg>
  );
});

function fixActionLabel(action: QuickFixAction, t: (k: string) => string): string {
  return t(`tasks.quickFix.${action}`);
}

function DiagnosisCard({
  diagnosis,
  failedJobId,
  onApplied,
}: {
  diagnosis: FailureDiagnosis;
  failedJobId: string | null;
  onApplied: () => void;
}) {
  const { t } = useTranslation();
  const openSettings = useAppStore((s) => s.openSettings);
  const [busy, setBusy] = useState(false);

  const runFix = async (action: QuickFixAction) => {
    setBusy(true);
    try {
      const res = unwrap(await commands.netApplyQuickFix(action, failedJobId));
      if (res.message) toast.success(res.message);
      if (res.openSettingsSection) {
        openSettings(res.openSettingsSection as SettingsSection, res.focusSettingId ?? null);
        onApplied();
      }
    } catch (error) {
      reportError(error, "执行自愈修复");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mx-3 mt-2 rounded-panel border border-warning/40 bg-warning/10 p-3 text-xs">
      <div className="flex items-start gap-2">
        <ShieldAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="font-medium text-fg">
            {diagnosis.summary}
            {diagnosis.host && (
              <span className="ml-1.5 font-mono text-2xs text-fg-muted">· {diagnosis.host}</span>
            )}
          </p>
          <p className="mt-0.5 text-fg-muted leading-relaxed">{diagnosis.explanation}</p>
          <div className="mt-2 flex flex-wrap gap-1.5">
            <Button
              variant="secondary"
              size="sm"
              disabled={busy}
              onClick={() => void runFix(diagnosis.primaryFix)}
            >
              {fixActionLabel(diagnosis.primaryFix, t)}
            </Button>
            {diagnosis.secondaryFix && (
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => void runFix(diagnosis.secondaryFix as QuickFixAction)}
              >
                {fixActionLabel(diagnosis.secondaryFix, t)}
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function JobCard({ job }: { job: JobSnapshot }) {
  const { t } = useTranslation();
  const cancel = useGameStore((s) => s.cancelJob);
  const pause = useGameStore((s) => s.pauseJob);
  const resume = useGameStore((s) => s.resumeJob);
  const dismiss = useGameStore((s) => s.dismissJob);
  const launch = useGameStore((s) => s.launch);
  const retry = useGameStore((s) => s.retryJob);
  const retryFailed = useGameStore((s) => s.retryFailedJob);
  const instanceId = instanceOf(job);
  const gameRunning = useGameStore((s) => (instanceId ? isActive(s.sessions[instanceId]) : false));
  const claim = useClaimStore((s) => Object.values(s.claims).find((c) => c.jobId === job.id));
  const showClaim = useClaimStore((s) => s.show);

  const running = job.state === "running";
  const paused = job.state === "paused";
  const active = running || paused;
  const title = jobTitle(job, t);
  const fraction = jobFraction(job.progress);
  const amount = active ? jobAmount(job.progress, t) : null;
  const status = running
    ? (jobProgressLabel(job.progress.message, t) ?? t("tasks.running"))
    : paused
      ? t("tasks.state.paused")
      : job.state === "failed"
        ? (job.error ?? t("tasks.failed"))
        : t(`tasks.state.${job.state}`);
  const pace = running ? [jobSpeed(job, t), jobEta(job, t)].filter(Boolean).join(" · ") : "";
  const retryable = canRetry(job);
  const failedPartialCount = job.state === "failed" ? (job.failedCount ?? 0) : 0;

  return (
    <li className="flex gap-3 px-4 py-3">
      <JobIcon job={job} />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-start gap-2">
          <p className="min-w-0 flex-1 truncate font-medium text-sm">{title}</p>
          {amount && <span className="shrink-0 text-fg-muted text-xs tabular-nums">{amount}</span>}
          {!active && (
            <IconButton
              label={t("tasks.dismiss")}
              size="sm"
              className="-my-1 -mr-1.5"
              onClick={() => dismiss(job.id)}
            >
              <X className="size-3.5" aria-hidden />
            </IconButton>
          )}
        </div>
        {active && fraction !== null && <ProgressBar value={fraction} label={title} />}
        <p className={`flex items-center gap-1.5 text-xs ${stateTone(job.state)}`}>
          {running && <LightSpinner className="size-3.5 shrink-0" />}
          {paused && <Pause className="size-3.5 shrink-0" aria-hidden />}
          {job.state === "succeeded" && <CircleCheck className="size-3.5 shrink-0" aria-hidden />}
          {job.state === "failed" && <CircleAlert className="size-3.5 shrink-0" aria-hidden />}
          {(job.state === "cancelled" || job.state === "interrupted") && (
            <CircleSlash className="size-3.5 shrink-0" aria-hidden />
          )}
          <span className={job.state === "failed" ? "line-clamp-2" : "truncate"}>{status}</span>
        </p>
        {pace && <p className="text-fg-muted text-xs tabular-nums">{pace}</p>}
        {(job.reusedBytes ?? 0) > 0 && (
          <p className="text-accent text-2xs tabular-nums">
            {t("tasks.reusedInline", {
              size: formatSize(job.reusedBytes ?? 0),
              n: job.reusedFiles,
            })}
          </p>
        )}
        {(active ||
          retryable ||
          failedPartialCount > 0 ||
          (job.state === "succeeded" && playableAfter(job.key) && !gameRunning)) && (
          <div className="flex flex-wrap items-center gap-2 pt-0.5">
            {running && claim && !isComplete(claim) && (
              <Button variant="outline" size="sm" onClick={() => showClaim(claim.id)}>
                <FileDown className="size-3.5" aria-hidden />
                {t("tasks.claim", { n: claim.entries.length - settledCount(claim) })}
              </Button>
            )}
            {running && (
              <Button variant="outline" size="sm" onClick={() => void pause(job.id)}>
                <Pause className="size-3.5" aria-hidden />
                {t("tasks.pause")}
              </Button>
            )}
            {paused && (
              <Button variant="outline" size="sm" onClick={() => void resume(job.id)}>
                <Play className="size-3.5 fill-current" aria-hidden />
                {t("tasks.resume")}
              </Button>
            )}
            {active && (
              <Button variant="danger" size="sm" onClick={() => void cancel(job.id)}>
                <X className="size-3.5" aria-hidden />
                {t("tasks.cancel")}
              </Button>
            )}
            {failedPartialCount > 0 ? (
              <Button variant="outline" size="sm" onClick={() => void retryFailed(job.id)}>
                <RotateCw className="size-3.5" aria-hidden />
                {t("tasks.retryFailed", { n: failedPartialCount })}
              </Button>
            ) : (
              retryable && (
                <Button variant="outline" size="sm" onClick={() => void retry(job.id)}>
                  <RotateCw className="size-3.5" aria-hidden />
                  {t("tasks.retry")}
                </Button>
              )
            )}
            {job.state === "succeeded" && playableAfter(job.key) && !gameRunning && instanceId && (
              <Button variant="outline" size="sm" onClick={() => void launch(instanceId)}>
                <Play className="size-3.5 fill-current" aria-hidden />
                {t("tasks.play")}
              </Button>
            )}
          </div>
        )}
      </div>
    </li>
  );
}

/** 懒加载的 420px 下载驾驶舱面板（仅首次点击下载按钮时加载独立 chunk）。 */
export default function TaskCenterCockpit({
  list,
  runningCount,
  pausedCount,
  onClose,
}: {
  list: JobSnapshot[];
  runningCount: number;
  pausedCount: number;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const openHistory = useGameStore((s) => s.setHistoryOpen);
  const pauseJob = useGameStore((s) => s.pauseJob);
  const resumeJob = useGameStore((s) => s.resumeJob);
  const metrics = useDownloadMetricsStore((s) => s.current);
  const samples = useDownloadMetricsStore((s) => s.samples);
  const peakSpeed = useDownloadMetricsStore((s) => s.peakSpeed);
  const settings = useAppStore((s) => s.settings);
  const saveSettings = useAppStore((s) => s.saveSettings);
  const [eventsOpen, setEventsOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    commands
      .netRuntimeStatus()
      .then((snap) => {
        if (alive) seedDownloadMetrics(snap);
      })
      .catch((error: unknown) => logError(error, "读取下载实时状态"));
    return () => {
      alive = false;
    };
  }, []);

  const runningIds = useMemo(
    () => list.filter((j) => j.state === "running").map((j) => j.id),
    [list],
  );
  const pausedIds = useMemo(
    () => list.filter((j) => j.state === "paused").map((j) => j.id),
    [list],
  );

  const { diagnosis, diagnosisJobId } = useMemo(() => {
    const active = metrics?.activeDiagnosis ?? null;
    if (active) {
      const matchingJob = list.find(
        (j) =>
          j.state === "failed" &&
          j.diagnosis?.cause === active.cause &&
          j.diagnosis?.summary === active.summary &&
          (j.failedCount ?? 0) > 0,
      );
      return { diagnosis: active, diagnosisJobId: matchingJob?.id ?? null };
    }
    const fallbackJob = list.find((j) => j.state === "failed" && j.diagnosis);
    return {
      diagnosis: fallbackJob?.diagnosis ?? null,
      diagnosisJobId: fallbackJob && (fallbackJob.failedCount ?? 0) > 0 ? fallbackJob.id : null,
    };
  }, [list, metrics?.activeDiagnosis]);

  const speedBps = metrics?.speedBytesPerSec ?? 0;
  const limitKib = metrics?.effectiveSpeedLimitKib ?? 0;
  const limitBps = limitKib * 1024;
  const recentEvents = metrics?.recentEvents ?? [];
  const latestEvent = recentEvents.length > 0 ? recentEvents[recentEvents.length - 1] : null;
  const showMetricsBlock = runningCount > 0 || pausedCount > 0 || speedBps > 0;

  const enableMeteredThrottle = () => {
    if (!settings) return;
    void saveSettings({
      ...settings,
      download: { ...settings.download, meteredPolicy: "throttle" },
    });
  };

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between gap-2 border-line border-b px-4 py-2.5">
        <div className="flex items-center gap-2">
          <span className="font-medium text-sm">{t("tasks.cockpitTitle")}</span>
          {(runningCount > 0 || pausedCount > 0) && (
            <span className="text-fg-muted text-xs tabular-nums">
              {t("tasks.cockpitCounts", { running: runningCount, paused: pausedCount })}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1.5">
          {runningIds.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                for (const id of runningIds) void pauseJob(id);
              }}
            >
              <Pause className="size-3.5" aria-hidden />
              {t("tasks.pauseAll")}
            </Button>
          )}
          {runningIds.length === 0 && pausedIds.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                for (const id of pausedIds) void resumeJob(id);
              }}
            >
              <Play className="size-3.5 fill-current" aria-hidden />
              {t("tasks.resumeAll")}
            </Button>
          )}
        </div>
      </div>

      {metrics?.metered && settings?.download.meteredPolicy === "warn" && (
        <div className="mx-3 mt-2 flex items-center justify-between gap-2 rounded-panel border border-warning/40 bg-warning/10 px-3 py-2 text-xs">
          <span className="flex min-w-0 items-center gap-1.5 text-fg">
            <Wifi className="size-3.5 shrink-0 text-warning" aria-hidden />
            <span className="truncate">{t("tasks.meteredBanner")}</span>
          </span>
          <Button variant="secondary" size="sm" onClick={enableMeteredThrottle}>
            {t("tasks.meteredThrottleNow")}
          </Button>
        </div>
      )}

      {showMetricsBlock && metrics && (
        <div className="flex flex-col gap-2 border-line border-b px-4 py-2.5">
          <div className="flex items-baseline justify-between gap-2 text-xs">
            <div className="flex items-baseline gap-2 tabular-nums">
              <span className="font-semibold text-sm text-fg">
                {speedBps > 0 ? `${formatSize(speedBps)}/s` : t("tasks.idleRate")}
              </span>
              {peakSpeed > 0 && (
                <span className="text-fg-muted">
                  {t("tasks.peakSpeed", { speed: `${formatSize(peakSpeed)}/s` })}
                </span>
              )}
            </div>
            <span className="text-fg-muted tabular-nums">
              {limitKib > 0
                ? t("tasks.speedLimitActive", { limit: `${formatSize(limitBps)}/s` })
                : t("tasks.speedLimitNone")}
            </span>
          </div>

          <SpeedSparkline samples={samples} limitBps={limitBps} />

          <div className="flex flex-wrap gap-1.5 text-2xs text-fg-muted tabular-nums">
            <span className="inline-flex items-center gap-1 rounded-tag bg-surface-2 px-2 py-0.5">
              <Gauge className="size-3 text-accent" aria-hidden />
              {metrics.routeLabel}
            </span>
            <span className="inline-flex items-center gap-1 rounded-tag bg-surface-2 px-2 py-0.5">
              <Activity className="size-3 text-accent" aria-hidden />
              {t(`tasks.proto.${metrics.protocol}`)} · {metrics.activeConnections}/
              {metrics.maxConnections}
            </span>
            <span className="inline-flex items-center gap-1 rounded-tag bg-surface-2 px-2 py-0.5">
              <HardDrive className="size-3 text-accent" aria-hidden />
              {metrics.diskKind.toUpperCase()} · {metrics.diskWritersActive}/
              {metrics.diskWritersLimit} · {Math.round(metrics.diskWriteEwmaMs ?? 0)}ms
            </span>
          </div>

          {(metrics.reusedBytes ?? 0) > 0 && (
            <div className="flex items-center gap-1.5 rounded-panel bg-accent-soft px-2.5 py-1 text-2xs text-accent tabular-nums">
              <Sparkles className="size-3.5 shrink-0" aria-hidden />
              <span>
                {t("tasks.reusedBanner", {
                  size: formatSize(metrics.reusedBytes ?? 0),
                  n: metrics.reusedFiles,
                })}
              </span>
            </div>
          )}

          {latestEvent && (
            <div className="text-2xs text-fg-muted">
              <button
                type="button"
                onClick={() => setEventsOpen((v) => !v)}
                className="flex w-full items-center justify-between gap-2 rounded-tag py-0.5 text-left hover:text-fg"
              >
                <span className="truncate">{latestEvent.message}</span>
                <ChevronDown
                  className={`size-3 shrink-0 transition-transform ${eventsOpen ? "rotate-180" : ""}`}
                  aria-hidden
                />
              </button>
              {eventsOpen && (
                <ul className="mt-1 max-h-28 space-y-1 overflow-y-auto border-line border-t pt-1.5">
                  {[...recentEvents].reverse().map((ev, i) => (
                    <li
                      key={`${ev.atMs}-${i}`}
                      className="flex items-center justify-between gap-2 leading-snug"
                    >
                      <span className="min-w-0 flex-1 truncate">{ev.message}</span>
                      <span className="shrink-0 tabular-nums">
                        {new Date(ev.atMs ?? 0).toLocaleTimeString()}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      )}

      {diagnosis && (
        <DiagnosisCard diagnosis={diagnosis} failedJobId={diagnosisJobId} onApplied={onClose} />
      )}

      {list.length === 0 ? (
        <div className="flex flex-col items-center gap-1 px-4 pt-4 pb-5 text-center">
          <DownloadRestArt />
          <p className="font-semibold text-sm">{t("tasks.empty")}</p>
          <p className="text-fg-muted text-xs">{t("tasks.emptyHint")}</p>
        </div>
      ) : (
        <ul className="max-h-[24rem] divide-y divide-line overflow-y-auto">
          {list.map((job) => (
            <JobCard key={job.id} job={job} />
          ))}
        </ul>
      )}

      <div className="flex justify-end border-line border-t px-4 py-2.5">
        <Button variant="secondary" size="sm" onClick={() => openHistory(true)}>
          <History className="size-3.5" aria-hidden />
          {t("tasks.history")}
        </Button>
      </div>
    </div>
  );
}
