// 「日志」页签用的小工具：启动记录的时间说法、结局的颜色、在输出里搜索和按错误 / 警告 / 信息筛选。
import type { TFunction } from "i18next";
import type { LaunchOutcome, LaunchRecord, LogLine } from "./bindings";

/** 筛选用的三种：错误（含 fatal）、警告、信息（含 debug、trace） */
export type LevelGroup = "error" | "warn" | "info";
export const LEVEL_GROUPS: LevelGroup[] = ["error", "warn", "info"];

export function levelGroup(level: LogLine["level"]): LevelGroup {
  if (level === "error" || level === "fatal") return "error";
  if (level === "warn") return "warn";
  return "info";
}

/** 按下了哪几种；一个都没按是全都看 */
export type LevelFilter = ReadonlySet<LevelGroup>;

/** 每种各有几行 */
export function countLevels(lines: LogLine[]): Record<LevelGroup, number> {
  const counts = { error: 0, warn: 0, info: 0 };
  for (const line of lines) counts[levelGroup(line.level)] += 1;
  return counts;
}

/** 在输出里搜索（不分大小写，线程名也算）、只看按下的那几种 */
export function filterLines(lines: LogLine[], query: string, levels: LevelFilter): LogLine[] {
  const q = query.trim().toLowerCase();
  if (!q && levels.size === 0) return lines;
  return lines.filter(
    (line) =>
      (levels.size === 0 || levels.has(levelGroup(line.level))) &&
      (!q ||
        line.message.toLowerCase().includes(q) ||
        (line.thread ?? "").toLowerCase().includes(q)),
  );
}

/** 复制出去的一行：`[线程/级别] 内容`，和游戏自己的 latest.log 差不多，发给别人看得懂 */
export function lineText(line: LogLine): string {
  const level = line.level.toUpperCase();
  return line.thread ? `[${line.thread}/${level}] ${line.message}` : `[${level}] ${line.message}`;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 「今天 15:30」「昨天 20:12」「11月10日 20:12」，不是今年的带上年份 */
export function recordTime(ms: number, t: TFunction, now = new Date()): string {
  const at = new Date(ms);
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const day = 24 * 60 * 60 * 1000;
  if (ms >= startOfToday) return t("detail.logTab.today", { time });
  if (ms >= startOfToday - day) return t("detail.logTab.yesterday", { time });
  if (at.getFullYear() === now.getFullYear())
    return t("detail.logTab.date", { month: at.getMonth() + 1, day: at.getDate(), time });
  return t("detail.logTab.fullDate", {
    year: at.getFullYear(),
    month: at.getMonth() + 1,
    day: at.getDate(),
    time,
  });
}

/** 这一次玩了多久（秒）；还在运行时算到现在 */
export function recordSeconds(record: LaunchRecord, now = Date.now()): number {
  const start = record.startedAt ?? now;
  const end = record.endedAt ?? (record.outcome === "running" ? now : start);
  return Math.max(0, Math.round((end - start) / 1000));
}

/** 结局标签的颜色：崩溃、没启动起来是红色（危险），运行中是主色，其余是灰色（只是信息） */
export const outcomeTone: Record<LaunchOutcome, "neutral" | "accent" | "danger"> = {
  running: "accent",
  exited: "neutral",
  crashed: "danger",
  stopped: "neutral",
  failed: "danger",
  interrupted: "neutral",
};

/** 内存：6144 → 「6 GB」，3584 → 「3.5 GB」 */
export function memoryText(mb: number): string {
  const gb = mb / 1024;
  return `${Number.isInteger(gb) ? gb : gb.toFixed(1)} GB`;
}
