/** 一个光点沿细圆轨道转动。流畅、减少动效下保留静态图形。 */
export function LightSpinner({ className = "" }: { className?: string }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      className={className}
    >
      <circle cx="12" cy="12" r="8" className="opacity-25" />
      <g className="lumi-spinner-dot origin-center">
        <circle cx="12" cy="4" r="2.25" stroke="none" fill="currentColor" />
      </g>
    </svg>
  );
}
