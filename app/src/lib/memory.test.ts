import { describe, expect, it } from "vitest";
import { barParts, gb, SLIDER_MIN_MB, sliderMax, snapToSlider } from "./memory";

describe("内存数字", () => {
  it("GB 保留一位小数，整数不带 .0", () => {
    expect(gb(4096)).toBe("4");
    expect(gb(6656)).toBe("6.5");
    expect(gb(32_563)).toBe("31.8");
    expect(gb(0)).toBe("0");
  });

  it("滑块最大值按电脑总内存取 512 的整数倍", () => {
    expect(sliderMax(32_563)).toBe(32_256);
    expect(sliderMax(8192)).toBe(8192);
    expect(sliderMax(600)).toBe(SLIDER_MIN_MB + 512);
    expect(sliderMax(200_000)).toBe(65_536);
  });

  it("任意值拉到滑块的格子上", () => {
    expect(snapToSlider(4000, 16_384)).toBe(4096);
    expect(snapToSlider(100, 16_384)).toBe(SLIDER_MIN_MB);
    expect(snapToSlider(99_999, 16_384)).toBe(16_384);
  });

  it("内存条：已用、游戏、剩余", () => {
    const parts = barParts(32_768, 20_480, 4096);
    expect(parts.used).toBeCloseTo(0.375);
    expect(parts.usedAndGame).toBeCloseTo(0.5);
    expect(parts.over).toBe(false);
    expect(parts.freeMb).toBe(16_384);
  });

  it("游戏要的比可用多时标出来、截到最右边", () => {
    const parts = barParts(16_384, 3072, 8192);
    expect(parts.over).toBe(true);
    expect(parts.usedAndGame).toBe(1);
    expect(parts.freeMb).toBe(0);
  });
});
