// 世界封面：按需要的宽度取缩小版的截图，从截图里挑主色（招牌 2「随实例变色」），
// 只有小图标的世界再做一张顺滑的颜色背景。
import { useEffect, useState } from "react";
import { useAppStore } from "@/stores/app";
import type { Settings } from "./bindings";
import { type AccentColor, pickAccent } from "./color";
import { logError } from "./report";
import { applyAccent, DEFAULT_ACCENT } from "./themeEngine";

/** 自己选的主色（设置「外观 → 主色」）；没选过是默认的天空蓝 */
export function ownAccent(settings: Settings | null | undefined): { hue: number; chroma: number } {
  return {
    hue: settings?.appearance.accentHue ?? DEFAULT_ACCENT.hue,
    chroma: settings?.appearance.accentChroma ?? DEFAULT_ACCENT.chroma,
  };
}

/** 宽度按 320 像素一档往上取，同一个窗口大小只会生成一张缓存 */
const STEP = 320;

/** 给启动器图片协议的地址加上宽度：后端把大图缩到这么宽（小图原样返回） */
export function imageAt(url: string, cssWidth: number): string {
  const px = Math.ceil((cssWidth * (window.devicePixelRatio || 1)) / STEP) * STEP;
  return `${url}${url.includes("?") ? "&" : "?"}w=${px}`;
}

/** 一张封面采样一次得到的两样东西 */
interface CoverSample {
  /** 主色；整张很灰、挑不出来时是 null */
  accent: AccentColor | null;
  /** 缩成 8×8 的颜色格（data URL）。放大后是一片顺滑的渐变，给只有小图标的世界当背景；做不出来时是 null */
  soft: string | null;
  /** 有一块地方是透明的（透明底的立绘）：后面不垫它自己的颜色格，不然会多出一个糊掉的影子 */
  transparent: boolean;
}

const cache = new Map<string, CoverSample>();
const pending = new Map<string, Promise<CoverSample>>();
const SAMPLE = 32;
const SOFT = 8;
const EMPTY: CoverSample = { accent: null, soft: null, transparent: false };
/** 透明的像素超过这么多算「透明底」 */
const TRANSPARENT_SHARE = 0.05;

function sample(url: string, idle: boolean): Promise<CoverSample> {
  const cached = cache.get(url);
  if (cached) return Promise.resolve(cached);
  const running = pending.get(url);
  if (running) return running;
  const task = new Promise<CoverSample>((resolve) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.decoding = "async";
    const readPixels = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = SAMPLE;
        canvas.height = SAMPLE;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        if (!ctx) return resolve(EMPTY);
        ctx.drawImage(img, 0, 0, SAMPLE, SAMPLE);
        const pixels = ctx.getImageData(0, 0, SAMPLE, SAMPLE).data;
        const accent = pickAccent(pixels);
        let clear = 0;
        for (let i = 3; i < pixels.length; i += 4) if ((pixels[i] ?? 255) < 128) clear += 1;
        const transparent = clear / (SAMPLE * SAMPLE) > TRANSPARENT_SHARE;
        // 再缩一次到 8×8：每一格是一小片的平均色，放大后没有马赛克的硬边
        const small = document.createElement("canvas");
        small.width = SOFT;
        small.height = SOFT;
        const smallCtx = small.getContext("2d");
        let soft: string | null = null;
        if (smallCtx) {
          smallCtx.imageSmoothingQuality = "high";
          smallCtx.drawImage(canvas, 0, 0, SOFT, SOFT);
          soft = small.toDataURL("image/png");
        }
        resolve({ accent, soft, transparent });
      } catch (error) {
        logError(error, "从截图取色");
        resolve(EMPTY);
      }
    };
    img.onload = () => {
      if (idle) requestIdleCallback(readPixels);
      else readPixels();
    };
    img.onerror = () => resolve(EMPTY);
    // 取色只要一张很小的图
    img.src = imageAt(url, 64);
  }).then((result) => {
    cache.set(url, result);
    pending.delete(url);
    return result;
  });
  pending.set(url, task);
  return task;
}

/** 采样结果：还在算时是 undefined；没有封面时是 null */
function useCoverSample(
  url: string | null | undefined,
  idle = false,
): CoverSample | null | undefined {
  const [result, setResult] = useState<CoverSample | null | undefined>(() =>
    url ? cache.get(url) : null,
  );
  useEffect(() => {
    if (!url) {
      setResult(null);
      return;
    }
    let cancelled = false;
    setResult(cache.get(url));
    void sample(url, idle).then((r) => {
      if (!cancelled) setResult(r);
    });
    return () => {
      cancelled = true;
    };
  }, [url, idle]);
  return result;
}

/** 截图的主色：还在算时是 undefined，算不出（整张很灰）是 null */
export function useCoverAccent(
  url: string | null | undefined,
  idle = false,
): AccentColor | null | undefined {
  const result = useCoverSample(url, idle);
  return result === undefined ? undefined : (result?.accent ?? null);
}

/**
 * 只有小图标的世界用的柔和背景（8×8 颜色格的 data URL）。`enabled` 为 false 时不算。
 * 和主色是同一次采样，不会多读一次图。
 */
export function useSoftCover(
  url: string | null | undefined,
  enabled: boolean,
  idle = false,
): string | null {
  const result = useCoverSample(enabled ? url : null, idle);
  return result?.soft ?? null;
}

/** 这张图是不是透明底（`enabled` 为 false 时不算，返回 false）；和主色是同一次采样 */
export function useCoverTransparent(
  url: string | null | undefined,
  enabled: boolean,
  idle = false,
): boolean {
  const result = useCoverSample(enabled ? url : null, idle);
  return result?.transparent ?? false;
}

/**
 * 整套界面换成这个世界的颜色（招牌 2）。算不出颜色（整张很灰、还没有世界）时用自己选的主色；还在算时不动。
 * 设置里关了「跟着世界变」时一直用自己选的主色。
 */
export function useWorldAccent(color: AccentColor | null | undefined): void {
  const settings = useAppStore((s) => s.settings);
  const follow = settings?.appearance.accentFollowWorld ?? true;
  const { hue: ownHue, chroma: ownChroma } = ownAccent(settings);
  useEffect(() => {
    if (!follow) {
      applyAccent(ownHue, ownChroma);
      return;
    }
    if (color === undefined) return;
    const { hue, chroma } = color ?? { hue: ownHue, chroma: ownChroma };
    applyAccent(hue, chroma);
  }, [color, follow, ownHue, ownChroma]);
}

/** 不在世界里的「画面」（首次运行引导）：用自己选的主色 */
export function useOwnAccent(): void {
  const settings = useAppStore((s) => s.settings);
  const { hue, chroma } = ownAccent(settings);
  useEffect(() => {
    applyAccent(hue, chroma);
  }, [hue, chroma]);
}
