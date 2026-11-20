import { Check, Minus } from "lucide-react";

/**
 * 多选用的勾选框：圆的（能点的控件一律胶囊）。选中是实心主色 + 勾，和开关打开时一个样子；
 * `mixed` 是「选了一部分」（全选框用），显示一条横线。
 * `onChange` 的第二个参数是有没有按住 Shift（列表里按住 Shift 选一段）。
 */
export function Checkbox({
  checked,
  mixed = false,
  onChange,
  label,
  className = "",
  disabled = false,
}: {
  checked: boolean;
  mixed?: boolean;
  /** 锁住（游戏开着时）：变淡、点不动 */
  disabled?: boolean;
  onChange: (checked: boolean, shift: boolean) => void;
  label: string;
  className?: string;
}) {
  const on = checked || mixed;
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={mixed ? "mixed" : checked}
      aria-label={label}
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation();
        onChange(!checked, event.shiftKey);
      }}
      className={`grid size-5 shrink-0 place-items-center rounded-icon border transition-[background-color,border-color,opacity] duration-(--motion-toggle) disabled:pointer-events-none disabled:opacity-40 ${
        on
          ? "border-accent bg-accent text-on-accent"
          : "border-fg-muted/45 bg-surface hover:border-accent"
      } ${className}`}
    >
      {mixed ? (
        <Minus className="size-3" strokeWidth={3} aria-hidden />
      ) : checked ? (
        <Check className="size-3" strokeWidth={3} aria-hidden />
      ) : null}
    </button>
  );
}
