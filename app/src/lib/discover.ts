// 资源中心共用的东西：分类、平台、两个平台的结果合并、「已安装」比对、更新时间换算、页面级缓存。
import type { TFunction } from "i18next";
import type {
  ContentItem,
  ContentKind,
  ContentSource,
  FavoriteKind,
  InstanceSummary,
  ProjectHit,
  SearchSort,
} from "./bindings";

/**
 * 「装到」哪个实例：地址里给了就用它；否则是首页那个世界所在的实例（最近玩的）。
 * 装模组要有加载器：最近那个是原版时，换成最近玩的有加载器的实例。
 */
export function pickTarget(instances: InstanceSummary[], param: string | null): string {
  if (param && instances.some((i) => i.id === param)) return param;
  const first = instances[0];
  return (first?.loader ? first : instances.find((i) => i.loader))?.id ?? first?.id ?? "";
}

/** 能装的东西：整合包（装成新实例）、实例里的内容、地图（进存档）、服务器（进多人游戏列表） */
export type InstallKind = "modpack" | ContentKind | "map" | "server";

/**
 * 资源中心的七类（顺序就是页面上的顺序，全站同名）。收藏夹不是平台上的类型，是自己存的。
 */
export type DiscoverKind = InstallKind | "favorites";
export const KINDS: DiscoverKind[] = [
  "modpack",
  "mod",
  "resourcePack",
  "map",
  "shader",
  "server",
  "favorites",
];

/** 只在一个平台上有的类：地图只有 CurseForge，服务器只有 Modrinth */
export const fixedSource = (kind: DiscoverKind): ContentSource | null =>
  kind === "map" ? "curseForge" : kind === "server" ? "modrinth" : null;
/** 装进已有实例的类型（整合包装成新实例） */
export const isContentKind = (kind: DiscoverKind): kind is ContentKind =>
  kind === "mod" || kind === "shader" || kind === "resourcePack";

/** 搜哪个平台：全部（合在一个列表里）或者只搜一个 */
export type SourceFilter = "all" | ContentSource;
export const SOURCES: SourceFilter[] = ["all", "modrinth", "curseForge"];

/** 一条结果：同一个项目两个平台都有时合成一条（装的时候优先用 Modrinth） */
export interface Entry {
  /** 显示用的那一份；合成一条时 `downloads` 是两个平台加起来的总下载量 */
  hit: ProjectHit;
  sources: ContentSource[];
  /** 各平台的项目页（「在浏览器中打开」用） */
  pages: Partial<Record<ContentSource, string>>;
  /** 各平台上的项目编号（判断「已安装」时两边都要比） */
  ids: Partial<Record<ContentSource, string>>;
  /** 各平台各自的下载量 */
  counts: Partial<Record<ContentSource, number>>;
  /** 各平台上的作者（按名字合并时作者也要一样） */
  authors: Partial<Record<ContentSource, string>>;
  /** 后端确认过的、另一个平台上的编号（那一条还没搜到时也先记着） */
  twins: Partial<Record<ContentSource, string>>;
}

/** 另一个平台 */
export const otherSource = (source: ContentSource): ContentSource =>
  source === "modrinth" ? "curseForge" : "modrinth";

/** 显示用的名字：有中文名时「中文名」+ 灰色的英文名，没有时只有英文名 */
export const displayName = (hit: Pick<ProjectHit, "title" | "titleZh">) =>
  hit.titleZh ? `${hit.titleZh}（${hit.title}）` : hit.title;

export const entryKey = (e: Entry) => `${e.hit.source}:${e.hit.projectId}`;

/** 搜索用：只留字母和数字，不分大小写 */
export const sameKey = (title: string) => title.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");

/** 名字后面的括号里只有加载器和版本（「(Fabric/Forge/NeoForge)」「[Forge & Fabric]」「(1.21+)」）时不算名字 */
const NOTE = /\s*[([（【]([^)\]）】]*)[)\]）】]/g;
const NOTE_WORD = /^(?:fabric|forge|neoforge|neo|quilt|and|\d+(?:\.(?:\d+|x))*)$/i;

/**
 * 按名字认同一个项目（两个平台的编号不通用）。同一个模组在两个平台上常常一个带「(Fabric/Forge/NeoForge)」一个不带，
 * 这种括号不算；「[Unofficial]」这类括号照样算，免得把别人的移植版和原版当成一个。
 */
export function projectKey(title: string): string {
  const stripped = title.replace(NOTE, (all, inner: string) => {
    const words = inner.split(/[\s&+/,|-]+/).filter(Boolean);
    return words.length > 0 && words.every((w) => NOTE_WORD.test(w)) ? "" : all;
  });
  return sameKey(stripped) || sameKey(title);
}

export function toEntry(hit: ProjectHit): Entry {
  return {
    hit,
    sources: [hit.source],
    pages: hit.pageUrl ? { [hit.source]: hit.pageUrl } : {},
    ids: { [hit.source]: hit.projectId },
    counts: { [hit.source]: hit.downloads ?? 0 },
    authors: { [hit.source]: hit.author },
    twins: hit.twinId ? { [otherSource(hit.source)]: hit.twinId } : {},
  };
}

const total = (counts: Partial<Record<ContentSource, number>>) =>
  Object.values(counts).reduce<number>((sum, n) => sum + (n ?? 0), 0);

/** 两个平台的结果交替排（按相关性），或者按「最近更新」每次取几列开头里最新的那个 */
export function interleave(pages: ProjectHit[][], sort: SearchSort = "relevance"): ProjectHit[] {
  const out: ProjectHit[] = [];
  if (sort !== "updated") {
    const longest = Math.max(0, ...pages.map((p) => p.length));
    for (let i = 0; i < longest; i++) {
      for (const page of pages) {
        const hit = page[i];
        if (hit) out.push(hit);
      }
    }
    return out;
  }
  // 最近更新：每个平台自己的顺序不变（CurseForge 的顺序和它返回的日期对不齐，重排就和官网不一样了）
  const time = (hit: ProjectHit) => {
    const at = hit.updated ? Date.parse(hit.updated) : Number.NaN;
    return Number.isNaN(at) ? Number.NEGATIVE_INFINITY : at;
  };
  const heads = pages.map(() => 0);
  for (;;) {
    let best = -1;
    let bestValue = Number.NEGATIVE_INFINITY;
    pages.forEach((page, i) => {
      const hit = page[heads[i] ?? 0];
      if (!hit) return;
      const value = time(hit);
      if (best === -1 || value > bestValue) {
        best = i;
        bestValue = value;
      }
    });
    const hit = pages[best]?.[heads[best] ?? 0];
    if (!hit) break;
    out.push(hit);
    heads[best] = (heads[best] ?? 0) + 1;
  }
  return out;
}

/**
 * 两个平台上的同一个项目合成一条（只认有证据的，见 {@link sameProject}）：下载量加起来，
 * 显示首选平台那份（设置「内容与下载」→「首选平台」，默认 Modrinth：装的时候不会遇到作者禁止第三方下载），
 * 展示图、中文名哪边有用哪边。
 */
export function merge(
  existing: Entry[],
  pages: ProjectHit[][],
  sort: SearchSort = "relevance",
  preferred: ContentSource = "modrinth",
): Entry[] {
  const out: Entry[] = existing.map((e) => ({
    hit: e.hit,
    sources: [...e.sources],
    pages: { ...e.pages },
    ids: { ...e.ids },
    counts: { ...e.counts },
    authors: { ...e.authors },
    twins: { ...e.twins },
  }));
  /** 「平台:编号」→ 第几条（自己的编号和确认过的另一个平台的编号都记） */
  const byId = new Map<string, number>();
  /** 名字 → 这个名字的几条 */
  const byName = new Map<string, number[]>();
  const remember = (entry: Entry, index: number) => {
    for (const [source, id] of Object.entries({ ...entry.twins, ...entry.ids })) {
      if (id) byId.set(`${source}:${id}`, index);
    }
    const key = projectKey(entry.hit.title);
    const list = byName.get(key);
    if (list) {
      if (!list.includes(index)) list.push(index);
    } else byName.set(key, [index]);
  };
  out.forEach(remember);

  for (const hit of interleave(pages, sort)) {
    const mine = byId.get(`${hit.source}:${hit.projectId}`);
    const mineEntry = mine === undefined ? undefined : out[mine];
    // 翻页时平台偶尔会把同一条再给一次：不重复放
    if (mineEntry?.ids[hit.source] === hit.projectId) continue;
    const twin = hit.twinId ? byId.get(`${otherSource(hit.source)}:${hit.twinId}`) : undefined;
    const target =
      [mine, twin].find((i) => i !== undefined && out[i] && !out[i].sources.includes(hit.source)) ??
      byName
        .get(projectKey(hit.title))
        ?.find((i) => out[i] !== undefined && sameProject(out[i], hit));
    const entry = target === undefined ? undefined : out[target];
    if (target === undefined || !entry) {
      const fresh = toEntry(hit);
      out.push(fresh);
      remember(fresh, out.length - 1);
      continue;
    }
    entry.sources.push(hit.source);
    if (hit.pageUrl) entry.pages[hit.source] = hit.pageUrl;
    entry.ids[hit.source] = hit.projectId;
    entry.counts[hit.source] = hit.downloads ?? 0;
    entry.authors[hit.source] = hit.author;
    if (hit.twinId) entry.twins[otherSource(hit.source)] = hit.twinId;
    const [shown, other] = hit.source === preferred ? [hit, entry.hit] : [entry.hit, hit];
    entry.hit = {
      ...shown,
      galleryUrl: shown.galleryUrl ?? other.galleryUrl,
      titleZh: shown.titleZh ?? other.titleZh,
      downloads: total(entry.counts),
    };
    remember(entry, target);
  }
  return out;
}

/** 作者名比较用：不分大小写，只看字母和数字（「jellysquid3_」和「jellysquid3」算一个人） */
const authorKey = (name: string | null | undefined) => sameKey(name ?? "");

/**
 * 没有确认过的关联时，按名字认同一个项目要两条都满足：名字一样（加载器括号不算），作者也一样。
 * 只是名字一样的（别人的移植版、同名的另一个模组）不合并，两条都显示、各自标着作者和平台。
 * 已经确认过另一个平台是别的项目的，也不合并。
 */
function sameProject(entry: Entry, hit: ProjectHit): boolean {
  if (entry.sources.includes(hit.source)) return false;
  const known = entry.twins[hit.source];
  if (known && known !== hit.projectId) return false;
  const mine = hit.twinId;
  const theirs = entry.ids[otherSource(hit.source)];
  if (mine && theirs && mine !== theirs) return false;
  const author = authorKey(hit.author);
  return author !== "" && Object.values(entry.authors).some((a) => authorKey(a) === author);
}

// ---------- 按下载量、最新发布：两个平台真正归并 ----------
// 只交替的话：CurseForge 的下载量普遍比 Modrinth 大好几倍，第一页里 Modrinth 的第一名（2.5 亿）排在
// CurseForge 第 20 名（2.9 亿）后面，下一页 CurseForge 的 2.5～2.9 亿又接在它后面，顺序就乱了。
// 做法：一个平台还有下一页时，它拿到的最后一条就是它以后的上限；比这个上限小的先留着，
// 等那个平台翻到下面再放进列表。所以后加载的永远排在已经显示的后面，列表不会跳。

/** 每个平台搜到哪了 */
export interface Cursor {
  offset: number;
  /** 这个平台一共有多少条；还不知道时是无穷大 */
  total: number;
  /** 按下载量、最新发布排时：拿到了但还不能放进列表的（平台自己的顺序，带上归并用的值） */
  held: { hit: ProjectHit; value: number }[];
  /** 这个平台拿到的最后一条的值：它以后的结果不会比这个大；还没拿过时是无穷大 */
  floor: number;
}

export const freshCursors = (): Record<ContentSource, Cursor> => ({
  modrinth: {
    offset: 0,
    total: Number.POSITIVE_INFINITY,
    held: [],
    floor: Number.POSITIVE_INFINITY,
  },
  curseForge: {
    offset: 0,
    total: Number.POSITIVE_INFINITY,
    held: [],
    floor: Number.POSITIVE_INFINITY,
  },
});

/** 这两种排序要真正归并（值在两个平台之间可比） */
const ranked = (sort: SearchSort) => sort === "downloads" || sort === "newest";
/** 留着的够多时这个平台先不翻页 */
const HELD_ENOUGH = 10;

/** 归并用的值；读不出来时是 null（跟着这个平台的前一条走） */
function rankValue(hit: ProjectHit, sort: SearchSort): number | null {
  if (sort === "downloads") return hit.downloads ?? null;
  const at = hit.published ? Date.parse(hit.published) : Number.NaN;
  return Number.isNaN(at) ? null : at;
}

const open = (c: Cursor) => c.offset < c.total;

/** 还有没有没显示的：有平台没翻完，或者还留着几条 */
export const hasMoreIn = (cursors: Record<ContentSource, Cursor>, sources: ContentSource[]) =>
  sources.some((s) => open(cursors[s]) || cursors[s].held.length > 0);

/** 下一次要去翻页的平台（按下载量排时，留着的够多的平台先不翻） */
export const wantedSources = (
  cursors: Record<ContentSource, Cursor>,
  sources: ContentSource[],
  sort: SearchSort,
) =>
  sources.filter(
    (s) => open(cursors[s]) && (!ranked(sort) || cursors[s].held.length < HELD_ENOUGH),
  );

/**
 * 把新搜到的几页放进列表。调用前 `cursors` 的 offset / total 要先更新好；这里会改它的 held、floor。
 * 相关性、最近更新见 {@link interleave}；下载量、最新发布见上面的说明，按下载量时再按总下载量排一次
 * （合成一条后总数变大的往前挪；新放进来的一定比已经显示的小，所以只会在已经看过的那部分里挪）。
 */
export function addPages(
  existing: Entry[],
  cursors: Record<ContentSource, Cursor>,
  pages: Partial<Record<ContentSource, ProjectHit[]>>,
  sources: ContentSource[],
  sort: SearchSort,
  preferred: ContentSource = "modrinth",
): Entry[] {
  if (!ranked(sort))
    return merge(
      existing,
      sources.map((s) => pages[s] ?? []),
      sort,
      preferred,
    );
  for (const s of sources) {
    const cursor = cursors[s];
    for (const hit of pages[s] ?? []) {
      cursor.floor = rankValue(hit, sort) ?? cursor.floor;
      cursor.held.push({ hit, value: cursor.floor });
    }
  }
  const opened = sources.filter((s) => open(cursors[s]));
  const limit =
    opened.length === 0
      ? Number.NEGATIVE_INFINITY
      : Math.max(...opened.map((s) => cursors[s].floor));
  const emitted: ProjectHit[] = [];
  for (;;) {
    let best: Cursor | null = null;
    for (const s of sources) {
      const head = cursors[s].held[0];
      if (!head || head.value < limit) continue;
      if (!best || head.value > (best.held[0]?.value ?? Number.NEGATIVE_INFINITY)) {
        best = cursors[s];
      }
    }
    const next = best?.held.shift();
    if (!next) break;
    emitted.push(next.hit);
  }
  const merged = merge(existing, [emitted], "relevance", preferred);
  if (sort !== "downloads") return merged;
  return merged.sort((a, b) => (b.hit.downloads ?? 0) - (a.hit.downloads ?? 0));
}

/**
 * 实例里已经装了这个项目吗：先比两个平台的编号（包括确认过的另一个平台的编号）；
 * 只有两个平台上都认不出来的文件（从文件自带的说明读的名字）才按名字比。
 */
export function findInstalled(entry: Entry, items: ContentItem[]): ContentItem | undefined {
  const ids: Partial<Record<ContentSource, string>> = { ...entry.twins, ...entry.ids };
  const byId = items.find((item) => {
    if (item.projectId === null || (item.source !== "modrinth" && item.source !== "curseForge")) {
      return false;
    }
    return (
      ids[item.source] === item.projectId ||
      (item.twinId !== null && ids[otherSource(item.source)] === item.twinId)
    );
  });
  if (byId) return byId;
  const key = projectKey(entry.hit.title);
  return items.find(
    (item) => (item.source === "file" || item.source === null) && projectKey(item.title) === key,
  );
}

export { formatCount } from "./format";

/** 「4 小时前」「3 个月前」；读不出时间时是 null */
export function formatSince(iso: string | null | undefined, t: TFunction): string | null {
  if (!iso) return null;
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return null;
  const minutes = Math.max(0, (Date.now() - at) / 60_000);
  if (minutes < 60) return t("discover.since.minutes", { n: Math.max(1, Math.round(minutes)) });
  const hours = minutes / 60;
  if (hours < 24) return t("discover.since.hours", { n: Math.round(hours) });
  const days = hours / 24;
  if (days < 30) return t("discover.since.days", { n: Math.round(days) });
  const months = days / 30.4;
  if (months < 12) return t("discover.since.months", { n: Math.round(months) });
  return t("discover.since.years", { n: Math.max(1, Math.round(months / 12)) });
}

/** 每条最多显示几个分类标签 */
const MAX_TAGS = 3;

/**
 * Modrinth 把加载器（fabric、neoforge……）也混在分类里，不当分类标签显示。
 * 来自 Modrinth 的 `/v2/tag/loader`（2026-11-11 拉取，共 29 个）
 */
const MODRINTH_LOADERS = new Set([
  "babric",
  "bta-babric",
  "bukkit",
  "bungeecord",
  "canvas",
  "datapack",
  "fabric",
  "folia",
  "forge",
  "geyser",
  "iris",
  "java-agent",
  "legacy-fabric",
  "liteloader",
  "minecraft",
  "modloader",
  "neoforge",
  "nilloader",
  "optifine",
  "ornithe",
  "paper",
  "purpur",
  "quilt",
  "rift",
  "spigot",
  "sponge",
  "vanilla",
  "velocity",
  "waterfall",
]);

/** Modrinth 服务器上不显示的标签：不验证正版的服务器（Lumi 只用正版账户） */
const HIDDEN_MODRINTH_TAGS = new Set(["offline-mode"]);

/** 分类标签：两个平台的分类各有一份中文对照表，没收录的新分类原样显示 */
export const tagsOf = (hit: ProjectHit, t: TFunction, max = MAX_TAGS) =>
  hit.displayCategories
    .filter(
      (c) =>
        hit.source !== "modrinth" || (!MODRINTH_LOADERS.has(c) && !HIDDEN_MODRINTH_TAGS.has(c)),
    )
    .slice(0, max)
    .map((c) => t(`discover.tag.${hit.source}.${c}`, { defaultValue: c }));

/** MC百科搜索页（只是打开网页，不用他们的数据）。名字里的「[Fabric & Forge]」之类去掉，搜得更准 */
export const mcmodSearchUrl = (title: string) => {
  const name = title.replace(/[[(（【].*?[\])）】]/g, "").trim() || title;
  return `https://search.mcmod.cn/s?key=${encodeURIComponent(name)}`;
};

/** 任务键里的来源名（和后端一致） */
export const sourceKey = (source: ContentSource) =>
  source === "curseForge" ? "curseforge" : "modrinth";

/** 收藏夹里记的类型 */
export const favoriteKindOf = (kind: InstallKind): FavoriteKind => kind;

// ---------- 页面级缓存 ----------
// 后端已经缓存了 10 分钟的搜索结果；这里再记一份「已经合并好的列表」，
// 切回同样的分类、关键词时直接显示，不等打字防抖、不闪加载（目标：有缓存时 < 300 毫秒）。

export interface CachedResults {
  entries: Entry[];
  cursors: Record<ContentSource, Cursor>;
  hasMore: boolean;
  cfNotice: string | null;
  /** 回来时滚到原来的位置 */
  scrollTop: number;
  at: number;
}

const RESULTS_TTL = 10 * 60_000;
const MAX_CACHED = 40;
const results = new Map<string, CachedResults>();

export const resultsKey = (
  kind: DiscoverKind,
  source: SourceFilter,
  target: string,
  query: string,
  sort: SearchSort = "relevance",
  category: string | null = null,
  extra = "",
) => `${kind}|${source}|${target}|${sort}|${category ?? ""}|${extra}|${query.trim().toLowerCase()}`;

export function cachedResults(key: string): CachedResults | undefined {
  const hit = results.get(key);
  if (!hit) return undefined;
  if (Date.now() - hit.at > RESULTS_TTL) {
    results.delete(key);
    return undefined;
  }
  return hit;
}

export function rememberResults(key: string, value: Omit<CachedResults, "at">): void {
  results.delete(key);
  results.set(key, { ...value, at: Date.now() });
  while (results.size > MAX_CACHED) {
    const oldest = results.keys().next().value;
    if (oldest === undefined) break;
    results.delete(oldest);
  }
}

export function rememberScroll(key: string, scrollTop: number): void {
  const hit = results.get(key);
  if (hit) hit.scrollTop = scrollTop;
}

// ---------- 详情页的兼容性 ----------

/**
 * 支持的游戏版本（已经从新到旧）压缩成几组：同一个「主.次」版本只有一个就原样显示（26.3），
 * 有好几个就合成「1.21.x」。最多显示 `max` 组，多的放进 `hidden`（界面上收成「+N」，悬停能看）。
 */
export function groupVersions(versions: string[], max = 10): { shown: string[]; hidden: string[] } {
  const order: string[] = [];
  const members = new Map<string, string[]>();
  for (const version of versions) {
    const key = version.split(".").slice(0, 2).join(".");
    const list = members.get(key);
    if (list) list.push(version);
    else {
      members.set(key, [version]);
      order.push(key);
    }
  }
  const groups = order.map((key) => {
    const list = members.get(key) ?? [];
    return list.length > 1 ? `${key}.x` : (list[0] ?? key);
  });
  return { shown: groups.slice(0, max), hidden: groups.slice(max) };
}

export type Support = "yes" | "noVersion" | "noLoader";

/**
 * 这个项目能不能装进这个实例：先看游戏版本，模组再看加载器（资源包、光影包不看加载器）。
 * 平台没给版本信息时返回 null（不下结论）。
 */
export function supportOf(
  versions: string[],
  loaders: string[],
  kind: InstallKind,
  instance: Pick<InstanceSummary, "gameVersion" | "loader">,
): Support | null {
  if (versions.length === 0) return null;
  if (!versions.includes(instance.gameVersion)) return "noVersion";
  if (kind === "mod" && loaders.length > 0) {
    // 实例的加载器写成「NeoForge 21.1.265」，取第一个词比
    const mine = instance.loader?.split(" ")[0]?.toLowerCase();
    if (!mine || !loaders.includes(mine)) return "noLoader";
  }
  return "yes";
}
