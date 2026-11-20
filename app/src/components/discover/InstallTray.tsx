import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CtaButton } from "@/components/ui/CtaButton";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { duration, ease, spring } from "@/lib/motion";
import { isActive, useGameStore } from "@/stores/game";

/**
 * 从实例进来添加内容时，页面底下的小胶囊：「装好了 3 个 · 进世界试试」。
 * 只数这次进来以后装的；一点直接进这个实例最近玩的世界（不支持直接进存档的版本就是「开始游戏」）。
 * 游戏开起来以后收起，交给启动条。
 */
export function InstallTray({ instanceId }: { instanceId: string }) {
  const { t } = useTranslation();
  const jobs = useGameStore((s) => s.jobs);
  const launch = useGameStore((s) => s.launch);
  const running = useGameStore((s) => isActive(s.sessions[instanceId]));
  const recent = useGameStore((s) => s.recentWorlds?.find((r) => r.instanceId === instanceId));
  const [since] = useState(() => Date.now());

  const { done, busy } = useMemo(() => {
    const mine = Object.values(jobs).filter(
      (j) => j.key.startsWith(`content:${instanceId}:`) && (j.startedAt ?? 0) >= since,
    );
    return {
      done: mine.filter((j) => j.state === "succeeded").length,
      busy: mine.filter((j) => j.state === "running").length,
    };
  }, [jobs, instanceId, since]);

  const show = (done > 0 || busy > 0) && !running;
  const world = recent?.quickPlay ? recent.world : undefined;

  return (
    <AnimatePresence>
      {show && (
        <motion.div
          className="-translate-x-1/2 absolute bottom-6 left-1/2 z-30 flex items-center gap-4 rounded-cta border border-line bg-overlay py-1.5 pr-1.5 pl-6 shadow-overlay"
          initial={{ opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0, transition: spring.gentle }}
          exit={{ opacity: 0, y: 16, transition: { duration: duration.overlay, ease: ease.in } }}
        >
          <span className="flex items-center gap-2 whitespace-nowrap text-sm">
            {busy > 0 && <LightSpinner className="size-4 text-accent" />}
            {busy > 0
              ? t("discover.tray.busy", { n: busy, done })
              : t("discover.tray.done", { n: done })}
          </span>
          <CtaButton
            size="md"
            label={world ? t("discover.tray.play") : t("play.start")}
            sub={world ? t("home.world.resumeSub", { world: world.name }) : undefined}
            disabled={busy > 0}
            onClick={() => void launch(instanceId, world?.folder)}
          />
        </motion.div>
      )}
    </AnimatePresence>
  );
}
