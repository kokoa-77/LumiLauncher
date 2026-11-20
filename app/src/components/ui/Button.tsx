import type { ButtonHTMLAttributes } from "react";

type Variant = "primary" | "secondary" | "outline" | "ghost" | "danger";
type Size = "sm" | "md";

const variants: Record<Variant, string> = {
  primary: "bg-accent text-on-accent hover:bg-accent-hover",
  secondary: "border border-line bg-surface text-fg shadow-raised hover:bg-surface-hover",
  /** 反复出现的操作（每个世界上的「进入」、每行的「更新」）：主色描边 + 主色字，悬停才铺浅底 */
  outline: "border border-accent/55 bg-surface text-accent hover:bg-accent-soft",
  ghost: "text-fg-muted hover:bg-surface-hover hover:text-fg",
  danger: "bg-danger text-on-danger hover:bg-danger/90",
};

const sizes: Record<Size, string> = {
  md: "h-9 gap-2 px-4 text-sm",
  sm: "h-8 gap-1.5 px-3 text-xs",
};

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
}

/** 普通按钮：胶囊（圆角「控件」档），和输入框、下拉框一样高。全页最重要的那个（继续、开始游戏）用 CtaButton。 */
export function Button({
  variant = "secondary",
  size = "md",
  className = "",
  ...props
}: ButtonProps) {
  return (
    <button
      type="button"
      className={`inline-flex shrink-0 items-center justify-center whitespace-nowrap rounded-control font-medium transition-[background-color,color,scale,opacity] duration-(--motion-press) ease-out active:scale-97 disabled:pointer-events-none disabled:opacity-45 ${sizes[size]} ${variants[variant]} ${className}`}
      {...props}
    />
  );
}
