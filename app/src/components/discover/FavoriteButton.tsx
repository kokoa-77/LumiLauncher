import { Heart } from "lucide-react";
import { motion } from "motion/react";
import { useTranslation } from "react-i18next";
import { FloatingTooltip, useHover } from "@/components/ui/HoverCard";
import type { FavoriteKind } from "@/lib/bindings";
import type { Entry } from "@/lib/discover";
import { spring } from "@/lib/motion";
import { useFavorites, useIsFavorite } from "@/stores/favorites";

/** 收藏按钮：空心 ♡ / 实心 ♥，点下去心形弹一下 */
export function FavoriteButton({
  entry,
  kind,
  tone = "default",
}: {
  entry: Entry;
  kind: FavoriteKind;
  /** 压在截图上（详情页横幅）：和横幅上别的圆钮一样，半透明底 + 细线 */
  tone?: "default" | "hero";
}) {
  const { t } = useTranslation();
  const toggle = useFavorites((s) => s.toggle);
  const on = useIsFavorite(entry.hit, entry.ids);
  const label = on ? t("discover.unfavorite") : t("discover.favorite");
  const { ref, anchor, handlers } = useHover<HTMLButtonElement>();
  return (
    <>
      <button
        ref={ref}
        type="button"
        aria-label={label}
        aria-pressed={on}
        onClick={(event) => {
          event.stopPropagation();
          void toggle(kind, entry.hit);
        }}
        {...handlers}
        className={`grid size-9 shrink-0 place-items-center rounded-icon transition-colors duration-(--motion-press) ${
          tone === "hero"
            ? `bg-hero-chip ring-1 ring-hero-chip-line ring-inset hover:bg-hero-chip-hover ${on ? "text-accent" : "text-on-hero"}`
            : `hover:bg-surface-hover ${on ? "text-accent" : "text-fg-muted hover:text-fg"}`
        }`}
      >
        <motion.span
          key={on ? "on" : "off"}
          initial={on ? { scale: 0.6 } : false}
          animate={{ scale: 1, transition: spring.snappy }}
          className="grid place-items-center"
        >
          <Heart className={`size-4 ${on ? "fill-current" : ""}`} aria-hidden />
        </motion.span>
      </button>
      <FloatingTooltip anchor={anchor} label={label} />
    </>
  );
}
