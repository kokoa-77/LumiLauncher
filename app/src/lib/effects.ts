// 画面效果：自动 / 流畅 / 精美。
// 「自动」在老电脑上选流畅：没有显卡加速（网页内核退回软件绘制）、内存很小、或者 CPU 和内存都弱。
// 流畅模式：过渡变短，位移、缩放类动画关掉只留淡入淡出；以后首页的封面也不铺满整窗。
import type { EffectsMode } from "./bindings";

export type EffectsLevel = "rich" | "lite";

export interface Hardware {
  /** 逻辑处理器个数 */
  cores: number;
  /** 电脑总内存（MB）；读不到为 null */
  memoryMb: number | null;
  /** 网页内核报告的显卡名字；拿不到（没有 WebGL）为 null */
  renderer: string | null;
}

/** 这些名字表示没有显卡加速，界面靠 CPU 画 */
const SOFTWARE_RENDERERS = ["swiftshader", "basic render", "llvmpipe", "software", "softpipe"];

export function isSoftwareRenderer(renderer: string | null): boolean {
  if (renderer === null) return true;
  const name = renderer.toLowerCase();
  return SOFTWARE_RENDERERS.some((s) => name.includes(s));
}

/** 判断是不是低配电脑（「自动」时用） */
export function isLowEnd({ cores, memoryMb, renderer }: Hardware): boolean {
  if (isSoftwareRenderer(renderer)) return true;
  if (memoryMb !== null && memoryMb < 6 * 1024) return true;
  return cores <= 4 && memoryMb !== null && memoryMb <= 8 * 1024;
}

export function resolveEffects(mode: EffectsMode, lowEnd: boolean): EffectsLevel {
  if (mode === "lite") return "lite";
  if (mode === "rich") return "rich";
  return lowEnd ? "lite" : "rich";
}

/** 问网页内核用的是哪块显卡（只读一次名字，马上释放）。没有 WebGL 时返回 null */
export function probeRenderer(): string | null {
  try {
    const canvas = document.createElement("canvas");
    const gl = canvas.getContext("webgl");
    if (!gl) return null;
    const info = gl.getExtension("WEBGL_debug_renderer_info");
    const name = String(
      info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER),
    );
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    return name;
  } catch {
    return null;
  }
}

const CACHE_KEY = "mcl.lowEnd";

/** 上次检测的结果（启动时先用它，免得首页先按精美画、过一会儿又变流畅） */
export function cachedLowEnd(): boolean | null {
  try {
    const value = localStorage.getItem(CACHE_KEY);
    return value === null ? null : value === "1";
  } catch {
    return null;
  }
}

export function rememberLowEnd(lowEnd: boolean): void {
  try {
    localStorage.setItem(CACHE_KEY, lowEnd ? "1" : "0");
  } catch {
    // 存不了就每次重新检测，不影响使用
  }
}

export function applyEffects(level: EffectsLevel): void {
  document.documentElement.dataset.fx = level;
}
