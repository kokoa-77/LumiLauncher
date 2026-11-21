/** 内存条、滑块、启动前检查用的小工具 */

/** 滑块最小 1 GB、每格 512 MB */
export const SLIDER_MIN_MB = 1024;
export const SLIDER_STEP_MB = 512;
/** 和后端设置的上限一致 */
const SETTINGS_MAX_MB = 65536;

/** 4096 → "4"，6656 → "6.5"，1500 → "1.5"（保留一位小数，整数不带 .0） */
export function gb(mb: number): string {
  const value = Math.round((mb / 1024) * 10) / 10;
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

/** 滑块最大值：电脑总内存向下取到 512 MB 的整数倍（至少比最小值大一格） */
export function sliderMax(totalMb: number): number {
  const max = Math.floor(totalMb / SLIDER_STEP_MB) * SLIDER_STEP_MB;
  return Math.min(SETTINGS_MAX_MB, Math.max(SLIDER_MIN_MB + SLIDER_STEP_MB, max));
}

/** 把任意值拉到滑块的格子上 */
export function snapToSlider(mb: number, totalMb: number): number {
  const snapped = Math.round(mb / SLIDER_STEP_MB) * SLIDER_STEP_MB;
  return Math.min(sliderMax(totalMb), Math.max(SLIDER_MIN_MB, snapped));
}

/** 内存条三段的比例（0–1）。`game` 超过可用时 `over` 为真，那一段画成警告色、截到最右边 */
export function barParts(totalMb: number, availableMb: number, gameMb: number) {
  const total = Math.max(1, totalMb);
  const used = Math.min(total, Math.max(0, total - availableMb));
  const over = gameMb > availableMb;
  return {
    used: used / total,
    /** 已用 + 游戏 一共到哪 */
    usedAndGame: Math.min(total, used + gameMb) / total,
    over,
    usedMb: used,
    freeMb: Math.max(0, availableMb - gameMb),
  };
}
