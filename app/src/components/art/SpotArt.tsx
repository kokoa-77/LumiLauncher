import type { ReactNode } from "react";

export type SpotArtKind = "rollback" | "empty-library" | "empty-worlds" | "empty-search" | "error";

const illustrations: Record<SpotArtKind, ReactNode> = {
  /** 改动回滚：两层柔光快照卡 + 右上角主色回溯徽章 */
  rollback: (
    <svg aria-hidden viewBox="0 0 160 160" fill="none" className="relative size-full">
      <rect
        x="40"
        y="40"
        width="64"
        height="78"
        rx="16"
        transform="rotate(-8 72 79)"
        fill="var(--art-back)"
      />
      <rect
        x="48"
        y="42"
        width="66"
        height="80"
        rx="16"
        fill="var(--art-front)"
        stroke="var(--art-stroke)"
        strokeWidth="1.4"
      />
      <rect x="61" y="60" width="38" height="7" rx="3.5" fill="var(--art-fill-mid)" />
      <rect x="61" y="75" width="32" height="5" rx="2.5" fill="var(--art-line)" />
      <rect x="61" y="86" width="22" height="5" rx="2.5" fill="var(--art-line)" />
      <circle cx="110" cy="48" r="15" fill="var(--accent)" />
      <path
        d="M104.5 47.5 A6 6 0 1 1 107.5 53"
        stroke="var(--on-accent)"
        strokeWidth="2.2"
        strokeLinecap="round"
      />
      <path
        d="M101.8 45.2 L104.6 48.2 L107.8 45.4"
        stroke="var(--on-accent)"
        strokeWidth="2.1"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  ),

  /** 空白实例库：双层通透实例卡 + 右下角主色新建徽章 */
  "empty-library": (
    <svg aria-hidden viewBox="0 0 160 160" fill="none" className="relative size-full">
      <rect x="44" y="34" width="72" height="60" rx="15" fill="var(--art-back)" />
      <rect
        x="34"
        y="48"
        width="92"
        height="68"
        rx="18"
        fill="var(--art-front)"
        stroke="var(--art-stroke)"
        strokeWidth="1.4"
      />
      <rect x="44" y="58" width="44" height="48" rx="11" fill="var(--art-fill-light)" />
      <circle cx="74" cy="70" r="4" fill="var(--accent)" opacity="0.72" />
      <path
        d="M44 96 C52 86, 61 86, 70 94 C76 89, 81 90, 88 96 V99 A7 7 0 0 1 81 106 H51 A7 7 0 0 1 44 99 Z"
        fill="var(--art-fill-mid)"
      />
      <rect x="96" y="68" width="21" height="6" rx="3" fill="var(--art-fill-mid)" />
      <rect x="96" y="80" width="15" height="5" rx="2.5" fill="var(--art-line)" />
      <circle cx="118" cy="110" r="14.5" fill="var(--accent)" />
      <path
        d="M118 104.5v11M112.5 110h11"
        stroke="var(--on-accent)"
        strokeWidth="2.4"
        strokeLinecap="round"
      />
    </svg>
  ),

  /** 空白世界 / 截图 / 备份 / 自动封面：宽画幅柔光风景框 + 右上角微光星徽章 */
  "empty-worlds": (
    <svg aria-hidden viewBox="0 0 160 160" fill="none" className="relative size-full">
      <rect
        x="30"
        y="44"
        width="98"
        height="68"
        rx="16"
        transform="rotate(-6 79 78)"
        fill="var(--art-back)"
      />
      <rect
        x="28"
        y="48"
        width="104"
        height="70"
        rx="18"
        fill="var(--art-front)"
        stroke="var(--art-stroke)"
        strokeWidth="1.4"
      />
      <rect x="38" y="58" width="84" height="50" rx="11" fill="var(--art-fill-light)" />
      <circle cx="96" cy="72" r="6" fill="var(--accent)" opacity="0.75" />
      <rect x="48" y="67" width="20" height="6" rx="3" fill="var(--art-front)" opacity="0.88" />
      <path
        d="M38 97 C52 83, 66 83, 81 95 C91 87, 102 88, 122 98 V101 A7 7 0 0 1 115 108 H45 A7 7 0 0 1 38 101 Z"
        fill="var(--art-fill-mid)"
      />
      <circle cx="122" cy="50" r="14" fill="var(--accent)" />
      <path
        d="M122 43.2 L123.9 48.1 L128.8 50 L123.9 51.9 L122 56.8 L120.1 51.9 L115.2 50 L120.1 48.1 Z"
        fill="var(--on-accent)"
      />
    </svg>
  ),

  /** 搜索为空：柔光结果卡片 + 主色手柄与通透镜片 */
  "empty-search": (
    <svg aria-hidden viewBox="0 0 160 160" fill="none" className="relative size-full">
      <rect x="52" y="40" width="74" height="58" rx="16" fill="var(--art-back)" />
      <rect x="68" y="57" width="42" height="6" rx="3" fill="var(--art-front)" opacity="0.9" />
      <rect x="68" y="69" width="28" height="5" rx="2.5" fill="var(--art-front)" opacity="0.62" />
      <path d="M89 95 L112 118" stroke="var(--accent)" strokeWidth="8.5" strokeLinecap="round" />
      <circle
        cx="68"
        cy="74"
        r="31"
        fill="var(--art-front)"
        stroke="var(--art-stroke)"
        strokeWidth="1.5"
      />
      <circle cx="68" cy="74" r="22" fill="var(--art-fill-light)" />
      <path
        d="M55 64 A16 16 0 0 1 73 56"
        stroke="var(--surface)"
        strokeWidth="3"
        strokeLinecap="round"
        opacity="0.92"
      />
      <circle cx="61.5" cy="77" r="2.3" fill="var(--accent)" opacity="0.45" />
      <circle cx="69" cy="77" r="2.3" fill="var(--accent)" opacity="0.88" />
      <circle cx="76.5" cy="77" r="2.3" fill="var(--accent)" opacity="0.45" />
    </svg>
  ),

  /** 错误 / 断连：双层纯 2D 柔光云朵 + 主色断开徽章 */
  error: (
    <svg aria-hidden viewBox="0 0 160 160" fill="none" className="relative size-full">
      <path
        d="M46 104 H114 A17 17 0 0 0 116 70 A25 25 0 0 0 68.5 62 A19 19 0 0 0 46 104 Z"
        fill="var(--art-back)"
        transform="translate(7, -6)"
      />
      <path
        d="M38 106 H108 A18 18 0 0 0 110 70 A26 26 0 0 0 61 62 A19.5 19.5 0 0 0 38 106 Z"
        fill="var(--art-front)"
        stroke="var(--art-stroke)"
        strokeWidth="1.4"
      />
      <circle cx="112" cy="102" r="15" fill="var(--accent)" />
      <path
        d="M107 97 L117 107 M117 97 L107 107"
        stroke="var(--on-accent)"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
    </svg>
  ),
};

/** 纯 2D 柔光叠层小插画，颜色与微光背景全部取自同一套主题令牌。文字和操作由所在页面提供。 */
export function SpotArt({
  kind,
  src,
  className = "size-28",
}: {
  kind?: SpotArtKind;
  src?: string;
  className?: string;
}) {
  return (
    <span aria-hidden className={`relative grid shrink-0 place-items-center ${className}`}>
      <span className="absolute inset-2 bg-radial-[circle_closest-side] from-glow to-transparent" />
      {kind ? (
        illustrations[kind]
      ) : src ? (
        <img alt="" src={src} decoding="async" className="relative size-full object-contain" />
      ) : null}
    </span>
  );
}
