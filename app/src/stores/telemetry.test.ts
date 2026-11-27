import { describe, expect, it } from "vitest";
import type { TransferTelemetry } from "@/lib/bindings";
import { seedTelemetry, useTelemetryStore } from "@/stores/telemetry";

function makeSnapshot(overrides: Partial<TransferTelemetry> = {}): TransferTelemetry {
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

describe("telemetry store", () => {
  it("refreshes snapshot on repeated seedTelemetry calls even after a snapshot already exists", () => {
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
    seedTelemetry(stale);
    expect(useTelemetryStore.getState().current?.effectiveSpeedLimitKib).toBe(5120);
    expect(useTelemetryStore.getState().current?.activeDiagnosis).not.toBeNull();

    const updated = makeSnapshot({
      seq: 11,
      effectiveSpeedLimitKib: 0,
      activeDiagnosis: null,
      routeKind: "systemProxy",
      routeLabel: "系统代理",
    });
    seedTelemetry(updated);

    const current = useTelemetryStore.getState().current;
    expect(current?.seq).toBe(11);
    expect(current?.effectiveSpeedLimitKib).toBe(0);
    expect(current?.activeDiagnosis).toBeNull();
    expect(current?.routeKind).toBe("systemProxy");
  });
});
