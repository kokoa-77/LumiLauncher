import { afterEach, describe, expect, it, vi } from "vitest";
import { commands } from "@/lib/bindings";
import { useGameStore } from "./game";

afterEach(() => {
  vi.restoreAllMocks();
  useGameStore.setState(useGameStore.getInitialState());
});

describe("启动前预检与最近服务器加载", () => {
  it("预检发现问题时暂停启动并填充 preflightAsk", async () => {
    vi.spyOn(commands, "launchPreflight").mockResolvedValue({
      status: "ok",
      data: {
        checkedMods: 3,
        elapsedMs: 5,
        issues: [
          {
            kind: "missingDependency",
            modName: "Sodium",
            target: "fabric-api",
            detail: "sodium.jar",
          },
        ],
      },
    });
    const memorySpy = vi.spyOn(commands, "memoryCheck");
    const startSpy = vi.spyOn(commands, "launchStart");

    const started = await useGameStore.getState().launch("inst-1", undefined, "play.lumi.example");

    expect(started).toBe(false);
    expect(memorySpy).not.toHaveBeenCalled();
    expect(startSpy).not.toHaveBeenCalled();
    expect(useGameStore.getState().preflightAsk).toMatchObject({
      instanceId: "inst-1",
      server: "play.lumi.example",
      report: { checkedMods: 3 },
    });
  });

  it("选择仍然启动（skipPreflight）时跳过预检并继续启动", async () => {
    const preflightSpy = vi.spyOn(commands, "launchPreflight");
    vi.spyOn(commands, "memoryCheck").mockResolvedValue({
      status: "ok",
      data: null,
    });
    const startSpy = vi.spyOn(commands, "launchStart").mockResolvedValue({
      status: "ok",
      data: null,
    });

    const started = await useGameStore
      .getState()
      .launch("inst-1", undefined, "play.lumi.example", { skipPreflight: true });

    expect(started).toBe(true);
    expect(preflightSpy).not.toHaveBeenCalled();
    expect(startSpy).toHaveBeenCalledWith("inst-1", null, "play.lumi.example", null);
    expect(useGameStore.getState().preflightAsk).toBeNull();
  });

  it("预检接口自身异常时降级放行，不阻断启动", async () => {
    vi.spyOn(commands, "launchPreflight").mockResolvedValue({
      status: "error",
      error: { code: "io", message: "读取失败", detail: null },
    });
    vi.spyOn(commands, "memoryCheck").mockResolvedValue({ status: "ok", data: null });
    vi.spyOn(commands, "launchStart").mockResolvedValue({ status: "ok", data: null });
    vi.spyOn(commands, "appLog").mockResolvedValue(undefined);
    vi.spyOn(console, "error").mockImplementation(() => undefined);

    const started = await useGameStore.getState().launch("inst-1");

    expect(started).toBe(true);
    expect(useGameStore.getState().preflightAsk).toBeNull();
  });

  it("refreshRecentWorlds 并行拉取世界与服务器，单边失败互不影响", async () => {
    vi.spyOn(commands, "worldsRecent").mockResolvedValue({
      status: "ok",
      data: [],
    });
    vi.spyOn(commands, "worldsRecentServers").mockResolvedValue({
      status: "ok",
      data: [
        {
          instanceId: "inst-1",
          quickPlay: true,
          name: "微光服",
          address: "play.lumi.example",
        },
      ],
    });

    await useGameStore.getState().refreshRecentWorlds();

    expect(useGameStore.getState().recentWorlds).toEqual([]);
    expect(useGameStore.getState().recentServers).toHaveLength(1);
    expect(useGameStore.getState().recentServers?.[0]?.address).toBe("play.lumi.example");
  });
});
