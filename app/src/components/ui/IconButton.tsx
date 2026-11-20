import type { ButtonHTMLAttributes } from "react";
import { FloatingTooltip, useHover } from "./HoverCard";

type Variant = "ghost" | "secondary" | "card" | "primary" | "hero";
type Size = "sm" | "md" | "cta" | "lg";

const variants: Record<Variant, string> = {
  ghost: "text-fg-muted hover:bg-surface-hover hover:text-fg",
  secondary: "border border-line bg-surface text-fg shadow-raised hover:bg-surface-hover",
  /**
   * 每张卡片上的主操作（实例卡片的开始游戏）：平时主色描边（「反复出现的操作」不许每张一个实心），
   * 鼠标移到卡片上（外面要有 `group`）或按钮上才铺满主色
   */
  card: "border border-accent/55 bg-surface text-accent group-hover:border-accent group-hover:bg-accent group-hover:text-on-accent hover:border-accent hover:bg-accent hover:text-on-accent",
  primary: "bg-accent text-on-accent hover:bg-accent-hover",
  /** 压在截图上：白天是半透明白底，夜晚是半透明浅色底，加一圈细线，不用毛玻璃 */
  hero: "bg-hero-chip text-on-hero ring-1 ring-hero-chip-line ring-inset hover:bg-hero-chip-hover",
};

/** 按下去不放的样子（菜单开着时的「⋯」）：和悬停一样的底色，鼠标移开也不变回去 */
const pressedVariants: Partial<Record<Variant, string>> = {
  ghost: "bg-surface-hover text-fg",
  hero: "bg-hero-chip-hover text-on-hero ring-1 ring-hero-chip-line ring-inset",
};

const sizes: Record<Size, string> = {
  sm: "size-8",
  md: "size-9",
  /** 和中号主按钮（CtaButton md）一样高，放在它旁边 */
  cta: "size-12",
  lg: "size-16",
};

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** 读屏和鼠标悬停时显示的名字（图标按钮必须有） */
  label: string;
  variant?: Variant;
  size?: Size;
  /** 鼠标移上去变成危险色（结束、删除） */
  danger?: boolean;
  /** 按下去不放（它打开的菜单还开着） */
  pressed?: boolean;
  /** 是否在悬停/聚焦时显示自绘浮层提示（默认开启） */
  tooltip?: boolean;
}

/** 图标按钮：圆形（圆角「图标」档），只放一个图标。使用自绘浮层提示，不用浏览器默认灰色 title 框。 */
export function IconButton({
  label,
  variant = "ghost",
  size = "md",
  danger = false,
  pressed = false,
  tooltip = true,
  className = "",
  onMouseEnter,
  onMouseLeave,
  onMouseDown,
  onFocus,
  onBlur,
  ...props
}: IconButtonProps) {
  const look = (pressed && pressedVariants[variant]) || variants[variant];
  const { ref, anchor, handlers } = useHover<HTMLButtonElement>((el) => tooltip && !el.disabled);
  return (
    <>
      <button
        ref={ref}
        type="button"
        aria-label={label}
        className={`grid shrink-0 place-items-center rounded-icon transition-[background-color,color,scale,opacity] duration-(--motion-press) ease-out active:scale-95 disabled:pointer-events-none disabled:opacity-45 ${sizes[size]} ${look} ${danger ? "hover:text-danger" : ""} ${className}`}
        onMouseEnter={(event) => {
          handlers.onMouseEnter();
          onMouseEnter?.(event);
        }}
        onMouseLeave={(event) => {
          handlers.onMouseLeave();
          onMouseLeave?.(event);
        }}
        onMouseDown={(event) => {
          handlers.onMouseDown();
          onMouseDown?.(event);
        }}
        onFocus={(event) => {
          handlers.onFocus();
          onFocus?.(event);
        }}
        onBlur={(event) => {
          handlers.onBlur();
          onBlur?.(event);
        }}
        {...props}
      />
      <FloatingTooltip anchor={anchor} label={label} />
    </>
  );
}
