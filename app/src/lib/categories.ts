// 资源中心「筛选」面板里的排序和分类。
//
// 两个平台的分类各不相同：这里自己整理一张「我们的分类 → Modrinth 的分类名 / CurseForge 的分类编号」对照表，
// 界面上只显示一份中文分类。只有一个平台有的分类，选了以后只搜那个平台（面板里会写明）。
// 来源：Modrinth `/v2/tag/category`、CurseForge `/v1/categories?gameId=432`（2026-11-12 拉取）。
// CurseForge 只能按一个分类筛：给好几个编号时是「同时属于」，不是「属于其中一个」。
import type { ContentSource, SearchSort } from "./bindings";
import type { DiscoverKind } from "./discover";

export const SORTS: SearchSort[] = ["relevance", "downloads", "updated", "newest"];

export interface Category {
  /** 我们自己的名字（地址里、中文名的键都用它） */
  key: string;
  /** Modrinth 的分类名；没有时 Modrinth 不搜 */
  modrinth?: string;
  /** CurseForge 的分类编号；没有时 CurseForge 不搜 */
  curseForge?: number;
}

export interface CategoryGroup {
  /** 分组名的键（只有一组时不显示组名） */
  group: string;
  items: Category[];
}

const both = (key: string, curseForge: number, modrinth = key): Category => ({
  key,
  modrinth,
  curseForge,
});
const mr = (key: string, modrinth = key): Category => ({ key, modrinth });
const cf = (key: string, curseForge: number): Category => ({ key, curseForge });

const MOD: CategoryGroup[] = [
  {
    group: "type",
    items: [
      both("optimization", 6814),
      both("technology", 412),
      both("magic", 419),
      both("adventure", 422),
      both("worldgen", 406),
      both("mobs", 411),
      both("equipment", 434),
      both("storage", 420),
      both("food", 436),
      both("decoration", 424),
      both("transportation", 414),
      both("utility", 5191),
      both("library", 421),
      both("management", 435),
      mr("game-mechanics"),
      mr("economy"),
      mr("social"),
      mr("minigame"),
      mr("cursed"),
      cf("map-info", 423),
      cf("redstone", 4558),
      cf("bug-fixes", 6821),
      cf("addons", 426),
    ],
  },
];

const RESOURCE_PACK: CategoryGroup[] = [
  {
    group: "style",
    items: [
      both("vanilla-like", 403),
      both("realistic", 400),
      both("modded", 4465),
      both("fonts", 5244),
      mr("simplistic"),
      mr("themed"),
      mr("tweaks"),
      mr("gui"),
      mr("audio"),
      mr("models"),
      mr("cursed"),
      cf("animated", 404),
      cf("modern", 401),
      cf("medieval", 402),
      cf("steampunk", 399),
    ],
  },
  {
    group: "resolution",
    items: [
      mr("8x-"),
      both("16x", 393),
      both("32x", 394),
      mr("48x"),
      both("64x", 395),
      both("128x", 396),
      both("256x", 397),
      both("512x+", 398),
    ],
  },
];

const SHADER: CategoryGroup[] = [
  {
    group: "style",
    items: [
      both("realistic", 6553),
      mr("semi-realistic"),
      both("vanilla-like", 6555),
      both("fantasy", 6554),
      mr("cartoon"),
    ],
  },
  {
    group: "effect",
    items: [
      mr("path-tracing"),
      mr("pbr"),
      mr("reflections"),
      mr("shadows"),
      mr("colored-lighting"),
      mr("bloom"),
      mr("atmosphere"),
      mr("foliage"),
    ],
  },
  {
    group: "performance",
    items: [mr("potato"), mr("low"), mr("medium"), mr("high")],
  },
];

const MODPACK: CategoryGroup[] = [
  {
    group: "type",
    items: [
      both("technology", 4472),
      both("magic", 4473),
      both("adventure", 4475),
      both("quests", 4478),
      both("combat", 4483),
      both("multiplayer", 4484),
      both("lightweight", 4481),
      both("kitchen-sink", 4482),
      both("challenging", 4479),
      mr("optimization"),
      cf("skyblock", 4736),
      cf("exploration", 4476),
      cf("sci-fi", 4474),
      cf("vanilla-plus", 5128),
      cf("expert", 9243),
      cf("horror", 7418),
      cf("minigame", 4477),
      cf("map-based", 4480),
    ],
  },
];

const MAP: CategoryGroup[] = [
  {
    group: "type",
    items: [
      cf("adventure", 248),
      cf("survival", 253),
      cf("parkour", 251),
      cf("puzzle", 252),
      cf("game-map", 250),
      cf("creation", 249),
      cf("modded-world", 4464),
    ],
  },
];

const SERVER: CategoryGroup[] = [
  {
    group: "gameplay",
    items: [
      mr("smp"),
      mr("vanilla-like"),
      mr("skyblock"),
      mr("oneblock"),
      mr("minigames"),
      mr("bedwars"),
      mr("factions"),
      mr("towns"),
      mr("rpg"),
      mr("prison"),
      mr("lifesteal"),
      mr("anarchy"),
      mr("parkour"),
    ],
  },
  {
    group: "feature",
    items: [
      mr("pokemon"),
      mr("economy"),
      mr("pvp"),
      mr("pve"),
      mr("dungeons"),
      mr("custom-content"),
      mr("roleplay"),
    ],
  },
  {
    group: "rule",
    items: [mr("offline-mode"), mr("crossplay"), mr("whitelisted"), mr("hardcore-mode")],
  },
];

const TABLE: Partial<Record<DiscoverKind, CategoryGroup[]>> = {
  mod: MOD,
  resourcePack: RESOURCE_PACK,
  shader: SHADER,
  modpack: MODPACK,
  map: MAP,
  server: SERVER,
};

/** 这一类能按哪些分类筛（收藏夹没有） */
export const categoryGroups = (kind: DiscoverKind): CategoryGroup[] => TABLE[kind] ?? [];

export function findCategory(kind: DiscoverKind, key: string | null): Category | undefined {
  if (!key) return undefined;
  for (const group of categoryGroups(kind)) {
    const found = group.items.find((c) => c.key === key);
    if (found) return found;
  }
  return undefined;
}

/** 这个分类在某个平台上的参数；平台没有这个分类时是 `undefined`（这个平台不搜） */
export function categoryFor(
  category: Category | undefined,
  source: ContentSource,
): string | null | undefined {
  if (!category) return null;
  if (source === "modrinth") return category.modrinth;
  return category.curseForge === undefined ? undefined : String(category.curseForge);
}

/** 只有一个平台有的分类：返回那个平台 */
export function onlyOn(category: Category | undefined): ContentSource | null {
  if (!category) return null;
  if (category.modrinth && category.curseForge === undefined) return "modrinth";
  if (!category.modrinth && category.curseForge !== undefined) return "curseForge";
  return null;
}

export const isSort = (value: string | null): value is SearchSort =>
  value !== null && (SORTS as string[]).includes(value);
