import { useEffect } from "react";
import { toast } from "sonner";
import { create } from "zustand";
import { CommandError, unwrap } from "@/lib/api";
import { commands, type MemoryCleanResult, type MemoryStatus } from "@/lib/bindings";
import i18n from "@/lib/i18n";
import { gb } from "@/lib/memory";
import { logError, reportError } from "@/lib/report";

/** 内存条多久刷新一次 */
const POLL_MS = 2000;
/** 腾出的少于这么多就说「没腾出多少」 */
const NOTICEABLE_MB = 200;

interface MemoryStore {
  status: MemoryStatus | null;
  /** 正在整理（等管理员确认或者正在整理） */
  cleaning: boolean;
  refresh: () => Promise<void>;
  /** 整理内存：弹 Windows 的管理员确认，整理完弹提示。成功返回结果 */
  clean: () => Promise<MemoryCleanResult | null>;
}

export const useMemoryStore = create<MemoryStore>((set, get) => ({
  status: null,
  cleaning: false,

  async refresh() {
    try {
      set({ status: await commands.memoryStatus() });
    } catch (error) {
      logError(error, "读取内存情况");
    }
  },

  async clean() {
    if (get().cleaning) return null;
    set({ cleaning: true });
    try {
      const result = unwrap(await commands.memoryClean());
      toast.success(
        result.freedMb >= NOTICEABLE_MB
          ? i18n.t("memory.clean.done", { gb: gb(result.freedMb), free: gb(result.availableMb) })
          : i18n.t("memory.clean.little", { free: gb(result.availableMb) }),
      );
      void get().refresh();
      return result;
    } catch (error) {
      // 用户在管理员确认里点了「否」：不算出错，轻轻提示一句
      if (error instanceof CommandError && error.code === "memory.clean_cancelled") {
        toast(i18n.t("memory.clean.cancelled"));
      } else {
        reportError(error, "整理内存");
      }
      return null;
    } finally {
      set({ cleaning: false });
    }
  },
}));

/** 挂着这个钩子的组件显示期间，每两秒刷新一次内存情况 */
export function useMemoryPolling(enabled = true) {
  const refresh = useMemoryStore((s) => s.refresh);
  useEffect(() => {
    if (!enabled) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [enabled, refresh]);
}
