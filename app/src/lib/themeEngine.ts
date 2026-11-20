// 主题引擎：主题是数据，运行时写进 CSS 变量（tokens.css 里的默认值就是「底层外观」）。
// 组件只认语义类（bg-accent、rounded-panel……），换主题、随实例变色都只改变量，组件不用动。
import type { CSSProperties } from "react";
import { type SolvedAccent, solveAccent } from "./color";

export type RadiusKey = "row" | "control" | "panel" | "dialog" | "cta" | "icon" | "tag";

/** 一个主题能改的东西。没写的项保持底层默认值。以后的主题包（贴纸手帐、泡泡……）就是一份这样的数据。 */
export interface ThemeTokens {
  /** 主色：色相 0–360、彩度 0–0.3。亮度不用给，按对比度自动算 */
  accent?: { hue: number; chroma: number };
  /** 圆角（像素，999 表示胶囊 / 圆形） */
  radius?: Partial<Record<RadiusKey, number>>;
}

export const DEFAULT_ACCENT = { hue: 245, chroma: 0.15 } as const;

/** 圆角预设：「分级」是底层默认；另外两个给组件展示页对比用，也是主题包可以怎么改的例子 */
export const RADIUS_PRESETS = {
  tiered: { row: 8, control: 999, panel: 16, dialog: 20, cta: 999, icon: 999, tag: 999 },
  small: { row: 6, control: 8, panel: 10, dialog: 12, cta: 12, icon: 10, tag: 6 },
  round: { row: 999, control: 999, panel: 28, dialog: 32, cta: 999, icon: 999, tag: 999 },
} satisfies Record<string, Record<RadiusKey, number>>;

const root = () => document.documentElement.style;

const percent = (value: number) => `${Math.round(value * 1000) / 10}%`;

/** 现在写在 --accent-h 里的色相（可能超出 0–360：为了换色时走最短的那一圈） */
let currentHue: number = DEFAULT_ACCENT.hue;

/** 换主色：算出浅色、深色两套亮度和彩度，写进变量。返回算出来的结果（对比度等，展示页用）。
 *  精美模式下 tokens 里给这几个变量加了过渡，整套界面会平滑换色；色相走最短的那一圈（350° → 10° 不绕远路）。 */
export function applyAccent(hue: number, chroma: number): SolvedAccent {
  const solved = solveAccent(hue, chroma);
  const style = root();
  const delta = ((((solved.hue - currentHue) % 360) + 540) % 360) - 180;
  currentHue += delta;
  style.setProperty("--accent-h", String(Math.round(currentHue * 10) / 10));
  style.setProperty("--accent-l-light", percent(solved.lightL));
  style.setProperty("--accent-c-light", solved.lightC.toFixed(4));
  style.setProperty("--accent-l-dark", percent(solved.darkL));
  style.setProperty("--accent-c-dark", solved.darkC.toFixed(4));
  return solved;
}

export function applyRadius(radius: Partial<Record<RadiusKey, number>>): void {
  const style = root();
  for (const [key, px] of Object.entries(radius)) {
    style.setProperty(`--r-${key}`, `${px}px`);
  }
}

/** 套用一份主题数据（没写的项不动） */
export function applyThemeTokens(tokens: ThemeTokens): void {
  if (tokens.accent) applyAccent(tokens.accent.hue, tokens.accent.chroma);
  if (tokens.radius) applyRadius(tokens.radius);
}

const KEYS = [
  "--accent-h",
  "--accent-l-light",
  "--accent-c-light",
  "--accent-l-dark",
  "--accent-c-dark",
  ...Object.keys(RADIUS_PRESETS.tiered).map((key) => `--r-${key}`),
];

/** 回到底层外观（tokens.css 里的默认值） */
export function resetThemeTokens(): void {
  const style = root();
  for (const key of KEYS) style.removeProperty(key);
  currentHue = DEFAULT_ACCENT.hue;
}

/** 只给一小块区域换主色（实例卡片各用自己世界的颜色）：配合 `accent-scope` 类用 */
export function accentStyle(hue: number, chroma: number): CSSProperties {
  const solved = solveAccent(hue, chroma);
  return {
    "--accent-h": String(Math.round(solved.hue * 10) / 10),
    "--accent-l-light": percent(solved.lightL),
    "--accent-c-light": solved.lightC.toFixed(4),
    "--accent-l-dark": percent(solved.darkL),
    "--accent-c-dark": solved.darkC.toFixed(4),
  } as CSSProperties;
}
