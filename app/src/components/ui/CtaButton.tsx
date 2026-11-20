import { type ButtonHTMLAttributes, type ReactNode, useState } from "react";
import { playLumiSound } from "@/lib/sound";
import { LumiIcon } from "../art/LumiIcon";
import { Truncated } from "./HoverCard";

const burstAngles = [-155, -125, -95, -65, -35, -5];

type Size = "md" | "lg";
type Variant = "accent" | "neutral";

const sizes: Record<Size, { button: string; icon: string; label: string }> = {
  lg: { button: "h-16 gap-4 pr-11 pl-2.5", icon: "size-11", label: "text-xl" },
  md: { button: "h-12 gap-3 pr-7 pl-2", icon: "size-8", label: "text-base" },
};

const variants: Record<Variant, { button: string; icon: string }> = {
  accent: {
    button: "bg-accent text-on-accent hover:bg-accent-hover",
    icon: "bg-on-accent text-accent",
  },
  neutral: {
    button: "bg-surface text-fg hover:bg-surface-hover",
    icon: "bg-surface-hover text-fg",
  },
};

interface CtaButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> {
  label: string;
  /** 按钮里第二行的小字，比如「直接进入『新的世界』」 */
  sub?: string;
  /** 圆圈里的图标，默认是播放 */
  icon?: ReactNode;
  size?: Size;
  variant?: Variant;
  /** 直达世界时再散开六颗光点。 */
  burst?: boolean;
}

/**
 * 主按钮：一页只有一个（继续、开始游戏），胶囊形（圆角「主按钮」档），比别的按钮都大、都亮。
 * 三个参考的游戏启动器都是这样：右下角一个胶囊，旁边配一个圆形小按钮。
 */
export function CtaButton({
  label,
  sub,
  icon,
  size = "lg",
  variant = "accent",
  className = "",
  burst = false,
  onClick,
  ...props
}: CtaButtonProps) {
  const s = sizes[size];
  const v = variants[variant];
  const [press, setPress] = useState({ count: 0, burst: false });
  return (
    <button
      type="button"
      className={`lumi-cta relative isolate inline-flex shrink-0 items-center whitespace-nowrap rounded-cta font-semibold shadow-cta transition-[background-color,color,scale,opacity] duration-(--motion-press) ease-out active:scale-97 disabled:pointer-events-none disabled:opacity-45 ${s.button} ${v.button} ${className}`}
      {...props}
      onClick={(event) => {
        if (variant === "accent") {
          setPress((previous) => ({ count: previous.count + 1, burst }));
          playLumiSound("launch");
        }
        onClick?.(event);
      }}
    >
      {press.count > 0 && (
        <span
          key={press.count}
          aria-hidden
          className="lumi-cta-press pointer-events-none absolute inset-0"
        >
          <span className="lumi-cta-ring absolute inset-0 rounded-cta border border-accent" />
          {press.burst &&
            burstAngles.map((angle) => (
              <span
                key={angle}
                className="lumi-cta-ray absolute top-1/2 left-8"
                style={{ transform: `rotate(${angle}deg)` }}
              >
                <span className="lumi-cta-particle block size-2.5 rounded-full" />
              </span>
            ))}
        </span>
      )}
      <span className={`relative grid shrink-0 place-items-center rounded-cta ${s.icon} ${v.icon}`}>
        {variant === "accent" && (
          <span aria-hidden className="lumi-cta-breath absolute inset-0 rounded-cta bg-accent/20" />
        )}
        {icon ?? <LumiIcon name="play" className="lumi-cta-icon relative size-[55%]" />}
      </span>
      <span className="flex flex-col items-start leading-tight">
        <span className={`tracking-[0.14em] ${s.label}`}>{label}</span>
        {sub && (
          <Truncated
            className="max-w-64 truncate font-normal text-2xs tracking-normal opacity-85"
            text={sub}
          />
        )}
      </span>
    </button>
  );
}
