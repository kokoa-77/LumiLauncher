import type { TFunction } from "i18next";
import { describe, expect, it } from "vitest";
import type { LaunchRecord, LogLine } from "./bindings";
import {
  countLevels,
  filterLines,
  type LevelGroup,
  lineText,
  memoryText,
  recordSeconds,
  recordTime,
} from "./records";

// 把键和参数拼成字符串，测的是「选了哪个说法、给了什么参数」
const t = ((key: string, params?: Record<string, unknown>) =>
  `${key.split(".").pop()}:${JSON.stringify(params ?? {})}`) as unknown as TFunction;

const line = (
  level: LogLine["level"],
  message: string,
  thread: string | null = "main",
): LogLine => ({ level, thread, message });

describe("启动记录的输出", () => {
  const lines = [
    line("info", "Setting user: Steve", "Render thread"),
    line("warn", "Missing sound"),
    line("error", "java.lang.RuntimeException: boom"),
    line("info", "Loading Create"),
  ];

  const none = new Set<LevelGroup>();

  it("按下一种只看那一种，按下几种一起看", () => {
    const levels = (...groups: LevelGroup[]) =>
      filterLines(lines, "", new Set(groups)).map((l) => l.level);
    expect(levels("error")).toEqual(["error"]);
    expect(levels("warn", "error")).toEqual(["warn", "error"]);
    expect(levels("info")).toEqual(["info", "info"]);
    expect(filterLines([line("fatal", "x"), line("debug", "y")], "", new Set(["error"]))).toEqual([
      line("fatal", "x"),
    ]);
  });

  it("每种各有几行", () => {
    expect(countLevels(lines)).toEqual({ error: 1, warn: 1, info: 2 });
  });

  it("搜索内容和线程名，不分大小写", () => {
    expect(filterLines(lines, "CREATE", none)).toHaveLength(1);
    expect(filterLines(lines, "render", none)).toHaveLength(1);
    expect(filterLines(lines, "boom", new Set(["warn", "error"]))).toHaveLength(1);
  });

  it("没有条件时原样返回（不复制数组）", () => {
    expect(filterLines(lines, " ", none)).toBe(lines);
  });

  it("复制出去带线程和级别", () => {
    expect(lineText(line("warn", "x", "Server thread"))).toBe("[Server thread/WARN] x");
    expect(lineText(line("info", "y", null))).toBe("[INFO] y");
  });
});

describe("启动记录的时间", () => {
  const now = new Date(2026, 10, 12, 18, 0);
  const at = (...parts: [number, number, number, number, number]) => new Date(...parts).getTime();

  it("今天、昨天、今年、往年", () => {
    expect(recordTime(at(2026, 10, 12, 9, 5), t, now)).toBe('today:{"time":"09:05"}');
    expect(recordTime(at(2026, 10, 11, 23, 59), t, now)).toBe('yesterday:{"time":"23:59"}');
    expect(recordTime(at(2026, 0, 3, 7, 0), t, now)).toBe(
      'date:{"month":1,"day":3,"time":"07:00"}',
    );
    expect(recordTime(at(2025, 11, 31, 20, 0), t, now)).toBe(
      'fullDate:{"year":2025,"month":12,"day":31,"time":"20:00"}',
    );
  });

  it("玩了多久：运行中算到现在，没结束时间的算 0", () => {
    const base = { startedAt: 1_000, endedAt: 61_000, outcome: "exited" } as LaunchRecord;
    expect(recordSeconds(base)).toBe(60);
    expect(recordSeconds({ ...base, endedAt: null, outcome: "running" }, 31_000)).toBe(30);
    expect(recordSeconds({ ...base, endedAt: null, outcome: "interrupted" })).toBe(0);
  });

  it("内存写成 GB", () => {
    expect(memoryText(6144)).toBe("6 GB");
    expect(memoryText(3584)).toBe("3.5 GB");
  });
});
