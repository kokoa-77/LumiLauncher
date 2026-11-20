/** 下载的接收托盘。线条和光点随主题颜色改变。 */
export function DownloadRestArt({ className = "h-16 w-28" }: { className?: string }) {
  return (
    <svg aria-hidden viewBox="0 0 128 80" fill="none" className={className}>
      <ellipse cx="64" cy="70" rx="39" ry="4" className="fill-surface-hover" />
      <path
        d="M26 48 39 34h14m22 0h14l13 14"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        className="text-fg-muted opacity-55"
      />
      <path
        d="M26 48h23l6 7h18l6-7h23v12a6 6 0 0 1-6 6H32a6 6 0 0 1-6-6V48Z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
        className="fill-surface-2 text-fg-muted"
      />
      <path
        d="M52 60h24"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        className="text-accent opacity-60"
      />
      <g className="lumi-download-arrival text-accent">
        <circle cx="64" cy="24" r="15" className="fill-accent-soft" />
        <path
          d="M64 17v14m-5-5 5 5 5-5"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </g>
    </svg>
  );
}
