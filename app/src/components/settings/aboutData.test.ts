import { describe, expect, it } from "vitest";
import en from "@/locales/en.json";
import zhCN from "@/locales/zh-CN.json";
import { APP_LICENSE, PROJECT_LINKS, SERVICE_CREDITS, THIRD_PARTY_ENTRIES } from "./aboutData";

describe("about page compliance data", () => {
  it("uses HTTPS for all external project and service links", () => {
    for (const url of Object.values(PROJECT_LINKS)) {
      expect(url.startsWith("https://")).toBe(true);
    }
    for (const credit of SERVICE_CREDITS) {
      expect(credit.url.startsWith("https://")).toBe(true);
    }
  });

  it("includes required community service credits", () => {
    const ids = SERVICE_CREDITS.map((c) => c.id);
    expect(ids).toEqual(["bmclapi", "mcim", "modrinth", "curseforge"]);
    expect(APP_LICENSE.spdx).toBe("GPL-3.0-only");
  });

  it("keeps third-party notices bilingual and free of retired auth descriptions", () => {
    expect(THIRD_PARTY_ENTRIES.length).toBeGreaterThanOrEqual(20);
    for (const entry of THIRD_PARTY_ENTRIES) {
      expect(entry.name.trim().length).toBeGreaterThan(0);
      expect(entry.license.trim().length).toBeGreaterThan(0);
      expect(entry.usage.zh.trim().length).toBeGreaterThan(0);
      expect(entry.usage.en.trim().length).toBeGreaterThan(0);
      const combined = `${entry.name} ${entry.usage.zh} ${entry.usage.en}`.toLowerCase();
      expect(combined).not.toContain("littleskin");
      expect(combined).not.toContain("authlib-injector");
    }
  });

  it("defines matching Simplified Chinese and English keys for settings.aboutPage", () => {
    const zhKeys = Object.keys(zhCN.settings.aboutPage).sort();
    const enKeys = Object.keys(en.settings.aboutPage).sort();
    expect(zhKeys).toEqual(enKeys);
    expect(zhKeys.length).toBeGreaterThan(15);
  });
});
