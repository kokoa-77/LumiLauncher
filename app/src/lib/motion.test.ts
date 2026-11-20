import { describe, expect, it } from "vitest";
import { pageTransition, pageVariants, routeDepth, routeDirection } from "./motion";

describe("页面过渡方向与减少动效", () => {
  it("区分平级切页、下钻与返回", () => {
    expect(routeDepth("/")).toBe(0);
    expect(routeDepth("/instances")).toBe(1);
    expect(routeDepth("/instances/demo")).toBe(2);

    expect(routeDirection("/", "/instances")).toBe("peer");
    expect(routeDirection("/instances", "/discover")).toBe("peer");
    expect(routeDirection("/instances", "/instances/demo")).toBe("forward");
    expect(routeDirection("/instances/demo", "/instances")).toBe("back");
    expect(routeDirection("/discover", "/discover/modrinth/sodium")).toBe("forward");
    expect(routeDirection("/discover/modrinth/sodium", "/discover")).toBe("back");
  });

  it("在开启减少动效时清除全部位移", () => {
    for (const dir of ["peer", "forward", "back"] as const) {
      const transition = pageTransition(true, dir);
      expect(transition.initial.transform).toBe("none");
      expect(transition.exit.transform).toBe("none");
    }
  });

  it("精美模式下给出对应方向的合成器 transform", () => {
    expect(pageTransition(false, "peer").initial.transform).toBe("translateY(10px)");
    expect(pageTransition(false, "forward").initial.transform).toBe("translateX(14px)");
    expect(pageTransition(false, "back").initial.transform).toBe("translateX(-14px)");

    const variants = pageVariants(false);
    expect(variants.exit("forward").transform).toBe("translateX(-8px)");
    expect(variants.exit("back").transform).toBe("translateX(8px)");
    expect(variants.exit("peer").transform).toBe("none");
  });
});
