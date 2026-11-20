import type { ReactNode } from "react";

type Tone = "neutral" | "accent" | "warning" | "danger" | "hero";

const tones: Record<Tone, string> = {
  neutral: "bg-surface-2 text-fg-muted ring-1 ring-line ring-inset",
  accent: "bg-accent-soft text-accent",
  warning: "bg-warning-soft text-warning",
  danger: "bg-danger-soft text-danger",
  /** 压在截图上：和 IconButton 的 hero 一套 */
  hero: "bg-hero-chip text-on-hero ring-1 ring-hero-chip-line ring-inset",
};

/** 标签：小胶囊（圆角「标签」档）。「有更新」「仅客户端」「安装」这类短词用它。 */
export function Tag({
  tone = "neutral",
  className = "",
  children,
}: {
  tone?: Tone;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      className={`inline-flex shrink-0 items-center whitespace-nowrap rounded-tag px-2 py-0.5 text-xs ${tones[tone]} ${className}`}
    >
      {children}
    </span>
  );
}
