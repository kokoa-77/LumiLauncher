import { Check, ChevronDown } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { instanceSubtitle } from "@/components/InstanceCard";
import type { InstanceSummary, WorldView } from "@/lib/bindings";
import { imageAt } from "@/lib/cover";
import { duration, ease, spring } from "@/lib/motion";

/** 实例的小封面：最近那个世界的截图（只有图标的按像素放大），没有存档时是名字的第一个字 */
export function InstanceThumb({
  instance,
  world,
  className,
}: {
  instance: InstanceSummary;
  world: WorldView | undefined;
  className: string;
}) {
  const letter = Array.from(instance.name.trim())[0]?.toUpperCase() ?? "?";
  return (
    <span
      className={`relative grid shrink-0 place-items-center overflow-hidden bg-accent-soft ${className}`}
    >
      {world?.imageUrl ? (
        <img
          src={world.imageKind === "icon" ? world.imageUrl : imageAt(world.imageUrl, 96)}
          alt=""
          className={`absolute inset-0 size-full object-cover ${
            world.imageKind === "icon" ? "[image-rendering:pixelated]" : ""
          }`}
        />
      ) : (
        <span className="font-semibold text-accent text-xs">{letter}</span>
      )}
    </span>
  );
}

/**
 * 从侧栏进资源中心时，右上角的「装到」：一个实例胶囊（封面 + 名字 + ▾），点开换实例。
 * 默认是首页那个世界所在的实例。
 */
export function TargetPicker({
  instances,
  worldOf,
  value,
  onChange,
}: {
  instances: InstanceSummary[];
  worldOf: Map<string, WorldView>;
  value: string;
  onChange: (id: string) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = instances.find((i) => i.id === value);

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
  }, [open]);

  if (!current) {
    return <span className="text-fg-muted text-sm">{t("discover.noInstance")}</span>;
  }

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={t("discover.target")}
        onClick={() => setOpen((v) => !v)}
        className={`flex h-9 max-w-80 items-center gap-2.5 rounded-control border bg-surface pr-3 pl-1 text-sm shadow-raised transition-colors duration-(--motion-press) hover:bg-surface-hover ${
          open ? "border-accent" : "border-line"
        }`}
      >
        <InstanceThumb
          instance={current}
          world={worldOf.get(current.id)}
          className="h-7 w-10 rounded-control"
        />
        <span className="shrink-0 text-fg-muted">{t("discover.installToShort")}</span>
        <span className="truncate font-medium">{current.name}</span>
        <ChevronDown
          className={`size-4 shrink-0 text-fg-muted transition-transform duration-(--motion-toggle) ${open ? "rotate-180" : ""}`}
          aria-hidden
        />
      </button>
      <AnimatePresence>
        {open && (
          <motion.div
            role="listbox"
            aria-label={t("discover.target")}
            className="absolute top-full right-0 z-30 mt-2 max-h-96 w-96 origin-top-right overflow-y-auto rounded-panel border border-line bg-overlay p-1.5 shadow-overlay"
            initial={{ opacity: 0, scale: 0.97, y: -4 }}
            animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
            exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
          >
            <p className="px-2 pt-1 pb-2 text-fg-muted text-xs">{t("discover.target")}</p>
            {instances.map((instance) => {
              const selected = instance.id === value;
              return (
                <button
                  key={instance.id}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  onClick={() => {
                    setOpen(false);
                    onChange(instance.id);
                  }}
                  className={`flex w-full items-center gap-3 rounded-item p-1.5 text-left transition-colors duration-(--motion-press) ${
                    selected ? "bg-accent-soft" : "hover:bg-surface-hover"
                  }`}
                >
                  <InstanceThumb
                    instance={instance}
                    world={worldOf.get(instance.id)}
                    className="h-10 w-16 rounded-row"
                  />
                  <span className="min-w-0 flex-1">
                    <span
                      className={`block truncate font-medium text-sm ${selected ? "text-accent" : ""}`}
                    >
                      {instance.name}
                    </span>
                    <span className="block truncate text-fg-muted text-xs">
                      {instanceSubtitle(instance)}
                    </span>
                  </span>
                  {selected && <Check className="size-4 shrink-0 text-accent" aria-hidden />}
                </button>
              );
            })}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
