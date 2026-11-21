import { motion } from "motion/react";
import { useTranslation } from "react-i18next";
import { barParts, gb } from "@/lib/memory";
import { spring } from "@/lib/motion";

interface MemoryBarProps {
  totalMb: number;
  /** 可用（和任务管理器的「可用」是同一个数） */
  availableMb: number;
  /** 这次给游戏多少 */
  gameMb: number;
  /** 自动模式：游戏那一段写「最多」 */
  gameIsMax?: boolean;
}

/**
 * 内存条：已用（灰）| 游戏分到（主色）| 剩余（空轨道），下面一行写数字。
 * 游戏要的比可用多时，那一段换成警告色、截到最右边。
 * 只动 transform（docs/DESIGN.md 第 6 节不许动宽度）：两段都铺满整条再按比例 scaleX，游戏那段平移到「已用」后面，两段不重叠。
 */
export function MemoryBar({ totalMb, availableMb, gameMb, gameIsMax = false }: MemoryBarProps) {
  const { t } = useTranslation();
  const parts = barParts(totalMb, availableMb, gameMb);
  return (
    <div className="flex flex-col gap-2">
      <div
        role="img"
        aria-label={t("memory.bar.label", {
          used: gb(parts.usedMb),
          game: gb(gameMb),
          free: gb(parts.freeMb),
          total: gb(totalMb),
        })}
        className="relative h-2.5 overflow-hidden rounded-control bg-surface-hover"
      >
        <motion.div
          className={`absolute inset-0 origin-left ${parts.over ? "bg-warning" : "bg-accent"}`}
          initial={false}
          animate={{ x: `${parts.used * 100}%`, scaleX: parts.usedAndGame - parts.used }}
          transition={spring.gentle}
        />
        <motion.div
          className="absolute inset-0 origin-left bg-fg-muted/45"
          initial={false}
          animate={{ scaleX: parts.used }}
          transition={spring.gentle}
        />
      </div>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-fg-muted text-xs">
        <Legend dot="bg-fg-muted/45" text={t("memory.bar.used", { gb: gb(parts.usedMb) })} />
        <Legend
          dot={parts.over ? "bg-warning" : "bg-accent"}
          text={t(gameIsMax ? "memory.bar.gameMax" : "memory.bar.game", { gb: gb(gameMb) })}
        />
        <Legend
          dot="border border-line bg-surface-hover"
          text={t("memory.bar.free", { gb: gb(parts.freeMb) })}
        />
        <span className="ml-auto tabular-nums">{t("memory.bar.total", { gb: gb(totalMb) })}</span>
      </div>
    </div>
  );
}

function Legend({ dot, text }: { dot: string; text: string }) {
  return (
    <span className="inline-flex items-center gap-1.5 tabular-nums">
      <span className={`size-2 rounded-tag ${dot}`} aria-hidden />
      {text}
    </span>
  );
}
