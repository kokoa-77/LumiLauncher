import { create } from "zustand";
import { events, type TransferMetrics } from "@/lib/bindings";

const MAX_SAMPLES = 60;

interface DownloadMetricsStore {
  current: TransferMetrics | null;
  samples: number[];
  peakSpeed: number;
}

const initialSamples = () => new Array<number>(MAX_SAMPLES).fill(0);

export const useDownloadMetricsStore = create<DownloadMetricsStore>(() => ({
  current: null,
  samples: initialSamples(),
  peakSpeed: 0,
}));

// 模块级环形缓冲与 rAF 帧合并状态，避免 50 MB/s 突发时每帧多次触发 React 重绘
let ring = initialSamples();
let ringPeak = 0;
let latestFrame: TransferMetrics | null = null;
let rafId: number | null = null;
let lastActiveEventAt = 0;

function pushSample(speedBytesPerSec: number) {
  const safe = Number.isFinite(speedBytesPerSec) && speedBytesPerSec > 0 ? speedBytesPerSec : 0;
  ring = [...ring.slice(1), safe];
  if (safe > ringPeak) {
    ringPeak = safe;
  } else if (safe === 0 && ring.every((v) => v === 0)) {
    ringPeak = 0;
  }
}

function flushToStore() {
  rafId = null;
  if (!latestFrame) return;
  const frame = latestFrame;
  useDownloadMetricsStore.setState({
    current: frame,
    samples: ring,
    peakSpeed: ringPeak,
  });
}

function scheduleFlush() {
  if (typeof document !== "undefined" && document.hidden) {
    return;
  }
  if (rafId !== null) return;
  if (typeof requestAnimationFrame === "function") {
    rafId = requestAnimationFrame(flushToStore);
  } else {
    flushToStore();
  }
}

/** 用主动查询的运行时快照刷新状态（每次打开驾驶舱同步最新限速、路由与诊断）。 */
export function seedDownloadMetrics(frame: TransferMetrics): void {
  latestFrame = frame;
  useDownloadMetricsStore.setState({ current: frame });
}

/** 订阅后端 10 Hz `TransferMetricsUpdated` 事件（空闲 0 Hz，页面隐藏跳过 DOM 重绘）。 */
export async function initDownloadMetricsEvents(): Promise<() => void> {
  const onVisibility = () => {
    if (typeof document !== "undefined" && !document.hidden && latestFrame) {
      scheduleFlush();
    }
  };
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibility);
  }

  const unlisten = await events.transferMetricsUpdated.listen(({ payload }) => {
    const isActivePayload =
      (payload.speedBytesPerSec ?? 0) > 0 || (payload.activeConnections ?? 0) > 0;
    const isNonIdlePulse = payload.protocol !== "idle" || isActivePayload;
    const now = Date.now();
    if (isActivePayload && lastActiveEventAt > 0 && now - lastActiveEventAt > 2000) {
      ring = initialSamples();
      ringPeak = 0;
    }
    if (isNonIdlePulse) {
      lastActiveEventAt = now;
    }
    latestFrame = payload;
    pushSample(payload.speedBytesPerSec ?? 0);
    scheduleFlush();
  });

  return () => {
    unlisten();
    if (typeof document !== "undefined") {
      document.removeEventListener("visibilitychange", onVisibility);
    }
    if (rafId !== null && typeof cancelAnimationFrame === "function") {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  };
}
