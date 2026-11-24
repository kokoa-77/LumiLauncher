import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { commands, type JobSnapshot, type RecentWorld } from "@/lib/bindings";
import { setLanguagePreference } from "@/lib/i18n";
import { ONBOARDING_STEPS, useAccountStore } from "@/stores/accounts";
import { useGameStore } from "@/stores/game";
import { createFindSelection, runFindSelection } from "./findGames";
import { FindStep } from "./steps";
import type { Found } from "./useFound";

beforeAll(async () => {
  await setLanguagePreference("zh-CN");
});

const root: Found["roots"][number] = {
  id: "root",
  path: "example",
  name: "已有的游戏目录",
  launcher: "official",
  versions: 1,
  missing: false,
  hidden: 0,
};
const imports = ["will-import", "will-fail"].map((id) => ({
  id,
  name: id,
  launcher: "prism" as const,
  gameVersion: "1.21.1",
  loader: null,
  supported: true,
}));
const found: Found = { loading: false, linked: [], roots: [root], foreign: imports };

function setJob(
  id: string,
  state: JobSnapshot["state"],
  instanceId: string | null,
  error: string | null = null,
) {
  useGameStore.setState(({ jobs }) => ({
    jobs: {
      ...jobs,
      [id]: {
        id,
        state,
        instanceId,
        error,
        key: `instance-create:import:${id}`,
        title: id,
        progress: { done: 0, total: null, unit: "bytes", message: null },
        startedAt: 0,
        finishedAt: state === "running" ? null : 1,
        source: "local",
        iconUrl: null,
        steps: [],
        speed: null,
        eta: null,
        retryable: false,
        fallbacks: 0,
      },
    },
  }));
}

beforeEach(() => {
  vi.spyOn(commands, "instancesList").mockResolvedValue([]);
  vi.spyOn(commands, "worldsRecent").mockResolvedValue({ status: "ok", data: [] });
  vi.spyOn(commands, "worldsRecentServers").mockResolvedValue({ status: "ok", data: [] });
});
afterEach(() => {
  useGameStore.setState(useGameStore.getInitialState());
  vi.restoreAllMocks();
});

it("从独立语言页开始，登录在导入之前，重新引导时回到语言页", async () => {
  const initial = useAccountStore.getState();
  vi.spyOn(commands, "accountsRestartOnboarding").mockResolvedValue({ status: "ok", data: null });
  try {
    expect(ONBOARDING_STEPS).toEqual(["language", "account", "find", "look", "ready"]);
    expect(initial.onboardingStep).toBe("language");
    initial.setOnboardingStep("look");
    await initial.restartOnboarding();
    expect(useAccountStore.getState().onboardingStep).toBe("language");
    expect(useAccountStore.getState().onboardingReached).toBe(0);
  } finally {
    useAccountStore.setState(initial, true);
  }
});

describe("首次引导找游戏", () => {
  it("接入本机目录后更新首页存档，较早的空扫描无法覆盖新结果", async () => {
    const recentlyLinked: RecentWorld = {
      instanceId: "linked-instance",
      quickPlay: true,
      world: {
        folder: "my-world",
        name: "保留的世界",
        lastPlayed: 120,
        day: 136,
        gameMode: "survival",
        hardcore: false,
        version: null,
        playSeconds: null,
        imageUrl: null,
        imageKind: null,
        imageFit: "cover",
        seed: null,
        note: null,
      },
    };
    vi.spyOn(commands, "linkedRootsAdd").mockResolvedValue({ status: "ok", data: root });
    let finishStartupScan = () => {};
    vi.mocked(commands.worldsRecent)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishStartupScan = () => resolve({ status: "ok", data: [] });
          }),
      )
      .mockResolvedValue({ status: "ok", data: [recentlyLinked] });

    const startupScan = useGameStore.getState().refreshRecentWorlds();
    try {
      await runFindSelection({ ...found, foreign: [] }, createFindSelection());
      expect(useGameStore.getState().recentWorlds).toEqual([recentlyLinked]);
    } finally {
      finishStartupScan();
      await startupScan;
    }
    expect(useGameStore.getState().recentWorlds).toEqual([recentlyLinked]);
  });
  it("扫描时提供显式跳过按钮", () => {
    const markup = renderToStaticMarkup(
      <FindStep
        found={{ ...found, loading: true, roots: [], foreign: [] }}
        addFolder={() => ({ roots: [], foreign: [] })}
        selection={createFindSelection()}
        setSelection={() => undefined}
        onNext={() => undefined}
        onBack={() => undefined}
      />,
    );
    expect(markup).toContain("先跳过查找");
    expect(markup).not.toContain("下一步");
  });

  it("未处理失败项时不能用已解锁的标题栏胶囊跳过", () => {
    const initial = useAccountStore.getState();
    try {
      useAccountStore.setState({
        onboardingStep: "find",
        onboardingReached: 2,
        onboardingFindBlocked: true,
      });
      initial.setOnboardingStep("account");
      expect(useAccountStore.getState().onboardingStep).toBe("find");
      useAccountStore.setState({ onboardingFindBlocked: false });
      initial.setOnboardingStep("account");
      expect(useAccountStore.getState().onboardingStep).toBe("account");
    } finally {
      useAccountStore.setState(initial, true);
    }
  });

  it("等待实际复制，部分失败后只重试失败项", async () => {
    const link = vi
      .spyOn(commands, "linkedRootsAdd")
      .mockResolvedValue({ status: "ok", data: root });
    const foreignImport = vi
      .spyOn(commands, "foreignImport")
      .mockImplementation(async (id) => ({ status: "ok", data: id }));
    const selection = {
      ...createFindSelection(),
      imports: new Set(imports.map((item) => item.id)),
    };
    const first = runFindSelection(found, selection);

    setJob("will-import", "running", null);
    await vi.waitFor(() => expect(foreignImport).toHaveBeenCalledWith("will-import"));
    expect(foreignImport).not.toHaveBeenCalledWith("will-fail");
    setJob("will-import", "running", "registered");
    await vi.waitFor(() => expect(foreignImport).toHaveBeenCalledWith("will-fail"));
    setJob("will-fail", "failed", null, "复制失败");
    const partial = await first;

    expect(partial.retry.map((item) => item.id)).toEqual(["will-fail"]);
    expect(Object.values(partial.outcomes).map((result) => result.error)).toEqual([
      null,
      null,
      "复制失败",
    ]);
    const markup = renderToStaticMarkup(
      <FindStep
        found={found}
        addFolder={() => ({ roots: [], foreign: [] })}
        selection={partial}
        setSelection={() => undefined}
        onNext={() => undefined}
        onBack={() => undefined}
      />,
    );
    expect(markup).toContain("已完成 2 项，失败 1 项");
    expect(markup).toContain("重试失败项");
    expect(markup).toContain("仍然继续");

    // 新任务的编号不同。旧的失败快照必须不能让新任务提前结束。
    foreignImport.mockResolvedValueOnce({ status: "ok", data: "retry-job" });
    setJob("retry-job", "succeeded", "registered-on-retry");
    const retried = await runFindSelection(found, partial);
    expect(retried.retry).toEqual([]);
    expect(Object.values(retried.outcomes).every((result) => result.error === null)).toBe(true);
    expect(link).toHaveBeenCalledTimes(1);
    expect(foreignImport.mock.calls).toEqual([["will-import"], ["will-fail"], ["will-fail"]]);

    // 返回找游戏后，已完成的保持锁定，原来的选中项仍保留。
    expect(retried.imports).toEqual(selection.imports);
    await runFindSelection(found, retried);
    expect(link).toHaveBeenCalledTimes(1);
    expect(foreignImport).toHaveBeenCalledTimes(3);
  });

  it("安装阶段失败时保留已登记的实例，避免重试时复制第二份", async () => {
    vi.spyOn(commands, "foreignImport").mockResolvedValue({ status: "ok", data: "setup-job" });
    setJob("setup-job", "failed", "registered", "本体没装完");
    const result = await runFindSelection(found, {
      ...createFindSelection(),
      excludedLinks: new Set([root.id]),
      imports: new Set(["will-import"]),
    });
    expect(result.retry).toEqual([]);
    expect(result.outcomes["import:will-import"]?.error).toBeNull();
  });

  it("错过任务事件时从后端快照确认导入结果", async () => {
    vi.useFakeTimers();
    try {
      vi.spyOn(commands, "foreignImport").mockResolvedValue({ status: "ok", data: "snapshot-job" });
      setJob("snapshot-job", "succeeded", "registered");
      vi.spyOn(commands, "jobsList").mockResolvedValue(Object.values(useGameStore.getState().jobs));
      useGameStore.setState(useGameStore.getInitialState());

      const run = runFindSelection(found, {
        ...createFindSelection(),
        excludedLinks: new Set([root.id]),
        imports: new Set(["will-import"]),
      });
      await vi.advanceTimersByTimeAsync(5_000);
      expect((await run).retry).toEqual([]);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
