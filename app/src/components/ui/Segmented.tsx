import { motion } from "motion/react";
import { useId } from "react";
import { spring } from "@/lib/motion";

interface Option<T extends string> {
  value: T;
  label: string;
  /** 灰着、点不了（「正版 · 暂未开放」这种以后会有的） */
  disabled?: boolean;
}

interface SegmentedProps<T extends string> {
  value: T;
  options: Option<T>[];
  onChange: (value: T) => void;
  label: string;
}

/**
 * 分段选择器：一条胶囊，选中的那格是主色浅底 + 主色字（和侧栏、列表里的「选中」一个样子，
 * 见开发合同第 5 节「颜色的六种角色」），选中块平滑滑到新位置。
 * 和按钮、输入框一样高（36 像素），放在一排里边缘对齐。
 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: SegmentedProps<T>) {
  const id = useId();
  return (
    <fieldset className="inline-flex shrink-0 rounded-control border border-line bg-surface p-[3px] shadow-raised">
      <legend className="sr-only">{label}</legend>
      {options.map((option) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={active}
            disabled={option.disabled}
            onClick={() => onChange(option.value)}
            className={`relative h-7 whitespace-nowrap rounded-inner px-3.5 text-sm transition-colors duration-(--motion-toggle) disabled:cursor-default disabled:opacity-45 ${
              active ? "font-medium text-accent" : "text-fg-muted enabled:hover:text-fg"
            }`}
          >
            {active && (
              <motion.span
                layoutId={`segmented-${id}`}
                className="absolute inset-0 rounded-inner bg-accent-soft"
                transition={spring.snappy}
              />
            )}
            <span className="relative">{option.label}</span>
          </button>
        );
      })}
    </fieldset>
  );
}
