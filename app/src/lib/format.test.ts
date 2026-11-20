import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const languageKey = "mcl.language";

function stubLanguage(lang: string) {
  const storage = new Map<string, string>([[languageKey, lang]]);
  vi.stubGlobal("navigator", { languages: [lang], language: lang });
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  });
}

beforeEach(() => vi.resetModules());
afterEach(() => vi.unstubAllGlobals());

describe("双语数量与日期格式化", () => {
  it("中文界面使用万/亿与中文月日格式", async () => {
    stubLanguage("zh-CN");
    const { formatCount, formatDateTime } = await import("./format");
    expect(formatCount(125_000)).toBe("12.5 万");
    expect(formatCount(120_000_000)).toBe("1.2 亿");
    expect(formatCount(980)).toBe("980");

    const sample = new Date(2026, 10, 15, 14, 3).getTime();
    expect(formatDateTime(sample)).toBe("11月15日 14:03");
  });

  it("英文界面使用 K/M/B 与英文日期格式，不出现中文字符", async () => {
    stubLanguage("en");
    const { formatCount, formatDateTime } = await import("./format");
    expect(formatCount(125_000)).toBe("125.0K");
    expect(formatCount(1_500_000)).toBe("1.5M");
    expect(formatCount(2_300_000_000)).toBe("2.3B");
    expect(formatCount(980)).toBe("980");

    const sample = new Date(2026, 10, 15, 14, 3).getTime();
    const formatted = formatDateTime(sample);
    expect(formatted).not.toMatch(/[\u4e00-\u9fff]/);
    expect(formatted).toContain("14:03");
  });
});
