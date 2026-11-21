import { ChevronDown, Server } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { InstanceSummary, RecentServer, RecentWorld } from "@/lib/bindings";
import { imageAt } from "@/lib/cover";
import { formatAgo } from "@/lib/format";
import { duration, ease, spring } from "@/lib/motion";
import { IconButton } from "./ui/IconButton";

/** 首页当前展示的「继续目标」：单人世界或多人服务器。 */
export type HomeTarget =
  | { kind: "world"; instanceId: string }
  | { kind: "server"; instanceId: string; address: string };

function isSameTarget(a: HomeTarget, b: HomeTarget): boolean {
  if (a.kind !== b.kind || a.instanceId !== b.instanceId) return false;
  return a.kind === "world" || (b.kind === "server" && a.address === b.address);
}

/**
 * 首页主标题旁边的 ▾：弹出最近玩的世界与多人服务器。
 * 点一下首页就切换过去，世界还会同步切到它封面的主题色。
 * 开关由外面管（「⋯」菜单里的「换一个世界」也能打开它）。
 */
export function WorldPicker({
  worlds,
  servers,
  instances,
  current,
  open,
  onOpenChange,
  onPick,
}: {
  worlds: RecentWorld[];
  servers: RecentServer[];
  instances: InstanceSummary[];
  current: HomeTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onPick: (target: HomeTarget) => void;
}) {
  const { t } = useTranslation();
  const setOpen = onOpenChange;
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, setOpen]);

  if (worlds.length + servers.length < 2) return null;
  const nameOf = (id: string) => instances.find((i) => i.id === id)?.name ?? id;
  const hasBoth = worlds.length > 0 && servers.length > 0;

  return (
    <div ref={rootRef} className="relative">
      <IconButton
        label={t("home.world.pick")}
        variant="hero"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <ChevronDown
          className={`size-5 transition-transform duration-(--motion-toggle) ${open ? "rotate-180" : ""}`}
          aria-hidden
        />
      </IconButton>
      <AnimatePresence>
        {open && (
          <motion.div
            role="listbox"
            aria-label={t("home.world.pickTitle")}
            className="absolute bottom-full left-0 z-30 mb-3 w-[560px] origin-bottom-left rounded-panel border border-line bg-overlay p-2 text-fg shadow-overlay"
            initial={{ opacity: 0, scale: 0.97, y: 6 }}
            animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
            exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
          >
            <div className="max-h-96 space-y-2 overflow-y-auto">
              {worlds.length > 0 && (
                <div>
                  <p className="px-2 pt-1 pb-1.5 text-fg-muted text-xs">
                    {hasBoth ? t("home.world.sectionWorlds") : t("home.world.pickTitle")}
                  </p>
                  <div className="grid grid-cols-2 gap-1">
                    {worlds.slice(0, 12).map((entry) => {
                      const target: HomeTarget = { kind: "world", instanceId: entry.instanceId };
                      const selected = isSameTarget(current, target);
                      return (
                        <button
                          key={`world:${entry.instanceId}`}
                          type="button"
                          role="option"
                          aria-selected={selected}
                          onClick={() => {
                            setOpen(false);
                            onPick(target);
                          }}
                          className={`flex items-center gap-3 rounded-item p-1.5 text-left transition-colors duration-(--motion-press) ${
                            selected ? "bg-accent-soft" : "hover:bg-surface-hover"
                          }`}
                        >
                          <span className="relative h-14 w-24 shrink-0 overflow-hidden rounded-row bg-surface-hover">
                            {entry.world.imageUrl && (
                              <img
                                src={imageAt(entry.world.imageUrl, 96)}
                                alt=""
                                loading="lazy"
                                className={`size-full object-cover ${
                                  entry.world.imageKind === "icon"
                                    ? "[image-rendering:pixelated]"
                                    : ""
                                }`}
                              />
                            )}
                          </span>
                          <span className="min-w-0">
                            <span
                              className={`block truncate font-medium text-sm ${
                                selected ? "text-accent" : ""
                              }`}
                            >
                              {entry.world.name}
                            </span>
                            <span className="block truncate text-fg-muted text-xs">
                              {nameOf(entry.instanceId)}
                            </span>
                            <span className="block truncate text-fg-muted text-xs tabular-nums">
                              {t("home.world.day", { n: entry.world.day })} ·{" "}
                              {formatAgo(entry.world.lastPlayed ?? 0, t)}
                            </span>
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}

              {servers.length > 0 && (
                <div>
                  <p className="px-2 pt-1 pb-1.5 text-fg-muted text-xs">
                    {t("home.world.sectionServers")}
                  </p>
                  <div className="grid grid-cols-2 gap-1">
                    {servers.slice(0, 12).map((entry) => {
                      const target: HomeTarget = {
                        kind: "server",
                        instanceId: entry.instanceId,
                        address: entry.address,
                      };
                      const selected = isSameTarget(current, target);
                      return (
                        <button
                          key={`server:${entry.instanceId}:${entry.address}`}
                          type="button"
                          role="option"
                          aria-selected={selected}
                          onClick={() => {
                            setOpen(false);
                            onPick(target);
                          }}
                          className={`flex items-center gap-3 rounded-item p-1.5 text-left transition-colors duration-(--motion-press) ${
                            selected ? "bg-accent-soft" : "hover:bg-surface-hover"
                          }`}
                        >
                          <span className="grid size-12 shrink-0 place-items-center rounded-row bg-accent-soft text-accent">
                            <Server className="size-5" aria-hidden />
                          </span>
                          <span className="min-w-0">
                            <span
                              className={`block truncate font-medium text-sm ${
                                selected ? "text-accent" : ""
                              }`}
                            >
                              {entry.name || entry.address}
                            </span>
                            <span className="block truncate text-fg-muted text-xs">
                              {nameOf(entry.instanceId)}
                            </span>
                            <span className="block truncate text-fg-muted text-xs">
                              {entry.address}
                            </span>
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
