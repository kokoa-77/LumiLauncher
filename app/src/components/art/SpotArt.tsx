/** 透明小插画，主色光来自同一套主题令牌。文字和操作由所在页面提供。 */
export function SpotArt({ src, className = "size-28" }: { src: string; className?: string }) {
  return (
    <span aria-hidden className={`relative grid shrink-0 place-items-center ${className}`}>
      <span className="absolute inset-2 bg-radial-[circle_closest-side] from-glow to-transparent" />
      <img alt="" src={src} decoding="async" className="relative size-full object-contain" />
    </span>
  );
}
