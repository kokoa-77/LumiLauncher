import { ArrowLeftRight, Check, ChevronDown, ChevronRight, Download, Layers } from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { EmptyState } from "@/components/ui/EmptyState";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Tag } from "@/components/ui/Tag";
import { unwrap } from "@/lib/api";
import {
  type ContentItem,
  type ContentSource,
  type ContentVersion,
  commands,
  type InstanceSummary,
  type UpdateInfo,
} from "@/lib/bindings";
import { formatSince } from "@/lib/discover";
import { formatCount, formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useContentUpdatesStore } from "@/stores/contentUpdates";

const RichText = lazy(() => import("@/components/discover/RichText"));

function platformOf(item: ContentItem): ContentSource | null {
  return item.projectId && (item.source === "modrinth" || item.source === "curseForge")
    ? item.source
    : null;
}

function isSameVersion(item: ContentItem, v: ContentVersion): boolean {
  if (item.versionId && v.id === item.versionId) return true;
  if (item.fileName && v.fileName === item.fileName) return true;
  if (item.versionNumber && v.versionNumber === item.versionNumber) return true;
  return false;
}

/**
 * 实例内容「切换版本」弹窗：
 * 列出该项目兼容当前实例的版本（可一键放宽显示全部），支持展开更新日志，
 * 切换后旧文件保留在内容仓库并写入改动历史，随时一键撤销。
 */
export function SwitchVersionDialog({
  instance,
  item,
  onClose,
}: {
  instance: InstanceSummary;
  item: ContentItem | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const invalidateUpdates = useContentUpdatesStore((s) => s.invalidate);
  const [compatOnly, setCompatOnly] = useState(true);
  const [versions, setVersions] = useState<ContentVersion[] | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [changelogs, setChangelogs] = useState<Record<string, string>>({});
  const [loadingLog, setLoadingLog] = useState<string | null>(null);
  const [switching, setSwitching] = useState<string | null>(null);

  const platform = item ? platformOf(item) : null;
  const projectId = item?.projectId ?? null;
  const itemKind = item?.kind ?? "mod";
  const loaderSlug =
    itemKind === "mod" ? (instance.loader?.split(" ")[0]?.toLowerCase() ?? null) : null;

  useEffect(() => {
    if (!item || !platform || !projectId) {
      setVersions(null);
      setExpanded(null);
      setCompatOnly(true);
      return;
    }
    let cancelled = false;
    setVersions(null);
    commands
      .discoverVersions(
        platform,
        projectId,
        compatOnly ? instance.gameVersion : null,
        compatOnly ? loaderSlug : null,
      )
      .then(unwrap)
      .then((list) => {
        if (!cancelled) setVersions(list);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setVersions([]);
        reportError(error, "读取项目版本列表");
      });
    return () => {
      cancelled = true;
    };
  }, [item, platform, projectId, compatOnly, instance.gameVersion, loaderSlug]);

  const toggleChangelog = async (v: ContentVersion) => {
    if (expanded === v.id) {
      setExpanded(null);
      return;
    }
    setExpanded(v.id);
    if (v.changelog !== null || changelogs[v.id] !== undefined) return;
    setLoadingLog(v.id);
    try {
      const log = unwrap(await commands.discoverVersionChangelog(v.source, v.projectId, v.id));
      setChangelogs((prev) => ({ ...prev, [v.id]: log }));
    } catch (error) {
      reportError(error, "读取版本更新日志");
      setChangelogs((prev) => ({ ...prev, [v.id]: "" }));
    } finally {
      setLoadingLog((curr) => (curr === v.id ? null : curr));
    }
  };

  const applyVersion = async (v: ContentVersion) => {
    if (!item || switching) return;
    setSwitching(v.id);
    const update: UpdateInfo = {
      kind: item.kind,
      fileName: item.fileName,
      title: item.titleZh ?? item.title,
      currentVersion: item.versionNumber,
      versionId: v.id,
      versionNumber: v.versionNumber,
      source: v.source,
      projectId: v.projectId,
      delivery: v.delivery,
      pageUrl: v.pageUrl,
    };
    try {
      unwrap(await commands.contentSwitchVersion(instance.id, update));
      invalidateUpdates(instance.id);
      toast.success(
        t("content.switchStarted", {
          name: item.titleZh ?? item.title,
          version: v.versionNumber,
        }),
      );
      onClose();
    } catch (error) {
      reportError(error, "切换内容版本");
    } finally {
      setSwitching(null);
    }
  };

  const displayName = item ? (item.titleZh ?? item.title) : "";

  return (
    <Dialog
      open={item !== null}
      onClose={onClose}
      wide
      title={t("content.switchTitle", { name: displayName })}
      footer={
        <>
          <Button variant="secondary" onClick={() => setCompatOnly((v) => !v)}>
            {compatOnly ? t("content.switchShowAll") : t("content.switchOnlyCompat")}
          </Button>
          <Button variant="ghost" onClick={onClose}>
            {t("dialog.close")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-fg-muted text-sm">{t("content.switchHint")}</p>
        {versions === null ? (
          <div className="grid h-48 place-items-center">
            <LightSpinner className="size-6 text-accent" />
          </div>
        ) : versions.length === 0 ? (
          <EmptyState
            icon={Layers}
            title={t("content.switchEmpty")}
            hint={compatOnly ? t("discover.project.versionsEmptyHint") : ""}
          />
        ) : (
          <div className="flex max-h-96 flex-col gap-2 overflow-y-auto pr-1">
            {versions.map((v) => {
              const current = item ? isSameVersion(item, v) : false;
              const isOpen = expanded === v.id;
              const logText = v.changelog ?? changelogs[v.id] ?? "";
              const since = formatSince(v.published, t);
              return (
                <div
                  key={v.id}
                  className={`flex flex-col rounded-panel border transition-colors duration-(--motion-press) ${
                    current
                      ? "border-accent/60 bg-accent-soft/35"
                      : "border-line bg-surface hover:bg-surface-hover/50"
                  }`}
                >
                  <div className="flex items-center gap-3 px-3.5 py-2.5">
                    <button
                      type="button"
                      onClick={() => void toggleChangelog(v)}
                      aria-label={t("discover.project.versionsChangelog")}
                      className="grid size-6 shrink-0 place-items-center rounded-row text-fg-muted transition-colors duration-(--motion-press) hover:bg-surface-2 hover:text-fg"
                    >
                      {isOpen ? (
                        <ChevronDown className="size-4" aria-hidden />
                      ) : (
                        <ChevronRight className="size-4" aria-hidden />
                      )}
                    </button>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="truncate font-medium text-sm">{v.versionNumber}</span>
                        <Tag
                          className={
                            v.releaseType === "release"
                              ? "text-accent"
                              : v.releaseType === "beta"
                                ? "text-warning"
                                : "text-fg-muted"
                          }
                        >
                          {t(`discover.project.releaseType.${v.releaseType}`)}
                        </Tag>
                        {current && (
                          <Tag className="gap-1 bg-accent-soft text-accent">
                            <Check className="size-3" aria-hidden />
                            {t("content.switchCurrent")}
                          </Tag>
                        )}
                      </div>
                      <p className="mt-0.5 flex flex-wrap items-center gap-1.5 text-fg-muted text-xs tabular-nums">
                        {v.gameVersions.slice(0, 3).join(", ")}
                        {v.loaders.length > 0 && (
                          <>
                            <span aria-hidden>·</span>
                            <span>
                              {v.loaders
                                .map((l) => t(`discover.loader.${l}`, { defaultValue: l }))
                                .join(" / ")}
                            </span>
                          </>
                        )}
                        {since && (
                          <>
                            <span aria-hidden>·</span>
                            <span>{since}</span>
                          </>
                        )}
                        {v.fileSize != null && v.fileSize > 0 && (
                          <>
                            <span aria-hidden>·</span>
                            <span>{formatSize(v.fileSize)}</span>
                          </>
                        )}
                        {v.downloads != null && (
                          <>
                            <span aria-hidden>·</span>
                            <span className="inline-flex items-center gap-0.5">
                              <Download className="size-3" aria-hidden />
                              {formatCount(v.downloads)}
                            </span>
                          </>
                        )}
                      </p>
                    </div>
                    {!current && (
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={switching !== null}
                        onClick={() => void applyVersion(v)}
                      >
                        {switching === v.id ? (
                          <LightSpinner className="size-3.5" />
                        ) : (
                          <ArrowLeftRight className="size-3.5" aria-hidden />
                        )}
                        {t("content.switchApply")}
                      </Button>
                    )}
                  </div>
                  {isOpen && (
                    <div className="border-line border-t bg-surface-2/40 px-4 py-3 text-sm">
                      {loadingLog === v.id ? (
                        <LightSpinner className="size-4 text-accent" />
                      ) : logText.trim() ? (
                        <Suspense fallback={<LightSpinner className="size-4 text-accent" />}>
                          <RichText body={logText} format={v.changelogFormat} />
                        </Suspense>
                      ) : (
                        <p className="text-fg-muted text-xs">
                          {t("discover.project.versionsNoChangelog")}
                        </p>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </Dialog>
  );
}
