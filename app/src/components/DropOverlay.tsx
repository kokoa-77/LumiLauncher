import { FileDown } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation } from "react-router";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { duration, ease } from "@/lib/motion";
import { isComplete, useClaimStore } from "@/stores/claims";
import { useDropStore } from "@/stores/drops";
import { useGameStore } from "@/stores/game";

/**
 * 拖进来的文件默认装进哪个实例：在实例页（或从实例进的资源中心）就是那个实例，
 * 否则是首页那个（最近玩的）。弹窗里能改。
 */
export function useDropTarget(): string | undefined {
  const location = useLocation();
  const instances = useGameStore((s) => s.instances);
  const match = /^\/instances\/([^/]+)/.exec(location.pathname);
  const fromRoute = match?.[1] ? decodeURIComponent(match[1]) : null;
  const fromQuery = new URLSearchParams(location.search).get("instance");
  const wanted = fromRoute ?? fromQuery;
  if (wanted && instances.some((i) => i.id === wanted)) return wanted;
  return instances[0]?.id;
}

/** 放下以后后台认了超过这么久，才显示「正在看看是什么」（平常几十毫秒就认完，不闪一下） */
const READING_DELAY_MS = 300;

/**
 * 文件拖到窗口上方时，整个窗口一层提示「松手导入」，写出模组会装进哪个实例（D17g）。
 * 松手后如果要认一会儿（一包东西要先拆开），同一张卡片换成「正在看看是什么」。
 * 正在补件时不出现：那时补件面板自己会高亮放置区，文件交给补件。
 * 只压暗、不模糊（老电脑上实时模糊最吃力）。
 */
export function DropOverlay() {
  const { t } = useTranslation();
  const hovering = useDropStore((s) => s.hovering);
  const reading = useDropStore((s) => s.reading);
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (!reading) {
      setSlow(false);
      return;
    }
    const timer = window.setTimeout(() => setSlow(true), READING_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [reading]);
  const claiming = useClaimStore((s) => Object.values(s.claims).some((c) => !isComplete(c)));
  const target = useDropTarget();
  const name = useGameStore((s) => s.instances.find((i) => i.id === target)?.name);
  const waiting = slow && !hovering;
  const show = (hovering || waiting) && !claiming;
  return (
    <AnimatePresence>
      {show && (
        <motion.div
          aria-hidden
          className="pointer-events-none fixed inset-0 z-50 grid place-items-center bg-scrim p-10"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1, transition: { duration: duration.overlay, ease: ease.out } }}
          exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
        >
          <div className="flex w-full max-w-md flex-col items-center gap-3 rounded-dialog border-2 border-accent border-dashed bg-overlay px-8 py-10 text-center shadow-overlay">
            <div className="grid size-14 place-items-center rounded-row bg-accent-soft text-accent">
              {waiting ? (
                <LightSpinner className="size-7" />
              ) : (
                <FileDown className="size-7" aria-hidden />
              )}
            </div>
            <p className="font-semibold text-lg">
              {waiting ? t("drop.reading") : t("drop.overlayTitle")}
            </p>
            <p className="text-fg-muted text-sm">
              {waiting ? t("drop.readingHint") : t("drop.overlayKinds")}
            </p>
            {name && !waiting && <p className="text-sm">{t("drop.overlayTarget", { name })}</p>}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
