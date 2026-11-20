/**
 * 迷你首页：只用颜色令牌画的一个小窗口（标题栏三个点、侧栏、首页的天空、世界名、主按钮）。
 * 外观这一步左边放它，深浅色、主色一改它就跟着变；套在 `data-theme` 里就是那一种深浅色（选深浅色的小卡片用）。
 */
export function MiniHome({ small = false }: { small?: boolean }) {
  const size = small ? "w-full" : "w-80";
  return (
    <div
      aria-hidden
      className={`${size} overflow-hidden rounded-panel border border-line bg-bg text-left shadow-overlay`}
    >
      <div className={`flex items-center justify-between ${small ? "h-3.5 px-1.5" : "h-6 px-2.5"}`}>
        <span className={`rounded-tag bg-fg/70 ${small ? "h-1 w-3" : "h-1.5 w-6"}`} />
        <span className="flex gap-1">
          {[0, 1, 2].map((i) => (
            <span key={i} className={`rounded-icon bg-line ${small ? "size-1" : "size-1.5"}`} />
          ))}
        </span>
      </div>
      <div className={`flex ${small ? "h-16" : "h-44"}`}>
        <div
          className={`flex flex-col items-center ${small ? "w-4 gap-1 pt-1" : "w-9 gap-2 pt-2"}`}
        >
          {[0, 1, 2].map((i) => (
            <span
              key={i}
              className={`rounded-row ${small ? "size-2" : "size-4"} ${i === 0 ? "bg-accent-soft" : "bg-surface-2"}`}
            />
          ))}
        </div>
        <div className="relative flex-1 overflow-hidden rounded-tl-panel bg-linear-to-b from-sky-top to-sky-bottom">
          <span
            className={`absolute rounded-icon bg-radial from-glow to-transparent to-70% ${small ? "-top-3 -right-3 size-12" : "-top-6 -right-6 size-32"}`}
          />
          <div
            className={`absolute flex flex-col ${small ? "bottom-1.5 left-1.5 gap-0.5" : "bottom-4 left-4 gap-1.5"}`}
          >
            <span className={`rounded-tag bg-fg/80 ${small ? "h-1.5 w-10" : "h-3.5 w-28"}`} />
            <span className={`rounded-tag bg-fg-muted/50 ${small ? "h-1 w-6" : "h-2 w-16"}`} />
          </div>
          <div
            className={`absolute flex items-center ${small ? "right-1.5 bottom-1.5 gap-0.5" : "right-4 bottom-4 gap-1.5"}`}
          >
            <span className={`rounded-cta bg-accent ${small ? "h-2.5 w-6" : "h-7 w-20"}`} />
            <span className={`rounded-icon bg-surface ${small ? "size-2.5" : "size-7"}`} />
          </div>
        </div>
      </div>
    </div>
  );
}
