import { AlertTriangle, Link2, Search, Sparkles } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ImpactAction, ModDepInfo, ModImpactReport } from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { Button } from "../ui/Button";
import { Checkbox } from "../ui/Checkbox";
import { Dialog } from "../ui/Dialog";
import { Tag } from "../ui/Tag";

export interface PendingModImpact {
  action: ImpactAction;
  targets: { fileName: string; title: string }[];
  report: ModImpactReport;
}

export interface InspectingModDeps {
  fileName: string;
  title: string;
  info: ModDepInfo;
}

export interface ModTitleInfo {
  /** 优先展示名（中文译名，缺省为原名） */
  display: string;
  /** 英文原名（与 display 不同时作为副标题展示并参与搜索） */
  raw: string;
}

function depLabel(
  fileName: string,
  fallback: string,
  titles?: Record<string, ModTitleInfo>,
): { display: string; sub: string | null; search: string } {
  const hit = titles?.[fileName];
  if (!hit) {
    return {
      display: fallback,
      sub: null,
      search: `${fallback} ${fileName}`.toLowerCase(),
    };
  }
  const sub = hit.raw !== hit.display ? hit.raw : null;
  return {
    display: hit.display,
    sub,
    search: `${hit.display} ${hit.raw} ${fileName}`.toLowerCase(),
  };
}

/**
 * 模组依赖关系详情弹窗：当依赖方很多（如 Fabric API 被 200+ 模组依赖）时，
 * 点击「被 XX 等 N 项依赖」胶囊或「查看依赖关系」菜单弹出可搜索、可滚动、可复制、可定位的完整清单。
 */
export function ModDepsDetailDialog({
  inspecting,
  titleByFile,
  onClose,
  onLocate,
}: {
  inspecting: InspectingModDeps | null;
  titleByFile?: Record<string, ModTitleInfo>;
  onClose: () => void;
  onLocate: (query: string) => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (inspecting) setQuery("");
  }, [inspecting]);

  const q = query.trim().toLowerCase();
  const requiredBy = useMemo(
    () =>
      (inspecting?.info.requiredBy ?? [])
        .map((d) => ({ ...d, label: depLabel(d.fileName, d.title, titleByFile) }))
        .filter((d) => !q || d.label.search.includes(q)),
    [inspecting, titleByFile, q],
  );
  const requires = useMemo(
    () =>
      (inspecting?.info.requires ?? [])
        .map((d) => ({ ...d, label: depLabel(d.fileName, d.title, titleByFile) }))
        .filter((d) => !q || d.label.search.includes(q)),
    [inspecting, titleByFile, q],
  );

  return (
    <Dialog
      open={!!inspecting}
      onClose={onClose}
      wide
      title={t("content.deps.detailTitle", { name: inspecting?.title ?? "" })}
    >
      {inspecting && (
        <div className="flex select-text flex-col gap-4">
          <div className="relative">
            <Search
              className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3 size-3.5 text-fg-muted"
              aria-hidden
            />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("content.deps.detailSearch")}
              className="h-9 w-full rounded-control border border-line bg-surface pr-3 pl-9 text-fg text-sm outline-none placeholder:text-fg-muted focus:border-accent"
            />
          </div>

          {inspecting.info.requiredBy.length > 0 && (
            <section className="flex flex-col gap-2">
              <div className="flex items-center gap-2 px-0.5">
                <Link2 className="size-3.5 text-accent" aria-hidden />
                <h3 className="font-medium text-fg text-xs">
                  {t("content.deps.detailRequiredBy", {
                    n: inspecting.info.requiredBy.length,
                  })}
                </h3>
                {q && (
                  <span className="text-2xs text-fg-muted tabular-nums">({requiredBy.length})</span>
                )}
              </div>
              {requiredBy.length === 0 ? (
                <p className="px-1 py-2 text-fg-muted text-xs">{t("content.deps.detailEmpty")}</p>
              ) : (
                <ul className="max-h-64 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
                  {requiredBy.map((dep) => (
                    <li
                      key={dep.fileName}
                      className="flex items-center justify-between gap-3 px-3.5 py-2 text-xs transition-colors hover:bg-surface-hover"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5">
                          <span className="truncate font-medium text-fg">{dep.label.display}</span>
                          {dep.label.sub && (
                            <span className="truncate text-fg-muted">{dep.label.sub}</span>
                          )}
                          {!dep.enabled && (
                            <Tag tone="neutral">{t("content.deps.disabledTag")}</Tag>
                          )}
                        </div>
                        <p className="truncate font-mono text-2xs text-fg-muted">{dep.fileName}</p>
                      </div>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          onLocate(dep.label.display);
                          onClose();
                        }}
                      >
                        {t("content.deps.locateMod")}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          )}

          {inspecting.info.requires.length > 0 && (
            <section className="flex flex-col gap-2">
              <div className="flex items-center gap-2 px-0.5">
                <Sparkles className="size-3.5 text-fg-muted" aria-hidden />
                <h3 className="font-medium text-fg text-xs">
                  {t("content.deps.detailRequires", {
                    n: inspecting.info.requires.length,
                  })}
                </h3>
              </div>
              {requires.length === 0 ? (
                <p className="px-1 py-2 text-fg-muted text-xs">{t("content.deps.detailEmpty")}</p>
              ) : (
                <ul className="max-h-44 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
                  {requires.map((dep) => (
                    <li
                      key={dep.fileName}
                      className="flex items-center justify-between gap-3 px-3.5 py-2 text-xs transition-colors hover:bg-surface-hover"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5">
                          <span className="truncate font-medium text-fg">{dep.label.display}</span>
                          {dep.label.sub && (
                            <span className="truncate text-fg-muted">{dep.label.sub}</span>
                          )}
                          {!dep.enabled && (
                            <Tag tone="neutral">{t("content.deps.disabledTag")}</Tag>
                          )}
                        </div>
                        <p className="truncate font-mono text-2xs text-fg-muted">{dep.fileName}</p>
                      </div>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          onLocate(dep.label.display);
                          onClose();
                        }}
                      >
                        {t("content.deps.locateMod")}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          )}
        </div>
      )}
    </Dialog>
  );
}

/**
 * 模组停用 / 卸载前的依赖影响确认弹窗（V1.1-B2，懒加载）：
 * 1. 若目标模组仍被其他已启用模组依赖，清晰展示依赖方列表进行风险预警（大列表自动滚动）；
 * 2. 若卸载后有前置库失去全部依赖方，列出孤儿前置供一键勾选顺带清理（自动前置默认勾选）。
 */
export function ModImpactDialog({
  pending,
  titleByFile,
  busy,
  onCancel,
  onConfirm,
}: {
  pending: PendingModImpact | null;
  titleByFile?: Record<string, ModTitleInfo>;
  busy: boolean;
  onCancel: () => void;
  onConfirm: (extraOrphanFiles: string[]) => void;
}) {
  const { t } = useTranslation();
  const [checkedOrphans, setCheckedOrphans] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!pending) return;
    // 自动安装的前置默认勾选清理，玩家手动安装的库默认不勾选
    setCheckedOrphans(
      new Set(pending.report.orphans.filter((o) => o.autoDependency).map((o) => o.fileName)),
    );
  }, [pending]);

  const warnings = pending?.report.dependentWarnings ?? [];
  const orphans = pending?.report.orphans ?? [];
  const isRemove = pending?.action === "remove";
  const hasWarnings = warnings.length > 0;
  const hasOrphans = isRemove && orphans.length > 0;

  const freedBytes = useMemo(
    () => orphans.reduce((sum, o) => sum + (checkedOrphans.has(o.fileName) ? (o.size ?? 0) : 0), 0),
    [orphans, checkedOrphans],
  );

  const rawToDisplay = useMemo(() => {
    const map = new Map<string, string>();
    for (const info of Object.values(titleByFile ?? {})) {
      map.set(info.raw.toLowerCase(), info.display);
    }
    return map;
  }, [titleByFile]);

  const allOrphansChecked =
    orphans.length > 0 && orphans.every((o) => checkedOrphans.has(o.fileName));
  const someOrphansChecked = orphans.some((o) => checkedOrphans.has(o.fileName));

  const title = !pending
    ? ""
    : pending.action === "disable"
      ? t("content.deps.disableTitle")
      : hasWarnings && hasOrphans
        ? t("content.deps.removeBothTitle")
        : hasWarnings
          ? t("content.deps.removeWarnTitle")
          : t("content.deps.orphanTitle");

  const confirmLabel = !pending
    ? ""
    : pending.action === "disable"
      ? t("content.deps.confirmDisable")
      : checkedOrphans.size > 0
        ? t("content.deps.confirmRemoveWithOrphans", {
            n: pending.targets.length + checkedOrphans.size,
          })
        : hasWarnings
          ? t("content.deps.confirmRemoveAnyway")
          : t("content.deps.confirmRemoveOnly");

  return (
    <Dialog
      open={!!pending}
      onClose={() => {
        if (!busy) onCancel();
      }}
      wide
      title={title}
      footer={
        <>
          {hasOrphans && checkedOrphans.size > 0 && (
            <span className="mr-auto self-center text-fg-muted text-xs tabular-nums">
              {t("content.deps.orphanCountSize", {
                n: checkedOrphans.size,
                size: formatSize(freedBytes),
              })}
            </span>
          )}
          <Button variant="ghost" disabled={busy} onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button
            variant={hasWarnings ? "danger" : "primary"}
            disabled={busy}
            onClick={() => onConfirm([...checkedOrphans])}
          >
            {confirmLabel}
          </Button>
        </>
      }
    >
      {pending && (
        <div className="flex select-text flex-col gap-4">
          {hasWarnings && (
            <section className="flex flex-col gap-2.5 rounded-panel border border-line bg-warning-soft px-4 py-3">
              <div className="flex items-start gap-2.5">
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
                <p className="text-fg text-sm">
                  {t(isRemove ? "content.deps.warnRemoveIntro" : "content.deps.warnDisableIntro")}
                </p>
              </div>
              <ul className="flex flex-col gap-2.5 pl-6">
                {warnings.map((w) => (
                  <li key={w.targetFile} className="flex flex-col gap-1.5">
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-fg text-xs">
                        {depLabel(w.targetFile, w.targetTitle, titleByFile).display}
                      </span>
                      <span className="inline-flex items-center gap-1 text-2xs text-fg-muted tabular-nums">
                        <Link2 className="size-3" aria-hidden />
                        {t("content.deps.neededByCount", { n: w.dependents.length })}
                      </span>
                    </div>
                    <div className="flex max-h-36 flex-wrap gap-1.5 overflow-y-auto pr-1">
                      {w.dependents.map((dep) => (
                        <Tag key={dep.fileName} tone="warning">
                          {depLabel(dep.fileName, dep.title, titleByFile).display}
                        </Tag>
                      ))}
                    </div>
                  </li>
                ))}
              </ul>
            </section>
          )}

          {hasOrphans && (
            <section className="flex flex-col gap-2">
              <div className="flex items-center gap-2.5 px-1">
                <Checkbox
                  checked={allOrphansChecked}
                  mixed={someOrphansChecked && !allOrphansChecked}
                  label={t("content.deps.selectAllOrphans")}
                  onChange={(on) =>
                    setCheckedOrphans(new Set(on ? orphans.map((o) => o.fileName) : []))
                  }
                />
                <Sparkles className="size-3.5 shrink-0 text-accent" aria-hidden />
                <h3 className="font-medium text-fg text-sm">{t("content.deps.orphanHeading")}</h3>
              </div>
              <p className="px-1 text-fg-muted text-xs">{t("content.deps.orphanIntro")}</p>
              <ul className="max-h-52 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
                {orphans.map((orphan) => {
                  const label = depLabel(orphan.fileName, orphan.title, titleByFile);
                  return (
                    <li
                      key={orphan.fileName}
                      className="flex items-center gap-3 px-3.5 py-2.5 text-sm"
                    >
                      <Checkbox
                        checked={checkedOrphans.has(orphan.fileName)}
                        label={label.display}
                        onChange={(on) =>
                          setCheckedOrphans((prev) => {
                            const next = new Set(prev);
                            if (on) next.add(orphan.fileName);
                            else next.delete(orphan.fileName);
                            return next;
                          })
                        }
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="truncate font-medium text-fg text-sm">
                            {label.display}
                          </span>
                          {orphan.autoDependency ? (
                            <Tag tone="accent">{t("content.deps.autoBadge")}</Tag>
                          ) : (
                            <Tag tone="neutral">{t("content.deps.manualLibBadge")}</Tag>
                          )}
                        </div>
                        <p className="truncate text-fg-muted text-xs">
                          {orphan.version ? `${orphan.version} · ` : ""}
                          {t("content.deps.freedBy", {
                            names: orphan.freedBy
                              .map((n) => rawToDisplay.get(n.toLowerCase()) ?? n)
                              .join("、"),
                          })}
                        </p>
                      </div>
                      <span className="shrink-0 text-fg-muted text-xs tabular-nums">
                        {formatSize(orphan.size ?? 0)}
                      </span>
                    </li>
                  );
                })}
              </ul>
            </section>
          )}

          {isRemove && <p className="text-fg-muted text-xs">{t("content.deps.safeUndoNote")}</p>}
        </div>
      )}
    </Dialog>
  );
}

export default ModImpactDialog;
