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
        className={`absolute top-[2px] left-[2px] size-[18px] rounded-full shadow-sm ${checked ? "bg-on-accent" : "bg-surface"}`}
        animate={{ x: checked ? 16 : 0 }}
        transition={spring.snappy}
      />
    </button>
  );
}
