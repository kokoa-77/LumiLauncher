import { AlertTriangle, CircleCheck, ScrollText, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { duration, ease, spring } from "@/lib/motion";
import { isActive, useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { IconButton } from "./ui/IconButton";
import { ProgressBar } from "./ui/ProgressBar";

/** 游戏就绪或正常退出后，胶囊停留多久再自动收起（毫秒） */
const AUTO_HIDE_MS = 2500;

/** 启动胶囊：启动时在底部弹出一条，显示进度和状态；进入游戏后自动收起，出问题时留着。 */
export function LaunchCapsule() {
  const { t } = useTranslation();
  const sessions = useGameStore((s) => s.sessions);
  const jobs = useGameStore((s) => s.jobs);
  const instances = useGameStore((s) => s.instances);
  const openLog = useGameStore((s) => s.openLog);
  const navigate = useNavigate();
  const requestStop = useGameStore((s) => s.requestStop);
  // 游戏就绪后自动收起。之后发生崩溃时仍显示这一次的故障入口。
  const [hidden, setHidden] = useState<Record<string, { at: number | null; failed: boolean }>>({});

  const session = Object.values(sessions)
    .filter((s) => {
      const dismissed = hidden[s.instanceId];
      return (
        dismissed?.at !== s.startedAt ||
        (!dismissed.failed && (s.state.kind === "crashed" || !!s.error))
      );
    })
    .sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0))[0];
  const kind = session?.state.kind;
  const failed = !!session && (kind === "crashed" || !!session.error);

  useEffect(() => {
    if (!session) return;
    const settled =
      kind === "mainMenu" || kind === "inWorld" || (kind === "exited" && !session.error);
    if (!settled) return;
    const timer = setTimeout(
      () =>
        setHidden((h) => ({
          ...h,
          [session.instanceId]: { at: session.startedAt, failed: false },
        })),
      AUTO_HIDE_MS,
    );
    return () => clearTimeout(timer);
  }, [session, kind]);

  const instance = session && instances.find((i) => i.id === session.instanceId);
  const name = instance
    ? [instance.name, instance.loader].filter(Boolean).join(" · ")
    : session?.instanceId;
  const prepareJob = session
    ? Object.values(jobs).find(
        (j) => j.key === `launch:${session.instanceId}` && j.state === "running",
      )
    : undefined;
  const progress = prepareJob?.progress;

  let title = "";
  if (session) {
    if (session.error) title = t("capsule.failed", { name });
    else if (kind === "crashed") title = t("capsule.crashed", { name });
    else title = t(`capsule.state.${kind}`, { name });
  }

  return (
    <AnimatePresence>
      {session && (
        <motion.div
          key={`${session.instanceId}-${session.startedAt}`}
          className="-translate-x-1/2 fixed bottom-5 left-1/2 z-30 w-[min(560px,calc(100%-2rem))]"
          initial={{ opacity: 0, y: 24 }}
          animate={{ opacity: 1, y: 0, transition: spring.gentle }}
          exit={{ opacity: 0, y: 16, transition: { duration: duration.enter, ease: ease.in } }}
        >
          <div className="relative flex flex-col gap-2 overflow-hidden rounded-panel border border-line bg-overlay px-4 py-3 shadow-overlay">
            {kind === "inWorld" && (
              <span aria-hidden className="lumi-launch-light pointer-events-none absolute inset-0">
                <span className="absolute inset-y-0 right-1/2 w-32 bg-linear-to-r from-transparent to-accent/20" />
                <span className="absolute right-1/2 top-1/2 size-1.5 -translate-y-1/2 rounded-full bg-accent" />
                <span className="absolute right-[calc(50%+20px)] top-1/2 size-1 rounded-full bg-accent/75" />
                <span className="absolute right-[calc(50%+42px)] top-[45%] size-1 rounded-full bg-accent/50" />
              </span>
            )}
            <div className="relative flex items-center gap-3">
              <span className={failed ? "text-danger" : "text-accent"}>
                {failed ? (
                  <AlertTriangle className="size-5" aria-hidden />
                ) : isActive(session) && kind !== "mainMenu" && kind !== "inWorld" ? (
                  <LightSpinner className="size-5" />
                ) : (
                  <CircleCheck className="size-5" aria-hidden />
                )}
              </span>
              <div className="min-w-0 flex-1">
                <p className="truncate font-medium text-sm">{title}</p>
                {(session.error || progress?.message) && (
                  <p className="truncate text-fg-muted text-xs">
                    {session.error ?? progress?.message}
                  </p>
                )}
              </div>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  if (isActive(session) || !session.recordId) {
                    void openLog(session.instanceId);
                    return;
                  }
                  setHidden((h) => ({
                    ...h,
                    [session.instanceId]: { at: session.startedAt, failed },
                  }));
                  navigate(
                    `/instances/${encodeURIComponent(session.instanceId)}?tab=log&record=${encodeURIComponent(session.recordId)}`,
                  );
                }}
              >
                <ScrollText className="size-4" aria-hidden />
                {t(failed && session.recordId ? "diagnostics.show" : "capsule.log")}
              </Button>
              {isActive(session) ? (
                <Button
                  variant="ghost"
                  size="sm"
                  className="hover:text-danger"
                  onClick={() => requestStop(session.instanceId)}
                >
                  <Square className="size-3.5" aria-hidden />
                  {t("capsule.stop")}
                </Button>
              ) : (
                <IconButton
                  label={t("capsule.dismiss")}
                  size="sm"
                  onClick={() =>
                    setHidden((h) => ({
                      ...h,
                      [session.instanceId]: { at: session.startedAt, failed },
                    }))
                  }
                >
                  <X className="size-4" aria-hidden />
                </IconButton>
              )}
            </div>
            {progress?.total ? (
              <ProgressBar value={(progress.done ?? 0) / progress.total} label={title} />
            ) : null}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
