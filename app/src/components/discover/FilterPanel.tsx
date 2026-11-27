import { Check, Lock, RotateCcw, SlidersHorizontal } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { LoaderBadgeIcon } from "@/components/art/LoaderBadgeIcon";
import { Segmented } from "@/components/ui/Segmented";
import { Select } from "@/components/ui/Select";
import { Switch } from "@/components/ui/Switch";
import { unwrap } from "@/lib/api";
import { commands, type LoaderKind, type SearchSort } from "@/lib/bindings";
import { type Category, categoryFor, categoryGroups, onlyOn, SORTS } from "@/lib/categories";
import type { DiscoverKind, SourceFilter } from "@/lib/discover";
import { fixedSource, isContentKind } from "@/lib/discover";
import { duration, ease, spring } from "@/lib/motion";

export interface FilterValue {
  sort: SearchSort;
  /** 分类的键（见 `lib/categories.ts`），不筛时是 null */
  category: string | null;
  hideInstalled: boolean;
  /** 手动指定的 Minecraft 版本（整合包页签或解锁实例筛选时生效） */
  gameVersion: string | null;
  /** 手动指定的加载器（整合包/模组页签且未锁实例时生效） */
  loader: LoaderKind | null;
  /** 仅显示客户端模组（仅模组页签生效） */
  clientOnly: boolean;
}

export const DEFAULT_FILTER: FilterValue = {
  sort: "relevance",
  category: null,
  hideInstalled: false,
  gameVersion: null,
  loader: null,
  clientOnly: false,
};

const LOADER_OPTIONS: { kind: LoaderKind; label: string }[] = [
  { kind: "neoforge", label: "NeoForge" },
  { kind: "fabric", label: "Fabric" },
  { kind: "forge", label: "Forge" },
  { kind: "quilt", label: "Quilt" },
];

let cachedGameVersions: string[] | null = null;

/** 生效了几个条件（按钮上显示） */
export const activeCount = (value: FilterValue) =>
  Number(value.sort !== "relevance") +
  Number(value.category !== null) +
  Number(value.hideInstalled) +
  Number(value.gameVersion !== null) +
  Number(value.loader !== null) +
  Number(value.clientOnly);

/** 这个分类在现在搜的平台上有没有（只搜一个平台时，另一个平台独有的分类不显示） */
export function availableOn(category: Category, kind: DiscoverKind, source: SourceFilter): boolean {
  const only = fixedSource(kind) ?? (source === "all" ? null : source);
  return only === null || categoryFor(category, only) !== undefined;
}

/**
 * 资源中心的「筛选」：搜索框旁边一个胶囊按钮，点开是克制紧凑的滚动浮层（不常驻占一栏，不超出窗口底边）。
 * 排序、游戏版本、加载器、分类（单选）、仅客户端模组、隐藏已安装；改了马上生效，不用点「确定」。
 */
export function FilterPanel({
  kind,
  source,
  value,
  canHide,
  lockedInstance,
  onUnlockInstance,
  onChange,
}: {
  kind: DiscoverKind;
  source: SourceFilter;
  value: FilterValue;
  /** 有目标实例、这一类能判断「已安装」时才有「隐藏已安装」 */
  canHide: boolean;
  /** 已锁定到目标实例时展示跟随状态，点「自定义」解锁后可手动选版本和加载器 */
  lockedInstance: { gameVersion: string; loader: string | null } | null;
  onUnlockInstance?: () => void;
  onChange: (next: FilterValue) => void;
}) {
  const { t } = useTranslation();
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const [gameVersions, setGameVersions] = useState<string[] | null>(cachedGameVersions);
  const rootRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const open = anchor !== null;

  const supportsVersion = kind === "modpack" || isContentKind(kind) || kind === "map";
  const supportsLoader = kind === "modpack" || kind === "mod";
  const supportsClientOnly = kind === "mod" && source !== "curseForge";
  const showManualVersion = supportsVersion && !lockedInstance;

  const count = activeCount({
    ...value,
    hideInstalled: value.hideInstalled && canHide,
    gameVersion: showManualVersion ? value.gameVersion : null,
    loader: showManualVersion && supportsLoader ? value.loader : null,
    clientOnly: supportsClientOnly && value.clientOnly,
  });

  useEffect(() => {
    if (!open || !showManualVersion || cachedGameVersions !== null) return;
    let live = true;
    commands
      .catalogGameVersions()
      .then(unwrap)
      .then((list) => {
        cachedGameVersions = list;
        if (live) setGameVersions(list);
      })
      .catch(() => {
        if (live) setGameVersions([]);
      });
    return () => {
      live = false;
    };
  }, [open, showManualVersion]);

  useEffect(() => {
    if (!open) return;
    const close = () => setAnchor(null);
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !panelRef.current?.contains(target)) close();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", close);
    };
  }, [open]);

  const groups = categoryGroups(kind)
    .map((g) => ({ ...g, items: g.items.filter((c) => availableOn(c, kind, source)) }))
    .filter((g) => g.items.length > 0);
  const selected = groups.flatMap((g) => g.items).find((c) => c.key === value.category);
  const only = source === "all" && fixedSource(kind) === null ? onlyOn(selected) : null;
  const set = (patch: Partial<FilterValue>) => onChange({ ...value, ...patch });

  const chip = (key: string | null, label: string) => {
    const active = value.category === key;
    return (
      <button
        key={key ?? "all"}
        type="button"
        aria-pressed={active}
        onClick={() => set({ category: key })}
        className={`inline-flex h-7 items-center gap-1 rounded-control border px-2.5 text-xs transition-colors duration-(--motion-press) ${
          active
            ? "border-transparent bg-accent-soft font-medium text-accent"
            : "border-line bg-surface text-fg hover:bg-surface-hover"
        }`}
      >
        {active && <Check className="size-3" aria-hidden />}
        {label}
      </button>
    );
  };

  // 动态限制高度：始终紧贴按钮下方展开，且在任何窗口尺寸下底部至少保留 20px 呼吸边距，绝不裁切内容
  const panelTop = anchor ? anchor.bottom + 6 : 0;
  const panelMaxHeight = anchor
    ? Math.max(240, Math.min(460, window.innerHeight - panelTop - 20))
    : 460;

  return (
    <div ref={rootRef} className="shrink-0">
      <button
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={(event) => setAnchor(open ? null : event.currentTarget.getBoundingClientRect())}
        className={`inline-flex h-9 items-center gap-2 rounded-control border px-4 font-medium text-sm shadow-raised transition-colors duration-(--motion-press) ${
          count > 0
            ? "border-transparent bg-accent-soft text-accent"
            : "border-line bg-surface text-fg hover:bg-surface-hover"
        }`}
      >
        <SlidersHorizontal className="size-4" aria-hidden />
        {t("discover.filter.button")}
        {count > 0 && (
          <span className="grid min-w-5 place-items-center rounded-tag bg-accent px-1.5 text-on-accent text-xs leading-5">
            {count}
          </span>
        )}
      </button>
      {createPortal(
        <AnimatePresence>
          {anchor && (
            <motion.div
              ref={panelRef}
              role="dialog"
              aria-label={t("discover.filter.title")}
              className="fixed z-50 flex w-[min(436px,calc(100vw-32px))] origin-top-right flex-col overflow-hidden rounded-panel border border-line bg-overlay text-fg shadow-overlay"
              style={{
                right: window.innerWidth - anchor.right,
                top: panelTop,
                maxHeight: panelMaxHeight,
              }}
              initial={{ opacity: 0, scale: 0.97, y: -4 }}
              animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
              exit={{
                opacity: 0,
                scale: 0.98,
                transition: { duration: duration.press, ease: ease.in },
              }}
            >
              {/* 顶部固定栏：标题与一键恢复默认 */}
              <div className="flex shrink-0 items-center justify-between border-line border-b px-4 py-2.5">
                <span className="font-medium text-fg text-sm">{t("discover.filter.title")}</span>
                {count > 0 && (
                  <button
                    type="button"
                    onClick={() => onChange(DEFAULT_FILTER)}
                    className="inline-flex items-center gap-1 rounded-control px-2 py-0.5 text-accent text-xs transition-colors duration-(--motion-press) hover:bg-accent-soft"
                  >
                    <RotateCcw className="size-3" aria-hidden />
                    {t("discover.filter.reset")}
                  </button>
                )}
              </div>

              {/* 中部紧凑可滚动主体 */}
              <div className="flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto p-4">
                <section className="flex flex-col gap-1.5">
                  <h3 className="font-medium text-fg-muted text-xs">{t("discover.filter.sort")}</h3>
                  <Segmented<SearchSort>
                    label={t("discover.filter.sort")}
                    value={value.sort}
                    onChange={(sort) => set({ sort })}
                    fullWidth
                    options={SORTS.map((s) => ({
                      value: s,
                      label: t(`discover.filter.sortName.${s}`),
                    }))}
                  />
                </section>

                {supportsVersion && lockedInstance && (
                  <div className="flex items-center justify-between gap-2 rounded-control border border-line bg-surface-2 px-3 py-2 text-xs">
                    <span className="flex min-w-0 items-center gap-1.5 text-fg-muted">
                      <Lock className="size-3.5 shrink-0" aria-hidden />
                      <span className="truncate">
                        {t("discover.filter.followingInstance", {
                          spec: [
                            `Minecraft ${lockedInstance.gameVersion}`,
                            supportsLoader ? lockedInstance.loader : null,
                          ]
                            .filter(Boolean)
                            .join(" · "),
                        })}
                      </span>
                    </span>
                    {onUnlockInstance && (
                      <button
                        type="button"
                        onClick={onUnlockInstance}
                        className="shrink-0 rounded-control px-2 py-0.5 font-medium text-accent transition-colors duration-(--motion-press) hover:bg-accent-soft"
                      >
                        {t("discover.filter.customize")}
                      </button>
                    )}
                  </div>
                )}

                {showManualVersion && (
                  <section className="flex flex-col gap-2.5">
                    <div className="flex flex-col gap-1.5">
                      <h3 className="font-medium text-fg-muted text-xs">
                        {t("discover.filter.gameVersion")}
                      </h3>
                      <Select
                        label={t("discover.filter.gameVersion")}
                        value={value.gameVersion ?? ""}
                        onChange={(next) => set({ gameVersion: next || null })}
                        options={[
                          { value: "", label: t("discover.filter.allVersions") },
                          ...(gameVersions ?? []).map((gv) => ({
                            value: gv,
                            label: `Minecraft ${gv}`,
                          })),
                        ]}
                      />
                    </div>

                    {supportsLoader && (
                      <div className="flex flex-col gap-1.5">
                        <h3 className="font-medium text-fg-muted text-xs">
                          {t("discover.filter.loader")}
                        </h3>
                        <div className="flex flex-wrap gap-1.5">
                          <button
                            type="button"
                            aria-pressed={value.loader === null}
                            onClick={() => set({ loader: null })}
                            className={`inline-flex h-7 items-center gap-1 rounded-control border px-2.5 text-xs transition-colors duration-(--motion-press) ${
                              value.loader === null
                                ? "border-transparent bg-accent-soft font-medium text-accent"
                                : "border-line bg-surface text-fg hover:bg-surface-hover"
                            }`}
                          >
                            {value.loader === null && <Check className="size-3" aria-hidden />}
                            {t("discover.filter.allLoaders")}
                          </button>
                          {LOADER_OPTIONS.map((opt) => {
                            const active = value.loader === opt.kind;
                            return (
                              <button
                                key={opt.kind}
                                type="button"
                                aria-pressed={active}
                                onClick={() => set({ loader: active ? null : opt.kind })}
                                className={`inline-flex h-7 items-center gap-1 rounded-control border px-2.5 text-xs transition-colors duration-(--motion-press) ${
                                  active
                                    ? "border-transparent bg-accent-soft font-medium text-accent"
                                    : "border-line bg-surface text-fg hover:bg-surface-hover"
                                }`}
                              >
                                <LoaderBadgeIcon loader={opt.label} className="size-3.5 shrink-0" />
                                {opt.label}
                              </button>
                            );
                          })}
                        </div>
                      </div>
                    )}
                  </section>
                )}

                {groups.length > 0 && (
                  <section className="flex flex-col gap-1.5">
                    <h3 className="font-medium text-fg-muted text-xs">
                      {t("discover.filter.category")}
                    </h3>
                    {groups.length === 1 ? (
                      <div className="flex flex-wrap gap-1.5">
                        {chip(null, t("discover.filter.allCategories"))}
                        {groups[0]?.items.map((c) => chip(c.key, t(`discover.category.${c.key}`)))}
                      </div>
                    ) : (
                      <div className="flex flex-col gap-2">
                        <div className="flex flex-wrap gap-1.5">
                          {chip(null, t("discover.filter.allCategories"))}
                        </div>
                        {groups.map((group) => (
                          <div key={group.group} className="flex flex-col gap-1">
                            <p className="text-2xs text-fg-muted">
                              {t(`discover.filter.group.${group.group}`)}
                            </p>
                            <div className="flex flex-wrap gap-1.5">
                              {group.items.map((c) => chip(c.key, t(`discover.category.${c.key}`)))}
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                    {only && selected && (
                      <p className="text-fg-muted text-xs">
                        {t("discover.filter.onlyOn", {
                          category: t(`discover.category.${selected.key}`),
                          source: t(`discover.sourceName.${only}`),
                        })}
                      </p>
                    )}
                  </section>
                )}

                {(supportsClientOnly || canHide) && (
                  <section className="flex flex-col gap-2.5 border-line border-t pt-3">
                    {supportsClientOnly && (
                      <label className="flex items-center justify-between gap-3 text-sm">
                        <span className="min-w-0">
                          <span className="block">{t("discover.filter.clientOnly")}</span>
                          <span className="block text-fg-muted text-xs">
                            {t("discover.filter.clientOnlyHint")}
                          </span>
                        </span>
                        <Switch
                          label={t("discover.filter.clientOnly")}
                          checked={value.clientOnly}
                          onChange={(clientOnly) => set({ clientOnly })}
                        />
                      </label>
                    )}
                    {canHide && (
                      <label className="flex items-center justify-between gap-3 text-sm">
                        <span>
                          {t(
                            kind === "server"
                              ? "discover.filter.hideJoined"
                              : "discover.filter.hideInstalled",
                          )}
                        </span>
                        <Switch
                          label={t("discover.filter.hideInstalled")}
                          checked={value.hideInstalled}
                          onChange={(hideInstalled) => set({ hideInstalled })}
                        />
                      </label>
                    )}
                  </section>
                )}
              </div>
            </motion.div>
          )}
        </AnimatePresence>,
        document.body,
      )}
    </div>
  );
}
