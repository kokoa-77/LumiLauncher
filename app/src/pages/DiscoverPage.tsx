import { ArrowUp, CircleAlert, Compass, Heart, Lock, LockOpen, Search } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";
import emptySearch from "@/assets/art/empty-search.webp";
import {
  availableOn,
  DEFAULT_FILTER,
  FilterPanel,
  type FilterValue,
} from "@/components/discover/FilterPanel";
import { InstallTray } from "@/components/discover/InstallTray";
import { ResultCard, ResultRow } from "@/components/discover/ResultItem";
import { InstanceThumb, TargetPicker } from "@/components/discover/TargetPicker";
import {
  type InstallKind,
  useInstall,
  useInstalledContent,
} from "@/components/discover/useInstall";
import { instanceSubtitle } from "@/components/InstanceCard";
import { EmptyState } from "@/components/ui/EmptyState";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Segmented } from "@/components/ui/Segmented";
import { Tooltip } from "@/components/ui/Tooltip";
import { VirtualList } from "@/components/VirtualList";
import { CommandError, unwrap } from "@/lib/api";
import {
  type ContentKind,
  type ContentSource,
  commands,
  type NameMatch,
  type ProjectHit,
  type SearchPage,
} from "@/lib/bindings";
import { categoryFor, findCategory, isSort } from "@/lib/categories";
import {
  addPages,
  cachedResults,
  type DiscoverKind,
  type Entry,
  entryKey,
  fixedSource,
  freshCursors,
  hasMoreIn,
  isContentKind,
  KINDS,
  pickTarget,
  rememberResults,
  rememberScroll,
  resultsKey,
  SOURCES,
  type SourceFilter,
  sameKey,
  toEntry,
  wantedSources,
} from "@/lib/discover";
import { PAGE_CONTENT_MAX, PAGE_X } from "@/lib/layout";
import { duration, ease, spring } from "@/lib/motion";
import { logError, reportError, userMessage } from "@/lib/report";
import { useAppStore, useEffectsLevel } from "@/stores/app";
import { useFavorites } from "@/stores/favorites";
import { useGameStore } from "@/stores/game";

/** 打字停下多久后再搜索（毫秒）；切分类、切平台不等 */
const SEARCH_DELAY = 250;
/** 卡片最窄多宽（像素），按窗口宽度决定一排放几张 */
const CARD_MIN = 290;
const GAP = 16;
/** 页面左右留白（和 px-8 一致） */
const PAD = 32;
/** 往下滚过这么多（像素）才出现「回到顶部」 */
const TOP_BUTTON_AFTER = 900;
/** 「隐藏已安装」后剩下的太少时自动多加载几页，但最多连着加载这么多页 */
const MAX_AUTO_PAGES = 5;
const ENOUGH_ROWS = 12;

type View = "cards" | "list";
const VIEW_KEY = "mcl.discoverView";

function savedView(): View | null {
  try {
    const v = localStorage.getItem(VIEW_KEY);
    return v === "cards" || v === "list" ? v : null;
  } catch {
    return null;
  }
}

/** 一条结果 + 它是哪一类（收藏夹里每条的类型不一样） */
interface Row {
  entry: Entry;
  kind: InstallKind;
}

/** 只有每行列数变化时才重新分组卡片。 */
function useColumns() {
  const ref = useRef<HTMLDivElement>(null);
  const [columns, setColumns] = useState<number | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const width = entry.contentRect.width;
      const inner = Math.min(width - 2 * PAD, PAGE_CONTENT_MAX);
      const count = Math.max(1, Math.min(4, Math.floor((inner + GAP) / (CARD_MIN + GAP))));
      setColumns(width > 0 ? count : null);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return { ref, columns };
}

/**
 * 资源中心：一个页面，两种进法。
 * - 从实例「添加内容」进来（`?from=instance&instance=…`）：页头是这个实例，游戏版本和加载器锁定（可以解锁），
 *   装过的显示「已安装」，装完底下出「进世界试试」
 * - 从侧栏进来：右上角选「装到」哪个实例，默认是首页那个世界所在的实例
 *
 * 同时搜 Modrinth 和 CurseForge（也可以只搜一个），同名的合成一条；输入就搜，滚到底自动加载下一页。
 * 精美模式默认大图卡片，流畅模式默认列表，可以手动换。
 */
export function DiscoverPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const instances = useGameStore((s) => s.instances);
  const recent = useGameStore((s) => s.recentWorlds);
  const effects = useEffectsLevel();
  const favorites = useFavorites((s) => s.items);
  const loadFavorites = useFavorites((s) => s.load);

  const kindParam = params.get("kind") as DiscoverKind | null;
  const kind: DiscoverKind = kindParam && KINDS.includes(kindParam) ? kindParam : "mod";
  const sourceParam = params.get("source") as SourceFilter | null;
  const sourceFilter: SourceFilter =
    sourceParam && SOURCES.includes(sourceParam) ? sourceParam : "all";
  const fromInstance = params.get("from") === "instance";
  const unlocked = params.get("unlock") === "1";
  // 筛选：都记在地址里，从详情页回来还在
  const sortParam = params.get("sort");
  const sort = isSort(sortParam) ? sortParam : "relevance";
  const foundCategory = findCategory(kind, params.get("cat"));
  const category =
    foundCategory && availableOn(foundCategory, kind, sourceFilter) ? foundCategory : undefined;
  const hideInstalled = params.get("hide") === "1";
  const instanceId = pickTarget(instances, params.get("instance"));
  const instance = instances.find((i) => i.id === instanceId);
  const worldOf = useMemo(
    () => new Map((recent ?? []).map((r) => [r.instanceId, r.world])),
    [recent],
  );

  const [text, setText] = useState(() => params.get("q") ?? "");
  const [query, setQuery] = useState(text);
  const typing = useRef(false);
  const [view, setView] = useState<View>(
    () => savedView() ?? (effects === "rich" ? "cards" : "list"),
  );

  // 中文搜索：模组的关键词里有汉字时先查中文名表，拿第一个的英文名去平台搜；
  // 结果上方列出其他可能的，点一下换成它，也能按原文搜
  const wantsNames = kind === "mod" && HAN.test(query);
  const [names, setNames] = useState<{ query: string; matches: NameMatch[] } | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  useEffect(() => {
    if (!wantsNames || names?.query === query) return;
    let live = true;
    commands
      .namesMatch(query)
      .then((matches) => {
        if (!live) return;
        setNames({ query, matches });
        setPicked(null);
      })
      .catch((error: unknown) => {
        logError(error, "查中文名表");
        if (live) setNames({ query, matches: [] });
      });
    return () => {
      live = false;
    };
  }, [wantsNames, query]);
  const namesReady = !wantsNames || names?.query === query;
  const matches = wantsNames && names?.query === query ? names.matches : [];
  const searchText = matches.length > 0 ? (picked ?? matches[0]?.en ?? query) : query;

  // 按实例筛：只有模组、资源包、光影包看实例；解锁后不筛
  const searchTarget = (isContentKind(kind) || kind === "map") && !unlocked ? instanceId : "";
  // 首选平台：两个平台都有的项目显示、安装哪边的；「全部来源」时也先排它
  const preferred = useAppStore((s) => s.settings?.download.preferredPlatform ?? "modrinth");
  const key = `${resultsKey(kind, sourceFilter, searchTarget, searchText, sort, category?.key ?? null)}|${preferred}`;
  // 要搜的平台：只有一个平台有的分类，只搜那一个
  const only = fixedSource(kind);
  const activeSources: ContentSource[] = (
    only
      ? [only]
      : sourceFilter === "all"
        ? preferred === "curseForge"
          ? (["curseForge", "modrinth"] as const)
          : (["modrinth", "curseForge"] as const)
        : [sourceFilter]
  ).filter((s) => categoryFor(category, s) !== undefined);
  const cached = kind === "favorites" ? undefined : cachedResults(key);

  const [entries, setEntries] = useState<Entry[] | null>(cached?.entries ?? null);
  const [hasMore, setHasMore] = useState(cached?.hasMore ?? false);
  const [loading, setLoading] = useState(false);
  const [cfNotice, setCfNotice] = useState<string | null>(cached?.cfNotice ?? null);
  const requestId = useRef(0);
  const cursors = useRef(cached?.cursors ?? freshCursors());
  const scrollTop = useRef(cached?.scrollTop ?? 0);
  const scroller = useRef<HTMLDivElement | null>(null);
  const [showTop, setShowTop] = useState((cached?.scrollTop ?? 0) > TOP_BUTTON_AFTER);

  const installedKinds: ContentKind[] = isContentKind(kind)
    ? [kind]
    : kind === "favorites"
      ? ["mod", "resourcePack", "shader"]
      : [];
  const installed = useInstalledContent(instanceId, installedKinds);
  const installer = useInstall(instanceId, instance?.name ?? "");

  useEffect(() => {
    if (favorites === null) void loadFavorites();
  }, [favorites, loadFavorites]);

  const setParam = (name: string, value: string | null) => setParamsOf({ [name]: value });
  const setParamsOf = (changes: Record<string, string | null>) => {
    const next = new URLSearchParams(params);
    for (const [name, value] of Object.entries(changes)) {
      if (value === null) next.delete(name);
      else next.set(name, value);
    }
    setParams(next, { replace: true });
  };
  const changeFilter = (next: FilterValue) =>
    setParamsOf({
      sort: next.sort === "relevance" ? null : next.sort,
      cat: next.category,
      hide: next.hideInstalled ? "1" : null,
    });

  // 地址里的关键词从外面变了（已经在资源中心时又用命令面板搜了一次）：跟着换
  const qParam = params.get("q") ?? "";
  useEffect(() => {
    if (qParam === query) return;
    setText(qParam);
    setQuery(qParam);
    // 只跟着地址走；打字时地址是在搜之后才改的，那时 query 已经一样了
  }, [qParam]);

  // 打字：停一会儿再搜；关键词也记进地址，从详情页回来还在
  useEffect(() => {
    if (text === query) return;
    const timer = setTimeout(() => {
      typing.current = true;
      setQuery(text);
      setParam("q", text.trim() ? text : null);
    }, SEARCH_DELAY);
    return () => clearTimeout(timer);
    // 只跟着输入走
  }, [text]);

  const fetchPage = async (source: ContentSource, offset: number): Promise<SearchPage> => {
    const cat = categoryFor(category, source) ?? null;
    const text = searchText;
    if (kind === "modpack")
      return unwrap(await commands.modpackSearch(text, source, sort, cat, offset));
    if (kind === "map")
      return unwrap(await commands.mapsSearch(text, searchTarget || null, sort, cat, offset));
    if (kind === "server") return unwrap(await commands.serversSearch(text, sort, cat, offset));
    const target = searchTarget || null;
    const contentKind = kind as ContentKind;
    return unwrap(
      source === "curseForge"
        ? await commands.curseforgeSearch(text, contentKind, target, sort, cat, offset)
        : await commands.modrinthSearch(text, contentKind, target, sort, cat, offset),
    );
  };

  const search = async (reset: boolean) => {
    if (kind === "favorites") return;
    const id = ++requestId.current;
    if (reset) {
      cursors.current = freshCursors();
      scrollTop.current = 0;
    }
    const sources = activeSources;
    const wanted = wantedSources(cursors.current, sources, sort);
    if (wanted.length === 0) return;
    setLoading(true);
    const results = await Promise.all(
      wanted.map(async (source) => {
        try {
          return { source, page: await fetchPage(source, cursors.current[source].offset) };
        } catch (error) {
          return { source, error };
        }
      }),
    );
    if (id !== requestId.current) return;
    const pages: Partial<Record<ContentSource, ProjectHit[]>> = {};
    let notice = reset ? null : cfNotice;
    for (const result of results) {
      const cursor = cursors.current[result.source];
      if ("page" in result && result.page) {
        cursor.offset += result.page.hits.length;
        cursor.total = result.page.hits.length === 0 ? cursor.offset : result.page.total;
        pages[result.source] = result.page.hits;
        continue;
      }
      cursor.total = cursor.offset;
      if (result.source === "curseForge") {
        // CurseForge 搜不了不挡 Modrinth：在列表上方说明原因
        logError(result.error, "搜索 CurseForge");
        const denied =
          result.error instanceof CommandError && result.error.code === "curseforge.denied";
        notice = denied
          ? t("discover.cfDenied")
          : t("discover.cfFailed", { message: userMessage(result.error) });
      } else {
        reportError(result.error, "搜索资源");
      }
    }
    const next = addPages(
      reset ? [] : (entries ?? []),
      cursors.current,
      pages,
      sources,
      sort,
      preferred,
    );
    const more = hasMoreIn(cursors.current, sources);
    setEntries(next);
    setHasMore(more);
    setCfNotice(notice);
    setLoading(false);
    rememberResults(key, {
      entries: next,
      cursors: structuredClone(cursors.current),
      hasMore: more,
      cfNotice: notice,
      scrollTop: scrollTop.current,
    });
  };

  // 分类、平台、实例、关键词变了：有缓存就直接显示，没有就搜
  // key 已经包含了所有搜索条件
  useEffect(() => {
    typing.current = false;
    if (kind === "favorites") {
      setLoading(false);
      return;
    }
    // 中文名表还没查完：先别拿中文原文去搜
    if (!namesReady) {
      setEntries(null);
      return;
    }
    const hit = cachedResults(key);
    if (hit) {
      requestId.current++;
      cursors.current = structuredClone(hit.cursors);
      scrollTop.current = hit.scrollTop;
      setEntries(hit.entries);
      setHasMore(hit.hasMore);
      setCfNotice(hit.cfNotice);
      setLoading(false);
      return;
    }
    setEntries(null);
    void search(true);
  }, [key, namesReady]);

  // 「隐藏已安装」：装了的（服务器是已加入的）不显示；在这一页里刚点了安装的留着，不会一点就消失
  const canHide = Boolean(instanceId) && (isContentKind(kind) || kind === "server");
  const keepVisible = useRef(new Set<string>());
  const isInstalled = (entry: Entry, itemKind: InstallKind) => {
    if (keepVisible.current.has(entryKey(entry))) return false;
    const state = installer.stateOf(entry, itemKind, installed).kind;
    return state === "done" || state === "joined";
  };

  // 收藏夹：本地筛，不联网
  const rows: Row[] = useMemo(() => {
    if (kind === "favorites") {
      const q = sameKey(query);
      return (favorites ?? [])
        .filter((f) => sourceFilter === "all" || f.hit.source === sourceFilter)
        .filter((f) => !q || sameKey(f.hit.title).includes(q))
        .map((f) => ({ entry: toEntry(f.hit), kind: f.kind }));
    }
    const all = (entries ?? []).map((entry) => ({ entry, kind: kind as InstallKind }));
    return hideInstalled && canHide ? all.filter((r) => !isInstalled(r.entry, r.kind)) : all;
    // isInstalled 跟着已安装列表变；列表变了要重新筛
  }, [kind, favorites, entries, query, sourceFilter, hideInstalled, canHide, installed]);
  const hiddenCount = kind === "favorites" ? 0 : (entries?.length ?? 0) - rows.length;

  // 隐藏已安装后剩下的太少：自动再加载几页（最多 MAX_AUTO_PAGES 页，免得全装过时一直翻）
  const autoPages = useRef(0);
  useEffect(() => {
    autoPages.current = 0;
  }, [key, hideInstalled]);
  useEffect(() => {
    if (!hideInstalled || hiddenCount === 0 || loading || !hasMore || entries === null) return;
    if (rows.length >= ENOUGH_ROWS || autoPages.current >= MAX_AUTO_PAGES) return;
    autoPages.current++;
    void search(false);
  }, [hideInstalled, hiddenCount, loading, hasMore, rows.length, entries]);

  const { ref: listBox, columns } = useColumns();
  const cols = columns ?? 1;
  const grid = useMemo(() => {
    const out: Row[][] = [];
    for (let i = 0; i < rows.length; i += cols) out.push(rows.slice(i, i + cols));
    return out;
  }, [rows, cols]);

  const changeView = (next: View) => {
    setView(next);
    try {
      localStorage.setItem(VIEW_KEY, next);
    } catch {
      // 记不住就按画面效果选，不影响使用
    }
  };

  const openDetail = ({ entry, kind: itemKind }: Row) => {
    const next = new URLSearchParams(params);
    next.set("kind", itemKind);
    if (kind === "favorites") next.set("fromKind", "favorites");
    navigate(
      `/discover/${entry.hit.source}/${encodeURIComponent(entry.hit.projectId)}?${next.toString()}`,
      { state: { entry } },
    );
  };

  const itemProps = (row: Row) => ({
    entry: row.entry,
    kind: row.kind,
    state: installer.stateOf(row.entry, row.kind, installed),
    disabled:
      row.kind !== "modpack" &&
      !(row.kind === "server" && row.entry.hit.server?.modpackVersion) &&
      !instanceId,
    onOpen: () => openDetail(row),
    onInstall: () => {
      keepVisible.current.add(entryKey(row.entry));
      void installer.start(row.entry, row.kind);
    },
    showKind: kind === "favorites",
  });

  const placeholder = t(`discover.placeholder.${kind}`);
  const showTarget = !fromInstance && kind !== "modpack";
  const empty =
    kind === "favorites"
      ? rows.length === 0 && favorites !== null
      : entries !== null && rows.length === 0 && !loading && !(hiddenCount > 0 && hasMore);
  const filterValue: FilterValue = {
    ...DEFAULT_FILTER,
    sort,
    category: category?.key ?? null,
    hideInstalled,
  };

  const footer = (
    <div className="flex h-16 items-center justify-center text-fg-muted text-sm">
      {loading && entries !== null && <LightSpinner className="size-5 text-accent" />}
      {!loading && !hasMore && rows.length > 0 && kind !== "favorites" && t("discover.end")}
    </div>
  );

  return (
    <div className="relative flex h-full flex-col">
      <div className={`${PAGE_X} flex flex-col gap-4 pt-8 pb-4`}>
        {fromInstance && instance ? (
          <header className="flex items-center gap-4">
            <InstanceThumb
              instance={instance}
              world={worldOf.get(instance.id)}
              className="h-12 w-20 rounded-item"
            />
            <div className="min-w-0 flex-1">
              <p className="text-fg-muted text-xs">{t("discover.addTo")}</p>
              <h1 className="truncate font-semibold text-2xl">{instance.name}</h1>
              <p className="truncate text-fg-muted text-xs">{instanceSubtitle(instance)}</p>
            </div>
          </header>
        ) : (
          <header className="flex items-center gap-3">
            <h1 className="mr-auto font-semibold text-2xl">{t("discover.title")}</h1>
            {showTarget && (
              <TargetPicker
                instances={instances}
                worldOf={worldOf}
                value={instanceId}
                onChange={(id) => setParam("instance", id)}
              />
            )}
          </header>
        )}

        <div className="flex flex-wrap items-center gap-3">
          <Segmented
            label={t("discover.kind")}
            value={kind}
            // 每一类的分类不一样：换类型时分类清空（排序、隐藏已安装留着）
            onChange={(value) => setParamsOf({ kind: value, cat: null })}
            options={KINDS.filter((k) => !fromInstance || k !== "modpack").map((k) => ({
              value: k,
              label: t(`kind.${k}`),
            }))}
          />
          {fixedSource(kind) === null && (
            <Segmented
              label={t("discover.source")}
              value={sourceFilter}
              onChange={(value) => setParam("source", value)}
              options={SOURCES.map((s) => ({ value: s, label: t(`discover.sourceName.${s}`) }))}
            />
          )}
          <div className="flex min-w-72 flex-1 items-center gap-3">
            <div className="relative min-w-0 flex-1">
              <Search
                className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3.5 size-4 text-fg-muted"
                aria-hidden
              />
              <input
                value={text}
                onChange={(event) => setText(event.target.value)}
                placeholder={placeholder}
                aria-label={placeholder}
                className="h-9 w-full rounded-control border border-line bg-surface pr-4 pl-10 text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus:border-accent"
              />
            </div>
            {kind !== "favorites" && (
              <FilterPanel
                kind={kind}
                source={sourceFilter}
                value={filterValue}
                canHide={canHide}
                onChange={changeFilter}
              />
            )}
          </div>
        </div>

        {/* 第二排：左边是提示（版本锁定、整合包说明……），右边一直是视图切换，位置不会跟着分类跳 */}
        <div className="flex min-h-9 items-center gap-3">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            {(isContentKind(kind) || kind === "map") && instance && (
              <div className="flex flex-wrap items-center gap-2 text-fg-muted text-xs">
                {unlocked ? (
                  <LockOpen className="size-3.5 text-warning" aria-hidden />
                ) : (
                  <Lock className="size-3.5" aria-hidden />
                )}
                <span>
                  {unlocked
                    ? t("discover.unlockedHint")
                    : kind === "map"
                      ? t("discover.filteredMap", { version: instance.gameVersion })
                      : t("discover.filtered", {
                          version: instance.gameVersion,
                          loader: instance.loader ?? t("create.vanilla"),
                        })}
                </span>
                <button
                  type="button"
                  onClick={() => setParam("unlock", unlocked ? null : "1")}
                  className="rounded-control px-2 py-0.5 text-accent transition-colors duration-(--motion-press) hover:bg-accent-soft"
                >
                  {unlocked ? t("discover.relock") : t("discover.unlock")}
                </button>
              </div>
            )}
            {kind === "modpack" && (
              <p className="text-fg-muted text-xs">{t("discover.modpackHint")}</p>
            )}
            {kind === "server" && (
              <p className="text-fg-muted text-xs">{t("discover.serverHint")}</p>
            )}
            {kind === "favorites" && (
              <p className="text-fg-muted text-xs">{t("discover.favoritesHint")}</p>
            )}
            {hideInstalled && canHide && hiddenCount > 0 && (
              <p className="text-fg-muted text-xs">
                {t(kind === "server" ? "discover.filter.hiddenJoined" : "discover.filter.hidden", {
                  n: hiddenCount,
                })}
              </p>
            )}
          </div>
          <Segmented<View>
            label={t("discover.view")}
            value={view}
            onChange={changeView}
            options={[
              { value: "cards", label: t("discover.cards") },
              { value: "list", label: t("discover.list") },
            ]}
          />
        </div>
        {cfNotice && kind !== "favorites" && (
          <div className="flex items-start gap-2 rounded-panel border border-line bg-surface p-3 text-sm">
            <CircleAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
            <p className="min-w-0 flex-1">{cfNotice}</p>
          </div>
        )}
        {matches.length > 0 && (
          <NameChoices
            matches={matches}
            raw={query.trim()}
            current={searchText}
            onPick={setPicked}
          />
        )}
      </div>

      <div ref={listBox} className="flex min-h-0 flex-1 flex-col">
        {empty ? (
          <div className={`${PAGE_X} pb-8`}>
            {kind === "favorites" ? (
              <EmptyState
                icon={Heart}
                title={t("discover.favoritesEmpty")}
                hint={t("discover.favoritesEmptyHint")}
              />
            ) : hiddenCount > 0 ? (
              <EmptyState
                icon={Compass}
                title={t("discover.filter.allHidden")}
                hint={t("discover.filter.allHiddenHint")}
              />
            ) : (
              <EmptyState
                icon={Compass}
                illustration={emptySearch}
                title={t("discover.noResult")}
                hint={
                  category || sort !== "relevance"
                    ? t("discover.filter.noResultHint")
                    : t("discover.noResultHint")
                }
              />
            )}
          </div>
        ) : entries === null && kind !== "favorites" ? (
          <DiscoverSkeleton view={view} />
        ) : view === "cards" && columns !== null ? (
          <VirtualList
            key={`${key}|cards`}
            className="min-h-0 flex-1"
            items={grid}
            estimate={340}
            overscan={1}
            getKey={(row) => row.map((r) => entryKey(r.entry)).join("|")}
            initialOffset={scrollTop.current}
            scrollRef={(el) => {
              scroller.current = el;
            }}
            onScrollOffset={(top) => {
              scrollTop.current = top;
              setShowTop(top > TOP_BUTTON_AFTER);
              rememberScroll(key, top);
            }}
            onEndReached={() => hasMore && !loading && void search(false)}
            footer={footer}
            render={(row) => (
              <div
                className={`${PAGE_X} grid gap-4 pb-4`}
                style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }}
              >
                {row.map((r) => (
                  <ResultCard key={entryKey(r.entry)} {...itemProps(r)} />
                ))}
              </div>
            )}
          />
        ) : view === "list" ? (
          <VirtualList
            key={`${key}|list`}
            className="min-h-0 flex-1"
            items={rows}
            estimate={96}
            overscan={4}
            getKey={(r) => entryKey(r.entry)}
            initialOffset={scrollTop.current}
            scrollRef={(el) => {
              scroller.current = el;
            }}
            onScrollOffset={(top) => {
              scrollTop.current = top;
              setShowTop(top > TOP_BUTTON_AFTER);
              rememberScroll(key, top);
            }}
            onEndReached={() => hasMore && !loading && void search(false)}
            footer={footer}
            itemClassName="pb-2"
            // 行是绝对定位、铺满宽度的，里面再套一层页面容器才能居中
            render={(r) => (
              <div className={PAGE_X}>
                <ResultRow {...itemProps(r)} />
              </div>
            )}
          />
        ) : null}
      </div>

      <AnimatePresence>
        {showTop && (
          <motion.div
            className="absolute right-8 bottom-6 z-30"
            initial={{ opacity: 0, scale: 0.9 }}
            animate={{ opacity: 1, scale: 1, transition: spring.snappy }}
            exit={{
              opacity: 0,
              scale: 0.9,
              transition: { duration: duration.press, ease: ease.in },
            }}
          >
            <Tooltip label={t("discover.backToTop")} side="top">
              <button
                type="button"
                aria-label={t("discover.backToTop")}
                onClick={() => {
                  // 流畅模式、减少动效时直接跳上去，不做平滑滚动
                  const root = document.documentElement.dataset;
                  const instant = root.fx === "lite" || root.reduceMotion === "true";
                  scroller.current?.scrollTo({ top: 0, behavior: instant ? "auto" : "smooth" });
                }}
                className="grid size-11 place-items-center rounded-icon border border-line bg-overlay text-fg shadow-overlay transition-colors duration-(--motion-press) hover:text-accent"
              >
                <ArrowUp className="size-5" aria-hidden />
              </button>
            </Tooltip>
          </motion.div>
        )}
      </AnimatePresence>
      {fromInstance && instance && <InstallTray instanceId={instance.id} />}
      {installer.dialogs}
    </div>
  );
}

/** 有没有汉字（中文搜索要先查中文名表） */
const HAN = /\p{Script=Han}/u;

const SKELETON_ITEMS = [0, 1, 2, 3, 4, 5];

/** 发现内容加载态骨架占位：按当前卡片 / 列表视图展示低对比占位，首帧即自适应列数避免跳变 */
function DiscoverSkeleton({ view }: { view: View }) {
  if (view === "list") {
    return (
      <div aria-busy className={`${PAGE_X} flex flex-col gap-2 pb-6`}>
        {SKELETON_ITEMS.map((item) => (
          <div
            key={item}
            className="lumi-skeleton flex items-center gap-4 rounded-panel border border-line bg-surface p-3"
          >
            <div className="size-12 shrink-0 rounded-row bg-surface-hover" />
            <div className="flex min-w-0 flex-1 flex-col gap-2">
              <div className="h-4 w-44 rounded-row bg-surface-hover" />
              <div className="h-3.5 w-3/4 rounded-row bg-surface-hover" />
            </div>
            <div className="h-9 w-28 shrink-0 rounded-control bg-surface-hover" />
          </div>
        ))}
      </div>
    );
  }
  return (
    <div
      aria-busy
      className={`${PAGE_X} grid grid-cols-[repeat(auto-fill,minmax(290px,1fr))] gap-4 pb-6`}
    >
      {SKELETON_ITEMS.map((item) => (
        <div
          key={item}
          className="lumi-skeleton flex flex-col rounded-panel border border-line bg-surface p-1.5"
        >
          <div className="aspect-[2/1] rounded-item bg-surface-hover" />
          <div className="flex flex-col gap-2.5 px-2.5 pt-3 pb-2">
            <div className="flex items-center gap-3">
              <div className="size-11 shrink-0 rounded-row bg-surface-hover" />
              <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                <div className="h-4 w-2/3 rounded-row bg-surface-hover" />
                <div className="h-3 w-1/3 rounded-row bg-surface-hover" />
              </div>
            </div>
            <div className="h-3.5 w-full rounded-row bg-surface-hover" />
            <div className="h-3.5 w-4/5 rounded-row bg-surface-hover" />
            <div className="flex items-center justify-between pt-2">
              <div className="h-3.5 w-20 rounded-row bg-surface-hover" />
              <div className="h-9 w-28 rounded-control bg-surface-hover" />
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

/**
 * 中文搜索换成了哪个英文名：只有一个时说一句「已按 Create 搜索」；
 * 好几个时列出来点着换；最后一个是按原文搜（平台上有中文名字的项目）。
 */
function NameChoices({
  matches,
  raw,
  current,
  onPick,
}: {
  matches: NameMatch[];
  raw: string;
  current: string;
  onPick: (en: string) => void;
}) {
  const { t } = useTranslation();
  const chip = (selected: boolean) =>
    `inline-flex h-7 shrink-0 items-center gap-1.5 rounded-control px-3 text-xs transition-colors duration-(--motion-press) ${
      selected ? "bg-accent-soft text-accent" : "border border-line hover:bg-surface-hover"
    }`;
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
      <span className="text-fg-muted">
        {matches.length === 1
          ? t("discover.zh.searchedAs", { name: matches[0]?.en ?? "" })
          : t("discover.zh.maybe")}
      </span>
      {matches.length > 1 &&
        matches.map((m) => {
          const selected = m.en === current;
          return (
            <button
              key={`${m.modrinth ?? ""}:${m.curseforge ?? ""}:${m.en}`}
              type="button"
              aria-pressed={selected}
              onClick={() => onPick(m.en)}
              className={chip(selected)}
            >
              {m.zh}
              <span className={selected ? "" : "text-fg-muted"}>{m.en}</span>
            </button>
          );
        })}
      <button
        type="button"
        aria-pressed={current === raw}
        onClick={() => onPick(raw)}
        className={chip(current === raw)}
      >
        {t("discover.zh.raw", { text: raw })}
      </button>
    </div>
  );
}
