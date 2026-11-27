import { useVirtualizer } from "@tanstack/react-virtual";
import {
  AlertTriangle,
  ArrowRight,
  ChevronDown,
  ChevronUp,
  FileCode2,
  FileMinus2,
  FilePlus2,
  FileText,
  FolderOpen,
  Image as ImageIcon,
  Package,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  type BodyFormat,
  commands,
  type InstanceSummary,
  type LoaderKind,
  type LoaderRef,
  type PackDiffItem,
  type PackDiffKind,
  type PackFileCategory,
  type PackUpdateChoice,
  type PackVersion,
} from "@/lib/bindings";
import { formatSince } from "@/lib/discover";
import { formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { TextInput } from "../ui/Field";
import { HoverHint, Truncated } from "../ui/HoverCard";
import { LightSpinner } from "../ui/LightSpinner";
import { Select } from "../ui/Select";
import { Switch } from "../ui/Switch";
import { Tag } from "../ui/Tag";

const RichText = lazy(() => import("@/components/discover/RichText"));

type DiffFilter = "all" | PackDiffKind;

const LOADER_LABELS: Record<LoaderKind, string> = {
  neoforge: "NeoForge",
  fabric: "Fabric",
  forge: "Forge",
  quilt: "Quilt",
  legacyfabric: "Legacy Fabric",
  babric: "Babric",
};

function splitPath(path: string): { fileName: string; dirPath: string } {
  const idx = path.lastIndexOf("/");
  if (idx < 0) return { fileName: path, dirPath: "" };
  return { fileName: path.slice(idx + 1), dirPath: path.slice(0, idx + 1) };
}

function formatLoader(loader: LoaderRef | null, vanillaLabel: string): string {
  return loader ? `${LOADER_LABELS[loader.kind]} ${loader.version}` : vanillaLabel;
}

function CategoryIcon({ category }: { category: PackFileCategory }) {
  const Icon =
    category === "mod"
      ? Package
      : category === "shader"
        ? Sparkles
        : category === "resourcePack"
          ? ImageIcon
          : category === "config"
            ? FileCode2
            : FileText;
  return <Icon className="size-3.5 shrink-0 text-fg-muted" aria-hidden />;
}

function DiffBadge({ item, overwriteConfigs }: { item: PackDiffItem; overwriteConfigs: boolean }) {
  const { t } = useTranslation();
  const willOverwrite =
    item.kind === "playerModified" &&
    overwriteConfigs &&
    item.hasTarget &&
    item.category === "config";

  const tone =
    item.kind === "removed" || willOverwrite
      ? "text-danger"
      : item.kind === "added" || item.kind === "updated"
        ? "text-accent"
        : "text-fg-muted";

  const Icon =
    item.kind === "added"
      ? FilePlus2
      : item.kind === "updated"
        ? RefreshCw
        : item.kind === "removed"
          ? FileMinus2
          : item.kind === "playerAdded"
            ? ShieldCheck
            : FileCode2;

  const label = willOverwrite
    ? t("packUpdate.diff.willOverwrite")
    : t(`packUpdate.diff.${item.kind}`);

  return (
    <span className={`inline-flex shrink-0 items-center gap-1 text-xs ${tone}`}>
      <Icon className="size-3.5" aria-hidden />
      {label}
    </span>
  );
}

/**
 * 整合包一键无损更新弹窗（懒加载，不计入首屏 JS 体积）。
 * 支持两种来源：
 * 1. Modrinth / CurseForge 在线版本列表（含更新日志）直接拉取并做三方比对；
 * 2. 选择本地 `.mrpack` / `.zip` 文件进行无损三方比对更新；
 * 同时在存在回滚记录时提供「一键无损回滚」入口。
 */
export default function UpdatePackDialog({
  instance,
  open,
  onClose,
}: {
  instance: InstanceSummary;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const isOnlineManaged = Boolean(instance.packSource && instance.packProjectId);

  const [versions, setVersions] = useState<PackVersion[] | null>(null);
  const [versionsError, setVersionsError] = useState<string | null>(null);
  const [mcFilter, setMcFilter] = useState<string>("all");
  const [releaseOnly, setReleaseOnly] = useState(false);
  const [selectedVersionId, setSelectedVersionId] = useState<string>("");
  const [expandedChangelogId, setExpandedChangelogId] = useState<string | null>(null);
  const [loadedChangelogs, setLoadedChangelogs] = useState<
    Record<string, { body: string; format: BodyFormat } | "loading" | "empty">
  >({});

  const [checking, setChecking] = useState(false);
  const [choice, setChoice] = useState<PackUpdateChoice | null>(null);
  const [previewChangelogOpen, setPreviewChangelogOpen] = useState(false);
  const [filter, setFilter] = useState<DiffFilter>("all");
  const [query, setQuery] = useState("");
  const [overwriteConfigs, setOverwriteConfigs] = useState(false);
  const [starting, setStarting] = useState(false);
  const [rollingBack, setRollingBack] = useState(false);
  const [confirmRollback, setConfirmRollback] = useState(false);

  const tokenRef = useRef<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const releaseToken = (token: string | null) => {
    if (!token) return;
    void commands.modpackUpdateDiscard(token);
  };

  useEffect(() => {
    if (!open) return;
    setVersions(null);
    setVersionsError(null);
    setMcFilter("all");
    setReleaseOnly(false);
    setSelectedVersionId("");
    setExpandedChangelogId(null);
    setLoadedChangelogs({});
    setChecking(false);
    setChoice(null);
    setPreviewChangelogOpen(false);
    setFilter("all");
    setQuery("");
    setOverwriteConfigs(false);
    setStarting(false);
    setRollingBack(false);
    setConfirmRollback(false);

    if (!isOnlineManaged) return;

    let cancelled = false;
    commands
      .modpackUpdateVersions(instance.id)
      .then(unwrap)
      .then((list) => {
        if (cancelled) return;
        setVersions(list);
        const preferred =
          list.find((v) => v.recommended && v.id !== instance.packVersionId) ??
          list.find((v) => v.id !== instance.packVersionId) ??
          list[0];
        if (preferred) setSelectedVersionId(preferred.id);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setVersionsError(error instanceof Error ? error.message : t("error.unexpected"));
      });

    return () => {
      cancelled = true;
    };
  }, [open, instance.id, instance.packVersionId, isOnlineManaged, t]);

  useEffect(() => {
    tokenRef.current = choice?.token ?? null;
  }, [choice]);

  useEffect(() => {
    return () => {
      releaseToken(tokenRef.current);
      tokenRef.current = null;
    };
  }, []);

  const handleClose = () => {
    releaseToken(tokenRef.current);
    tokenRef.current = null;
    onClose();
  };

  const toggleVersionChangelog = async (version: PackVersion) => {
    if (expandedChangelogId === version.id) {
      setExpandedChangelogId(null);
      return;
    }
    setExpandedChangelogId(version.id);
    if (loadedChangelogs[version.id]) return;
    if (version.changelog) {
      setLoadedChangelogs((prev) => ({
        ...prev,
        [version.id]: { body: version.changelog ?? "", format: version.changelogFormat },
      }));
      return;
    }
    if (!instance.packSource || !instance.packProjectId) {
      setLoadedChangelogs((prev) => ({ ...prev, [version.id]: "empty" }));
      return;
    }
    setLoadedChangelogs((prev) => ({ ...prev, [version.id]: "loading" }));
    try {
      const text = unwrap(
        await commands.discoverVersionChangelog(
          instance.packSource,
          instance.packProjectId,
          version.id,
        ),
      ).trim();
      setLoadedChangelogs((prev) => ({
        ...prev,
        [version.id]: text ? { body: text, format: version.changelogFormat } : "empty",
      }));
    } catch {
      setLoadedChangelogs((prev) => ({ ...prev, [version.id]: "empty" }));
    }
  };

  const previewRemote = async () => {
    if (!selectedVersionId || checking) return;
    setChecking(true);
    try {
      const result = unwrap(
        await commands.modpackUpdatePreviewRemote(instance.id, selectedVersionId),
      );
      releaseToken(tokenRef.current);
      setChoice(result);
      setFilter("all");
      setQuery("");
      setOverwriteConfigs(false);
    } catch (error) {
      reportError(error, t("packUpdate.checkFailed"));
    } finally {
      setChecking(false);
    }
  };

  const previewLocalFile = async () => {
    if (checking) return;
    setChecking(true);
    try {
      const result = unwrap(await commands.modpackUpdatePreviewFile(instance.id));
      if (!result) return;
      releaseToken(tokenRef.current);
      setChoice(result);
      setFilter("all");
      setQuery("");
      setOverwriteConfigs(false);
    } catch (error) {
      reportError(error, t("packUpdate.checkFailed"));
    } finally {
      setChecking(false);
    }
  };

  const startUpdate = async () => {
    if (!choice || starting) return;
    setStarting(true);
    try {
      const token = choice.token;
      tokenRef.current = null;
      const jobId = unwrap(
        await commands.modpackUpdateStart(token, {
          overwriteModifiedConfigs: overwriteConfigs,
        }),
      );
      useGameStore.getState().showTasksFor(jobId);
      onClose();
    } catch (error) {
      reportError(error, t("packUpdate.failed"));
      setStarting(false);
    }
  };

  const runRollback = async () => {
    if (rollingBack) return;
    setRollingBack(true);
    try {
      const outcome = unwrap(await commands.modpackUpdateRollback(instance.id));
      await useGameStore.getState().refreshInstances();
      void useGameStore.getState().refreshRecentWorlds();
      useGameStore.getState().bumpContent();
      toast.success(t("packUpdate.rollbackDone"), {
        description: t("packUpdate.rollbackDoneDesc", {
          version: outcome.restoredVersion,
          restored: outcome.restoredFiles,
          removed: outcome.removedFiles,
        }),
      });
      handleClose();
    } catch (error) {
      reportError(error, t("packUpdate.rollbackFailed"));
      setRollingBack(false);
    }
  };

  const preview = choice?.preview ?? null;

  const mcVersions = useMemo(() => {
    if (!versions) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const v of versions) {
      for (const gv of v.gameVersions) {
        if (!seen.has(gv)) {
          seen.add(gv);
          out.push(gv);
        }
      }
    }
    return out;
  }, [versions]);

  const hasBetaVersions = useMemo(() => Boolean(versions?.some((v) => !v.release)), [versions]);

  const visibleVersions = useMemo(() => {
    if (!versions) return [];
    return versions.filter((v) => {
      if (releaseOnly && !v.release) return false;
      if (mcFilter !== "all" && !v.gameVersions.includes(mcFilter)) return false;
      return true;
    });
  }, [versions, mcFilter, releaseOnly]);

  const applyVersionFilter = (nextMc: string, nextReleaseOnly: boolean) => {
    setMcFilter(nextMc);
    setReleaseOnly(nextReleaseOnly);
    if (!versions) return;
    const nextList = versions.filter((v) => {
      if (nextReleaseOnly && !v.release) return false;
      if (nextMc !== "all" && !v.gameVersions.includes(nextMc)) return false;
      return true;
    });
    if (nextList.length > 0 && !nextList.some((v) => v.id === selectedVersionId)) {
      const preferred =
        nextList.find((v) => v.release && v.id !== instance.packVersionId) ??
        nextList.find((v) => v.id !== instance.packVersionId) ??
        nextList[0];
      if (preferred) setSelectedVersionId(preferred.id);
    }
  };

  const filteredItems = useMemo(() => {
    if (!preview) return [];
    const needle = query.trim().toLowerCase();
    return preview.items.filter((item) => {
      if (filter !== "all" && item.kind !== filter) return false;
      if (!needle) return true;
      return item.path.toLowerCase().includes(needle);
    });
  }, [preview, filter, query]);

  const virtualizer = useVirtualizer({
    count: filteredItems.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 38,
    overscan: 8,
  });

  const hasChanges = preview
    ? preview.addedCount > 0 ||
      preview.updatedCount > 0 ||
      preview.removedCount > 0 ||
      preview.mcVersionChanged ||
      preview.loaderChanged ||
      (overwriteConfigs && preview.playerModifiedCount > 0)
    : false;

  const vanillaLabel = t("create.vanilla");

  return (
    <Dialog
      open={open}
      onClose={handleClose}
      title={t("packUpdate.title", { name: instance.name })}
      wide
    >
      <div className="flex flex-col gap-4">
        {instance.packRollback && !preview && (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-panel border border-accent/35 bg-accent-soft px-3.5 py-2.5 text-sm">
            <div className="flex min-w-0 items-center gap-2.5">
              <RotateCcw className="size-4 shrink-0 text-accent" aria-hidden />
              <div className="min-w-0">
                <p className="font-medium text-fg">{t("packUpdate.rollbackAvailable")}</p>
                <p className="text-fg-muted text-xs">
                  {t("packUpdate.rollbackHint", {
                    from: instance.packRollback.fromVersion,
                    to: instance.packRollback.toVersion,
                  })}
                </p>
              </div>
            </div>
            {confirmRollback ? (
              <div className="flex shrink-0 items-center gap-1.5">
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setConfirmRollback(false)}
                  disabled={rollingBack}
                >
                  {t("common.cancel")}
                </Button>
                <Button size="sm" variant="primary" onClick={runRollback} disabled={rollingBack}>
                  {rollingBack ? t("packUpdate.rollingBack") : t("packUpdate.rollbackConfirm")}
                </Button>
              </div>
            ) : (
              <Button size="sm" variant="secondary" onClick={() => setConfirmRollback(true)}>
                <RotateCcw className="size-3.5" aria-hidden />
                {t("packUpdate.rollbackAction")}
              </Button>
            )}
          </div>
        )}

        {!preview ? (
          <>
            <div className="flex flex-wrap items-center justify-between gap-2 rounded-panel border border-line bg-surface-2 px-3.5 py-2.5 text-sm">
              <div className="flex flex-wrap items-center gap-2 text-fg-muted">
                <span>{t("packUpdate.current")}</span>
                <span className="font-medium text-fg">
                  {instance.pack
                    ? instance.packVersion
                      ? `${instance.pack} · ${instance.packVersion}`
                      : instance.pack
                    : instance.gameVersion}
                </span>
                {instance.packSource && (
                  <Tag>{instance.packSource === "modrinth" ? "Modrinth" : "CurseForge"}</Tag>
                )}
              </div>
              <span className="text-fg-muted text-xs tabular-nums">
                {[instance.gameVersion, instance.loader ?? vanillaLabel].join(" · ")}
              </span>
            </div>

            {isOnlineManaged ? (
              <div className="flex flex-col gap-2">
                <div className="flex items-center justify-between">
                  <p className="font-medium text-fg text-sm">{t("packUpdate.chooseVersion")}</p>
                  {versions && (
                    <span className="text-fg-muted text-xs tabular-nums">
                      {t("packUpdate.versionCount", { n: visibleVersions.length })}
                    </span>
                  )}
                </div>

                {versions && versions.length > 0 && (mcVersions.length > 1 || hasBetaVersions) && (
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    {mcVersions.length > 1 ? (
                      <div className="min-w-44 flex-1">
                        <Select
                          label={t("packUpdate.filterMc")}
                          value={mcFilter}
                          disabled={checking}
                          onChange={(next) => applyVersionFilter(next, releaseOnly)}
                          options={[
                            { value: "all", label: t("packUpdate.allMcVersions") },
                            ...mcVersions.map((gv) => ({
                              value: gv,
                              label:
                                gv === instance.gameVersion
                                  ? t("packUpdate.currentMcOption", { version: gv })
                                  : `Minecraft ${gv}`,
                            })),
                          ]}
                        />
                      </div>
                    ) : (
                      <span />
                    )}
                    {hasBetaVersions && (
                      <button
                        type="button"
                        disabled={checking}
                        onClick={() => applyVersionFilter(mcFilter, !releaseOnly)}
                        className={`rounded-control border px-3 py-1.5 text-xs transition-colors duration-(--motion-press) ${
                          releaseOnly
                            ? "border-accent/40 bg-accent-soft font-medium text-accent"
                            : "border-line bg-surface text-fg-muted hover:text-fg"
                        }`}
                      >
                        {t("packUpdate.releaseOnly")}
                      </button>
                    )}
                  </div>
                )}

                {versionsError ? (
                  <div className="flex items-center justify-between gap-3 rounded-panel border border-danger/35 bg-danger-soft px-3.5 py-3 text-sm">
                    <span className="text-danger">{versionsError}</span>
                    <Button size="sm" variant="secondary" onClick={previewLocalFile}>
                      <FolderOpen className="size-3.5" aria-hidden />
                      {t("packUpdate.pickLocal")}
                    </Button>
                  </div>
                ) : versions === null ? (
                  <div className="flex items-center justify-center gap-2 rounded-row border border-line bg-surface/60 py-10 text-fg-muted text-sm">
                    <LightSpinner className="size-4" />
                    <span>{t("packUpdate.loadingVersions")}</span>
                  </div>
                ) : visibleVersions.length === 0 ? (
                  <div className="rounded-row border border-line bg-surface/60 px-4 py-6 text-center text-fg-muted text-sm">
                    {versions.length === 0 ? t("import.noVersion") : t("packUpdate.noFilterMatch")}
                  </div>
                ) : (
                  <div className="max-h-72 space-y-1.5 overflow-y-auto rounded-row border border-line bg-surface/50 p-2">
                    {visibleVersions.map((v) => {
                      const isCurrent =
                        (instance.packVersionId && v.id === instance.packVersionId) ||
                        (!instance.packVersionId &&
                          instance.packVersion &&
                          v.versionNumber === instance.packVersion);
                      const isSelected = v.id === selectedVersionId;
                      const expanded = expandedChangelogId === v.id;
                      const changelogState = loadedChangelogs[v.id];

                      return (
                        <div
                          key={v.id}
                          className={`rounded-row border transition-colors ${
                            isSelected
                              ? "border-accent/40 bg-accent-soft"
                              : "border-transparent bg-surface-2/60 hover:border-line hover:bg-surface-hover"
                          }`}
                        >
                          <div className="flex items-center gap-2 px-3 py-2">
                            <button
                              type="button"
                              className="flex min-w-0 flex-1 items-center justify-between gap-2 text-left"
                              onClick={() => setSelectedVersionId(v.id)}
                            >
                              <div className="min-w-0">
                                <div className="flex flex-wrap items-center gap-1.5">
                                  <span className="truncate font-medium text-fg text-sm">
                                    {v.versionNumber}
                                  </span>
                                  {isCurrent && <Tag>{t("packUpdate.currentTag")}</Tag>}
                                  {v.recommended && !isCurrent && (
                                    <Tag>{t("packUpdate.recommendedTag")}</Tag>
                                  )}
                                  {!v.release && <Tag>{t("import.beta")}</Tag>}
                                </div>
                                <p className="mt-0.5 text-fg-muted text-xs tabular-nums">
                                  {[
                                    v.gameVersions.slice(0, 3).join(", "),
                                    v.published ? formatSince(v.published, t) : null,
                                  ]
                                    .filter(Boolean)
                                    .join(" · ")}
                                </p>
                              </div>
                            </button>
                            <button
                              type="button"
                              onClick={() => void toggleVersionChangelog(v)}
                              className="inline-flex shrink-0 items-center gap-1 rounded-control px-2 py-1 text-fg-muted text-xs transition-colors duration-(--motion-press) hover:bg-surface hover:text-fg"
                            >
                              {t("packUpdate.changelog")}
                              {expanded ? (
                                <ChevronUp className="size-3.5" aria-hidden />
                              ) : (
                                <ChevronDown className="size-3.5" aria-hidden />
                              )}
                            </button>
                          </div>

                          {expanded && (
                            <div className="border-line/70 border-t px-3 py-2.5 text-xs">
                              {changelogState === "loading" ? (
                                <div className="flex items-center gap-2 text-fg-muted">
                                  <LightSpinner className="size-4" />
                                  <span>{t("packUpdate.loadingChangelog")}</span>
                                </div>
                              ) : changelogState && changelogState !== "empty" ? (
                                <div className="max-h-40 overflow-y-auto">
                                  <Suspense
                                    fallback={
                                      <p className="text-fg-muted">
                                        {t("packUpdate.loadingChangelog")}
                                      </p>
                                    }
                                  >
                                    <RichText
                                      body={changelogState.body}
                                      format={changelogState.format}
                                    />
                                  </Suspense>
                                </div>
                              ) : (
                                <p className="text-fg-muted">{t("packUpdate.noChangelog")}</p>
                              )}
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            ) : (
              <div className="flex flex-col items-start gap-3 rounded-panel border border-line bg-surface-2 p-4 text-sm">
                <div className="flex items-start gap-2.5">
                  <ShieldCheck className="mt-0.5 size-5 shrink-0 text-accent" aria-hidden />
                  <div className="space-y-1">
                    <p className="font-medium text-fg">{t("packUpdate.localIntroTitle")}</p>
                    <p className="text-fg-muted text-xs leading-relaxed">
                      {t("packUpdate.localIntroDesc")}
                    </p>
                  </div>
                </div>
                <Button variant="primary" onClick={previewLocalFile} disabled={checking}>
                  <FolderOpen className="size-4" aria-hidden />
                  {checking ? t("packUpdate.checking") : t("packUpdate.pickLocal")}
                </Button>
              </div>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
              {isOnlineManaged ? (
                <Button variant="ghost" onClick={previewLocalFile} disabled={checking}>
                  <FolderOpen className="size-4" aria-hidden />
                  {t("packUpdate.pickLocal")}
                </Button>
              ) : (
                <span />
              )}
              <div className="flex items-center gap-2">
                <Button variant="ghost" onClick={handleClose} disabled={checking}>
                  {t("common.cancel")}
                </Button>
                {isOnlineManaged && (
                  <Button
                    variant="primary"
                    onClick={previewRemote}
                    disabled={!selectedVersionId || checking}
                  >
                    {checking ? t("packUpdate.checking") : t("packUpdate.compare")}
                  </Button>
                )}
              </div>
            </div>
          </>
        ) : (
          <>
            {/* 三方比对预览头：整合包版本 & MC/加载器变化 */}
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-panel border border-line bg-surface-2 px-3.5 py-2.5 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium text-fg">{preview.packName}</span>
                <span className="text-fg-muted tabular-nums">
                  {preview.currentPackVersion ?? preview.currentGameVersion}
                </span>
                <ArrowRight className="size-3.5 text-fg-muted" aria-hidden />
                <span className="font-semibold text-accent tabular-nums">
                  {preview.targetPackVersion}
                </span>
              </div>
              <div className="flex flex-wrap items-center gap-1.5 text-fg-muted text-xs tabular-nums">
                <span>
                  {[
                    preview.currentGameVersion,
                    formatLoader(preview.currentLoader, vanillaLabel),
                  ].join(" · ")}
                </span>
                {(preview.mcVersionChanged || preview.loaderChanged) && (
                  <>
                    <ArrowRight className="size-3" aria-hidden />
                    <span className="font-medium text-fg">
                      {[
                        preview.targetGameVersion,
                        formatLoader(preview.targetLoader, vanillaLabel),
                      ].join(" · ")}
                    </span>
                  </>
                )}
              </div>
            </div>

            {/* 安心保护提示 & 风险提示 */}
            <div className="space-y-2">
              <div className="flex items-start gap-2 rounded-panel border border-accent/30 bg-accent-soft px-3 py-2 text-xs">
                <ShieldCheck className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
                <p className="text-fg">
                  {preview.playerAddedCount > 0 && preview.playerModifiedCount > 0
                    ? t("packUpdate.losslessGuarantee", {
                        playerAdded: preview.playerAddedCount,
                        playerModified: preview.playerModifiedCount,
                      })
                    : preview.playerAddedCount > 0
                      ? t("packUpdate.losslessGuaranteeAddedOnly", {
                          playerAdded: preview.playerAddedCount,
                        })
                      : preview.playerModifiedCount > 0
                        ? t("packUpdate.losslessGuaranteeModifiedOnly", {
                            playerModified: preview.playerModifiedCount,
                          })
                        : t("packUpdate.losslessGuaranteeNone")}
                </p>
              </div>

              {!preview.hasOriginBaseline && (
                <div className="flex items-start gap-2 rounded-panel border border-line bg-surface-2 px-3 py-2 text-fg-muted text-xs">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0 text-fg-muted" aria-hidden />
                  <p>{t("packUpdate.noOriginHint")}</p>
                </div>
              )}

              {preview.isDowngrade && (
                <div className="flex items-start gap-2 rounded-panel border border-danger/35 bg-danger-soft px-3 py-2 text-danger text-xs">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
                  <p>{t("packUpdate.downgradeWarning")}</p>
                </div>
              )}

              {preview.mcVersionChanged && preview.hasWorlds && !preview.isDowngrade && (
                <div className="flex items-start gap-2 rounded-panel border border-line bg-surface-2 px-3 py-2 text-fg-muted text-xs">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
                  <p>{t("packUpdate.worldVersionHint")}</p>
                </div>
              )}

              {preview.manualFiles.length > 0 && (
                <div className="flex items-start gap-2 rounded-panel border border-line bg-surface-2 px-3 py-2 text-fg-muted text-xs">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
                  <p>{t("packUpdate.manualHint", { n: preview.manualFiles.length })}</p>
                </div>
              )}
            </div>

            {preview.changelog && (
              <div className="rounded-panel border border-line bg-surface/50">
                <button
                  type="button"
                  onClick={() => setPreviewChangelogOpen((v) => !v)}
                  className="flex w-full items-center justify-between px-3 py-2 font-medium text-fg text-xs transition-colors duration-(--motion-press) hover:bg-surface-hover"
                >
                  <span>{t("packUpdate.changelog")}</span>
                  {previewChangelogOpen ? (
                    <ChevronUp className="size-3.5 text-fg-muted" aria-hidden />
                  ) : (
                    <ChevronDown className="size-3.5 text-fg-muted" aria-hidden />
                  )}
                </button>
                {previewChangelogOpen && (
                  <div className="max-h-36 overflow-y-auto border-line border-t px-3 py-2 text-xs">
                    <Suspense
                      fallback={<p className="text-fg-muted">{t("packUpdate.loadingChangelog")}</p>}
                    >
                      <RichText body={preview.changelog} format={preview.changelogFormat} />
                    </Suspense>
                  </div>
                )}
              </div>
            )}

            {/* 筛选胶囊 + 搜索框 */}
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex flex-wrap items-center gap-1.5">
                {(
                  [
                    ["all", preview.items.length],
                    ["updated", preview.updatedCount],
                    ["added", preview.addedCount],
                    ["removed", preview.removedCount],
                    ["playerAdded", preview.playerAddedCount],
                    ["playerModified", preview.playerModifiedCount],
                  ] as [DiffFilter, number][]
                ).map(([key, count]) => {
                  if (key !== "all" && count === 0) return null;
                  const active = filter === key;
                  return (
                    <button
                      key={key}
                      type="button"
                      onClick={() => setFilter(key)}
                      className={`inline-flex items-center gap-1 rounded-control border px-2.5 py-1 text-xs tabular-nums transition-colors duration-(--motion-press) ${
                        active
                          ? "border-accent/40 bg-accent-soft font-medium text-accent"
                          : "border-line bg-surface text-fg-muted hover:text-fg"
                      }`}
                    >
                      <span>{t(`packUpdate.filter.${key}`)}</span>
                      <span>{count}</span>
                    </button>
                  );
                })}
              </div>
              {preview.items.length > 6 && (
                <div className="relative w-48">
                  <Search
                    className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-fg-muted"
                    aria-hidden
                  />
                  <TextInput
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    placeholder={t("packUpdate.searchPlaceholder")}
                    className="h-8 pl-8 text-xs"
                  />
                </div>
              )}
            </div>

            {/* 虚拟滚动差异文件列表 */}
            <div
              ref={listRef}
              className="h-52 overflow-y-auto rounded-row border border-line bg-surface/50"
            >
              {filteredItems.length === 0 ? (
                <div className="flex h-full items-center justify-center px-4 text-center text-fg-muted text-sm">
                  {preview.items.length === 0
                    ? t("packUpdate.upToDate")
                    : t("packUpdate.noFilterMatch")}
                </div>
              ) : (
                <div
                  className="relative w-full"
                  style={{ height: `${virtualizer.getTotalSize()}px` }}
                >
                  {virtualizer.getVirtualItems().map((row) => {
                    const item = filteredItems[row.index];
                    if (!item) return null;
                    const { fileName, dirPath } = splitPath(item.path);
                    return (
                      <div
                        key={item.path}
                        style={{
                          position: "absolute",
                          top: 0,
                          left: 0,
                          width: "100%",
                          height: `${row.size}px`,
                          transform: `translateY(${row.start}px)`,
                        }}
                        className="flex items-center justify-between gap-3 border-line/50 border-b px-3 text-xs last:border-b-0"
                      >
                        <div className="flex min-w-0 items-center gap-2">
                          <CategoryIcon category={item.category} />
                          <HoverHint hint={item.path}>
                            <div className="flex min-w-0 items-baseline gap-1.5">
                              <Truncated text={fileName} className="truncate font-medium text-fg" />
                              {dirPath && (
                                <span className="hidden shrink-0 truncate font-mono text-2xs text-fg-muted sm:inline">
                                  {dirPath}
                                </span>
                              )}
                            </div>
                          </HoverHint>
                          {item.manual && <Tag>{t("packUpdate.manualTag")}</Tag>}
                        </div>
                        <DiffBadge item={item} overwriteConfigs={overwriteConfigs} />
                      </div>
                    );
                  })}
                </div>
              )}
            </div>

            {/* 可选开关：是否覆盖玩家改过的配置文件（默认关） */}
            {preview.playerModifiedCount > 0 && (
              <div className="flex items-center justify-between gap-3 rounded-panel border border-line bg-surface-2 px-3.5 py-2.5">
                <div className="min-w-0">
                  <p className="font-medium text-fg text-sm">{t("packUpdate.overwriteConfigs")}</p>
                  <p className="text-fg-muted text-xs">{t("packUpdate.overwriteConfigsHint")}</p>
                </div>
                <Switch
                  checked={overwriteConfigs}
                  onChange={setOverwriteConfigs}
                  label={t("packUpdate.overwriteConfigs")}
                />
              </div>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
              <div className="flex items-center gap-2">
                <Button
                  variant="ghost"
                  onClick={() => {
                    releaseToken(tokenRef.current);
                    tokenRef.current = null;
                    setChoice(null);
                  }}
                  disabled={starting}
                >
                  {t("packUpdate.back")}
                </Button>
                {(preview.downloadBytes ?? 0) > 0 && (
                  <span className="text-fg-muted text-xs tabular-nums">
                    {t("packUpdate.downloadSize", {
                      size: formatSize(preview.downloadBytes ?? 0),
                    })}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2">
                <Button variant="ghost" onClick={handleClose} disabled={starting}>
                  {t("common.cancel")}
                </Button>
                <Button variant="primary" onClick={startUpdate} disabled={starting || !hasChanges}>
                  {starting ? t("packUpdate.starting") : t("packUpdate.confirm")}
                </Button>
              </div>
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}
