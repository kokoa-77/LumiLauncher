import { describe, expect, it } from "vitest";
import type { ContentItem, UpdateInfo } from "./bindings";
import { originsIn, toggleSelection, viewContent } from "./contentView";

const item = (title: string, extra: Partial<ContentItem> = {}): ContentItem => ({
  kind: "mod",
  fileName: `${title.toLowerCase()}.jar`,
  title,
  enabled: true,
  size: null,
  projectId: null,
  versionId: null,
  versionNumber: null,
  locked: false,
  iconUrl: null,
  source: null,
  isFolder: false,
  author: null,
  authorUrl: null,
  pageUrl: null,
  modified: null,
  titleZh: null,
  twinId: null,
  ...extra,
});

const items = [
  item("Sodium", { author: "jellysquid3", modified: 300 }),
  item("Create", { enabled: false, modified: 100 }),
  item("AppleSkin", { modified: null }),
  item("Jade", { author: "Snownee", modified: 200 }),
];

const update: UpdateInfo = {
  kind: "mod",
  fileName: "jade.jar",
  title: "Jade",
  currentVersion: "1",
  versionId: "v2",
  versionNumber: "2",
};

const titles = (list: ContentItem[]) => list.map((i) => i.title);
const base = { query: "", filter: "all" as const, sort: "name" as const, updates: {} };

describe("实例内容的排序筛选", () => {
  it("按名字排，反过来也行", () => {
    expect(titles(viewContent(items, base))).toEqual(["AppleSkin", "Create", "Jade", "Sodium"]);
    expect(titles(viewContent(items, { ...base, sort: "nameDesc" }))).toEqual([
      "Sodium",
      "Jade",
      "Create",
      "AppleSkin",
    ]);
  });

  it("按修改时间：新的在前，没有时间的放最后", () => {
    expect(titles(viewContent(items, { ...base, sort: "modified" }))).toEqual([
      "Sodium",
      "Jade",
      "Create",
      "AppleSkin",
    ]);
  });

  it("筛选启用、停用、有更新", () => {
    expect(titles(viewContent(items, { ...base, filter: "disabled" }))).toEqual(["Create"]);
    expect(viewContent(items, { ...base, filter: "enabled" })).toHaveLength(3);
    expect(
      titles(viewContent(items, { ...base, filter: "updates", updates: { "jade.jar": update } })),
    ).toEqual(["Jade"]);
  });

  it("搜索名字、文件名和作者，不分大小写", () => {
    expect(titles(viewContent(items, { ...base, query: "SNOW" }))).toEqual(["Jade"]);
    expect(titles(viewContent(items, { ...base, query: "create.jar" }))).toEqual(["Create"]);
  });

  it("中文名也能搜", () => {
    const named = [item("Create", { titleZh: "机械动力" }), item("Jade")];
    expect(titles(viewContent(named, { ...base, query: "机械" }))).toEqual(["Create"]);
  });

  it("按来源筛：没认出来的算「未识别」", () => {
    const mixed = [
      item("Sodium", { source: "modrinth" }),
      item("Create", { source: "curseForge" }),
      item("Jade", { source: "file" }),
      item("Mystery"),
    ];
    expect(originsIn(mixed)).toEqual(["modrinth", "curseForge", "file", "unknown"]);
    expect(originsIn([item("A", { source: "modrinth" })])).toEqual(["modrinth"]);
    expect(titles(viewContent(mixed, { ...base, origin: "curseForge" }))).toEqual(["Create"]);
    expect(titles(viewContent(mixed, { ...base, origin: "unknown" }))).toEqual(["Mystery"]);
    expect(viewContent(mixed, { ...base, origin: "all" })).toHaveLength(4);
  });

  it("不改原来的数组", () => {
    const copy = [...items];
    viewContent(items, { ...base, sort: "nameDesc" });
    expect(items).toEqual(copy);
  });
});

describe("多选", () => {
  const visible = ["a", "b", "c", "d"];

  it("点一下切换一项", () => {
    const one = toggleSelection(new Set(), visible, "b", null, false);
    expect([...one]).toEqual(["b"]);
    expect(toggleSelection(one, visible, "b", "b", false).size).toBe(0);
  });

  it("按住 Shift 选中一段", () => {
    const one = toggleSelection(new Set(), visible, "a", null, false);
    const range = toggleSelection(one, visible, "c", "a", true);
    expect([...range].sort()).toEqual(["a", "b", "c"]);
  });

  it("按住 Shift 点已选中的，把这一段取消", () => {
    const all = new Set(visible);
    const range = toggleSelection(all, visible, "b", "d", true);
    expect([...range]).toEqual(["a"]);
  });
});
