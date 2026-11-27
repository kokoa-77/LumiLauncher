import { useId } from "react";
import type { InstanceSummary } from "@/lib/bindings";

export type CoverPaletteId =
  | "dusk"
  | "meadow"
  | "amber"
  | "amethyst"
  | "sakura"
  | "ocean"
  | "ember"
  | "frost";

export type CoverEmblemId =
  | "lantern"
  | "grass"
  | "compass"
  | "crystal"
  | "anvil"
  | "scroll"
  | "book"
  | "portal";

export interface CoverPalette {
  id: CoverPaletteId;
  hue: number;
  chroma: number;
  /** 深底色（左上） */
  deep: string;
  /** 中景底色（右下） */
  mid: string;
  /** 微光高光色 */
  glow: string;
  /** 徽章亮面色 */
  accent: string;
}

export const COVER_PALETTES: readonly [CoverPalette, ...CoverPalette[]] = [
  {
    id: "dusk",
    hue: 235,
    chroma: 0.12,
    deep: "#131c33",
    mid: "#1e325c",
    glow: "#60a5fa",
    accent: "#93c5fd",
  },
  {
    id: "meadow",
    hue: 152,
    chroma: 0.11,
    deep: "#11241d",
    mid: "#1f4736",
    glow: "#4ade80",
    accent: "#86efac",
  },
  {
    id: "amber",
    hue: 55,
    chroma: 0.13,
    deep: "#261b12",
    mid: "#4a3319",
    glow: "#f59e0b",
    accent: "#fcd34d",
  },
  {
    id: "amethyst",
    hue: 292,
    chroma: 0.13,
    deep: "#1e1530",
    mid: "#3a245e",
    glow: "#c084fc",
    accent: "#e9d5ff",
  },
  {
    id: "sakura",
    hue: 350,
    chroma: 0.11,
    deep: "#291722",
    mid: "#52283f",
    glow: "#f472b6",
    accent: "#fbcfe8",
  },
  {
    id: "ocean",
    hue: 196,
    chroma: 0.12,
    deep: "#0e222e",
    mid: "#174559",
    glow: "#38bdf8",
    accent: "#7dd3fc",
  },
  {
    id: "ember",
    hue: 28,
    chroma: 0.13,
    deep: "#291612",
    mid: "#57281d",
    glow: "#fb923c",
    accent: "#fdba74",
  },
  {
    id: "frost",
    hue: 215,
    chroma: 0.08,
    deep: "#16202b",
    mid: "#2b3e52",
    glow: "#94c5eb",
    accent: "#cbd5e1",
  },
] as const;

export const COVER_EMBLEMS: readonly CoverEmblemId[] = [
  "lantern",
  "grass",
  "compass",
  "crystal",
  "anvil",
  "scroll",
  "book",
  "portal",
] as const;

function hashString(input: string): number {
  let hash = 2166136261;
  for (let i = 0; i < input.length; i += 1) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

/** 解析实例默认矢量封面预设（`<emblem>:<palette>`）；未设置时按实例 ID 与加载器稳定分配 */
export function resolveDefaultCover(
  instance: Pick<InstanceSummary, "id" | "loader" | "coverPreset">,
): {
  emblem: CoverEmblemId;
  palette: CoverPalette;
  isCustomPreset: boolean;
} {
  if (instance.coverPreset) {
    const [rawEmblem, rawPalette] = instance.coverPreset.split(":");
    const emblem = COVER_EMBLEMS.find((e) => e === rawEmblem);
    const palette = COVER_PALETTES.find((p) => p.id === rawPalette);
    if (emblem && palette) {
      return { emblem, palette, isCustomPreset: true };
    }
  }
  const h = hashString(instance.id);
  const palette = COVER_PALETTES[h % COVER_PALETTES.length] ?? COVER_PALETTES[0];
  const loaderLower = instance.loader?.toLowerCase() ?? "";
  // 根据加载器给一个自然偏置池，同时仍按哈希分散，避免同加载器实例长得一模一样
  const pool: readonly CoverEmblemId[] =
    loaderLower.includes("neoforge") || loaderLower.includes("forge")
      ? ["anvil", "lantern", "crystal", "portal"]
      : loaderLower.includes("fabric") ||
          loaderLower.includes("quilt") ||
          loaderLower.includes("babric")
        ? ["scroll", "lantern", "compass", "book"]
        : ["grass", "lantern", "compass", "crystal"];
  const emblem = pool[(h >>> 4) % pool.length] ?? "lantern";
  return { emblem, palette, isCustomPreset: false };
}

/** 精致原创 48×48 等距体素/微光意象矢量徽章 */
export function CoverEmblem({
  emblem,
  accent,
  glow,
  className = "size-12",
}: {
  emblem: CoverEmblemId;
  accent: string;
  glow: string;
  className?: string;
}) {
  switch (emblem) {
    case "lantern":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <path d="M20 7h8v3h-8z" fill={accent} fillOpacity="0.85" />
          <path d="M22 4h4v3h-4z" fill={accent} fillOpacity="0.65" />
          <path d="M16 10h16l3 4H13l3-4z" fill={accent} fillOpacity="0.9" />
          <rect
            x="13"
            y="14"
            width="22"
            height="22"
            rx="2.5"
            fill="#0d1322"
            fillOpacity="0.55"
            stroke={accent}
            strokeWidth="2"
          />
          <rect x="17.5" y="18" width="13" height="14" rx="1.5" fill={glow} />
          <rect x="20.5" y="20.5" width="7" height="9" rx="1" fill="#fffbeb" fillOpacity="0.92" />
          <path d="M14 36h20l-2 4H16l-2-4z" fill={accent} fillOpacity="0.9" />
        </svg>
      );
    case "grass":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <polygon points="24,7 41,16 24,25 7,16" fill={glow} />
          <polygon points="24,7 41,16 24,25 7,16" fill="#ffffff" fillOpacity="0.22" />
          <polygon points="7,16 24,25 24,41 7,32" fill={accent} fillOpacity="0.52" />
          <polygon points="41,16 24,25 24,41 41,32" fill={accent} fillOpacity="0.32" />
          <path
            d="M7 16l17 9 17-9v6l-5 2-4-2-4 3-4-2-4 2-4-3-4 2-5-2v-6z"
            fill={glow}
            fillOpacity="0.9"
          />
          <path
            d="M24 7l17 9v16l-17 9-17-9V16l17-9z"
            stroke={accent}
            strokeOpacity="0.7"
            strokeWidth="1.5"
          />
        </svg>
      );
    case "compass":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <circle
            cx="24"
            cy="24"
            r="16"
            fill="#0d1322"
            fillOpacity="0.45"
            stroke={accent}
            strokeWidth="2"
          />
          <circle
            cx="24"
            cy="24"
            r="11.5"
            stroke={accent}
            strokeOpacity="0.35"
            strokeWidth="1.2"
            strokeDasharray="2 3"
          />
          <polygon points="24,11 28.5,24 24,21.5 19.5,24" fill={glow} />
          <polygon points="24,37 28.5,24 24,26.5 19.5,24" fill={accent} fillOpacity="0.55" />
          <circle cx="24" cy="24" r="2.5" fill="#ffffff" />
        </svg>
      );
    case "crystal":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <polygon points="24,6 34,20 24,42 14,20" fill={glow} fillOpacity="0.88" />
          <polygon points="24,6 34,20 24,24" fill="#ffffff" fillOpacity="0.42" />
          <polygon points="24,6 14,20 24,24" fill={accent} fillOpacity="0.5" />
          <polygon points="24,24 34,20 24,42" fill="#0b1020" fillOpacity="0.28" />
          <polygon points="36,12 40,18 36,26 32,18" fill={accent} fillOpacity="0.55" />
          <polygon points="12,15 15,20 12,27 9,20" fill={accent} fillOpacity="0.45" />
        </svg>
      );
    case "anvil":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <path d="M9 15h30l-3 7H12l-3-7z" fill={accent} fillOpacity="0.92" />
          <path d="M12 15h24v2.5H12z" fill="#ffffff" fillOpacity="0.35" />
          <path d="M18 22h12l2 7H16l2-7z" fill={accent} fillOpacity="0.6" />
          <path d="M11 29h26l3 7H8l3-7z" fill={glow} fillOpacity="0.85" />
          <path
            d="M13 31.5h22"
            stroke="#ffffff"
            strokeOpacity="0.35"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      );
    case "scroll":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <rect
            x="13"
            y="9"
            width="22"
            height="30"
            rx="3.5"
            fill={accent}
            fillOpacity="0.25"
            stroke={accent}
            strokeWidth="2"
          />
          <rect x="10" y="9" width="28" height="6" rx="3" fill={glow} fillOpacity="0.9" />
          <rect x="10" y="33" width="28" height="6" rx="3" fill={accent} fillOpacity="0.85" />
          <path
            d="M18 20h12M18 25h9"
            stroke="#ffffff"
            strokeOpacity="0.85"
            strokeWidth="2"
            strokeLinecap="round"
          />
        </svg>
      );
    case "book":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <path
            d="M11 11.5A3.5 3.5 0 0 1 14.5 8H36v29H14.5A3.5 3.5 0 0 0 11 40.5v-29z"
            fill={accent}
            fillOpacity="0.32"
            stroke={accent}
            strokeWidth="2"
          />
          <path
            d="M11 36.5A3.5 3.5 0 0 1 14.5 33H36v7H14.5A3.5 3.5 0 0 1 11 36.5z"
            fill={glow}
            fillOpacity="0.85"
          />
          <path
            d="M24 14l2 4.5 4.5 2-4.5 2-2 4.5-2-4.5-4.5-2 4.5-2 2-4.5z"
            fill="#fffbeb"
            fillOpacity="0.92"
          />
        </svg>
      );
    case "portal":
      return (
        <svg viewBox="0 0 48 48" fill="none" className={className} aria-hidden>
          <rect
            x="12"
            y="7"
            width="24"
            height="34"
            rx="3"
            fill="#0d1322"
            fillOpacity="0.6"
            stroke={accent}
            strokeWidth="3.5"
          />
          <rect x="16.5" y="11.5" width="15" height="25" rx="1.5" fill={glow} fillOpacity="0.82" />
          <path
            d="M20 17c4 2 4 6 0 8s-4 6 0 8M28 15c-4 2-4 6 0 8s4 6 0 8"
            stroke="#ffffff"
            strokeOpacity="0.65"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </svg>
      );
  }
}

/**
 * 全幅微光矢量默认封面（16:9 完整景深画布）：
 * - 当实例还没有高清游戏截图或自定义大图时展示，彻底取代死板的「纯色+单字」或「64×64 糊底邮票」；
 * - 若有世界小图标采样出的 `worldHue`，优先继承该世界的色相（保留「随世界变色」灵魂），否则使用实例预设/稳定色板。
 */
export function InstanceDefaultCover({
  instance,
  bare = false,
  compact = false,
  worldHue,
  className = "",
}: {
  instance: Pick<InstanceSummary, "id" | "loader" | "coverPreset">;
  /** 实例页横幅只画背景光场，不画居中徽章（徽章放左边头像位） */
  bare?: boolean;
  /** 列表小缩略图模式 */
  compact?: boolean;
  /** 若有世界 64×64 图标采样出的色相且用户没指定预设，继承世界主色相 */
  worldHue?: number | null;
  className?: string;
}) {
  const uid = useId();
  const { emblem, palette, isCustomPreset } = resolveDefaultCover(instance);

  const useWorldTint = !isCustomPreset && worldHue != null;
  const deep = useWorldTint ? `oklch(0.22 0.045 ${worldHue})` : palette.deep;
  const mid = useWorldTint ? `oklch(0.34 0.075 ${worldHue})` : palette.mid;
  const glow = useWorldTint ? `oklch(0.72 0.14 ${worldHue})` : palette.glow;
  const accent = useWorldTint ? `oklch(0.84 0.09 ${worldHue})` : palette.accent;

  return (
    <div
      className={`relative size-full overflow-hidden ${className}`}
      style={{
        background: `linear-gradient(135deg, ${deep} 0%, ${mid} 100%)`,
      }}
    >
      {/* 全幅矢量景深：微光地平线 + 几何体素轮廓 + 星点 */}
      <svg
        viewBox="0 0 360 202"
        preserveAspectRatio="xMidYMid slice"
        className="pointer-events-none absolute inset-0 size-full"
        aria-hidden
      >
        <defs>
          <radialGradient id={`glow-${uid}`} cx="50%" cy="46%" r="58%">
            <stop offset="0%" stopColor={glow} stopOpacity="0.42" />
            <stop offset="55%" stopColor={glow} stopOpacity="0.12" />
            <stop offset="100%" stopColor={glow} stopOpacity="0" />
          </radialGradient>
          <linearGradient id={`horizon-${uid}`} x1="0%" y1="0%" x2="0%" y2="100%">
            <stop offset="0%" stopColor={accent} stopOpacity="0.16" />
            <stop offset="100%" stopColor="#000000" stopOpacity="0.32" />
          </linearGradient>
          <pattern
            id={`grid-${uid}`}
            width="28"
            height="28"
            patternUnits="userSpaceOnUse"
            patternTransform="rotate(30)"
          >
            <path
              d="M 28 0 L 0 0 0 28"
              fill="none"
              stroke={accent}
              strokeOpacity="0.09"
              strokeWidth="1"
            />
          </pattern>
        </defs>

        <rect width="360" height="202" fill={`url(#grid-${uid})`} />
        <rect width="360" height="202" fill={`url(#glow-${uid})`} />

        {/* 远景像素/体素山脊剪影，让整张 16:9 画布完整饱满 */}
        <path
          d="M0 156 L42 156 L42 142 L78 142 L78 149 L116 149 L116 132 L152 132 L152 144 L214 144 L214 128 L256 128 L256 140 L304 140 L304 148 L360 148 L360 202 L0 202 Z"
          fill={`url(#horizon-${uid})`}
        />
        <path
          d="M0 174 L68 174 L68 164 L132 164 L132 170 L204 170 L204 158 L286 158 L286 168 L360 168 L360 202 L0 202 Z"
          fill="#000000"
          fillOpacity="0.2"
        />

        {/* 微光星点 */}
        <circle cx="56" cy="44" r="1.6" fill={accent} fillOpacity="0.55" />
        <circle cx="94" cy="72" r="1.1" fill={accent} fillOpacity="0.38" />
        <circle cx="272" cy="38" r="1.8" fill={accent} fillOpacity="0.6" />
        <circle cx="308" cy="68" r="1.2" fill={accent} fillOpacity="0.4" />
        <circle cx="240" cy="86" r="1" fill={accent} fillOpacity="0.35" />
      </svg>

      {!bare && (
        <div className="relative grid size-full place-items-center">
          <div className="drop-shadow-[0_6px_18px_rgba(0,0,0,0.38)] transition-transform duration-(--motion-enter) ease-out group-hover:scale-105">
            <CoverEmblem
              emblem={emblem}
              accent={accent}
              glow={glow}
              className={compact ? "size-8" : "size-14"}
            />
          </div>
        </div>
      )}
    </div>
  );
}
