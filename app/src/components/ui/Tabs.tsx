import type { LucideIcon } from "lucide-react";
import { motion } from "motion/react";
import { type KeyboardEvent, useId, useRef } from "react";
import { spring } from "@/lib/motion";

export interface TabOption<T extends string> {
  value: T;
  label: string;
  icon: LucideIcon;
}

/**
 * 页面里的页签（实例页的内容 / 世界 / 截图 / 日志）：一排胶囊，没有外框。
 * 选中的是主色浅底 + 主色字（和侧栏、分段选择的「选中」一个样子），浅底平滑滑到新的一格；
 * 没有外框这一点和分段选择分开：页签换的是整块内容，分段选择只换一个列表的条件。
 * 键盘：左右方向键切换。
 */
export function Tabs<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: TabOption<T>[];
  onChange: (value: T) => void;
  label: string;
}) {
  const id = useId();
  const listRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const index = options.findIndex((o) => o.value === value);
    const step = event.key === "ArrowRight" ? 1 : -1;
    const next = options[(index + step + options.length) % options.length];
    if (!next) return;
    onChange(next.value);
    listRef.current?.querySelector<HTMLButtonElement>(`[data-tab="${next.value}"]`)?.focus();
  };

  return (
    <div
      ref={listRef}
      role="tablist"
      aria-label={label}
      onKeyDown={onKeyDown}
      className="flex shrink-0 items-center gap-1"
    >
      {options.map(({ value: v, label: text, icon: Icon }) => {
        const active = v === value;
        return (
          <button
            key={v}
            type="button"
            role="tab"
            data-tab={v}
            aria-selected={active}
            tabIndex={active ? 0 : -1}
            onClick={() => onChange(v)}
            className={`relative inline-flex h-9 items-center gap-2 rounded-control px-4 text-sm transition-colors duration-(--motion-toggle) ${
              active
                ? "font-medium text-accent"
                : "text-fg-muted hover:bg-surface-hover hover:text-fg"
            }`}
          >
            {active && (
              <motion.span
                layoutId={`tabs-${id}`}
                className="absolute inset-0 rounded-control bg-accent-soft"
                transition={spring.snappy}
              />
            )}
            <Icon className="relative size-4" aria-hidden />
            <span className="relative">{text}</span>
          </button>
        );
      })}
    </div>
  );
}
