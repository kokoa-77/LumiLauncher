import { useId, useSyncExternalStore } from "react";
import { resolveTheme, systemPrefersDark, watchSystemTheme } from "@/lib/theme";
import { useAppStore, useEffectsLevel } from "@/stores/app";

export type SceneKind = "home" | "find" | "account" | "look" | "ready" | "about";

export interface SceneArt {
  night: string;
  day?: string;
}

function HomeScene({ uid, dark }: { uid: string; dark: boolean }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 1200 750"
      preserveAspectRatio="xMidYMid slice"
      fill="none"
      className="size-full"
    >
      <defs>
        <linearGradient
          id={`${uid}-beam`}
          x1="1180"
          y1="-40"
          x2="540"
          y2="540"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="var(--accent)" stopOpacity={dark ? 0.24 : 0.16} />
          <stop offset="48%" stopColor="var(--art-front)" stopOpacity={dark ? 0.08 : 0.34} />
          <stop offset="100%" stopColor="var(--art-front)" stopOpacity="0" />
        </linearGradient>
        <radialGradient id={`${uid}-horizon`} cx="68%" cy="66%" r="44%">
          <stop offset="0%" stopColor="var(--glow)" stopOpacity={dark ? 0.9 : 0.75} />
          <stop offset="62%" stopColor="var(--art-fill-light)" stopOpacity={dark ? 0.2 : 0.35} />
          <stop offset="100%" stopColor="var(--art-fill-light)" stopOpacity="0" />
        </radialGradient>
      </defs>

      {/* 地平线柔光晕与右上斜落微光束（纯渐变零模糊） */}
      <ellipse cx="810" cy="490" rx="520" ry="240" fill={`url(#${uid}-horizon)`} />
      <polygon points="1220,-40 1220,210 560,640 410,490" fill={`url(#${uid}-beam)`} />
      <polygon
        points="1220,60 1220,175 740,540 660,460"
        fill="var(--art-front)"
        opacity={dark ? 0.045 : 0.22}
      />

      {/* 天际静谧星点与几何四角微光星 */}
      <g fill={dark ? "var(--mote)" : "var(--art-front)"} opacity={dark ? 0.65 : 0.72}>
        <circle cx="940" cy="118" r="2.2" />
        <circle cx="842" cy="166" r="1.6" />
        <circle cx="1064" cy="196" r="2" />
        <circle cx="718" cy="104" r="1.5" />
        <circle cx="290" cy="142" r="1.5" opacity={dark ? 0.5 : 0.45} />
        <circle cx="468" cy="96" r="1.8" opacity={dark ? 0.55 : 0.45} />
        <path
          d="M998 86 L1000.6 92.4 L1007 95 L1000.6 97.6 L998 104 L995.4 97.6 L989 95 L995.4 92.4 Z"
          opacity={dark ? 0.8 : 0.78}
        />
      </g>

      {/* 三层纯 2D 远近柔和山脊 */}
      <path
        d="M-20 518 C170 492, 340 498, 520 508 C710 518, 860 472, 1020 448 C1105 435, 1165 440, 1220 452 V760 H-20 Z"
        fill="var(--scene-hill-far)"
      />
      <path
        d="M-20 532 C190 486, 370 508, 570 526 C790 546, 985 480, 1220 442 V760 H-20 Z"
        fill="var(--scene-hill-mid)"
      />
      <path
        d="M-20 558 C230 504, 460 548, 725 538 C935 530, 1085 482, 1220 462 V760 H-20 Z"
        fill="var(--scene-hill-near)"
      />
      {/* 脊线极细柔光边 */}
      <path
        d="M-20 558 C230 504, 460 548, 725 538 C935 530, 1085 482, 1220 462"
        stroke="var(--art-stroke)"
        strokeWidth="1.2"
        opacity={dark ? 0.55 : 0.65}
      />
    </svg>
  );
}

function FindScene({ uid, dark }: { uid: string; dark: boolean }) {
  const houses = [
    { x: 64, y: 214, s: 1.0 },
    { x: 126, y: 252, s: 0.86 },
    { x: 186, y: 274, s: 0.94 },
    { x: 438, y: 266, s: 0.88 },
    { x: 518, y: 222, s: 0.96 },
  ];
  return (
    <svg
      aria-hidden
      viewBox="0 0 600 900"
      preserveAspectRatio="xMidYMid slice"
      fill="none"
      className="size-full"
    >
      <defs>
        <radialGradient id={`${uid}-valley`} cx="50%" cy="32%" r="48%">
          <stop offset="0%" stopColor="var(--glow)" stopOpacity={dark ? 0.55 : 0.45} />
          <stop offset="100%" stopColor="var(--glow)" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${uid}-lamp`} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="var(--scene-beacon)" stopOpacity="0.65" />
          <stop offset="100%" stopColor="var(--scene-beacon)" stopOpacity="0" />
        </radialGradient>
      </defs>

      <ellipse cx="300" cy="285" rx="280" ry="165" fill={`url(#${uid}-valley)`} />

      {/* 顶部星屑 */}
      <g fill={dark ? "var(--mote)" : "var(--art-front)"} opacity={dark ? 0.55 : 0.7}>
        <circle cx="118" cy="86" r="1.8" />
        <circle cx="244" cy="126" r="1.4" />
        <circle cx="446" cy="92" r="2" />
        <circle cx="516" cy="142" r="1.5" />
      </g>

      {/* 上部远山山谷 */}
      <path
        d="M-10 218 C115 232, 205 282, 300 288 C395 294, 485 240, 610 214 V900 H-10 Z"
        fill="var(--scene-hill-far)"
      />

      {/* 远山暖光小屋剪影（呼应「找到电脑里已有的世界」） */}
      {houses.map((h) => (
        <g key={`${h.x}-${h.y}`} transform={`translate(${h.x} ${h.y}) scale(${h.s})`}>
          <circle cx="0" cy="6" r="24" fill={`url(#${uid}-lamp)`} opacity={dark ? 0.72 : 0.48} />
          <path d="M-22 2 L0 -14 L22 2 Z" fill="var(--art-fill-mid)" opacity={dark ? 0.82 : 0.75} />
          <rect
            x="-16"
            y="1"
            width="32"
            height="19"
            rx="2.5"
            fill="var(--art-front)"
            stroke="var(--art-stroke)"
            strokeWidth="1"
          />
          <rect x="-7" y="6" width="5.5" height="6" rx="1.2" fill="var(--scene-beacon)" />
          <rect
            x="1.5"
            y="6"
            width="5.5"
            height="6"
            rx="1.2"
            fill="var(--scene-beacon)"
            opacity="0.88"
          />
        </g>
      ))}

      {/* 中部大面积柔和留白，让前景重叠卡片清晰通透 */}
      <path
        d="M-10 258 C120 278, 215 322, 308 320 C405 318, 495 276, 610 256 V900 H-10 Z"
        fill="var(--scene-hill-mid)"
      />
      <path d="M-10 832 C165 808, 325 846, 610 818 V910 H-10 Z" fill="var(--scene-hill-near)" />
      <path
        d="M-10 832 C165 808, 325 846, 610 818"
        stroke="var(--art-stroke)"
        strokeWidth="1.2"
        opacity="0.55"
      />
    </svg>
  );
}

function AccountScene({ uid, dark }: { uid: string; dark: boolean }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 600 900"
      preserveAspectRatio="xMidYMid slice"
      fill="none"
      className="size-full"
    >
      <defs>
        <radialGradient id={`${uid}-aura`} cx="50%" cy="48%" r="44%">
          <stop offset="0%" stopColor="var(--glow)" stopOpacity={dark ? 0.65 : 0.52} />
          <stop offset="65%" stopColor="var(--art-fill-light)" stopOpacity={dark ? 0.14 : 0.24} />
          <stop offset="100%" stopColor="var(--art-fill-light)" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${uid}-stage`} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="var(--art-front)" stopOpacity={dark ? 0.24 : 0.92} />
          <stop offset="55%" stopColor="var(--glow)" stopOpacity={dark ? 0.42 : 0.35} />
          <stop offset="100%" stopColor="var(--glow)" stopOpacity="0" />
        </radialGradient>
      </defs>

      {/* 角色背后的静谧星环微光 */}
      <circle cx="300" cy="430" r="245" fill={`url(#${uid}-aura)`} />
      <circle
        cx="300"
        cy="430"
        r="142"
        stroke="var(--art-stroke)"
        strokeWidth="1"
        strokeDasharray="4 6"
        opacity={dark ? 0.38 : 0.45}
      />
      <circle
        cx="300"
        cy="430"
        r="196"
        stroke="var(--art-stroke)"
        strokeWidth="0.9"
        opacity={dark ? 0.24 : 0.32}
      />

      {/* 左右地平线微星 */}
      <g fill={dark ? "var(--mote)" : "var(--art-front)"} opacity={dark ? 0.72 : 0.85}>
        <circle cx="48" cy="596" r="2.2" />
        <circle cx="556" cy="588" r="2.2" />
        <circle cx="132" cy="212" r="1.5" />
        <circle cx="478" cy="186" r="1.8" />
        <path
          d="M526 266 L528 271 L533 273 L528 275 L526 280 L524 275 L519 273 L524 271 Z"
          opacity={dark ? 0.75 : 0.65}
        />
      </g>

      {/* 远处平缓地平线与角色脚下柔光舞台 */}
      <path
        d="M-10 644 C140 634, 240 648, 345 642 C455 636, 535 640, 610 634 V910 H-10 Z"
        fill="var(--scene-hill-far)"
      />
      <path d="M-10 668 C165 654, 385 688, 610 658 V910 H-10 Z" fill="var(--scene-hill-mid)" />
      <ellipse cx="300" cy="726" rx="220" ry="62" fill={`url(#${uid}-stage)`} />
      <path d="M-10 784 C170 816, 390 778, 610 762 V910 H-10 Z" fill="var(--scene-hill-near)" />
      <path
        d="M-10 784 C170 816, 390 778, 610 762"
        stroke="var(--art-stroke)"
        strokeWidth="1.2"
        opacity="0.55"
      />
    </svg>
  );
}

function LookScene({ uid }: { uid: string }) {
  // O3 外观步骤同时展现「晴空白天 × 静谧夜空」斜向分界与微光星河带
  return (
    <svg
      aria-hidden
      viewBox="0 0 600 900"
      preserveAspectRatio="xMidYMid slice"
      fill="none"
      className="size-full"
    >
      <defs>
        <linearGradient
          id={`${uid}-day`}
          x1="0"
          y1="0"
          x2="460"
          y2="520"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="oklch(89% 0.055 232)" />
          <stop offset="55%" stopColor="oklch(95.5% 0.028 232)" />
          <stop offset="100%" stopColor="oklch(98% 0.012 232)" />
        </linearGradient>
        <linearGradient
          id={`${uid}-night`}
          x1="210"
          y1="240"
          x2="600"
          y2="900"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="oklch(28% 0.075 262)" />
          <stop offset="55%" stopColor="oklch(20% 0.052 268)" />
          <stop offset="100%" stopColor="oklch(14% 0.036 268)" />
        </linearGradient>
        <linearGradient
          id={`${uid}-ribbon`}
          x1="40"
          y1="730"
          x2="570"
          y2="95"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="oklch(98% 0.02 230)" stopOpacity="0.88" />
          <stop offset="48%" stopColor="var(--accent)" stopOpacity="0.46" />
          <stop offset="100%" stopColor="oklch(82% 0.1 245)" stopOpacity="0.22" />
        </linearGradient>
        <radialGradient id={`${uid}-orb-warm`} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="oklch(98% 0.06 85)" stopOpacity="0.95" />
          <stop offset="45%" stopColor="oklch(86% 0.13 80)" stopOpacity="0.45" />
          <stop offset="100%" stopColor="oklch(86% 0.13 80)" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${uid}-orb-cool`} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="oklch(99% 0.01 235)" stopOpacity="0.95" />
          <stop offset="50%" stopColor="var(--accent)" stopOpacity="0.4" />
          <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
        </radialGradient>
      </defs>

      {/* 白天底 */}
      <rect width="600" height="900" fill={`url(#${uid}-day)`} />
      {/* 左上晴空极简流云 */}
      <path
        d="M28 186 C78 168, 148 164, 212 146 C158 180, 96 196, 28 198 Z"
        fill="oklch(100% 0 0 / 0.52)"
      />
      <path
        d="M94 238 C146 226, 206 218, 258 202 C214 228, 152 246, 94 248 Z"
        fill="oklch(100% 0 0 / 0.36)"
      />

      {/* 右下斜切静谧夜空 */}
      <path
        d="M600 48 C488 188, 376 346, 248 492 C148 608, 66 710, -20 778 L-20 910 L610 910 Z"
        fill={`url(#${uid}-night)`}
      />

      {/* 斜向微光星河带（纯矢量分层光带） */}
      <path
        d="M-20 732 C108 620, 232 466, 360 310 C452 198, 526 112, 620 20 L620 118 C528 216, 446 318, 356 426 C234 574, 114 708, -20 822 Z"
        fill={`url(#${uid}-ribbon)`}
      />

      {/* 星河光珠 */}
      <circle cx="464" cy="176" r="24" fill={`url(#${uid}-orb-warm)`} />
      <circle cx="388" cy="274" r="32" fill={`url(#${uid}-orb-warm)`} />
      <circle cx="318" cy="342" r="20" fill={`url(#${uid}-orb-cool)`} />
      <circle cx="272" cy="414" r="22" fill={`url(#${uid}-orb-cool)`} />
      <circle cx="114" cy="586" r="24" fill={`url(#${uid}-orb-warm)`} />
      <circle cx="546" cy="796" r="24" fill={`url(#${uid}-orb-cool)`} />

      {/* 夜空星点 */}
      <g fill="oklch(97% 0.02 240)">
        <circle cx="496" cy="290" r="1.8" opacity="0.8" />
        <circle cx="536" cy="384" r="1.5" opacity="0.72" />
        <circle cx="436" cy="446" r="2" opacity="0.78" />
        <circle cx="510" cy="540" r="1.4" opacity="0.65" />
        <circle cx="368" cy="596" r="1.8" opacity="0.7" />
        <circle cx="466" cy="654" r="1.6" opacity="0.75" />
        <circle cx="372" cy="148" r="1.5" opacity="0.7" />
        <path
          d="M526 218 L528.2 223.8 L534 226 L528.2 228.2 L526 234 L523.8 228.2 L518 226 L523.8 223.8 Z"
          opacity="0.88"
        />
      </g>

      {/* 底部柔和晨昏地平线山脊 */}
      <path d="M-10 752 C145 776, 330 762, 610 732 V910 H-10 Z" fill="oklch(82% 0.04 248 / 0.22)" />
      <path
        d="M-10 766 C165 796, 385 758, 610 742 V910 H-10 Z"
        fill="var(--scene-hill-near)"
        opacity="0.72"
      />
    </svg>
  );
}

function ReadyScene({ uid, dark }: { uid: string; dark: boolean }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 600 900"
      preserveAspectRatio="xMidYMid slice"
      fill="none"
      className="size-full"
    >
      <defs>
        <radialGradient id={`${uid}-door-glow`} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="var(--scene-beacon)" stopOpacity={dark ? 0.92 : 0.85} />
          <stop offset="42%" stopColor="var(--accent)" stopOpacity={dark ? 0.35 : 0.25} />
          <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${uid}-sun`} cx="92%" cy="6%" r="36%">
          <stop offset="0%" stopColor="var(--art-front)" stopOpacity={dark ? 0.14 : 0.55} />
          <stop offset="100%" stopColor="var(--art-front)" stopOpacity="0" />
        </radialGradient>
      </defs>

      <rect width="600" height="900" fill={`url(#${uid}-sun)`} />

      {/* 远方山脊与山顶亮起的微光之门（呼应「一切就绪，马上踏入世界」） */}
      <circle cx="378" cy="176" r="78" fill={`url(#${uid}-door-glow)`} />
      <circle cx="378" cy="176" r="34" fill={`url(#${uid}-door-glow)`} />

      {/* 极简山顶发光拱门 */}
      <path
        d="M370 186 V171 A8 8 0 0 1 386 171 V186 Z"
        fill="var(--art-front)"
        stroke="var(--scene-beacon)"
        strokeWidth="1.4"
      />
      <path
        d="M373.5 186 V173 A4.5 4.5 0 0 1 382.5 173 V186 Z"
        fill="var(--scene-beacon)"
        opacity="0.92"
      />

      {/* 高地柔弧山丘 + 底部近景留白 */}
      <path
        d="M-10 226 C125 212, 245 186, 378 182 C475 179, 545 162, 610 166 V910 H-10 Z"
        fill="var(--scene-hill-mid)"
      />
      <path
        d="M-10 226 C125 212, 245 186, 378 182 C475 179, 545 162, 610 166"
        stroke="var(--art-stroke)"
        strokeWidth="1.2"
        opacity="0.55"
      />
      {/* 门前洒落的一条微光小径 */}
      <path
        d="M370 186 C354 202, 330 210, 304 216 C336 212, 366 206, 386 186 Z"
        fill="var(--scene-beacon)"
        opacity={dark ? 0.34 : 0.28}
      />

      {/* 细碎微星 */}
      <g fill={dark ? "var(--mote)" : "var(--art-front)"} opacity={dark ? 0.6 : 0.72}>
        <circle cx="96" cy="96" r="1.6" />
        <circle cx="212" cy="64" r="1.8" />
        <circle cx="486" cy="92" r="1.5" />
        <circle cx="524" cy="238" r="1.6" />
      </g>

      <path d="M-10 832 C165 802, 345 844, 610 818 V910 H-10 Z" fill="var(--scene-hill-near)" />
      <path
        d="M-10 832 C165 802, 345 844, 610 818"
        stroke="var(--art-stroke)"
        strokeWidth="1.2"
        opacity="0.55"
      />
    </svg>
  );
}

function AboutScene({ uid, dark }: { uid: string; dark: boolean }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 960 240"
      preserveAspectRatio="xMidYMid slice"
      fill="none"
      className="size-full"
    >
      <defs>
        <linearGradient
          id={`${uid}-aurora`}
          x1="80"
          y1="60"
          x2="880"
          y2="90"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="var(--accent)" stopOpacity="0" />
          <stop offset="28%" stopColor="var(--accent)" stopOpacity={dark ? 0.36 : 0.22} />
          <stop offset="56%" stopColor="var(--glow)" stopOpacity={dark ? 0.56 : 0.38} />
          <stop offset="82%" stopColor="var(--accent)" stopOpacity={dark ? 0.32 : 0.2} />
          <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
        </linearGradient>
      </defs>

      {/* 横向天际极光柔带 */}
      <path
        d="M-20 82 C190 42, 360 112, 560 72 C710 42, 830 88, 980 62 V136 C820 162, 700 114, 550 144 C350 184, 180 116, -20 152 Z"
        fill={`url(#${uid}-aurora)`}
      />

      {/* 远处静谧地平线 */}
      <path
        d="M-20 156 C220 132, 460 170, 720 144 C825 134, 905 140, 980 148 V250 H-20 Z"
        fill="var(--scene-hill-far)"
      />
      <path d="M-20 186 C260 162, 540 198, 980 166 V250 H-20 Z" fill="var(--scene-hill-mid)" />

      {/* 精致星点与微光四角星 */}
      <g fill={dark ? "var(--mote)" : "var(--accent)"} opacity={dark ? 0.78 : 0.45}>
        <circle cx="104" cy="56" r="1.8" />
        <circle cx="282" cy="64" r="1.5" />
        <circle cx="490" cy="58" r="1.8" />
        <circle cx="718" cy="66" r="1.6" />
        <circle cx="864" cy="48" r="2" />
        <path d="M622 36 L624 41.2 L629.2 43.2 L624 45.2 L622 50.4 L620 45.2 L614.8 43.2 L620 41.2 Z" />
      </g>
    </svg>
  );
}

/**
 * 纯 2D 柔光矢量场景背景：零位图模糊、零滤镜开销，随昼夜主题与实例主色实时呼吸。
 * 流畅模式默认沿用调用方的渐变底色，传入 `showInLite` 时仍展示轻盈矢量底景。
 */
export function SceneBackground({
  kind,
  art,
  className = "",
  showInLite = false,
}: {
  kind?: SceneKind;
  art?: SceneArt;
  className?: string;
  showInLite?: boolean;
}) {
  const uid = useId();
  const effects = useEffectsLevel();
  const mode = useAppStore((s) => s.settings?.appearance.theme ?? "system");
  const systemDark = useSyncExternalStore(watchSystemTheme, systemPrefersDark, () => false);
  const theme = resolveTheme(mode, systemDark);
  const dark = theme === "dark";

  if (effects === "lite" && !showInLite) return null;

  if (kind) {
    return (
      <div
        aria-hidden
        data-scene={kind}
        className={`pointer-events-none absolute inset-0 size-full overflow-hidden ${className}`}
      >
        {kind === "home" && <HomeScene uid={uid} dark={dark} />}
        {kind === "find" && <FindScene uid={uid} dark={dark} />}
        {kind === "account" && <AccountScene uid={uid} dark={dark} />}
        {kind === "look" && <LookScene uid={uid} />}
        {kind === "ready" && <ReadyScene uid={uid} dark={dark} />}
        {kind === "about" && <AboutScene uid={uid} dark={dark} />}
      </div>
    );
  }

  if (!art) return null;

  return (
    <img
      alt=""
      aria-hidden
      decoding="async"
      src={theme === "light" ? (art.day ?? art.night) : art.night}
      className={`pointer-events-none absolute inset-0 size-full object-cover ${className}`}
    />
  );
}
