// 主色的数学：OKLCH → sRGB → 亮度 → 对比度。
// 主色只由「色相 + 彩度」两个数决定，亮度按对比度自动算：主色上的字、浅色底上的主色字都要看得清。
// 以后「随实例变色」从截图取出任意颜色，也走这里，保证对比度合格。

/** 一个 OKLCH 颜色：l 0–1，c 0–0.4，h 0–360 度 */
export interface Oklch {
  l: number;
  c: number;
  h: number;
}

/** OKLCH → 线性 sRGB（每个通道 0–1，可能超出范围，表示 sRGB 显示不了） */
export function oklchToLinearSrgb({ l, c, h }: Oklch): [number, number, number] {
  const rad = (h * Math.PI) / 180;
  const a = c * Math.cos(rad);
  const b = c * Math.sin(rad);
  const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = l - 0.0894841775 * a - 1.291485548 * b;
  const L = l_ ** 3;
  const M = m_ ** 3;
  const S = s_ ** 3;
  return [
    4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S,
    -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S,
    -0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S,
  ];
}

const EPSILON = 1e-4;

export function inSrgbGamut(color: Oklch): boolean {
  return oklchToLinearSrgb(color).every((v) => v >= -EPSILON && v <= 1 + EPSILON);
}

/** 在这个亮度和色相下，sRGB 能显示的最大彩度（不超过 `wanted`） */
export function fitChroma(l: number, h: number, wanted: number): number {
  if (inSrgbGamut({ l, c: wanted, h })) return wanted;
  let lo = 0;
  let hi = wanted;
  for (let i = 0; i < 20; i += 1) {
    const mid = (lo + hi) / 2;
    if (inSrgbGamut({ l, c: mid, h })) lo = mid;
    else hi = mid;
  }
  return lo;
}

/** WCAG 相对亮度（0 黑 – 1 白） */
export function relativeLuminance(color: Oklch): number {
  const [r, g, b] = oklchToLinearSrgb(color).map((v) => Math.min(1, Math.max(0, v)));
  return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
}

/** WCAG 对比度（1–21） */
export function contrast(a: Oklch, b: Oklch): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** 正文要求的对比度（WCAG AA） */
export const MIN_CONTRAST = 4.5;

export const WHITE: Oklch = { l: 1, c: 0, h: 0 };
/** 浅色主题的底色（和 tokens.css 的 --bg 一致，色相不影响结果） */
const LIGHT_BG: Oklch = { l: 0.976, c: 0.004, h: 0 };
/** 深色主题的底色和主色上的深色字 */
const DARK_BG: Oklch = { l: 0.185, c: 0.008, h: 0 };
const darkOnAccent = (h: number): Oklch => ({ l: 0.2, c: 0.03, h });

export interface SolvedAccent {
  hue: number;
  /** 浅色主题的主色亮度（0–1）：白字在上面看得清，主色字在浅色底上也看得清 */
  lightL: number;
  /** 浅色主题实际用的彩度（这个亮度下 sRGB 显示不了时会降一点） */
  lightC: number;
  /** 深色主题的主色亮度：深色字在上面看得清，主色字在深色底上也看得清 */
  darkL: number;
  darkC: number;
  /** 两套主题里实际做到的最低对比度 */
  lightContrast: number;
  darkContrast: number;
}

const STEP = 0.005;

/** 由色相和彩度算出浅色、深色两套主色亮度。黄、青这类天生很亮的颜色会自动压暗（浅色主题）或提亮（深色主题）。 */
export function solveAccent(hue: number, chroma: number): SolvedAccent {
  const h = ((hue % 360) + 360) % 360;
  const at = (l: number): Oklch => ({ l, c: fitChroma(l, h, chroma), h });
  const lightScore = (l: number) => Math.min(contrast(at(l), WHITE), contrast(at(l), LIGHT_BG));
  const darkScore = (l: number) =>
    Math.min(contrast(at(l), darkOnAccent(h)), contrast(at(l), DARK_BG));

  let lightL = 0.58;
  while (lightScore(lightL) < MIN_CONTRAST && lightL > 0.3) lightL -= STEP;
  let darkL = 0.76;
  while (darkScore(darkL) < MIN_CONTRAST && darkL < 0.95) darkL += STEP;

  return {
    hue: h,
    lightL,
    lightC: fitChroma(lightL, h, chroma),
    darkL,
    darkC: fitChroma(darkL, h, chroma),
    lightContrast: lightScore(lightL),
    darkContrast: darkScore(darkL),
  };
}

/** sRGB（0–255）→ OKLCH */
export function srgbToOklch(r: number, g: number, b: number): Oklch {
  const lin = (v: number) => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const R = lin(r);
  const G = lin(g);
  const B = lin(b);
  const l_ = Math.cbrt(0.4122214708 * R + 0.5363325363 * G + 0.0514459929 * B);
  const m_ = Math.cbrt(0.2119034982 * R + 0.6806995451 * G + 0.1073969566 * B);
  const s_ = Math.cbrt(0.0883024619 * R + 0.2817188376 * G + 0.6299787005 * B);
  const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_;
  const A = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_;
  const Bb = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_;
  const h = (Math.atan2(Bb, A) * 180) / Math.PI;
  return { l: L, c: Math.hypot(A, Bb), h: (h + 360) % 360 };
}

export interface AccentColor {
  hue: number;
  chroma: number;
}

const HUE_BINS = 36;
/** 太暗、太亮、太灰的像素不算颜色 */
const MIN_L = 0.2;
const MAX_L = 0.97;
const MIN_C = 0.03;

/**
 * 从截图的像素（RGBA，通常是缩到 32×32 的）里挑主色：按色相分 36 格，彩度越高、亮度越适中的像素分量越重，
 * 取分量最大的那一格附近的平均色相。整张图几乎是灰的（夜里、山洞）就返回 null，用默认主色。
 */
export function pickAccent(pixels: ArrayLike<number>): AccentColor | null {
  const bins = new Array<number>(HUE_BINS).fill(0);
  const samples: { h: number; c: number; w: number }[] = [];
  let count = 0;
  let colorful = 0;
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    if ((pixels[i + 3] ?? 0) < 128) continue;
    count += 1;
    const { l, c, h } = srgbToOklch(pixels[i] ?? 0, pixels[i + 1] ?? 0, pixels[i + 2] ?? 0);
    if (l < MIN_L || l > MAX_L || c < MIN_C) continue;
    const w = c * (1 - Math.abs(l - 0.62));
    colorful += 1;
    const bin = Math.floor(h / (360 / HUE_BINS)) % HUE_BINS;
    bins[bin] = (bins[bin] ?? 0) + w;
    samples.push({ h, c, w });
  }
  if (count === 0 || colorful / count < 0.08) return null;

  let best = 0;
  for (let i = 1; i < HUE_BINS; i += 1) if ((bins[i] ?? 0) > (bins[best] ?? 0)) best = i;
  const center = (best + 0.5) * (360 / HUE_BINS);
  let x = 0;
  let y = 0;
  let cSum = 0;
  let wSum = 0;
  for (const s of samples) {
    const diff = Math.abs(((s.h - center + 540) % 360) - 180);
    if (diff > 20) continue;
    const rad = (s.h * Math.PI) / 180;
    x += Math.cos(rad) * s.w;
    y += Math.sin(rad) * s.w;
    cSum += s.c * s.w;
    wSum += s.w;
  }
  if (wSum === 0) return null;
  const hue = ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360;
  const chroma = Math.min(0.17, Math.max(0.07, (cSum / wSum) * 1.3));
  return { hue: Math.round(hue * 10) / 10, chroma: Math.round(chroma * 1000) / 1000 };
}
