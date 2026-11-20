import type { ReactNode } from "react";

export type LumiIconName = "home" | "library" | "discover" | "add" | "play" | "settings";

const shapes: Record<LumiIconName, { outline: ReactNode; dot: [number, number] }> = {
  home: {
    outline: <path d="M5 14v5a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-5a7 7 0 0 0-14 0Z" />,
    dot: [12, 14],
  },
  library: {
    outline: (
      <path d="M7 7H5a1 1 0 0 0-1 1v11m4-15H6m5 0h7a2 2 0 0 1 2 2v13a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V7a3 3 0 0 1 3-3Z" />
    ),
    dot: [15, 9],
  },
  discover: {
    outline: <path d="M12 3c1 5 4 8 9 9-5 1-8 4-9 9-1-5-4-8-9-9 5-1 8-4 9-9Z" />,
    dot: [12, 12],
  },
  add: { outline: <path d="M12 4v5m0 6v5M4 12h5m6 0h5" />, dot: [12, 12] },
  play: {
    outline: <path d="M14 9 7.5 5.3A1.5 1.5 0 0 0 5 6.6v10.8a1.5 1.5 0 0 0 2.5 1.3L14 15" />,
    dot: [17.5, 12],
  },
  settings: {
    outline: (
      <path d="m10 4-.7 2.3-2 .9-2.1-.8-1.9 3.2L5 11.1v2l-1.7 1.5 1.9 3.2 2.1-.8 2 .9L10 20h4l.7-2.1 2-.9 2.1.8 1.9-3.2-1.7-1.5v-2l1.7-1.5-1.9-3.2-2.1.8-2-.9L14 4Z" />
    ),
    dot: [12, 12],
  },
};

/** 微光的常用入口图标。24 像素网格和 2 像素圆头线与 lucide 保持一致。 */
export function LumiIcon({
  name,
  active = false,
  className = "",
}: {
  name: LumiIconName;
  active?: boolean;
  className?: string;
}) {
  const { outline, dot } = shapes[name];
  return (
    <svg
      aria-hidden
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      {outline}
      <circle
        cx={dot[0]}
        cy={dot[1]}
        r="1.5"
        stroke="none"
        fill="currentColor"
        className={active ? "lumi-icon-lit text-accent" : undefined}
      />
    </svg>
  );
}
