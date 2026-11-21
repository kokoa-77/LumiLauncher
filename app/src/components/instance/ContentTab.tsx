import {
  ArrowLeftRight,
  ArrowUpCircle,
  BookOpen,
  Copy,
  ExternalLink,
  FolderSearch,
  Heart,
  History,
  Lock,
  LockOpen,
  PackageOpen,
  Plus,
  RefreshCw,
  Search,
  SearchX,
  Trash2,
  UserRound,
} from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { ProjectName } from "@/components/discover/ProjectName";
import { open } from "@/components/discover/ResultItem";
import { HistoryDialog } from "@/components/HistoryDialog";
import { Button } from "@/components/ui/Button";
import { Checkbox } from "@/components/ui/Checkbox";
import { EmptyState } from "@/components/ui/EmptyState";
import { HoverHint, Truncated } from "@/components/ui/HoverCard";
import { IconButton } from "@/components/ui/IconButton";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Menu, type MenuItem } from "@/components/ui/Menu";
import { Segmented } from "@/components/ui/Segmented";
import { Select } from "@/components/ui/Select";
import { Switch } from "@/components/ui/Switch";
import { VirtualList } from "@/components/VirtualList";
import { unwrap } from "@/lib/api";
import {
  type ContentItem,
  type ContentKind,
  type ContentSource,
  commands,
  type InstanceSummary,
  type ProjectHit,
  type UpdateInfo,
} from "@/lib/bindings";
import {
  CONTENT_SORTS,
  type ContentFilter,
  type ContentOrigin,
  type ContentSort,
  loadSort,
  originsIn,
  saveSort,
  toggleSelection,
  viewContent,
} from "@/lib/contentView";
import { mcmodSearchUrl, toEntry } from "@/lib/discover";
import { logError, reportError } from "@/lib/report";
import { updateCacheKey, updatesMapOf, useContentUpdatesStore } from "@/stores/contentUpdates";
import { useFavorites, useIsFavorite } from "@/stores/favorites";
import { isActive, useGameStore } from "@/stores/game";

const SwitchVersionDialog = lazy(async () => ({
  default: (await import("@/components/instance/SwitchVersionDialog")).SwitchVersionDialog,
}));

/** 另一个平台（判断「已收藏」时两边都要比） */
const otherSource = (source: ContentSource): ContentSource =>
  source === "modrinth" ? "curseForge" : "modrinth";

/** 这一项是怎么认出来的（第二行小字） */
function sourceKey(item: ContentItem) {
  if (item.source) return item.source;
  return item.isFolder ? "folder" : "unknown";
}

/** Modrinth、CurseForge 上认出来的才有详情页、能收藏 */
function platformOf(item: ContentItem): ContentSource | null {
  return item.projectId && (item.source === "modrinth" || item.source === "curseForge")
    ? item.source
    : null;
}

/** 用内容列表里的信息拼一份搜索结果：点进详情页时先画出页头；收藏时读不到详情也用它 */
function hitOf(item: ContentItem, source: ContentSource): ProjectHit {
  return {
    projectId: item.projectId ?? "",
    slug: item.projectId ?? "",
    title: item.title,
    description: "",
    author: item.author ?? "",
    iconUrl: item.iconUrl,
    downloads: null,
    displayCategories: [],
    source,
    side: null,
    pageUrl: item.pageUrl,
    authorUrl: item.authorUrl,
    titleZh: item.titleZh,
    twinId: item.twinId,
  };
}

/** 没有平台的那一行也要调 `useIsFavorite`（钩子不能有条件），给它一个谁都对不上的 */
const NO_HIT = hitOf({ projectId: null, title: "" } as ContentItem, "modrinth");

/** 已收藏：主色实心的心（和资源中心的收藏按钮一个样子） */
function FilledHeart({ className }: { className?: string }) {
  return <Heart className={`${className ?? ""} fill-current text-accent`} aria-hidden />;
}

/** 每一行「⋮」里的东西：切换版本、锁定版本、在文件夹中显示、在平台打开、复制链接、收藏、作者主页、MC百科 */
function useRowMenu(
  instanceId: string,
  item: ContentItem,
  running: boolean,
  onToggleLock: () => void,
  onSwitchVersion: () => void,
): MenuItem[] {
  const { t } = useTranslation();
  const { pageUrl, authorUrl, author, source } = item;
  const platform = platformOf(item);
  const hit = platform ? hitOf(item, platform) : NO_HIT;
  const favorite = useIsFavorite(
    hit,
    item.twinId ? { [otherSource(hit.source)]: item.twinId } : {},
  );
  const toggleFavorite = useFavorites((s) => s.toggle);
  const name = item.titleZh ?? item.title;
  const favoriteItem = async () => {
    // 收藏是「切换」：先确定收藏夹读过了，按读到的判断，别把收藏过的又取消掉
    const store = useFavorites.getState();
    if (store.items === null) await store.load();
    const twin = item.twinId;
    const saved = useFavorites
      .getState()
      .items?.find(
        (f) =>
          (f.hit.source === hit.source && f.hit.projectId === hit.projectId) ||
          (twin !== null && f.hit.source === otherSource(hit.source) && f.hit.projectId === twin),
      );
    if (saved) {
      // 可能是在另一个平台上收藏的：按收藏夹里存的那一份取消
      await toggleFavorite(saved.kind, saved.hit);
      toast.success(t("content.unfavorited", { name }));
      return;
    }
    // 收藏夹里要显示简介、下载量：先读一下详情，读不到就用列表里有的
    let full = hit;
    try {
      const detail = unwrap(await commands.discoverProject(hit.source, hit.projectId)).hit;
      // 平台和编号照列表里的存，「已收藏」才对得上
      full = { ...detail, source: hit.source, projectId: hit.projectId };
    } catch (error) {
      logError(error, "收藏前读取项目详情");
    }
    await toggleFavorite(item.kind, full);
    toast.success(t("content.favorited", { name }));
  };
  return [
    ...(platform && !item.isFolder && !running
      ? [
          {
            key: "switchVersion",
            label: t("content.switchVersion"),
            icon: ArrowLeftRight,
            onSelect: onSwitchVersion,
          },
        ]
      : []),
    ...(!item.isFolder
      ? [
          {
            key: "lock",
            label: item.locked ? t("content.unlockVersion") : t("content.lockVersion"),
            icon: item.locked ? LockOpen : Lock,
            onSelect: onToggleLock,
          },
        ]
      : []),
    {
      key: "reveal",
      label: t("content.reveal"),
      icon: FolderSearch,
      onSelect: () => {
        commands
          .contentReveal(instanceId, item.kind, item.fileName)
          .then(unwrap)
          .catch((error: unknown) => reportError(error, "在文件夹中显示"));
      },
    },
    ...(pageUrl && (source === "modrinth" || source === "curseForge")
      ? [
          {
            key: "page",
            label: t("discover.openOn", { source: t(`discover.sourceName.${source}`) }),
            icon: ExternalLink,
            onSelect: () => open(pageUrl, "打开项目页"),
          },
          {
            key: "copy",
            label: t("discover.copyLink"),
            icon: Copy,
            onSelect: () => {
              navigator.clipboard
                .writeText(pageUrl)
                .then(() => toast.success(t("discover.copied")))
                .catch((error: unknown) => reportError(error, "复制链接"));
            },
          },
        ]
      : []),
    ...(platform
      ? [
          {
            key: "favorite",
            label: favorite ? t("content.unfavorite") : t("content.favorite"),
            icon: favorite ? FilledHeart : Heart,
            onSelect: () => void favoriteItem(),
          },
        ]
      : []),
    ...(authorUrl
      ? [
          {
            key: "author",
            label: t("discover.openAuthor", { name: author }),
            icon: UserRound,
            onSelect: () => open(authorUrl, "打开作者主页"),
          },
        ]
      : []),
    {
      key: "mcmod",
      label: t("discover.mcmod"),
      icon: BookOpen,
      onSelect: () => open(mcmodSearchUrl(item.title), "打开 MC百科"),
    },
  ];
}

interface RowProps {
  instanceId: string;
  item: ContentItem;
  update?: UpdateInfo;
  updating: boolean;
  selected: boolean;
  selecting: boolean;
  /** 游戏开着：开关、删除、更新、勾选都锁住（游戏锁着文件，改了也不生效） */
  locked: boolean;
  onSelect: (shift: boolean) => void;
  onToggle: (enabled: boolean) => void;
  onToggleLock: () => void;
  onSwitchVersion: () => void;
  onUpdate: () => void;
  onRemove: () => void;
}

function ContentRow({
  instanceId,
  item,
  update,
  updating,
  selected,
  selecting,
  locked,
  onSelect,
  onToggle,
  onToggleLock,
  onSwitchVersion,
  onUpdate,
  onRemove,
}: RowProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const menu = useRowMenu(instanceId, item, locked, onToggleLock, onSwitchVersion);
  const { author, authorUrl } = item;
  const platform = platformOf(item);
  const nameClass = `truncate text-sm ${
    selected ? "text-accent" : item.enabled ? "text-fg" : "text-fg-muted"
  }`;
  // 点名字进资源详情页（从实例进来的那种：标出「已安装」，返回回到这个实例）
  const openDetail = () => {
    if (!platform || !item.projectId) return;
    const params = new URLSearchParams({ kind: item.kind, instance: instanceId, from: "content" });
    navigate(`/discover/${platform}/${encodeURIComponent(item.projectId)}?${params.toString()}`, {
      state: { entry: toEntry(hitOf(item, platform)) },
    });
  };
  return (
    // 多选时点整行就是选它（里面的按钮自己拦住点击；勾选框本身能用键盘）
    <div
      onClick={selecting ? (event) => onSelect(event.shiftKey) : undefined}
      className={`group flex items-center gap-3 rounded-item px-2 py-2 transition-colors duration-(--motion-press) ${
        selected ? "bg-accent-soft" : "hover:bg-surface-hover"
      } ${selecting ? "cursor-pointer" : ""}`}
    >
      {/* 勾选框单独一列、一直在：不压住图标，选不选都不会挤动别的东西 */}
      <Checkbox
        checked={selected}
        disabled={locked}
        onChange={(_, shift) => onSelect(shift)}
        label={t("content.select", { name: item.title })}
      />
      {item.iconUrl ? (
        <img
          src={item.iconUrl}
          alt=""
          className="size-9 shrink-0 rounded-row object-cover"
          loading="lazy"
        />
      ) : (
        <div className="grid size-9 shrink-0 place-items-center rounded-row bg-surface-2 text-fg-muted">
          <PackageOpen className="size-4" aria-hidden />
        </div>
      )}
      <div className="min-w-0 flex-1">
        {platform ? (
          <button
            type="button"
            onClick={(event) => {
              // 多选时点名字也是选这一行
              if (selecting) return;
              event.stopPropagation();
              openDetail();
            }}
            className="block max-w-full text-left transition-colors duration-(--motion-press) hover:text-accent hover:underline"
          >
            <ProjectName
              as="span"
              className={`block ${nameClass}`}
              title={item.title}
              titleZh={item.titleZh}
            />
          </button>
        ) : (
          <ProjectName as="p" className={nameClass} title={item.title} titleZh={item.titleZh} />
        )}
        <p className="flex min-w-0 items-center gap-1.5 text-fg-muted text-xs">
          {author &&
            (authorUrl ? (
              <HoverHint
                hint={t("discover.openAuthor", { name: author })}
                className="inline-flex max-w-40 shrink-0"
              >
                <button
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    open(authorUrl, "打开作者主页");
                  }}
                  className="truncate transition-colors duration-(--motion-press) hover:text-accent hover:underline"
                >
                  {author}
                </button>
              </HoverHint>
            ) : (
              <span className="max-w-40 shrink-0 truncate">{author}</span>
            ))}
          {author && <span aria-hidden>·</span>}
          <Truncated className="min-w-0 truncate" text={item.versionNumber ?? item.fileName} />
          {item.locked && (
            <HoverHint hint={t("content.lockedHint")} className="inline-flex shrink-0 items-center">
              <Lock className="size-3 text-accent" aria-label={t("content.lockedHint")} />
            </HoverHint>
          )}
          <span aria-hidden>·</span>
          <span className="shrink-0">{t(`content.source.${sourceKey(item)}`)}</span>
          {update && !item.locked && (
            <span className="shrink-0 text-accent">
              {t("content.updateTo", { version: update.versionNumber })}
            </span>
          )}
        </p>
      </div>
      {update && !item.locked && (
        <Button
          variant="outline"
          size="sm"
          disabled={updating || locked}
          onClick={(event) => {
            event.stopPropagation();
            onUpdate();
          }}
        >
          {updating ? (
            <LightSpinner className="size-3.5" />
          ) : (
            <ArrowUpCircle className="size-3.5" aria-hidden />
          )}
          {t("content.update")}
        </Button>
      )}
      {!item.isFolder && !locked && (
        <IconButton
          label={t("content.remove", { name: item.title })}
          size="sm"
          danger
          onClick={(event) => {
            event.stopPropagation();
            onRemove();
          }}
          className="opacity-0 focus-visible:opacity-100 group-hover:opacity-100"
        >
          <Trash2 className="size-4" aria-hidden />
        </IconButton>
      )}
      <Menu label={t("content.more", { name: item.title })} items={menu} />
      {/* 拦住点击：多选时点开关不会顺带选中这一行 */}
      <div onClick={(event) => event.stopPropagation()}>
        <Switch
          checked={item.enabled}
          disabled={locked}
          onChange={onToggle}
          label={t(item.kind === "shader" ? "content.useShader" : "content.enable", {
            name: item.title,
          })}
        />
      </div>
    </div>
  );
}

/**
 * 「内容」页签：模组 / 光影包 / 资源包的列表。
 * - 第一排：类型、检查更新、改动历史、添加内容
 * - 第二排：在已装的里搜索、筛选（全部 / 已启用 / 已停用 / 有更新）、排序（记住上次选的）
 * - 列表的表头：全选 + 共几项；选了东西时换成批量操作（启用、停用、更新、删除——都能一次撤销）
 * - 每一行：勾选框（单独一列）、图标、作者（能点）、版本、来源、更新、删除、「⋮」、开关
 */
export function ContentTab({
  instance,
  kind,
  onKind,
}: {
  instance: InstanceSummary;
  kind: ContentKind;
  onKind: (kind: ContentKind) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const id = instance.id;
  // 「⋮」里的收藏要知道哪些已经收藏了：没读过收藏夹就读一次（不然会把收藏过的当成没收藏）
  const favoritesLoaded = useFavorites((s) => s.items !== null);
  const loadFavorites = useFavorites((s) => s.load);
  useEffect(() => {
    if (!favoritesLoaded) void loadFavorites();
  }, [favoritesLoaded, loadFavorites]);
  const contentVersion = useGameStore((s) => s.contentVersion);
  const jobs = useGameStore((s) => s.jobs);
  const bumpContent = useGameStore((s) => s.bumpContent);
  const cacheKey = updateCacheKey(id, kind);
  const updateEntry = useContentUpdatesStore((s) => s.entries[cacheKey]);
  const checkingUpdates = useContentUpdatesStore((s) => !!s.checking[cacheKey]);
  const runCheckUpdates = useContentUpdatesStore((s) => s.check);
  const invalidateUpdates = useContentUpdatesStore((s) => s.invalidate);

  const [items, setItems] = useState<ContentItem[] | null>(null);
  const [refreshingLocal, setRefreshingLocal] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [switchingItem, setSwitchingItem] = useState<ContentItem | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ContentFilter>("all");
  const [origin, setOrigin] = useState<ContentOrigin | "all">("all");
  const [sort, setSort] = useState<ContentSort>(loadSort);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [batchBusy, setBatchBusy] = useState(false);
  const anchor = useRef<string | null>(null);
  const loadId = useRef(0);

  const updates = useMemo(() => {
    if (!updateEntry || updateEntry.contentVersion !== contentVersion) return {};
    const lockedNames = new Set((items ?? []).filter((i) => i.locked).map((i) => i.fileName));
    const map = updatesMapOf(updateEntry);
    if (lockedNames.size === 0) return map;
    return Object.fromEntries(Object.entries(map).filter(([name]) => !lockedNames.has(name)));
  }, [updateEntry, contentVersion, items]);
  const checking = checkingUpdates || refreshingLocal;

  // 先用缓存秒出列表，再在后台认出还不认识的文件、补作者
  const load = useCallback(async () => {
    const current = ++loadId.current;
    try {
      const quick = unwrap(await commands.contentList(id, kind));
      if (current !== loadId.current) return quick;
      setItems(quick);
      const refreshed = unwrap(await commands.contentRefresh(id, kind));
      if (current === loadId.current) setItems(refreshed);
      return refreshed;
    } catch (error) {
      if (current !== loadId.current) return null;
      setItems((old) => old ?? []);
      reportError(error, "读取实例内容");
      return null;
    }
  }, [id, kind]);

  useEffect(() => {
    void load();
  }, [load, contentVersion]);

  // 进入实例内容页或切换类型后自动静默检查更新（与首页共享会话缓存；纯原版模组页或空列表静默跳过）
  const hasItems = (items?.length ?? 0) > 0;
  const canAutoCheck = hasItems && (kind !== "mod" || instance.loader !== null);
  useEffect(() => {
    if (!canAutoCheck) return;
    void runCheckUpdates(id, kind).catch(() => {
      // 自动检查静默失败，避免离线时每次进实例都弹红框打扰
    });
  }, [canAutoCheck, id, kind, contentVersion, runCheckUpdates]);

  // 换了类型：条件和选择都清掉
  useEffect(() => {
    void kind;
    setItems(null);
    setQuery("");
    setFilter("all");
    setOrigin("all");
    setSelected(new Set());
    anchor.current = null;
  }, [kind]);

  // 已经没有的文件从选择里去掉
  useEffect(() => {
    if (!items) return;
    const present = new Set(items.map((i) => i.fileName));
    setSelected((prev) => {
      const next = new Set([...prev].filter((k) => present.has(k)));
      return next.size === prev.size ? prev : next;
    });
  }, [items]);

  // 「有更新」筛到最后一个也更新完了，回到全部
  useEffect(() => {
    if (filter === "updates" && Object.keys(updates).length === 0) setFilter("all");
  }, [filter, updates]);

  // 「来源」筛选：列表里有两种以上来源时才出现
  const origins = useMemo(() => originsIn(items ?? []), [items]);
  const activeOrigin = origins.includes(origin as ContentOrigin) ? origin : "all";
  const visible = useMemo(
    () => viewContent(items ?? [], { query, filter, sort, updates, origin: activeOrigin }),
    [items, query, filter, sort, updates, activeOrigin],
  );
  const visibleKeys = useMemo(() => visible.map((i) => i.fileName), [visible]);
  const running = useGameStore((s) => isActive(s.sessions[id]));
  const selecting = selected.size > 0 && !running;

  // Esc 取消选择
  useEffect(() => {
    if (!selecting) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSelected(new Set());
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selecting]);

  const updateJob = Object.values(jobs).find(
    (j) => j.key === `content:${id}:update` && j.state === "running",
  );

  const select = (key: string, shift: boolean) => {
    setSelected((prev) => toggleSelection(prev, visibleKeys, key, anchor.current, shift));
    anchor.current = key;
  };

  const toggle = async (item: ContentItem, enabled: boolean) => {
    // 乐观更新：光影同一时间只能用一个
    setItems(
      (list) =>
        list?.map((i) =>
          i.fileName === item.fileName
            ? { ...i, enabled }
            : kind === "shader" && enabled
              ? { ...i, enabled: false }
              : i,
        ) ?? null,
    );
    try {
      unwrap(await commands.contentSetEnabled(id, kind, item.fileName, enabled));
    } catch (error) {
      reportError(error, "切换内容开关");
    }
    void load();
  };

  /** 手动点「刷新」：重新扫描本地文件夹 + 强制绕过缓存重查远端更新 */
  const refreshAll = async () => {
    setRefreshingLocal(true);
    try {
      const refreshed = await load();
      if (!refreshed || refreshed.length === 0) return;
      if (kind === "mod" && instance.loader === null) return;
      const result = await runCheckUpdates(id, kind, { force: true });
      if (!result) return;
      const counts = { checked: result.checked, unchecked: result.unchecked };
      if (result.lookupFailed) {
        toast.error(t("content.updateCheckFailed", counts));
      } else if (result.updates.length > 0) {
        toast.success(t("content.updateCheckFound", { updates: result.updates.length }));
      } else {
        toast.success(t("content.updateCheckUpToDate"));
      }
    } catch (error) {
      reportError(error, "刷新内容并检查更新");
    } finally {
      setRefreshingLocal(false);
    }
  };

  const toggleLock = async (item: ContentItem) => {
    const nextLocked = !item.locked;
    setItems(
      (list) =>
        list?.map((i) => (i.fileName === item.fileName ? { ...i, locked: nextLocked } : i)) ?? null,
    );
    try {
      unwrap(await commands.contentSetLocked(id, kind, item.fileName, nextLocked));
      invalidateUpdates(id);
      bumpContent();
      const name = item.titleZh ?? item.title;
      toast.success(t(nextLocked ? "content.lockedToast" : "content.unlockedToast", { name }));
    } catch (error) {
      reportError(error, "切换版本锁定");
      void load();
    }
  };

  const runUpdate = async (list: UpdateInfo[]) => {
    try {
      unwrap(await commands.contentUpdate(id, list));
      toast.success(t("content.updateStarted", { n: list.length }));
    } catch (error) {
      reportError(error, "更新内容");
    }
  };

  /** 删完弹提示，带「撤销」（撤的是刚记下的那条改动历史） */
  const toastRemoved = async (message: string) => {
    const latest = unwrap(await commands.contentHistory(id))[0];
    bumpContent();
    toast.success(message, {
      action: latest && {
        label: t("history.undo"),
        onClick: () => {
          void commands
            .contentRollback(id, latest.id)
            .then(unwrap)
            .then(bumpContent)
            .catch((error: unknown) => reportError(error, "撤销删除"));
        },
      },
    });
  };

  const remove = async (item: ContentItem) => {
    try {
      unwrap(await commands.contentRemove(id, kind, item.fileName));
      await toastRemoved(t("content.removed", { name: item.title }));
    } catch (error) {
      reportError(error, "删除内容");
    }
  };

  const chosen = (items ?? []).filter((i) => selected.has(i.fileName));
  const chosenUpdates = chosen.flatMap((i) => (updates[i.fileName] ? [updates[i.fileName]!] : []));
  const removable = chosen.filter((i) => !i.isFolder);

  /** 把这些文件一起设成开或关（后端一次做完，出错就一个都不改），返回真的改了的文件名 */
  const applyEnabled = async (fileNames: string[], enabled: boolean) => {
    try {
      return unwrap(await commands.contentSetManyEnabled(id, kind, fileNames, enabled));
    } catch (error) {
      reportError(error, enabled ? "批量启用" : "批量停用");
      return [];
    }
  };

  // 批量启用 / 停用：提示条上带「撤销」，一点把刚才改的这几个全部改回去
  const setManyEnabled = async (enabled: boolean) => {
    const targets = chosen.filter((i) => i.enabled !== enabled).map((i) => i.fileName);
    setBatchBusy(true);
    const changed = await applyEnabled(targets, enabled);
    setBatchBusy(false);
    void load();
    if (changed.length === 0) return;
    setSelected(new Set());
    toast.success(
      t(enabled ? "content.batch.enabled" : "content.batch.disabled", { n: changed.length }),
      {
        action: {
          label: t("history.undo"),
          onClick: () => {
            void applyEnabled(changed, !enabled).then((back) => {
              void load();
              if (back.length > 0) toast.success(t("content.batch.undone", { n: back.length }));
            });
          },
        },
      },
    );
  };

  const removeMany = async () => {
    setBatchBusy(true);
    try {
      const n = unwrap(
        await commands.contentRemoveMany(
          id,
          kind,
          removable.map((i) => i.fileName),
        ),
      );
      setSelected(new Set());
      await toastRemoved(t("content.batch.removed", { n }));
    } catch (error) {
      reportError(error, "批量删除");
      void load();
    } finally {
      setBatchBusy(false);
    }
  };

  const pending = Object.values(updates);
  const narrowed = query.trim() !== "" || filter !== "all" || activeOrigin !== "all";
  const allVisibleChosen = visible.length > 0 && visible.every((i) => selected.has(i.fileName));

  const filterOptions: { value: ContentFilter; label: string }[] = [
    { value: "all", label: t("content.filter.all") },
    { value: "enabled", label: t("content.filter.enabled") },
    { value: "disabled", label: t("content.filter.disabled") },
    ...(pending.length > 0
      ? [{ value: "updates" as const, label: t("content.filter.updates", { n: pending.length }) }]
      : []),
  ];

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <Segmented
          label={t("detail.contentKind")}
          value={kind}
          onChange={onKind}
          options={[
            { value: "mod", label: t("kind.mod") },
            { value: "shader", label: t("kind.shader") },
            { value: "resourcePack", label: t("kind.resourcePack") },
          ]}
        />
        <div className="ml-auto flex flex-wrap items-center gap-2">
          {pending.length > 0 && (
            <Button
              variant="outline"
              disabled={!!updateJob}
              onClick={() => void runUpdate(pending)}
            >
              <ArrowUpCircle className="size-4" aria-hidden />
              {t("content.updateAll", { n: pending.length })}
            </Button>
          )}
          <HoverHint hint={t("content.refreshHint")}>
            <Button variant="secondary" disabled={checking} onClick={() => void refreshAll()}>
              <RefreshCw className={`size-4 ${checking ? "animate-spin" : ""}`} aria-hidden />
              {t("content.refresh")}
            </Button>
          </HoverHint>
          <Button variant="secondary" onClick={() => setHistoryOpen(true)}>
            <History className="size-4" aria-hidden />
            {t("history.open")}
          </Button>
          <Button
            variant="secondary"
            onClick={() =>
              navigate(`/discover?from=instance&instance=${encodeURIComponent(id)}&kind=${kind}`)
            }
          >
            <Plus className="size-4" aria-hidden />
            {t("detail.add")}
          </Button>
        </div>
      </div>

      {running && (
        <p className="flex shrink-0 items-center gap-2 text-fg-muted text-xs">
          <Lock className="size-3.5 shrink-0" aria-hidden />
          {t("content.lockedWhileRunning")}
        </p>
      )}
      {items !== null && items.length > 0 && (
        <div className="flex h-9 shrink-0 items-center gap-2">
          <div className="relative min-w-44 max-w-72 flex-1">
            <Search
              className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3.5 size-4 text-fg-muted"
              aria-hidden
            />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("content.search")}
              aria-label={t("content.search")}
              className="h-9 w-full rounded-control border border-line bg-surface pr-4 pl-10 text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus:border-accent"
            />
          </div>
          <Segmented
            label={t("content.filter.label")}
            value={filter}
            onChange={setFilter}
            options={filterOptions}
          />
          {origins.length > 1 && (
            <div className="w-36 shrink-0">
              <Select
                label={t("content.filter.source")}
                value={activeOrigin}
                onChange={(value) => setOrigin(value as ContentOrigin | "all")}
                options={[
                  { value: "all", label: t("content.filter.sourceAll") },
                  ...origins.map((o) => ({
                    value: o,
                    label: t(`content.filter.sourceName.${o}`),
                  })),
                ]}
              />
            </div>
          )}
          <div className="w-36 shrink-0">
            <Select
              label={t("content.sort.label")}
              value={sort}
              onChange={(value) => {
                const next = value as ContentSort;
                setSort(next);
                saveSort(next);
              }}
              options={CONTENT_SORTS.map((s) => ({ value: s, label: t(`content.sort.${s}`) }))}
            />
          </div>
        </div>
      )}

      <section className="min-h-0 flex-1">
        {items === null ? null : items.length === 0 ? (
          <EmptyState
            icon={PackageOpen}
            title={t(`detail.empty.${kind}`)}
            hint={t("detail.emptyHint")}
          />
        ) : visible.length === 0 ? (
          <EmptyState
            icon={SearchX}
            title={t("content.noMatch")}
            hint={t("content.noMatchHint")}
            action={
              <Button
                variant="secondary"
                onClick={() => {
                  setQuery("");
                  setFilter("all");
                  setOrigin("all");
                }}
              >
                {t("content.clearFilter")}
              </Button>
            }
          />
        ) : (
          <div className="flex h-full flex-col overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
            {/* 表头：勾选框和每一行的勾选框对齐 */}
            <div className="flex h-12 shrink-0 items-center gap-2 border-line border-b px-3.5">
              <Checkbox
                checked={allVisibleChosen}
                mixed={selecting && !allVisibleChosen}
                disabled={running}
                onChange={() => setSelected(allVisibleChosen ? new Set() : new Set(visibleKeys))}
                label={t("content.selectAll")}
              />
              {selecting ? (
                <>
                  <span className="mr-1 ml-1 font-medium text-sm tabular-nums">
                    {t("content.selected", { n: selected.size })}
                  </span>
                  {kind !== "shader" && (
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={batchBusy || chosen.every((i) => i.enabled)}
                      onClick={() => void setManyEnabled(true)}
                    >
                      {t("content.batch.enable")}
                    </Button>
                  )}
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={batchBusy || chosen.every((i) => !i.enabled)}
                    onClick={() => void setManyEnabled(false)}
                  >
                    {t("content.batch.disable")}
                  </Button>
                  {chosenUpdates.length > 0 && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={batchBusy || !!updateJob}
                      onClick={() => {
                        void runUpdate(chosenUpdates);
                        setSelected(new Set());
                      }}
                    >
                      <ArrowUpCircle className="size-3.5" aria-hidden />
                      {t("content.batch.update", { n: chosenUpdates.length })}
                    </Button>
                  )}
                  <Button
                    variant="danger"
                    size="sm"
                    disabled={batchBusy || removable.length === 0}
                    onClick={() => void removeMany()}
                  >
                    <Trash2 className="size-3.5" aria-hidden />
                    {t("content.batch.remove", { n: removable.length })}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="ml-auto"
                    onClick={() => setSelected(new Set())}
                  >
                    {t("content.batch.cancel")}
                  </Button>
                </>
              ) : (
                <span className="ml-1 text-fg-muted text-sm tabular-nums">
                  {narrowed
                    ? t("content.shown", { shown: visible.length, total: items.length })
                    : t("content.total", { n: items.length })}
                </span>
              )}
            </div>
            <VirtualList
              className="min-h-0 flex-1 pt-1.5"
              itemClassName="px-1.5"
              items={visible}
              estimate={56}
              overscan={4}
              getKey={(item) => item.fileName}
              footer={<div className="h-1.5" />}
              render={(item) => (
                <ContentRow
                  instanceId={id}
                  item={item}
                  update={updates[item.fileName]}
                  updating={!!updateJob}
                  selected={selected.has(item.fileName)}
                  selecting={selecting}
                  locked={running}
                  onSelect={(shift) => select(item.fileName, shift)}
                  onToggle={(enabled) => void toggle(item, enabled)}
                  onToggleLock={() => void toggleLock(item)}
                  onSwitchVersion={() => setSwitchingItem(item)}
                  onUpdate={() => {
                    const update = updates[item.fileName];
                    if (update) void runUpdate([update]);
                  }}
                  onRemove={() => void remove(item)}
                />
              )}
            />
          </div>
        )}
      </section>
      <HistoryDialog instanceId={id} open={historyOpen} onClose={() => setHistoryOpen(false)} />
      {switchingItem && (
        <Suspense fallback={null}>
          <SwitchVersionDialog
            instance={instance}
            item={switchingItem}
            onClose={() => setSwitchingItem(null)}
          />
        </Suspense>
      )}
    </div>
  );
}
