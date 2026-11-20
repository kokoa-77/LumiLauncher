import { describe, expect, it } from "vitest";
import { isLowEnd, isSoftwareRenderer, resolveEffects } from "./effects";

const GPU = "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)";
const INTEL = "ANGLE (Intel, Intel(R) UHD Graphics 620 Direct3D11 vs_5_0 ps_5_0, D3D11)";

describe("画面效果的自动判断", () => {
  it("认得出软件绘制", () => {
    expect(isSoftwareRenderer("ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)))")).toBe(
      true,
    );
    expect(isSoftwareRenderer("ANGLE (Microsoft, Microsoft Basic Render Driver)")).toBe(true);
    expect(isSoftwareRenderer(null)).toBe(true);
    expect(isSoftwareRenderer(GPU)).toBe(false);
    expect(isSoftwareRenderer(INTEL)).toBe(false);
  });

  it("普通电脑用精美，老电脑用流畅", () => {
    expect(isLowEnd({ cores: 16, memoryMb: 32768, renderer: GPU })).toBe(false);
    expect(isLowEnd({ cores: 8, memoryMb: 8192, renderer: INTEL })).toBe(false);
    // 4 线程 + 8 GB：CPU 和内存都弱
    expect(isLowEnd({ cores: 4, memoryMb: 8192, renderer: INTEL })).toBe(true);
    // 4 GB 内存
    expect(isLowEnd({ cores: 8, memoryMb: 4096, renderer: INTEL })).toBe(true);
    // 显卡加速被关掉
    expect(isLowEnd({ cores: 16, memoryMb: 32768, renderer: null })).toBe(true);
    // 读不到内存时只看 CPU 和显卡
    expect(isLowEnd({ cores: 4, memoryMb: null, renderer: INTEL })).toBe(false);
  });

  it("手动选的优先", () => {
    expect(resolveEffects("rich", true)).toBe("rich");
    expect(resolveEffects("lite", false)).toBe("lite");
    expect(resolveEffects("auto", true)).toBe("lite");
    expect(resolveEffects("auto", false)).toBe("rich");
  });
});
