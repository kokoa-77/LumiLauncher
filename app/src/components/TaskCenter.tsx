import {
  CircleAlert,
  CircleCheck,
  CircleSlash,
  Download,
  FileDown,
  History,
  Play,
  RotateCw,
  X,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LightSpinner } from "@/components/ui/LightSpinner";
import type { JobSnapshot } from "@/lib/bindings";
import { jobProgressLabel, jobTitle } from "@/lib/jobLabels";
import {
  canRetry,
  instanceOf,
  jobAmount,
  jobEta,
  jobFraction,
  jobSpeed,
  overallFraction,
  playableAfter,
  recentDownloadJobs,
  stateTone,
} from "@/lib/jobs";
import { duration, ease } from "@/lib/motion";
import { isComplete, settledCount, useClaimStore } from "@/stores/claims";
import { isActive, useGameStore } from "@/stores/game";
import { DownloadRestArt } from "./art/DownloadRestArt";
import { JobIcon } from "./downloads/JobIcon";
import { Button } from "./ui/Button";
import { IconButton } from "./ui/IconButton";
import { ProgressBar } from "./ui/ProgressBar";
import { Tooltip } from "./ui/Tooltip";

/** 只显示最近的这么多条任务 */
const MAX_SHOWN = 12;
/** 自动打开的任务中心，任务结束后再开这么久就收起（毫秒）；鼠标在面板上时不收 */
const AUTO_CLOSE_MS = 5000;

/**
 * 一个任务一张卡：左边是模组图标或实例的小封面（都没有时按来源画图标），
 * 右边名字 + 数量（「3 / 5 个」「120 MB / 380 MB」）、进度条、现在在做什么、速度和还剩多久（D18）；
 * 进行中可以取消（红色，危险操作），失败或取消的能「重试」，做完了往实例里装东西的能直接「开始游戏」，
 * 结束的能用 × 收掉（下载历史里还在）。
 */
function JobCard({ job }: { job: JobSnapshot }) {
  const { t } = useTranslation();
  const cancel = useGameStore((s) => s.cancelJob);
  const dismiss = useGameStore((s) => s.dismissJob);
  const launch = useGameStore((s) => s.launch);
  const retry = useGameStore((s) => s.retryJob);
  const instanceId = instanceOf(job);
  const gameRunning = useGameStore((s) => (instanceId ? isActive(s.sessions[instanceId]) : false));
  const claim = useClaimStore((s) => Object.values(s.claims).find((c) => c.jobId === job.id));
  const showClaim = useClaimStore((s) => s.show);
  const running = job.state === "running";
  const title = jobTitle(job, t);
  const fraction = jobFraction(job.progress);
  const amount = running ? jobAmount(job.progress, t) : null;
  const status = running
    ? (jobProgressLabel(job.progress.message, t) ?? t("tasks.running"))
    : job.state === "failed"
      ? (job.error ?? t("tasks.failed"))
      : t(`tasks.state.${job.state}`);
  const pace = running ? [jobSpeed(job, t), jobEta(job, t)].filter(Boolean).join(" · ") : "";
  const retryable = canRetry(job);

  return (
    <li className="flex gap-3 px-4 py-3">
      <JobIcon job={job} />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-start gap-2">
          <p className="min-w-0 flex-1 truncate font-medium text-sm">{title}</p>
          {amount && <span className="shrink-0 text-fg-muted text-xs tabular-nums">{amount}</span>}
          {!running && (
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
        {running && fraction !== null && <ProgressBar value={fraction} label={title} />}
        <p className={`flex items-center gap-1.5 text-xs ${stateTone(job.state)}`}>
          {running && <LightSpinner className="size-3.5 shrink-0" />}
          {job.state === "succeeded" && <CircleCheck className="size-3.5 shrink-0" aria-hidden />}
          {job.state === "failed" && <CircleAlert className="size-3.5 shrink-0" aria-hidden />}
          {(job.state === "cancelled" || job.state === "interrupted") && (
            <CircleSlash className="size-3.5 shrink-0" aria-hidden />
          )}
          <span className={job.state === "failed" ? "line-clamp-2" : "truncate"}>{status}</span>
        </p>
        {pace && <p className="text-fg-muted text-xs tabular-nums">{pace}</p>}
        {(running ||
          retryable ||
          (job.state === "succeeded" && playableAfter(job.key) && !gameRunning)) && (
          <div className="flex flex-wrap items-center gap-2 pt-0.5">
            {running && claim && !isComplete(claim) && (
              <Button variant="outline" size="sm" onClick={() => showClaim(claim.id)}>
                <FileDown className="size-3.5" aria-hidden />
                {t("tasks.claim", { n: claim.entries.length - settledCount(claim) })}
              </Button>
            )}
            {running && (
              <Button variant="danger" size="sm" onClick={() => void cancel(job.id)}>
                <X className="size-3.5" aria-hidden />
                {t("tasks.cancel")}
              </Button>
            )}
            {retryable && (
              <Button variant="outline" size="sm" onClick={() => void retry(job.id)}>
                <RotateCw className="size-3.5" aria-hidden />
                {t("tasks.retry")}
              </Button>
            )}
            {job.state === "succeeded" && playableAfter(job.key) && !gameRunning && instanceId && (
              <Button variant="outline" size="sm" onClick={() => void launch(instanceId)}>
                <Play className="size-3 fill-current" aria-hidden />
                {t("tasks.play")}
              </Button>
            )}
          </div>
        )}
      </div>
    </li>
  );
}

/**
 * 下载（D18 以前叫任务中心）：标题栏上的按钮 + 下拉面板。任何页面都能看进度、取消、重试；
 * 面板底下「查看下载历史」打开下载历史弹窗。
 * - 空闲时只显示图标；有任务在跑时，胶囊显示任务名、个数和进度（只动 scaleX）。
 * - 拖进来的文件、拖进来的整合包开始装时自动打开（`showTasksFor`），那个任务结束 5 秒后自动收起，
 *   鼠标停在面板上时不收；自己点开的不自动收
 */
export function TaskCenter() {
  const { t } = useTranslation();
  const jobs = useGameStore((s) => s.jobs);
  const open = useGameStore((s) => s.tasksOpen);
  const setOpen = useGameStore((s) => s.setTasksOpen);
  const autoJob = useGameStore((s) => s.tasksAutoJob);
  const openHistory = useGameStore((s) => s.setHistoryOpen);
  const autoState = autoJob ? jobs[autoJob]?.state : undefined;
  const [hovered, setHovered] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const { running: runningJobs, visible: list } = useMemo(
    () => recentDownloadJobs(Object.values(jobs), MAX_SHOWN),
    [jobs],
  );
  const running = runningJobs.length;
  const lead = runningJobs[0];
  const leadTitle = lead ? jobTitle(lead, t) : null;
  const overall = overallFraction(runningJobs);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (!panelRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && panelRef.current?.contains(document.activeElement)) {
        setOpen(false);
        triggerRef.current?.focus();
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, setOpen]);

  useEffect(() => {
    if (!open || hovered || !autoState || autoState === "running") return;
    const timer = window.setTimeout(() => setOpen(false), AUTO_CLOSE_MS);
    return () => window.clearTimeout(timer);
  }, [open, hovered, autoState, setOpen]);

  return (
    <div
      ref={panelRef}
      className="relative flex items-center"
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
    >
      <Tooltip label={t("tasks.title")} side="bottom">
        <motion.button
          ref={triggerRef}
          type="button"
          layout="size"
          layoutDependency={leadTitle === null ? null : `${leadTitle}:${running}`}
          transition={{ duration: duration.toggle, ease: ease.out }}
          aria-label={
            leadTitle === null
              ? t("tasks.title")
              : t("tasks.runningLabel", { title: leadTitle, n: running })
          }
          aria-expanded={open}
          aria-controls={open ? panelId : undefined}
          onClick={() => setOpen(!open)}
          className={`relative h-8 overflow-hidden rounded-icon transition-colors duration-(--motion-press) ${
            lead
              ? "flex max-w-60 min-w-0 items-center gap-1.5 border border-accent/55 bg-surface px-2.5 font-medium text-fg text-xs"
              : `grid size-8 place-items-center hover:bg-surface-hover hover:text-fg ${open ? "bg-surface-hover text-fg" : "text-fg-muted"}`
          }`}
        >
          {lead && overall !== null && (
            <span
              aria-hidden
              className="lumi-install-fill absolute inset-0 origin-left bg-accent-soft"
              style={{ transform: `scaleX(${overall})` }}
            />
          )}
          <Download
            className={`relative size-4 shrink-0 ${lead ? "text-accent" : ""}`}
            aria-hidden
          />
          <AnimatePresence initial={false} mode="popLayout">
            {lead && (
              <motion.span
                key="running"
                className="relative min-w-0 truncate"
                initial={{ opacity: 0, y: 4 }}
                animate={{
                  opacity: 1,
                  y: 0,
                  transition: { duration: duration.toggle, ease: ease.out },
                }}
                exit={{ opacity: 0, transition: { duration: duration.press } }}
              >
                {leadTitle}
              </motion.span>
            )}
          </AnimatePresence>
          {running > 0 && (
            <span className="relative grid min-w-4 shrink-0 place-items-center rounded-icon bg-accent px-1 font-medium text-2xs text-on-accent tabular-nums">
              {running}
            </span>
          )}
        </motion.button>
      </Tooltip>
      <span role="status" className="sr-only">
        {running > 0 ? t("tasks.runningCount", { n: running }) : ""}
      </span>
      <AnimatePresence>
        {open && (
          <motion.div
            id={panelId}
            className="absolute top-full right-0 z-40 mt-2 w-96 origin-top-right overflow-hidden rounded-panel border border-line bg-overlay shadow-overlay"
            initial={{ opacity: 0, scale: 0.98, y: -5 }}
            animate={{
              opacity: 1,
              scale: 1,
              y: 0,
              transition: { duration: duration.toggle, ease: ease.out },
            }}
            exit={{
              opacity: 0,
              scale: 0.98,
              transition: { duration: duration.press, ease: ease.in },
            }}
          >
            <p className="flex items-center gap-2 border-line border-b px-4 py-2.5 font-medium text-sm">
              {t("tasks.title")}
              {running > 0 && (
                <span className="font-normal text-fg-muted text-xs">
                  {t("tasks.runningCount", { n: running })}
                </span>
              )}
            </p>
            {list.length === 0 ? (
              <div className="flex flex-col items-center gap-1 px-4 pt-4 pb-5 text-center">
                <DownloadRestArt />
                <p className="font-semibold text-sm">{t("tasks.empty")}</p>
                <p className="text-fg-muted text-xs">{t("tasks.emptyHint")}</p>
              </div>
            ) : (
              <ul className="max-h-[28rem] divide-y divide-line overflow-y-auto">
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
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
