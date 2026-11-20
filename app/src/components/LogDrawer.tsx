import { X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useTranslation } from "react-i18next";
import type { LogLine } from "@/lib/bindings";
import { duration, ease, spring } from "@/lib/motion";
import { useGameStore } from "@/stores/game";
import { IconButton } from "./ui/IconButton";
import { VirtualList } from "./VirtualList";

/** 没有日志时用同一个空数组：每次返回新数组会让状态订阅以为一直在变，陷入无限重画 */
export const NO_LINES: LogLine[] = [];

const levelClass: Record<LogLine["level"], string> = {
  trace: "text-fg-muted",
  debug: "text-fg-muted",
  info: "text-fg",
  warn: "text-warning",
  error: "text-danger",
  fatal: "text-danger",
};

/** 一行行日志（虚拟滚动，新日志来了自动跟到底）：日志抽屉和实例页的「日志」页签共用 */
export function LogLines({ lines, className = "" }: { lines: LogLine[]; className?: string }) {
  return (
    <VirtualList
      className={`select-text font-mono text-xs leading-5 ${className}`}
      items={lines}
      estimate={20}
      followTail
      getKey={(_, index) => String(index)}
      render={(line) => (
        <p className={`whitespace-pre-wrap break-all ${levelClass[line.level]}`}>
          {line.thread && <span className="text-fg-muted">[{line.thread}] </span>}
          {line.message}
        </p>
      )}
    />
  );
}

/** 游戏日志抽屉：从右边滑出，虚拟滚动，新日志自动跟到底。 */
export function LogDrawer() {
  const { t } = useTranslation();
  const instanceId = useGameStore((s) => s.logInstance);
  const lines = useGameStore((s) => (instanceId ? (s.logs[instanceId] ?? NO_LINES) : NO_LINES));
  const instance = useGameStore((s) => s.instances.find((i) => i.id === instanceId));
  const close = () => void useGameStore.getState().openLog(null);

  return (
    <AnimatePresence>
      {instanceId && (
        <motion.aside
          className="fixed top-10 right-0 bottom-0 z-30 flex w-[min(640px,100%)] flex-col border-line border-l bg-overlay shadow-overlay"
          initial={{ opacity: 0, x: 48 }}
          animate={{ opacity: 1, x: 0, transition: spring.gentle }}
          exit={{ opacity: 0, x: 48, transition: { duration: duration.overlay, ease: ease.in } }}
        >
          <header className="flex items-center gap-3 border-line border-b px-4 py-3">
            <div className="min-w-0 flex-1">
              <p className="truncate font-medium text-sm">
                {t("log.title", { name: instance?.name ?? instanceId })}
              </p>
              <p className="text-fg-muted text-xs">{t("log.count", { n: lines.length })}</p>
            </div>
            <IconButton label={t("log.close")} size="sm" onClick={close}>
              <X className="size-4" aria-hidden />
            </IconButton>
          </header>
          {lines.length === 0 ? (
            <p className="p-6 text-center text-fg-muted text-sm">{t("log.empty")}</p>
          ) : (
            <LogLines lines={lines} className="min-h-0 flex-1 px-4 py-2" />
          )}
        </motion.aside>
      )}
    </AnimatePresence>
  );
}
