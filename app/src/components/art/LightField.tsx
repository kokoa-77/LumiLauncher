import { type CSSProperties, useMemo } from "react";

/** 一屏最多这么多颗（开发合同第 6 节「环境光效」） */
const MAX_MOTES = 24;

interface Mote {
  x: number;
  y: number;
  size: number;
  duration: number;
  delay: number;
  rise: number;
  sway: number;
  alpha: number;
}

/** 固定种子的伪随机：每次打开光点都在同样的位置，不会一闪一闪地重排 */
function random(seed: number) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function makeMotes(count: number, seed: number, area: "full" | "low"): Mote[] {
  const next = random(seed);
  return Array.from({ length: Math.min(count, MAX_MOTES) }, () => {
    // 近的大一点、亮一点、飘得快一点；远的小、淡、慢（有景深）
    const near = next();
    return {
      x: next() * 100,
      // 「low」：光点集中在下半部分（地平线附近多，越往上越稀）
      y: area === "low" ? 35 + next() ** 0.7 * 60 : next() * 100,
      size: 4 + near * 14,
      duration: 12 - near * 5 + next() * 2,
      delay: next() * 12,
      rise: 30 + near * 50,
      sway: (next() - 0.5) * 24,
      alpha: 0.35 + near * 0.6,
    };
  });
}

/**
 * 微光的光点：一颗颗柔光慢慢往上飘、亮起又淡掉，颜色跟着主色（换实例时一起变）。
 * 只动 transform 和 opacity；流畅模式、减少动效下停住（`global.css` 的 `.mote`）。
 * 用在「画面」里（首次运行引导、安装程序、以后的首页默认背景），不用在列表、设置这些干活的地方。
 */
export function LightField({
  count = 14,
  seed = 1,
  area = "full",
  className = "",
}: {
  count?: number;
  seed?: number;
  area?: "full" | "low";
  className?: string;
}) {
  const motes = useMemo(() => makeMotes(count, seed, area), [count, seed, area]);
  return (
    <div aria-hidden className={`pointer-events-none overflow-hidden ${className}`}>
      {motes.map((mote, index) => (
        <span
          key={index}
          className="mote"
          style={
            {
              left: `${mote.x}%`,
              top: `${mote.y}%`,
              width: mote.size,
              height: mote.size,
              "--mote-duration": `${mote.duration.toFixed(1)}s`,
              // 负的延迟：一打开就是飘到一半的样子，不会所有光点同时从零亮起
              "--mote-delay": `${(-mote.delay).toFixed(1)}s`,
              "--mote-rise": `${(-mote.rise).toFixed(0)}px`,
              "--mote-sway": `${mote.sway.toFixed(0)}px`,
              "--mote-alpha": mote.alpha.toFixed(2),
            } as CSSProperties
          }
        />
      ))}
    </div>
  );
}
