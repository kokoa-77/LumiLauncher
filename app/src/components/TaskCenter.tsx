import { Download, Pause } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { lazy, Suspense, useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { formatSize } from "@/lib/format";
import { jobTitle } from "@/lib/jobLabels";
import { overallFraction, recentDownloadJobs } from "@/lib/jobs";
import { duration, ease } from "@/lib/motion";
import { useDownloadMetricsStore } from "@/stores/downloadMetrics";
import { useGameStore } from "@/stores/game";
import { Tooltip } from "./ui/Tooltip";

const TaskCenterCockpit = lazy(() => import("./downloads/TaskCenterCockpit"));

/** 只显示最近的这么多条任务 */
const MAX_SHOWN = 12;
/** 自动打开的任务中心，任务结束后再开这么久就收起（毫秒）；鼠标在面板上时不收 */
const AUTO_CLOSE_MS = 5000;

/**
 * 标题栏极轻下载触发器（常驻首屏，零图表与重图标开销）；
 * 展开时通过 `React.lazy` 按需加载 420px 下载驾驶舱。
 */
export function TaskCenter() {
  const { t } = useTranslation();
  const jobs = useGameStore((s) => s.jobs);
  const open = useGameStore((s) => s.tasksOpen);
  const setOpen = useGameStore((s) => s.setTasksOpen);
  const autoJob = useGameStore((s) => s.tasksAutoJob);
  const speedBps = useDownloadMetricsStore((s) => s.current?.speedBytesPerSec ?? 0);
  const autoState = autoJob ? jobs[autoJob]?.state : undefined;
  const [hovered, setHovered] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();

  const { running: activeJobs, visible: list } = useMemo(
    () => recentDownloadJobs(Object.values(jobs), MAX_SHOWN),
    [jobs],
  );
  const runningJobs = useMemo(() => activeJobs.filter((j) => j.state === "running"), [activeJobs]);
  const pausedJobs = useMemo(() => activeJobs.filter((j) => j.state === "paused"), [activeJobs]);
  const hasFailed = useMemo(() => list.some((j) => j.state === "failed"), [list]);
  const runningCount = runningJobs.length;
  const pausedCount = pausedJobs.length;
  const lead = runningJobs[0] ?? pausedJobs[0];
  const leadTitle = lead ? jobTitle(lead, t) : null;
  const overall = overallFraction(activeJobs);

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
    if (!open || hovered || !autoState || autoState === "running" || autoState === "paused") {
      return;
    }
    const timer = window.setTimeout(() => setOpen(false), AUTO_CLOSE_MS);
    return () => window.clearTimeout(timer);
  }, [open, hovered, autoState, setOpen]);

  const activeTotal = runningCount + pausedCount;

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
          layoutDependency={leadTitle === null ? null : `${leadTitle}:${activeTotal}`}
          transition={{ duration: duration.toggle, ease: ease.out }}
          aria-label={
            leadTitle === null
              ? t("tasks.title")
              : t("tasks.runningLabel", { title: leadTitle, n: activeTotal })
          }
          aria-expanded={open}
          aria-controls={open ? panelId : undefined}
          onClick={() => setOpen(!open)}
          className={`relative h-8 overflow-hidden rounded-icon transition-colors duration-(--motion-press) ${
            lead
              ? "flex max-w-72 min-w-0 items-center gap-1.5 border border-accent/55 bg-surface px-2.5 font-medium text-fg text-xs"
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
          {lead?.state === "paused" ? (
            <Pause className="relative size-3.5 shrink-0 text-warning" aria-hidden />
          ) : (
            <Download
              className={`relative size-4 shrink-0 ${lead ? "text-accent" : ""}`}
              aria-hidden
            />
          )}
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
                {runningCount === 0 && pausedCount > 0
                  ? t("tasks.pausedPill", { n: pausedCount })
                  : leadTitle}
              </motion.span>
            )}
          </AnimatePresence>
          {runningCount > 0 && speedBps > 0 && (
            <span className="relative shrink-0 text-fg-muted text-2xs tabular-nums">
              {formatSize(speedBps)}/s
            </span>
          )}
          {activeTotal > 0 ? (
            <span className="relative grid min-w-4 shrink-0 place-items-center rounded-icon bg-accent px-1 font-medium text-2xs text-on-accent tabular-nums">
              {activeTotal}
            </span>
          ) : (
            hasFailed && (
              <span
                aria-hidden
                className="absolute top-1.5 right-1.5 size-1.5 rounded-full bg-danger"
              />
            )
          )}
        </motion.button>
      </Tooltip>
      <span role="status" className="sr-only">
        {runningCount > 0 ? t("tasks.runningCount", { n: runningCount }) : ""}
      </span>
      <AnimatePresence>
        {open && (
          <motion.div
            id={panelId}
            className="absolute top-full right-0 z-40 mt-2 w-[26.25rem] origin-top-right overflow-hidden rounded-panel border border-line bg-overlay shadow-overlay"
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
            <Suspense
              fallback={
                <div className="px-4 py-8 text-center text-fg-muted text-xs">
                  {t("tasks.loadingCockpit")}
                </div>
              }
            >
              <TaskCenterCockpit
                list={list}
                runningCount={runningCount}
                pausedCount={pausedCount}
                onClose={() => setOpen(false)}
              />
            </Suspense>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
