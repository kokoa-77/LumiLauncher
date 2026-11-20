import {
  ArrowLeftRight,
  Check,
  ChevronDown,
  ChevronRight,
  Download,
  Layers,
  Plus,
} from "lucide-react";
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { InstallDialog } from "@/components/InstallDialog";
import { Button } from "@/components/ui/Button";
import { EmptyState } from "@/components/ui/EmptyState";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Select } from "@/components/ui/Select";
import { Tag } from "@/components/ui/Tag";
import { unwrap } from "@/lib/api";
import {
  type ContentItem,
  type ContentKind,
  type ContentSource,
  type ContentVersion,
  commands,
  type InstanceSummary,
  type PlanItem,
  type UpdateInfo,
} from "@/lib/bindings";
import { formatSince } from "@/lib/discover";
import { formatCount, formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useContentUpdatesStore } from "@/stores/contentUpdates";

const RichText = lazy(() => import("@/components/discover/RichText"));

const ALL = "__all__";

/**
 * 项目详情「版本」页签（D25e）：
 * 支持按游戏版本与加载器筛选、展开更新日志、高亮当前实例已安装版本，
 * 并可一键安装指定版本或从已装版本直接切换过去。
 */
export function ProjectVersionsTab({
  source,
  projectId,
  kind,
  instance,
  installedItem,
  gameVersions,
  loaders,
}: {
  source: ContentSource;
  projectId: string;
  kind: ContentKind;
  instance: InstanceSummary | undefined;
  installedItem: ContentItem | null;
  gameVersions: string[];
  loaders: string[];
}) {
  const { t } = useTranslation();
  const autoDependencies = useAppStore((s) => s.settings?.download.autoDependencies ?? true);
  const invalidateUpdates = useContentUpdatesStore((s) => s.invalidate);

  const instanceLoaderSlug = instance?.loader?.split(" ")[0]?.toLowerCase() ?? null;
  const defaultGame =
    instance && gameVersions.includes(instance.gameVersion) ? instance.gameVersion : ALL;
  const defaultLoader =
    kind === "mod" && instanceLoaderSlug && loaders.includes(instanceLoaderSlug)
      ? instanceLoaderSlug
      : ALL;

  const [selectedGame, setSelectedGame] = useState<string>(defaultGame);
  const [selectedLoader, setSelectedLoader] = useState<string>(defaultLoader);
  const [versions, setVersions] = useState<ContentVersion[] | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [changelogs, setChangelogs] = useState<Record<string, string>>({});
  const [loadingLog, setLoadingLog] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmPlan, setConfirmPlan] = useState<PlanItem[] | null>(null);
  const [installing, setInstalling] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setVersions(null);
    commands
      .discoverVersions(
        source,
        projectId,
        selectedGame === ALL ? null : selectedGame,
        selectedLoader === ALL ? null : selectedLoader,
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
  }, [source, projectId, selectedGame, selectedLoader]);

  const gameOptions = useMemo(() => {
    const set = new Set(gameVersions);
    if (instance?.gameVersion) set.add(instance.gameVersion);
    return [
      { value: ALL, label: t("discover.project.versionsAllGames") },
      ...[...set].map((v) => ({ value: v, label: v })),
    ];
  }, [gameVersions, instance?.gameVersion, t]);

  const loaderOptions = useMemo(() => {
    const set = new Set(loaders);
    if (kind === "mod" && instanceLoaderSlug) set.add(instanceLoaderSlug);
    return [
      { value: ALL, label: t("discover.project.versionsAllLoaders") },
      ...[...set].map((l) => ({
        value: l,
        label: t(`discover.loader.${l}`, { defaultValue: l }),
      })),
    ];
  }, [loaders, kind, instanceLoaderSlug, t]);

  const isCurrentVersion = (v: ContentVersion): boolean => {
    if (!installedItem) return false;
    if (installedItem.versionId && installedItem.versionId === v.id) return true;
    if (installedItem.fileName === v.fileName) return true;
    if (installedItem.versionNumber && installedItem.versionNumber === v.versionNumber) return true;
    return false;
  };

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

  const runInstallPlan = async (items: PlanItem[]) => {
    if (!instance) return;
    setInstalling(true);
    try {
      unwrap(await commands.contentInstall(instance.id, items));
      invalidateUpdates(instance.id);
      toast.success(t("install.started", { name: items[0]?.title ?? "" }));
      setConfirmPlan(null);
    } catch (error) {
      reportError(error, "安装指定版本");
    } finally {
      setInstalling(false);
    }
  };

  const handleAction = async (v: ContentVersion) => {
    if (!instance || busyId) return;
    setBusyId(v.id);
    try {
      if (installedItem) {
        const update: UpdateInfo = {
          kind: installedItem.kind,
          fileName: installedItem.fileName,
          title: installedItem.titleZh ?? installedItem.title,
          currentVersion: installedItem.versionNumber,
          versionId: v.id,
          versionNumber: v.versionNumber,
          source: v.source,
          projectId: v.projectId,
          delivery: v.delivery,
          pageUrl: v.pageUrl,
        };
        unwrap(await commands.contentSwitchVersion(instance.id, update));
        invalidateUpdates(instance.id);
        toast.success(
          t("content.switchStarted", {
            name: installedItem.titleZh ?? installedItem.title,
            version: v.versionNumber,
          }),
        );
        return;
      }
      const plan = unwrap(
        await commands.contentResolveVersion(instance.id, v.projectId, v.id, kind, v.source),
      );
      const needsConfirm = plan.length > 1 || plan.some((p) => p.delivery !== "direct");
      if (needsConfirm) {
        setConfirmPlan(plan);
      } else {
        await runInstallPlan(plan);
      }
    } catch (error) {
      reportError(error, installedItem ? "切换内容版本" : "解析指定版本");
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-44">
          <Select
            label={t("discover.project.versions")}
            value={selectedGame}
            onChange={setSelectedGame}
            options={gameOptions}
          />
        </div>
        {kind === "mod" && loaderOptions.length > 1 && (
          <div className="w-40">
            <Select
              label={t("discover.project.loaders")}
              value={selectedLoader}
              onChange={setSelectedLoader}
              options={loaderOptions}
            />
          </div>
        )}
      </div>

      {versions === null ? (
        <div className="grid h-48 place-items-center rounded-panel border border-line bg-surface shadow-raised">
          <LightSpinner className="size-6 text-accent" />
        </div>
      ) : versions.length === 0 ? (
        <EmptyState
          icon={Layers}
          title={t("discover.project.versionsEmpty")}
          hint={t("discover.project.versionsEmptyHint")}
        />
      ) : (
        <div className="flex flex-col gap-2">
          {versions.map((v) => {
            const current = isCurrentVersion(v);
            const isOpen = expanded === v.id;
            const logText = v.changelog ?? changelogs[v.id] ?? "";
            const since = formatSince(v.published, t);
            return (
              <div
                key={v.id}
                className={`flex flex-col rounded-panel border shadow-raised transition-colors duration-(--motion-press) ${
                  current
                    ? "border-accent/60 bg-accent-soft/30"
                    : "border-line bg-surface hover:bg-surface-hover/50"
                }`}
              >
                <div className="flex items-center gap-3 px-4 py-3">
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
                          {t("discover.project.versionsInstalled")}
                        </Tag>
                      )}
                    </div>
                    <p className="mt-1 flex flex-wrap items-center gap-1.5 text-fg-muted text-xs tabular-nums">
                      {v.gameVersions.slice(0, 4).join(", ")}
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
                  {instance && !current && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busyId !== null}
                      onClick={() => void handleAction(v)}
                    >
                      {busyId === v.id ? (
                        <LightSpinner className="size-3.5" />
                      ) : installedItem ? (
                        <ArrowLeftRight className="size-3.5" aria-hidden />
                      ) : (
                        <Plus className="size-3.5" aria-hidden />
                      )}
                      {installedItem
                        ? t("discover.project.versionsSwitch")
                        : t("discover.project.versionsInstall")}
                    </Button>
                  )}
                </div>
                {isOpen && (
                  <div className="border-line border-t bg-surface-2/40 px-5 py-3.5 text-sm">
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

      {confirmPlan && instance && (
        <InstallDialog
          instanceName={instance.name}
          plan={confirmPlan}
          selectDependencies={autoDependencies}
          busy={installing}
          onClose={() => setConfirmPlan(null)}
          onConfirm={(chosen) => void runInstallPlan(chosen)}
        />
      )}
    </div>
  );
}
