import { describe, expect, it } from "vitest";
import type { ContentItem, ContentSource, ProjectHit } from "./bindings";
import { categoryFor, categoryGroups, findCategory, onlyOn } from "./categories";
import {
  addPages,
  type Cursor,
  type Entry,
  findInstalled,
  freshCursors,
  groupVersions,
  hasMoreIn,
  interleave,
  merge,
  projectKey,
  supportOf,
  toEntry,
  wantedSources,
} from "./discover";

describe("详情页的兼容性", () => {
  it("同一个主版本有好几个时合成 1.21.x", () => {
    expect(groupVersions(["26.3", "1.21.1", "1.21", "1.20.1", "1.19.2", "1.19"])).toEqual({
      shown: ["26.3", "1.21.x", "1.20.1", "1.19.x"],
      hidden: [],
    });
  });

  it("超过上限的放进 hidden", () => {
    const many = ["1.21", "1.20", "1.19", "1.18", "1.17"];
    expect(groupVersions(many, 3)).toEqual({
      shown: ["1.21", "1.20", "1.19"],
      hidden: ["1.18", "1.17"],
    });
  });

  it("看版本，模组再看加载器", () => {
    const neo = { gameVersion: "1.21.1", loader: "NeoForge 21.1.265" };
    expect(supportOf(["1.21.1"], ["neoforge", "fabric"], "mod", neo)).toBe("yes");
    expect(supportOf(["1.20.1"], ["neoforge"], "mod", neo)).toBe("noVersion");
    expect(supportOf(["1.21.1"], ["fabric"], "mod", neo)).toBe("noLoader");
    // 原版实例装不了模组
    expect(supportOf(["1.21.1"], ["fabric"], "mod", { gameVersion: "1.21.1", loader: null })).toBe(
      "noLoader",
    );
    // 资源包、光影包不看加载器
    expect(supportOf(["1.21.1"], ["iris"], "shader", neo)).toBe("yes");
    // 平台没给版本：不下结论
    expect(supportOf([], [], "mod", neo)).toBeNull();
  });
});

describe("两个平台的结果合在一起（D17c）", () => {
  const hit = (
    source: ProjectHit["source"],
    title: string,
    extra: Partial<ProjectHit> = {},
  ): ProjectHit => ({
    projectId: `${source}-${title}`,
    slug: title.toLowerCase(),
    title,
    description: "",
    author: "作者",
    iconUrl: null,
    downloads: 0,
    displayCategories: [],
    source,
    side: null,
    pageUrl: null,
    galleryUrl: null,
    authorUrl: null,
    updated: null,
    published: null,
    server: null,
    manualOnly: false,
    titleZh: null,
    twinId: null,
    ...extra,
  });
  const titles = (list: ProjectHit[]) => list.map((h) => h.title);

  it("按相关性时交替排", () => {
    const mr = [hit("modrinth", "A"), hit("modrinth", "B"), hit("modrinth", "C")];
    const cf = [hit("curseForge", "X")];
    expect(titles(interleave([mr, cf]))).toEqual(["A", "X", "B", "C"]);
  });

  it("按最近更新时归并，每个平台自己的顺序不变", () => {
    const at = (day: number) => `2026-11-${String(day).padStart(2, "0")}T00:00:00Z`;
    const mr = [
      hit("modrinth", "M12", { updated: at(12) }),
      hit("modrinth", "M9", { updated: at(9) }),
    ];
    // CurseForge 给的顺序和日期对不上（实测如此）：不能重排它
    const cf = [
      hit("curseForge", "C10", { updated: at(10) }),
      hit("curseForge", "C11", { updated: at(11) }),
      hit("curseForge", "C1", { updated: null }),
    ];
    expect(titles(interleave([mr, cf], "updated"))).toEqual(["M12", "C10", "C11", "M9", "C1"]);
  });

  it("同名的合成一条，下载量加起来", () => {
    const merged = merge(
      [],
      [[hit("modrinth", "Jade", { downloads: 9 })], [hit("curseForge", "Jade", { downloads: 3 })]],
    );
    expect(merged).toHaveLength(1);
    expect(merged[0]?.sources).toEqual(["modrinth", "curseForge"]);
    expect(merged[0]?.hit.downloads).toBe(12);
    expect(merged[0]?.counts).toEqual({ modrinth: 9, curseForge: 3 });
  });

  it("合成一条时显示首选平台那份（D18）", () => {
    const pages = [
      [hit("modrinth", "Jade", { description: "Modrinth 的简介" })],
      [hit("curseForge", "Jade", { description: "CurseForge 的简介" })],
    ];
    expect(merge([], pages)[0]?.hit.source).toBe("modrinth");
    const preferCf = merge([], pages, "relevance", "curseForge");
    expect(preferCf[0]?.hit.source).toBe("curseForge");
    expect(preferCf[0]?.hit.description).toBe("CurseForge 的简介");
    expect(preferCf[0]?.sources).toEqual(["modrinth", "curseForge"]);
  });

  it("名字后面只是加载器的括号不算名字，其他括号照样算", () => {
    expect(projectKey("Cloth Config API (Fabric/Forge/NeoForge)")).toBe(
      projectKey("Cloth Config API"),
    );
    expect(projectKey("Waystones [Forge & Fabric]")).toBe(projectKey("Waystones"));
    expect(projectKey("Spartan Shields [Unofficial]")).not.toBe(projectKey("Spartan Shields"));
    expect(projectKey("Just Enough Items (JEI)")).not.toBe(projectKey("Just Enough Items"));
    expect(projectKey("(Fabric)")).toBe("fabric");
  });

  it("名字一样、作者不一样的不合并（别人的移植版、同名的另一个模组）", () => {
    const merged = merge(
      [],
      [
        [hit("modrinth", "Waystones", { author: "BlayTheNinth" })],
        [hit("curseForge", "Waystones", { author: "SomeoneElse" })],
      ],
    );
    expect(merged).toHaveLength(2);
    // 作者名大小写、下划线不同算同一个人
    const same = merge(
      [],
      [
        [hit("modrinth", "Sodium", { author: "jellysquid3" })],
        [hit("curseForge", "Sodium", { author: "JellySquid3_" })],
      ],
    );
    expect(same).toHaveLength(1);
  });

  it("后端确认过的关联：名字、作者不一样也合成一条，中文名带过来", () => {
    const merged = merge(
      [],
      [
        [hit("modrinth", "Create", { projectId: "LNytGWDc", author: "Creators-of-Create" })],
        [
          hit("curseForge", "Create Mod", {
            projectId: "328085",
            author: "simibubi",
            twinId: "LNytGWDc",
            titleZh: "机械动力",
          }),
        ],
      ],
    );
    expect(merged).toHaveLength(1);
    expect(merged[0]?.hit.title).toBe("Create");
    expect(merged[0]?.hit.titleZh).toBe("机械动力");
    expect(merged[0]?.ids).toEqual({ modrinth: "LNytGWDc", curseForge: "328085" });
  });

  it("确认过另一个平台是别的项目时，同名同作者也不合并", () => {
    const merged = merge(
      [],
      [
        [hit("modrinth", "Jade", { projectId: "mr-jade", twinId: "1" })],
        [hit("curseForge", "Jade", { projectId: "2" })],
      ],
    );
    expect(merged).toHaveLength(2);
  });

  it("翻页时同一条又出现一次不重复放", () => {
    const first = merge([], [[hit("modrinth", "A")]]);
    expect(merge(first, [[hit("modrinth", "A")]])).toHaveLength(1);
  });

  it("同一个平台上的两个同名项目不合并", () => {
    const merged = merge(
      [],
      [[hit("modrinth", "Spartan Shields"), hit("modrinth", "Spartan Shields (Forge)")]],
    );
    expect(merged).toHaveLength(2);
  });
});

describe("按下载量、最新发布：两个平台真正归并", () => {
  const hit = (source: ProjectHit["source"], title: string, downloads: number): ProjectHit => ({
    projectId: `${source}-${title}`,
    slug: title.toLowerCase(),
    title,
    description: "",
    author: "作者",
    iconUrl: null,
    downloads,
    displayCategories: [],
    source,
    side: null,
    pageUrl: null,
    galleryUrl: null,
    authorUrl: null,
    updated: null,
    published: null,
    server: null,
    manualOnly: false,
    titleZh: null,
    twinId: null,
  });
  const shown = (list: Entry[]) => list.map((e) => `${e.hit.title} ${e.hit.downloads}`);
  const SOURCES: ContentSource[] = ["modrinth", "curseForge"];
  /** 模拟翻了一页：offset、total 先更新（和资源中心页一样） */
  const page = (
    cursors: Record<ContentSource, Cursor>,
    pages: Partial<Record<ContentSource, ProjectHit[]>>,
    totals: Partial<Record<ContentSource, number>>,
  ) => {
    for (const s of SOURCES) {
      const hits = pages[s];
      if (!hits) continue;
      cursors[s].offset += hits.length;
      cursors[s].total = totals[s] ?? cursors[s].total;
    }
    return pages;
  };

  it("另一个平台还没翻到的先留着，后加载的永远接在后面，数字一路往下", () => {
    const cursors = freshCursors();
    // 2026-11-12 实测的样子：CurseForge 的数字普遍比 Modrinth 大
    let list = addPages(
      [],
      cursors,
      page(
        cursors,
        {
          modrinth: [hit("modrinth", "Sodium", 251), hit("modrinth", "Iris", 194)],
          curseForge: [
            hit("curseForge", "JEI", 646),
            hit("curseForge", "Mouse Tweaks", 487),
            hit("curseForge", "Clumps", 290),
          ],
        },
        { modrinth: 4, curseForge: 5 },
      ),
      SOURCES,
      "downloads",
    );
    expect(shown(list)).toEqual(["JEI 646", "Mouse Tweaks 487", "Clumps 290"]);
    expect(cursors.modrinth.held).toHaveLength(2);
    expect(hasMoreIn(cursors, SOURCES)).toBe(true);
    // Modrinth 留着两条，够了就先只翻 CurseForge……这里留得不多，两个都翻
    expect(wantedSources(cursors, SOURCES, "downloads")).toEqual(["modrinth", "curseForge"]);

    list = addPages(
      list,
      cursors,
      page(
        cursors,
        {
          modrinth: [hit("modrinth", "Lithium", 143), hit("modrinth", "JEI", 85)],
          curseForge: [hit("curseForge", "GeckoLib", 270), hit("curseForge", "Waystones", 200)],
        },
        {},
      ),
      SOURCES,
      "downloads",
    );
    // 两个平台都翻完了：剩下的全放进来；JEI 合成一条，总数 731
    expect(shown(list)).toEqual([
      "JEI 731",
      "Mouse Tweaks 487",
      "Clumps 290",
      "GeckoLib 270",
      "Sodium 251",
      "Waystones 200",
      "Iris 194",
      "Lithium 143",
    ]);
    expect(hasMoreIn(cursors, SOURCES)).toBe(false);
  });

  it("合成一条后总数变大的往前挪", () => {
    const cursors = freshCursors();
    const list = addPages(
      [],
      cursors,
      page(
        cursors,
        {
          modrinth: [hit("modrinth", "AppleSkin", 99)],
          curseForge: [hit("curseForge", "Mouse Tweaks", 487), hit("curseForge", "AppleSkin", 424)],
        },
        { modrinth: 1, curseForge: 2 },
      ),
      SOURCES,
      "downloads",
    );
    expect(shown(list)).toEqual(["AppleSkin 523", "Mouse Tweaks 487"]);
  });

  it("留着的够多的平台先不翻页", () => {
    const cursors = freshCursors();
    const many = Array.from({ length: 12 }, (_, i) => hit("modrinth", `M${i}`, 100 - i));
    addPages(
      [],
      cursors,
      page(
        cursors,
        { modrinth: many, curseForge: [hit("curseForge", "C", 500)] },
        {
          modrinth: 40,
          curseForge: 40,
        },
      ),
      SOURCES,
      "downloads",
    );
    expect(cursors.modrinth.held).toHaveLength(12);
    expect(wantedSources(cursors, SOURCES, "downloads")).toEqual(["curseForge"]);
    // 按相关性不留，两个都翻
    expect(wantedSources(cursors, SOURCES, "relevance")).toEqual(["modrinth", "curseForge"]);
  });

  it("最新发布按创建时间归并；读不出时间的跟着前一条走", () => {
    const at = (year: number) => `${year}-01-01T00:00:00Z`;
    const dated = (source: ProjectHit["source"], title: string, year: number | null) => ({
      ...hit(source, title, 0),
      published: year === null ? null : at(year),
    });
    const cursors = freshCursors();
    const list = addPages(
      [],
      cursors,
      page(
        cursors,
        {
          modrinth: [dated("modrinth", "M2025", 2025), dated("modrinth", "M?", null)],
          curseForge: [dated("curseForge", "C2026", 2026), dated("curseForge", "C2020", 2020)],
        },
        { modrinth: 2, curseForge: 2 },
      ),
      SOURCES,
      "newest",
    );
    expect(list.map((e) => e.hit.title)).toEqual(["C2026", "M2025", "M?", "C2020"]);
  });
});

describe("筛选的分类对照", () => {
  it("两边都有的分类各自换成平台的参数", () => {
    const tech = findCategory("mod", "technology");
    expect(categoryFor(tech, "modrinth")).toBe("technology");
    expect(categoryFor(tech, "curseForge")).toBe("412");
    expect(onlyOn(tech)).toBeNull();
  });

  it("只有一边有的分类：另一边不搜", () => {
    const economy = findCategory("mod", "economy");
    expect(categoryFor(economy, "curseForge")).toBeUndefined();
    expect(onlyOn(economy)).toBe("modrinth");
    expect(onlyOn(findCategory("mod", "redstone"))).toBe("curseForge");
  });

  it("不筛分类时两边都搜", () => {
    expect(categoryFor(undefined, "curseForge")).toBeNull();
    expect(findCategory("mod", null)).toBeUndefined();
    expect(findCategory("favorites", "technology")).toBeUndefined();
  });

  it("每一类里分类的键不重复", () => {
    for (const kind of ["mod", "resourcePack", "shader", "modpack", "map", "server"] as const) {
      const keys = categoryGroups(kind).flatMap((g) => g.items.map((c) => c.key));
      expect(new Set(keys).size, kind).toBe(keys.length);
    }
  });
});

describe("已经装了吗（D17e）", () => {
  const item = (extra: Partial<ContentItem>): ContentItem => ({
    kind: "mod",
    fileName: "a.jar",
    title: "Jade",
    enabled: true,
    size: 1,
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
    modified: 0,
    titleZh: null,
    twinId: null,
    ...extra,
  });
  const entry = (source: ContentSource, projectId: string, title: string, twinId: string | null) =>
    toEntry({
      projectId,
      slug: "",
      title,
      description: "",
      author: "作者",
      iconUrl: null,
      downloads: 0,
      displayCategories: [],
      source,
      side: null,
      pageUrl: null,
      galleryUrl: null,
      authorUrl: null,
      updated: null,
      published: null,
      server: null,
      manualOnly: false,
      titleZh: null,
      twinId,
    });

  it("从 Modrinth 装的，在 CurseForge 的结果上也认得出（确认过的关联）", () => {
    const installed = [item({ source: "modrinth", projectId: "mr-jade" })];
    expect(findInstalled(entry("curseForge", "324717", "Jade 🔍", "mr-jade"), installed)).toBe(
      installed[0],
    );
  });

  it("文件这边知道另一个平台的编号也算", () => {
    const installed = [item({ source: "curseForge", projectId: "324717", twinId: "mr-jade" })];
    expect(findInstalled(entry("modrinth", "mr-jade", "Jade", null), installed)).toBe(installed[0]);
  });

  it("平台上认得出的文件不按名字比；只有本地认出的才按名字比", () => {
    const other = [item({ source: "modrinth", projectId: "someone-elses-jade" })];
    expect(findInstalled(entry("modrinth", "mr-jade", "Jade", null), other)).toBeUndefined();
    const local = [item({ source: "file" })];
    expect(findInstalled(entry("modrinth", "mr-jade", "Jade", null), local)).toBe(local[0]);
  });
});
