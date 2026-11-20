import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import en from "@/locales/en.json";
import zhCN from "@/locales/zh-CN.json";

const languageKey = "mcl.language";

function flattenLocale(
  obj: Record<string, unknown>,
  prefix = "",
  out: Record<string, string> = {},
) {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object" && !Array.isArray(v)) {
      flattenLocale(v as Record<string, unknown>, key, out);
    } else {
      out[key] = String(v);
    }
  }
  return out;
}

function extractPlaceholders(value: string): string[] {
  return [...value.matchAll(/\{\{\s*([^}\s,]+)/g)].map((m) => m[1] ?? "").sort();
}

function browser(languages: string[], saved?: string) {
  const storage = new Map<string, string>();
  if (saved !== undefined) storage.set(languageKey, saved);
  vi.stubGlobal("navigator", { languages, language: languages[0] });
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  });
  return storage;
}

beforeEach(() => vi.resetModules());
afterEach(() => vi.unstubAllGlobals());

describe("launcher language", () => {
  it("opens in English when the system language is unsupported", async () => {
    browser(["fr-FR"]);
    const { default: i18n } = await import("./i18n");
    expect(i18n.language).toBe("en");
  });

  it("opens in Simplified Chinese on a Chinese system before a preference is saved", async () => {
    browser(["zh-CN"]);
    const { default: i18n } = await import("./i18n");
    expect(i18n.language).toBe("zh-CN");
  });

  it("persists a manual choice and applies the system preference again when requested", async () => {
    const storage = browser(["en-US"], "zh-CN");
    const { default: i18n, setLanguagePreference } = await import("./i18n");

    expect(i18n.language).toBe("zh-CN");
    await setLanguagePreference("en");
    expect(storage.get(languageKey)).toBe("en");
    expect(i18n.language).toBe("en");

    vi.resetModules();
    vi.stubGlobal("navigator", { languages: ["zh-CN"] });
    const restarted = await import("./i18n");
    expect(restarted.default.language).toBe("en");
    await restarted.setLanguagePreference("system");
    expect(storage.get(languageKey)).toBe("system");
    expect(restarted.default.language).toBe("zh-CN");
  });

  it("treats invalid cached values and unsupported Chinese scripts as system fallback", async () => {
    browser(["zh-TW"], "invalid");
    const { default: i18n, getLanguagePreference } = await import("./i18n");
    expect(i18n.language).toBe("en");
    expect(getLanguagePreference()).toBe("system");
  });
});

describe("locale copy audit (D26)", () => {
  const flatZh = flattenLocale(zhCN as unknown as Record<string, unknown>);
  const flatEn = flattenLocale(en as unknown as Record<string, unknown>);
  const zhKeys = Object.keys(flatZh);

  it("defines every zh-CN key in en with non-empty values and matching placeholders", () => {
    const missingInEn = zhKeys.filter((k) => !(k in flatEn));
    expect(missingInEn).toEqual([]);

    for (const key of zhKeys) {
      const zhVal = flatZh[key] ?? "";
      const enVal = flatEn[key] ?? "";
      expect(zhVal.trim().length, `empty zh-CN key: ${key}`).toBeGreaterThan(0);
      expect(enVal.trim().length, `empty en key: ${key}`).toBeGreaterThan(0);
      expect(extractPlaceholders(zhVal), `placeholder mismatch at ${key}`).toEqual(
        extractPlaceholders(enVal),
      );
    }
  });

  it("keeps zh-CN punctuation tidy and avoids leaked Han characters in en", () => {
    const allowedHanInEn = new Set(["language.zh-CN", "settings.aboutPage.licenseBrandBody"]);
    for (const [key, val] of Object.entries(flatZh)) {
      expect(val, `ASCII ellipsis in ${key}`).not.toContain("...");
      expect(val, `leading/trailing whitespace in ${key}`).toBe(val.trim());
      expect(val, `duplicate punctuation in ${key}`).not.toMatch(/[。，、；：！？]{2}/);
    }
    for (const [key, val] of Object.entries(flatEn)) {
      expect(val, `leading/trailing whitespace in en:${key}`).toBe(val.trim());
      if (!allowedHanInEn.has(key)) {
        expect(val, `untranslated Chinese in en:${key}`).not.toMatch(/[\u4e00-\u9fff]/);
      }
    }
  });

  it("does not contain retired auth or pre-rename brand strings", () => {
    const forbidden = /littleskin|authlib-injector|皮肤站|外置登录|开发预览|unnamedlauncher/i;
    for (const [key, val] of Object.entries(flatZh)) {
      expect(val, `forbidden string in zh-CN:${key}`).not.toMatch(forbidden);
    }
    for (const [key, val] of Object.entries(flatEn)) {
      expect(val, `forbidden string in en:${key}`).not.toMatch(forbidden);
    }
  });
});
