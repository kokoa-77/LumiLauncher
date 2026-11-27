import { describe, expect, it } from "vitest";
import type { TransferMetrics } from "@/lib/bindings";
import { seedDownloadMetrics, useDownloadMetricsStore } from "@/stores/downloadMetrics";

function makeSnapshot(overrides: Partial<TransferMetrics> = {}): TransferMetrics {
  return {
    seq: 1,
    timestampMs: 1_700_000_000_000,
    speedBytesPerSec: 0,
    routeKind: "direct",
    routeLabel: "直连",
    proxyEpoch: 1,
    protocol: "idle",
    activeConnections: 0,
    maxConnections: 16,
    activeH2Streams: 0,
    activeH1Segments: 0,
    memBufferUsedBytes: 0,
    diskKind: "ssd",
    diskWritersActive: 0,
    diskWritersLimit: 12,
    diskQueueDepth: 0,
    diskWriteEwmaMs: 1,
    reusedBytes: 0,
    reusedFiles: 0,
    workSteals: 0,
    fallbacks: 0,
    metered: false,
    meteredThrottled: false,
    effectiveSpeedLimitKib: 0,
    recentEvents: [],
    activeDiagnosis: null,
    dohSummary: null,
    ...overrides,
  };
}

describe("downloadMetrics store", () => {
  it("refreshes snapshot on repeated seedDownloadMetrics calls even after a snapshot already exists", () => {
    const stale = makeSnapshot({
      seq: 10,
      effectiveSpeedLimitKib: 5120,
      activeDiagnosis: {
        cause: "rateLimited429",
        summary: "触发限流",
        explanation: "请求过多",
        host: "cdn.modrinth.com",
        primaryFix: "lowerConcurrencyAndRetry",
        secondaryFix: null,
      },
    });
    seedDownloadMetrics(stale);
    expect(useDownloadMetricsStore.getState().current?.effectiveSpeedLimitKib).toBe(5120);
    expect(useDownloadMetricsStore.getState().current?.activeDiagnosis).not.toBeNull();

    const updated = makeSnapshot({
      seq: 11,
      effectiveSpeedLimitKib: 0,
      activeDiagnosis: null,
      routeKind: "systemProxy",
      routeLabel: "系统代理",
    });
    seedDownloadMetrics(updated);

    const current = useDownloadMetricsStore.getState().current;
    expect(current?.seq).toBe(11);
    expect(current?.effectiveSpeedLimitKib).toBe(0);
    expect(current?.activeDiagnosis).toBeNull();
    expect(current?.routeKind).toBe("systemProxy");
  });
});
