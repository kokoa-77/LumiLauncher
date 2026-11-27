// 实例内容页的排序、筛选、搜索（纯函数，方便测试）。
import type { ContentItem, UpdateInfo } from "./bindings";
import { logError } from "./report";

export type ContentSort = "name" | "nameDesc" | "fileName" | "modified";
export type ContentFilter = "all" | "enabled" | "disabled" | "updates" | "orphans";
/** 这个文件是怎么认出来的：Modrinth、CurseForge、文件自带的说明、还没认出来 */
export type ContentOrigin = "modrinth" | "curseForge" | "file" | "unknown";
export const CONTENT_ORIGINS: ContentOrigin[] = ["modrinth", "curseForge", "file", "unknown"];

export const originOf = (item: ContentItem): ContentOrigin => item.source ?? "unknown";

/** 列表里出现了哪几种来源（按固定顺序）；只有一种时不用给「来源」筛选 */
export function originsIn(items: ContentItem[]): ContentOrigin[] {
  const present = new Set(items.map(originOf));
  return CONTENT_ORIGINS.filter((o) => present.has(o));
}

export const CONTENT_SORTS: ContentSort[] = ["name", "nameDesc", "fileName", "modified"];

const SORT_KEY = "mcl.content.sort";

const collator = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });

/** 读上次选的排序（读不到就按名字） */
export function loadSort(): ContentSort {
  try {
    const saved = localStorage.getItem(SORT_KEY);
    if (saved && (CONTENT_SORTS as string[]).includes(saved)) return saved as ContentSort;
  } catch (error) {
    logError(error, "读取内容排序");
  }
  return "name";
}

export function saveSort(sort: ContentSort) {
  try {
    localStorage.setItem(SORT_KEY, sort);
  } catch (error) {
    logError(error, "保存内容排序");
  }
}

const byName = (a: ContentItem, b: ContentItem) =>
  collator.compare(a.title, b.title) || collator.compare(a.fileName, b.fileName);

const compare: Record<ContentSort, (a: ContentItem, b: ContentItem) => number> = {
  name: byName,
  nameDesc: (a, b) => byName(b, a),
  fileName: (a, b) => collator.compare(a.fileName, b.fileName),
  // 新改过的在前；没有时间的放最后
  modified: (a, b) => (b.modified ?? -1) - (a.modified ?? -1) || byName(a, b),
};

/** 在已装的内容里搜索：名字、中文名、文件名、作者都算 */
function matches(item: ContentItem, query: string) {
  if (!query) return true;
  return [item.title, item.titleZh ?? "", item.fileName, item.author ?? ""].some((value) =>
    value.toLowerCase().includes(query),
  );
}

/** 按筛选、搜索、排序得到要显示的那些（不改原数组） */
export function viewContent(
  items: ContentItem[],
  options: {
    query: string;
    filter: ContentFilter;
    sort: ContentSort;
    updates: Record<string, UpdateInfo>;
    /** 孤儿前置文件名集合（仅模组页签传入） */
    orphans?: ReadonlySet<string>;
    /** 只看某一种来源；不给时都看 */
    origin?: ContentOrigin | "all";
  },
): ContentItem[] {
  const query = options.query.trim().toLowerCase();
  const origin = options.origin ?? "all";
  return items
    .filter((item) => {
      if (origin !== "all" && originOf(item) !== origin) return false;
      if (options.filter === "enabled" && !item.enabled) return false;
      if (options.filter === "disabled" && item.enabled) return false;
      if (options.filter === "updates" && !options.updates[item.fileName]) return false;
      if (options.filter === "orphans" && !options.orphans?.has(item.fileName)) return false;
      return matches(item, query);
    })
    .sort(compare[options.sort]);
}

/**
 * 多选：点一下切换这一项；按住 Shift 时把上次点的那项到这一项之间全部设成和这一项一样。
 * `visible` 是当前显示的顺序。
 */
export function toggleSelection(
  selected: ReadonlySet<string>,
  visible: string[],
  key: string,
  anchor: string | null,
  range: boolean,
): Set<string> {
  const next = new Set(selected);
  const on = !selected.has(key);
  const from = anchor === null ? -1 : visible.indexOf(anchor);
  const to = visible.indexOf(key);
  if (range && from >= 0 && to >= 0) {
    const [start, end] = from < to ? [from, to] : [to, from];
    for (const k of visible.slice(start, end + 1)) {
      if (on) next.add(k);
      else next.delete(k);
    }
    return next;
  }
  if (on) next.add(key);
  else next.delete(key);
  return next;
}
