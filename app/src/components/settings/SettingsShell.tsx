import { type LucideIcon, Search, X } from "lucide-react";
import { motion } from "motion/react";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { duration, ease } from "@/lib/motion";
import { Dialog } from "../ui/Dialog";
import { Truncated } from "../ui/HoverCard";
import { IconButton } from "../ui/IconButton";

export interface ShellSection<T extends string> {
  id: T;
  icon: LucideIcon;
  label: string;
}

export interface ShellSearchItem<T extends string> {
  id: string;
  section: T;
  label: string;
  keywords?: string;
}

/**
 * 设置弹窗的外壳（全局设置和实例设置共用，长一个样子）：
 * 左边一栏栏目（选中的是主色浅底 + 主色字，可选顶部搜索设置），右边是这一栏的内容，右上角「×」。Esc、点背后照旧能关。
 */
export function SettingsShell<T extends string>({
  open,
  onClose,
  title,
  subtitle,
  sections,
  current,
  onSection,
  searchItems,
  footer,
  headerAction,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  /** 标题下面的一行小字（实例设置写实例名） */
  subtitle?: string;
  sections: ShellSection<T>[];
  current: T;
  onSection: (id: T) => void;
  /** 可选设置项索引：提供时在左栏顶部渲染「搜索设置」输入框 */
  searchItems?: ShellSearchItem<T>[];
  /** 左栏最下面的一行小字（快捷键） */
  footer?: string;
  /** 右边标题那一行最右的操作（实例设置的「打开全局设置」） */
  headerAction?: ReactNode;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  const sectionLabels = useMemo(
    () => new Map(sections.map((section) => [section.id, section.label])),
    [sections],
  );
  const needle = query.trim().toLowerCase();
  const matches = useMemo(() => {
    if (!needle || !searchItems?.length) return [];
    return searchItems.filter((item) => {
      const sectionLabel = sectionLabels.get(item.section) ?? "";
      return `${item.label} ${sectionLabel} ${item.keywords ?? ""}`.toLowerCase().includes(needle);
    });
  }, [needle, searchItems, sectionLabels]);

  const label = sections.find((s) => s.id === current)?.label ?? "";
  return (
    <Dialog open={open} onClose={onClose} title={title} bare className="h-[640px] max-w-[940px]">
      <div className="grid h-full min-h-0 grid-cols-[208px_1fr]">
        <nav className="flex min-h-0 min-w-0 flex-col gap-1 border-line border-r bg-surface-2 px-3 pt-5 pb-4">
          <div className="px-3 pb-2">
            <h2 className="font-semibold text-lg">{title}</h2>
            {subtitle && (
              <Truncated as="p" text={subtitle} className="mt-0.5 truncate text-fg-muted text-xs" />
            )}
          </div>
          {searchItems && searchItems.length > 0 && (
            <div className="mb-2 flex h-8 items-center gap-1.5 rounded-control border border-line bg-surface px-2.5 transition-colors duration-(--motion-press) focus-within:border-accent">
              <Search className="size-3.5 shrink-0 text-fg-muted" aria-hidden />
              <input
                type="text"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t("settings.searchPlaceholder")}
                aria-label={t("settings.searchPlaceholder")}
                className="min-w-0 flex-1 bg-transparent text-fg text-xs outline-none placeholder:text-fg-muted"
              />
              {query && (
                <button
                  type="button"
                  onClick={() => setQuery("")}
                  aria-label={t("content.clearFilter")}
                  className="grid size-4 shrink-0 place-items-center rounded-tag text-fg-muted transition-colors duration-(--motion-press) hover:text-fg"
                >
                  <X className="size-3" aria-hidden />
                </button>
              )}
            </div>
          )}
          <div className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto">
            {needle ? (
              matches.length > 0 ? (
                matches.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    onClick={() => {
                      onSection(item.section);
                      setQuery("");
                    }}
                    className="flex flex-col items-start rounded-control px-3 py-1.5 text-left transition-colors duration-(--motion-press) hover:bg-surface-hover"
                  >
                    <span className="w-full truncate text-fg text-xs font-medium">
                      {item.label}
                    </span>
                    <span className="w-full truncate text-2xs text-fg-muted">
                      {sectionLabels.get(item.section)}
                    </span>
                  </button>
                ))
              ) : (
                <p className="px-3 py-4 text-center text-fg-muted text-xs">
                  {t("settings.searchEmpty")}
                </p>
              )
            ) : (
              sections.map(({ id, icon: Icon, label: sectionLabel }) => (
                <button
                  key={id}
                  type="button"
                  aria-current={id === current ? "page" : undefined}
                  data-autofocus={id === current ? true : undefined}
                  onClick={() => onSection(id)}
                  className={`flex h-9 shrink-0 items-center gap-2.5 rounded-control px-3 text-left text-sm transition-colors duration-(--motion-press) ${
                    id === current
                      ? "bg-accent-soft font-medium text-accent"
                      : "text-fg-muted hover:bg-surface-hover hover:text-fg"
                  }`}
                >
                  <Icon className="size-4 shrink-0" aria-hidden />
                  <span className="truncate">{sectionLabel}</span>
                </button>
              ))
            )}
          </div>
          {footer && <p className="mt-2 shrink-0 px-3 text-fg-muted text-xs">{footer}</p>}
        </nav>
        {/* relative：里面读屏用的隐藏文字是绝对定位的，要关在这个滚动区域里 */}
        <div className="relative min-h-0 overflow-y-auto">
          <IconButton
            label={t("settings.close")}
            onClick={onClose}
            className="absolute top-3 right-3"
            data-dialog-close
          >
            <X className="size-4" aria-hidden />
          </IconButton>
          <motion.div
            key={current}
            className="flex flex-col gap-5 px-7 pt-6 pb-8"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1, transition: { duration: duration.toggle, ease: ease.out } }}
          >
            {/* 右边留出「×」的位置 */}
            <div className="flex min-h-8 items-center justify-between gap-4 pr-8">
              <h2 className="font-semibold text-lg">{label}</h2>
              {headerAction}
            </div>
            {children}
          </motion.div>
        </div>
      </div>
    </Dialog>
  );
}
