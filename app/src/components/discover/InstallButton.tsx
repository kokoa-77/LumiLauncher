import { Check, Download, ExternalLink, Plus } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useTranslation } from "react-i18next";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { duration, ease, spring } from "@/lib/motion";
import { LumiIcon } from "../art/LumiIcon";
import type { InstallKind, InstallState } from "./useInstall";

/** 没装时按钮上写什么 */
function idleLabel(kind: InstallKind, t: (key: string) => string) {
  if (kind === "modpack") return t("discover.installPack");
  if (kind === "server") return t("discover.join");
  return t("discover.install");
}

/**
 * 安装按钮（三态 + 解析中）：安装 → 安装中（按钮底下有进度在走）→ 已安装。
 * 服务器：加入 → 已加入后变成「进入」（还能点，一点直接进服务器）。
 * 作者不许第三方下载的地图：「去下载」，点了打开 CurseForge 页面。
 * 状态切换时图标和字交叉淡入（上下各挪一点）；按钮宽度固定，不会把旁边的东西挤动。
 * 流畅模式下 MotionConfig 会关掉位移，只剩淡入淡出。
 */
export function InstallButton({
  state,
  kind,
  disabled = false,
  onClick,
}: {
  state: InstallState;
  kind: InstallKind;
  disabled?: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  // 「去下载」和「安装」长得一样（都是能点的、反复出现的操作），只是字和图标不同
  const idle = state.kind === "idle" || state.kind === "manual";
  const clickable = idle || state.kind === "joined";
  const label =
    state.kind === "done"
      ? t("discover.installed")
      : state.kind === "joined"
        ? t("discover.enter")
        : state.kind === "manual"
          ? t("discover.manualDownload")
          : state.kind === "running"
            ? t("discover.installing")
            : state.kind === "busy"
              ? t("discover.resolving")
              : idleLabel(kind, t);
  const icon =
    state.kind === "done" ? (
      <Check className="size-4" aria-hidden />
    ) : state.kind === "joined" ? (
      <LumiIcon name="play" className="size-3.5" />
    ) : state.kind === "manual" ? (
      <ExternalLink className="size-4" aria-hidden />
    ) : state.kind === "idle" ? (
      kind === "modpack" || kind === "server" ? (
        <Plus className="size-4" aria-hidden />
      ) : (
        <Download className="size-4" aria-hidden />
      )
    ) : (
      <LightSpinner className="size-4" />
    );

  return (
    <button
      type="button"
      disabled={!clickable || disabled}
      aria-live="polite"
      onClick={(event) => {
        event.stopPropagation();
        onClick();
      }}
      className={`relative inline-flex h-9 w-28 shrink-0 items-center justify-center overflow-hidden rounded-control font-medium text-sm transition-[background-color,color,border-color,opacity] duration-(--motion-toggle) ${
        idle
          ? "border border-accent/55 bg-surface text-accent hover:bg-accent-soft disabled:border-line disabled:text-fg-muted disabled:opacity-60"
          : state.kind === "joined"
            ? "border border-transparent bg-accent-soft text-accent hover:border-accent/55"
            : "border border-transparent bg-accent-soft text-accent"
      }`}
    >
      {state.kind === "running" && state.progress !== null && (
        <span
          aria-hidden
          className="lumi-install-fill absolute inset-0 origin-left bg-linear-to-r from-accent/10 to-accent/35"
          style={{ transform: `scaleX(${state.progress})` }}
        />
      )}
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={state.kind}
          className="relative inline-flex items-center gap-1.5"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0, transition: spring.snappy }}
          exit={{ opacity: 0, y: -8, transition: { duration: duration.press, ease: ease.in } }}
        >
          {icon}
          {label}
        </motion.span>
      </AnimatePresence>
    </button>
  );
}
