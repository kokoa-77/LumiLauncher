import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import { beforeAll, describe, expect, it } from "vitest";
import type { RecentWorld } from "@/lib/bindings";
import { setLanguagePreference } from "@/lib/i18n";
import { ReadyStep } from "./steps";
import { Welcome } from "./Welcome";

beforeAll(async () => {
  await setLanguagePreference("zh-CN");
});

const world: RecentWorld["world"] = {
  folder: "test",
  name: "白桦林",
  lastPlayed: null,
  day: null,
  gameMode: null,
  hardcore: false,
  version: null,
  playSeconds: null,
  imageUrl: null,
  imageKind: null,
  imageFit: "cover",
  seed: null,
  note: null,
};

const render = (quickPlay: boolean) =>
  renderToStaticMarkup(
    <MemoryRouter>
      <Welcome choice="continue" world={{ instanceId: "test", world, quickPlay }} />
    </MemoryRouter>,
  );

describe("首次引导欢迎页", () => {
  it("旧版世界说明会先启动游戏", () => {
    expect(render(false)).toContain("启动游戏，再从单人游戏进入「白桦林」");
  });

  it("支持直达时显示要进入的存档", () => {
    expect(render(true)).toContain("直接进入「白桦林」");
  });

  it("旧版世界在准备页也说明要从单人游戏进入", () => {
    const markup = renderToStaticMarkup(
      <ReadyStep
        ready={{ world: { instanceId: "test", world, quickPlay: false }, instance: undefined }}
        choice="continue"
        onChoose={() => undefined}
        onNext={() => undefined}
        onBack={() => undefined}
      />,
    );
    expect(markup).toContain("启动游戏");
    expect(markup).toContain("在单人游戏里进入「白桦林」");
  });
});
