import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { InstanceSummary } from "@/lib/bindings";
import { useGameStore } from "@/stores/game";
import { HomePage } from "./HomePage";

// SSR otherwise uses Zustand's initial snapshot, before instances have loaded.
vi.mock(import("@/stores/game"), async (importOriginal) => {
  const store = await importOriginal();
  return {
    ...store,
    useGameStore: Object.assign(
      <T,>(select: (state: ReturnType<typeof store.useGameStore.getState>) => T) =>
        select(store.useGameStore.getState()),
      store.useGameStore,
    ),
  };
});

const base: InstanceSummary = {
  id: "test",
  name: "1.21.11-NeoForge 21.11.14-beta",
  gameVersion: "1.21.11",
  loader: "NeoForge 21.11.14-beta",
  pack: null,
  createdAt: null,
  lastPlayed: null,
  group: null,
  pinnedAt: null,
  linked: null,
};

function renderHome(instance: InstanceSummary) {
  useGameStore.setState({
    instances: [instance],
    instancesLoaded: true,
    recentWorlds: [],
    recentServers: [],
  });
  return renderToStaticMarkup(
    <MemoryRouter>
      <HomePage />
    </MemoryRouter>,
  );
}

afterEach(() => {
  useGameStore.setState(useGameStore.getInitialState());
});

describe("首页未游玩实例标题", () => {
  it.each([
    "1.21.11-NeoForge 21.11.14-beta",
    "1.21.11-NeoForge_21.11.14-beta",
    "1.21.11-neoforge-21.11.14-beta",
  ])("将自动名称 %s 显示成游戏标题和完整加载器版本", (name) => {
    const markup = renderHome({ ...base, name });

    expect(markup).toContain(">Minecraft 1.21.11</h1>");
    expect(markup).toContain(">NeoForge 21.11.14-beta</p>");
    expect(markup).not.toContain(">1.21.11 · NeoForge");
  });

  it("原版自动名称只显示一次游戏版本", () => {
    const markup = renderHome({ ...base, name: "1.21.11", loader: null });

    expect(markup).toContain(">Minecraft 1.21.11</h1>");
    expect(markup).not.toContain(">1.21.11</p>");
  });

  it("保留超长自定义名称和实例版本信息", () => {
    const name = "1.21.11-NeoForge 21.11.14-beta 我的长期生存建造世界与朋友们的基地";
    const markup = renderHome({ ...base, name });

    expect(markup).toContain(`>${name}</h1>`);
    expect(markup).toContain(">1.21.11 · NeoForge 21.11.14-beta</p>");
  });

  it("保留与自动名称碰巧相同的整合包实例名称", () => {
    const markup = renderHome({ ...base, pack: base.name });

    expect(markup).toContain(">1.21.11-NeoForge 21.11.14-beta</h1>");
  });

  it("没有单人存档但存在多人服务器时，首页展示多人服直达入口", () => {
    useGameStore.setState({
      instances: [{ ...base, name: "联机实例" }],
      instancesLoaded: true,
      recentWorlds: [],
      recentServers: [
        {
          instanceId: "test",
          quickPlay: true,
          name: "微光生存服",
          address: "play.lumi.example",
        },
      ],
    });
    const markup = renderToStaticMarkup(
      <MemoryRouter>
        <HomePage />
      </MemoryRouter>,
    );

    expect(markup).toContain(">微光生存服</h1>");
    expect(markup).toContain("play.lumi.example");
  });
});
