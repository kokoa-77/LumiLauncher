import { describe, expect, it } from "vitest";
import {
  contrast,
  fitChroma,
  inSrgbGamut,
  MIN_CONTRAST,
  oklchToLinearSrgb,
  pickAccent,
  relativeLuminance,
  solveAccent,
  srgbToOklch,
  WHITE,
} from "./color";

describe("oklch 换算", () => {
  it("白和黑是 sRGB 的两端", () => {
    const white = oklchToLinearSrgb({ l: 1, c: 0, h: 0 });
    const black = oklchToLinearSrgb({ l: 0, c: 0, h: 0 });
    for (const v of white) expect(v).toBeCloseTo(1, 3);
    for (const v of black) expect(v).toBeCloseTo(0, 6);
    expect(relativeLuminance(WHITE)).toBeCloseTo(1, 3);
  });

  it("黑白对比度是 21", () => {
    expect(contrast(WHITE, { l: 0, c: 0, h: 0 })).toBeCloseTo(21, 1);
  });

  it("超出 sRGB 的彩度会被降到能显示的范围", () => {
    expect(inSrgbGamut({ l: 0.9, c: 0.3, h: 145 })).toBe(false);
    const c = fitChroma(0.9, 145, 0.3);
    expect(c).toBeLessThan(0.3);
    expect(inSrgbGamut({ l: 0.9, c, h: 145 })).toBe(true);
  });
});

describe("solveAccent", () => {
  it("任何色相，主色上的字和主色字的对比度都合格（浅色、深色两套）", () => {
    for (let hue = 0; hue < 360; hue += 15) {
      for (const chroma of [0.08, 0.15, 0.22]) {
        const solved = solveAccent(hue, chroma);
        expect(solved.lightContrast, `浅色 色相 ${hue} 彩度 ${chroma}`).toBeGreaterThanOrEqual(
          MIN_CONTRAST,
        );
        expect(solved.darkContrast, `深色 色相 ${hue} 彩度 ${chroma}`).toBeGreaterThanOrEqual(
          MIN_CONTRAST,
        );
      }
    }
  });

  it("天生很亮的黄色在浅色主题里会被压暗", () => {
    expect(solveAccent(100, 0.15).lightL).toBeLessThan(solveAccent(265, 0.15).lightL + 0.001);
    expect(solveAccent(100, 0.15).lightL).toBeLessThan(0.58);
  });

  it("默认的天空蓝和 tokens.css 里写的初始值一致", () => {
    const sky = solveAccent(245, 0.15);
    expect(Math.round(sky.lightL * 1000) / 10).toBe(54.5);
    expect(sky.lightC).toBeCloseTo(0.137, 3);
    expect(Math.round(sky.darkL * 1000) / 10).toBe(76);
    expect(sky.darkC).toBeCloseTo(0.132, 3);
  });

  it("浅色、深色各自用自己亮度下能显示的最大彩度", () => {
    const solved = solveAccent(235, 0.15);
    expect(solved.lightC).toBeLessThan(0.15);
    expect(solved.darkC).toBeLessThanOrEqual(0.15);
    expect(solved.lightC).not.toBeCloseTo(solved.darkC, 3);
  });

  it("色相超出 0–360 时按圆周折回", () => {
    expect(solveAccent(-115, 0.15).hue).toBe(245);
    expect(solveAccent(605, 0.15).hue).toBe(245);
  });
});

describe("从截图挑主色", () => {
  const fill = (rgb: [number, number, number], n: number) =>
    Array.from({ length: n }, () => [...rgb, 255]).flat();

  it("srgb → oklch 和 oklch → srgb 能来回", () => {
    const back = oklchToLinearSrgb(srgbToOklch(255, 255, 255));
    for (const v of back) expect(v).toBeCloseTo(1, 3);
    expect(srgbToOklch(0, 0, 0).l).toBeCloseTo(0, 5);
    // 天空蓝的色相在 230–250 之间
    const sky = srgbToOklch(110, 170, 230);
    expect(sky.h).toBeGreaterThan(230);
    expect(sky.h).toBeLessThan(260);
  });

  it("占多数的颜色胜出", () => {
    const sky = [110, 170, 230] as [number, number, number];
    const grass = [90, 150, 60] as [number, number, number];
    const picked = pickAccent([...fill(sky, 600), ...fill(grass, 300), ...fill([30, 30, 30], 100)]);
    expect(picked).not.toBeNull();
    expect(picked?.hue).toBeGreaterThan(230);
    expect(picked?.hue).toBeLessThan(260);
    expect(picked?.chroma).toBeGreaterThanOrEqual(0.07);
    expect(picked?.chroma).toBeLessThanOrEqual(0.17);
  });

  it("几乎全灰的截图（夜里、山洞）用默认主色", () => {
    expect(pickAccent([...fill([40, 40, 42], 900), ...fill([200, 60, 60], 20)])).toBeNull();
    expect(pickAccent([])).toBeNull();
  });

  it("红色跨过 0 度也能算对", () => {
    const picked = pickAccent([...fill([220, 60, 70], 500), ...fill([220, 60, 100], 500)]);
    expect(picked).not.toBeNull();
    const h = picked?.hue ?? 0;
    expect(h < 30 || h > 330).toBe(true);
  });
});
