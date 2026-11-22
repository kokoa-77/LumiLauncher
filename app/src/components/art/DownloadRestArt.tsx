/** 下载空闲托盘：与 SpotArt 同属纯 2D 柔光叠层风格，随主题主色与昼夜同步变化。 */
export function DownloadRestArt({ className = "h-20 w-32" }: { className?: string }) {
  return (
    <span aria-hidden className={`relative grid shrink-0 place-items-center ${className}`}>
      <span className="absolute inset-x-4 inset-y-1 bg-radial-[circle_closest-side] from-glow to-transparent" />
      <svg aria-hidden viewBox="0 0 144 92" fill="none" className="relative size-full">
        {/* 后层柔光托片 */}
        <rect x="28" y="42" width="88" height="34" rx="13" fill="var(--art-back)" />
        {/* 前层通透接收托盘 */}
        <path
          d="M20 52 H48 A6 6 0 0 1 53.5 55.6 L55.5 60.4 A6 6 0 0 0 61 64 H83 A6 6 0 0 0 88.5 60.4 L90.5 55.6 A6 6 0 0 1 96 52 H124 A6 6 0 0 1 130 58 V70 A14 14 0 0 1 116 84 H28 A14 14 0 0 1 14 70 V58 A6 6 0 0 1 20 52 Z"
          fill="var(--art-front)"
          stroke="var(--art-stroke)"
          strokeWidth="1.4"
        />
        {/* 托盘底部的细柔高光条 */}
        <rect x="54" y="72" width="36" height="4.5" rx="2.25" fill="var(--art-fill-mid)" />
        {/* 落入的实色主色微光下载徽章 */}
        <g className="lumi-download-arrival">
          <circle cx="72" cy="28" r="16" fill="var(--accent)" />
          <path
            d="M72 20.5 V34.5 M66.5 29.5 L72 35 L77.5 29.5"
            stroke="var(--on-accent)"
            strokeWidth="2.4"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </g>
      </svg>
    </span>
  );
}
