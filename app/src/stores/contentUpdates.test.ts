import { beforeEach, describe, expect, it, vi } from "vitest";
import { commands, type UpdateCheckResult } from "@/lib/bindings";
import { updateCacheKey, useContentUpdatesStore } from "@/stores/contentUpdates";
import { useGameStore } from "@/stores/game";

const sampleResult = (count: number): UpdateCheckResult => ({
  updates: Array.from({ length: count }, (_, i) => ({
    kind: "mod",
    fileName: `mod-${i}.jar`,
    title: `Mod ${i}`,
    currentVersion: "1.0.0",
    versionId: `v${i + 2}`,
    versionNumber: "2.0.0",
    source: "modrinth",
    projectId: `proj-${i}`,
    delivery: "direct",
    pageUrl: null,
  })),
  checked: count,
  unchecked: 0,
  lookupFailed: false,
});

describe("useContentUpdatesStore", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useContentUpdatesStore.getState().clear();
    useGameStore.setState({ contentVersion: 0 });
  });

  it("reuses fresh cached results across Home and ContentTab without duplicate IPC calls", async () => {
    const spy = vi
      .spyOn(commands, "contentCheckUpdates")
      .mockResolvedValue({ status: "ok", data: sampleResult(2) });

    const first = await useContentUpdatesStore.getState().check("inst-a", "mod");
    const second = await useContentUpdatesStore.getState().check("inst-a", "mod");

    expect(spy).toHaveBeenCalledTimes(1);
    expect(first?.updates).toHaveLength(2);
    expect(second?.updates).toHaveLength(2);
    expect(
      useContentUpdatesStore.getState().entries[updateCacheKey("inst-a", "mod")]?.result.updates,
    ).toHaveLength(2);
  });

  it("deduplicates concurrent in-flight checks for the same instance and kind", async () => {
    let resolveIpc!: (value: { status: "ok"; data: UpdateCheckResult }) => void;
    const spy = vi.spyOn(commands, "contentCheckUpdates").mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveIpc = resolve;
        }),
    );

    const p1 = useContentUpdatesStore.getState().check("inst-a", "mod");
    const p2 = useContentUpdatesStore.getState().check("inst-a", "mod");
    resolveIpc({ status: "ok", data: sampleResult(1) });

    const [r1, r2] = await Promise.all([p1, p2]);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(r1?.updates).toHaveLength(1);
    expect(r2?.updates).toHaveLength(1);
  });

  it("forces a fresh lookup when force=true or after contentVersion bumps", async () => {
    const spy = vi
      .spyOn(commands, "contentCheckUpdates")
      .mockResolvedValueOnce({ status: "ok", data: sampleResult(3) })
      .mockResolvedValueOnce({ status: "ok", data: sampleResult(1) })
      .mockResolvedValueOnce({ status: "ok", data: sampleResult(0) });

    await useContentUpdatesStore.getState().check("inst-a", "mod");
    const forced = await useContentUpdatesStore.getState().check("inst-a", "mod", { force: true });
    expect(forced?.updates).toHaveLength(1);

    useGameStore.getState().bumpContent();
    const afterBump = await useContentUpdatesStore.getState().check("inst-a", "mod");
    expect(afterBump?.updates).toHaveLength(0);
    expect(spy).toHaveBeenCalledTimes(3);
  });

  it("invalidates only the target instance when invalidate(id) is called", async () => {
    vi.spyOn(commands, "contentCheckUpdates").mockResolvedValue({
      status: "ok",
      data: sampleResult(1),
    });

    await useContentUpdatesStore.getState().check("inst-a", "mod");
    await useContentUpdatesStore.getState().check("inst-b", "mod");

    useContentUpdatesStore.getState().invalidate("inst-a");
    expect(
      useContentUpdatesStore.getState().entries[updateCacheKey("inst-a", "mod")],
    ).toBeUndefined();
    expect(
      useContentUpdatesStore.getState().entries[updateCacheKey("inst-b", "mod")],
    ).toBeDefined();
  });
});
