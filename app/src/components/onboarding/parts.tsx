import type { ReactNode } from "react";

/** 右边每一步的框架：标题 + 一句实用提示、能滚动的内容、底部一排按钮 */
export function StepFrame({
  title,
  hint,
  children,
  footer,
}: {
  title: string;
  hint?: string;
  children: ReactNode;
  footer: ReactNode;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex select-text flex-col gap-1.5 px-10 pt-10 pb-6">
        <h1 className="font-semibold text-2xl">{title}</h1>
        {hint && <p className="text-fg-muted text-sm">{hint}</p>}
      </header>
      <div className="min-h-0 flex-1 select-text overflow-y-auto px-10 pb-4">{children}</div>
      <footer className="flex items-center justify-between gap-3 px-10 pt-4 pb-8">{footer}</footer>
    </div>
  );
}

/**
 * 一团柔光（还没有东西可放的时候，画面中间就是它；以后应用图标放在它中间）。
 * 渐变按「最近的边」算、在边上正好淡到透明，没有一圈硬边（不靠圆角裁）
 */
export function GlowOrb({ className = "size-32" }: { className?: string }) {
  return (
    <div aria-hidden className={`relative ${className}`}>
      <span className="absolute inset-0 bg-radial-[circle_closest-side] from-glow via-glow/35 via-40% to-transparent" />
      <span className="absolute inset-[36%] bg-radial-[circle_closest-side] from-mote via-mote-core via-45% to-transparent" />
    </div>
  );
}

/** 小节标题（12 号灰字，和设置面板里的分组小字一样） */
export function Caption({ children }: { children: ReactNode }) {
  return <p className="mb-2 text-fg-muted text-xs">{children}</p>;
}
