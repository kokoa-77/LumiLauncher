import { Check, SlidersHorizontal } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { Segmented } from "@/components/ui/Segmented";
import { Switch } from "@/components/ui/Switch";
import type { SearchSort } from "@/lib/bindings";
import { type Category, categoryFor, categoryGroups, onlyOn, SORTS } from "@/lib/categories";
import type { DiscoverKind, SourceFilter } from "@/lib/discover";
import { fixedSource } from "@/lib/discover";
import { duration, ease, spring } from "@/lib/motion";

export interface FilterValue {
  sort: SearchSort;
  /** 分类的键（见 `lib/categories.ts`），不筛时是 null */
  category: string | null;
  hideInstalled: boolean;
}

export const DEFAULT_FILTER: FilterValue = {
  sort: "relevance",
  category: null,
  hideInstalled: false,
};

/** 生效了几个条件（按钮上显示） */
export const activeCount = (value: FilterValue) =>
  Number(value.sort !== "relevance") +
  Number(value.category !== null) +
  Number(value.hideInstalled);

/** 这个分类在现在搜的平台上有没有（只搜一个平台时，另一个平台独有的分类不显示） */
export function availableOn(category: Category, kind: DiscoverKind, source: SourceFilter): boolean {
  const only = fixedSource(kind) ?? (source === "all" ? null : source);
  return only === null || categoryFor(category, only) !== undefined;
}

/**
 * 资源中心的「筛选」：搜索框旁边一个胶囊按钮，点开是浮层（不常驻占一栏，卡片不会变窄）。
 * 排序、分类（单选）、隐藏已安装；改了马上生效，不用点「确定」。
 */
export function FilterPanel({
  kind,
  source,
  value,
  canHide,
  onChange,
}: {
  kind: DiscoverKind;
  source: SourceFilter;
  value: FilterValue;
  /** 有目标实例、这一类能判断「已安装」时才有「隐藏已安装」 */
  canHide: boolean;
  onChange: (next: FilterValue) => void;
}) {
  const { t } = useTranslation();
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const open = anchor !== null;
  const count = activeCount({ ...value, hideInstalled: value.hideInstalled && canHide });

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
        className={`inline-flex h-8 items-center gap-1 rounded-control border px-3 text-sm transition-colors duration-(--motion-press) ${
          active
            ? "border-transparent bg-accent-soft font-medium text-accent"
            : "border-line bg-surface text-fg hover:bg-surface-hover"
        }`}
      >
        {active && <Check className="size-3.5" aria-hidden />}
        {label}
      </button>
    );
  };

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
              className="fixed z-50 flex max-h-[min(600px,calc(100vh-120px))] w-[min(440px,calc(100vw-32px))] origin-top-right flex-col gap-4 overflow-y-auto rounded-panel border border-line bg-overlay p-4 text-fg shadow-overlay"
              style={{ right: window.innerWidth - anchor.right, top: anchor.bottom + 6 }}
              initial={{ opacity: 0, scale: 0.97, y: -4 }}
              animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
              exit={{
                opacity: 0,
                scale: 0.98,
                transition: { duration: duration.press, ease: ease.in },
              }}
            >
              <section className="flex flex-col gap-2">
                <h3 className="font-medium text-fg-muted text-xs">{t("discover.filter.sort")}</h3>
                <Segmented<SearchSort>
                  label={t("discover.filter.sort")}
                  value={value.sort}
                  onChange={(sort) => set({ sort })}
                  options={SORTS.map((s) => ({
                    value: s,
                    label: t(`discover.filter.sortName.${s}`),
                  }))}
                />
              </section>

              {groups.length > 0 && (
                <section className="flex flex-col gap-2">
                  <h3 className="font-medium text-fg-muted text-xs">
                    {t("discover.filter.category")}
                  </h3>
                  <div className="flex flex-wrap gap-1.5">
                    {chip(null, t("discover.filter.allCategories"))}
                  </div>
                  {groups.map((group) => (
                    <div key={group.group} className="flex flex-col gap-1.5">
                      {groups.length > 1 && (
                        <p className="text-fg-muted text-xs">
                          {t(`discover.filter.group.${group.group}`)}
                        </p>
                      )}
                      <div className="flex flex-wrap gap-1.5">
                        {group.items.map((c) => chip(c.key, t(`discover.category.${c.key}`)))}
                      </div>
                    </div>
                  ))}
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

              {count > 0 && (
                <button
                  type="button"
                  onClick={() => onChange(DEFAULT_FILTER)}
                  className="self-start rounded-control px-2 py-1 text-accent text-sm transition-colors duration-(--motion-press) hover:bg-accent-soft"
                >
                  {t("discover.filter.reset")}
                </button>
              )}
            </motion.div>
          )}
        </AnimatePresence>,
        document.body,
      )}
    </div>
  );
}
