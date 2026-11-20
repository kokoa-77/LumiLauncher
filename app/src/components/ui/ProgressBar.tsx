import { motion } from "motion/react";
import { spring } from "@/lib/motion";

/** 进度为 0–1，未知总量时传入 null。精美模式用微光表示仍在传输。 */
export function ProgressBar({ value, label }: { value: number | null; label: string }) {
  const clamped = Math.min(1, Math.max(0, Number.isFinite(value) ? (value ?? 0) : 0));
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value === null ? undefined : Math.round(clamped * 100)}
      className="relative h-1.5 overflow-hidden rounded-full bg-surface-hover"
    >
      <motion.div
        className="h-full origin-left rounded-full bg-accent"
        initial={false}
        animate={{ scaleX: clamped }}
        transition={spring.gentle}
      />
      {(value === null || (clamped > 0 && clamped < 1)) && (
        <span
          aria-hidden
          className="pointer-events-none absolute inset-0 overflow-hidden rounded-full"
          style={value === null ? undefined : { clipPath: `inset(0 ${(1 - clamped) * 100}% 0 0)` }}
        >
          <span className="lumi-progress-shimmer absolute inset-y-0 left-0 w-1/3 bg-linear-to-r from-transparent via-mote/60 to-transparent" />
        </span>
      )}
    </div>
  );
}
