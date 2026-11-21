import { motion } from "motion/react";
import { spring } from "@/lib/motion";

interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  /** 锁住（比如游戏开着时不能改模组）：变淡、点不动 */
  disabled?: boolean;
}

export function Switch({ checked, onChange, label, disabled = false }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative h-6 w-10 shrink-0 rounded-full border transition-[background-color,border-color,opacity] duration-(--motion-toggle) disabled:opacity-45 ${
        checked ? "border-accent bg-accent" : "border-line bg-surface-hover"
      }`}
    >
      <motion.span
        // 关着时滑块用灰字的颜色：深色下 bg-surface 和底槽几乎一个色，看不出是开关
        className={`absolute top-[2px] left-[2px] size-[18px] rounded-full shadow-sm ${checked ? "bg-on-accent" : "bg-fg-muted"}`}
        animate={{ x: checked ? 16 : 0 }}
        transition={spring.snappy}
      />
    </button>
  );
}
