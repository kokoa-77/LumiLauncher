import { useVirtualizer } from "@tanstack/react-virtual";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle,
  ArrowRight,
  ArrowUpCircle,
  Check,
  CircleAlert,
  CircleCheck,
  ExternalLink,
  Lock,
  Package,
  PauseCircle,
  RotateCcw,
  Search,
  ShieldAlert,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  commands,
  type GameVersionCategory,
  type GameVersionEntry,
  type InstanceSummary,
  type LoaderKind,
  type LoaderRef,
  type LoaderVersion,
  type ModUpgradeAction,
  type ModUpgradeItem,
  type ModUpgradeStatus,
  type UpgradeChoice,
  type UpgradeMode,
} from "@/lib/bindings";
import { fallbackLoaderChoice, isLoaderCompatible, LOADER_LABELS } from "@/lib/loaders";
import { logError, reportError } from "@/lib/report";
import { rememberCopy, useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field, TextInput } from "../ui/Field";
import { Truncated } from "../ui/HoverCard";
import { Segmented } from "../ui/Segmented";
import { Select } from "../ui/Select";
import { Switch } from "../ui/Switch";

type LoaderChoice = "vanilla" | LoaderKind;
type StatusFilter = "all" | ModUpgradeStatus;

const UPGRADE_CATEGORIES: { id: GameVersionCategory; labelKey: string }[] = [
  { id: "release", labelKey: "create.category.release" },
  { id: "snapshot", labelKey: "create.category.snapshot" },
  { id: "aprilFools", labelKey: "create.category.aprilFools" },
  { id: "ancient", labelKey: "create.category.ancient" },
];

const UPGRADE_LOADERS: { id: LoaderChoice; labelKey?: string }[] = [
  { id: "vanilla", labelKey: "create.vanilla" },
  { id: "neoforge" },
  { id: "fabric" },
  { id: "forge" },
  { id: "quilt" },
  { id: "legacyfabric" },
  { id: "babric" },
];

interface CatalogResult {
  key: string;
  versions: LoaderVersion[] | "unsupported" | "error";
}

function parseCurrentLoader(label: string | null): LoaderRef | null {
  if (!label) return null;
  const trimmed = label.trim();
  if (/^legacy\s+fabric\s+/i.test(trimmed)) {
    const version = trimmed.replace(/^legacy\s+fabric\s+/i, "").trim();
    return version ? { kind: "legacyfabric", version } : null;
  }
  const parts = trimmed.split(/\s+/);
  const rawKind = parts[0]?.toLowerCase();
  const version = parts.slice(1).join(" ");
  if (!version) return null;
  if (
    rawKind === "neoforge" ||
    rawKind === "fabric" ||
    rawKind === "forge" ||
    rawKind === "quilt" ||
    rawKind === "legacyfabric" ||
    rawKind === "babric"
  ) {
    return { kind: rawKind, version };
  }
  return null;
}

function formatLoader(loader: LoaderRef | null, vanillaLabel: string): string {
  return loader ? `${LOADER_LABELS[loader.kind]} ${loader.version}` : vanillaLabel;
}

function defaultActionFor(
  item: ModUpgradeItem,
  updateMods: boolean,
  disableIncompatible: boolean,
): ModUpgradeAction {
  if (item.status === "upgradable") return updateMods ? "upgrade" : "keep";
  if (item.status === "incompatible") return disableIncompatible ? "disable" : "keep";
  return "keep";
}

function StatusBadge({ status }: { status: ModUpgradeStatus }) {
  const { t } = useTranslation();
  const tone =
    status === "incompatible"
      ? "text-danger"
      : status === "upgradable" || status === "compatible"
        ? "text-accent"
        : "text-fg-muted";
  const Icon =
    status === "incompatible"
      ? CircleAlert
      : status === "upgradable"
        ? ArrowUpCircle
        : status === "compatible"
          ? CircleCheck
          : status === "locked"
            ? Lock
            : status === "disabled"
              ? PauseCircle
              : ShieldAlert;

  return (
    <span className={`inline-flex shrink-0 items-center gap-1 text-xs ${tone}`}>
      <Icon className="size-3.5" aria-hidden />
      {t(`upgrade.status.${status}`)}
    </span>
  );
}

function ActionButton({
  item,
  action,
  overridden,
  onChange,
}: {
  item: ModUpgradeItem;
  action: ModUpgradeAction;
  overridden: boolean;
  onChange: (next: ModUpgradeAction) => void;
}) {
  const { t } = useTranslation();
  if (item.status === "disabled") return null;

  const options: ModUpgradeAction[] =
    item.status === "upgradable" ? ["upgrade", "keep", "disable"] : ["keep", "disable"];

  const nextAction = () => {
    const idx = options.indexOf(action);
    const next = options[(idx + 1) % options.length] ?? "keep";
    onChange(next);
  };

  const tone =
    action === "upgrade"
      ? "border-accent/40 bg-accent-soft text-accent"
      : action === "disable"
        ? "border-danger/40 bg-danger/10 text-danger"
        : "border-line bg-bg text-fg-muted";

  return (
    <button
      type="button"
      onClick={nextAction}
      title={t("upgrade.action.cycleHint")}
      className={`inline-flex h-6 shrink-0 items-center gap-1 rounded-control border px-2 text-xs transition-colors duration-(--motion-press) hover:brightness-110 ${tone} ${
        overridden ? "font-medium ring-1 ring-accent/30" : ""
      }`}
    >
      {t(`upgrade.action.${action}`)}
    </button>
  );
}

/** 虚拟滚动的模组兼容列表（支持百级以上模组流畅浏览、图标与中文名展示、单条动作覆写） */
function ModCompatibilityList({
  mods,
  updateMods,
  disableIncompatible,
  modActions,
  onAction,
}: {
  mods: ModUpgradeItem[];
  updateMods: boolean;
  disableIncompatible: boolean;
  modActions: Record<string, ModUpgradeAction>;
  onAction: (fileName: string, action: ModUpgradeAction) => void;
}) {
  const { t, i18n } = useTranslation();
  const isZh = i18n.language === "zh-CN";
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: mods.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 54,
    overscan: 6,
  });

  if (mods.length === 0) {
    return (
      <div className="rounded-panel border border-line bg-surface px-4 py-6 text-center text-fg-muted text-sm">
        {t("upgrade.filter.empty")}
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      className="max-h-60 overflow-y-auto rounded-panel border border-line bg-surface"
    >
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => {
          const item = mods[row.index];
          if (!item) return null;
          const primaryTitle = isZh && item.titleZh ? item.titleZh : item.title;
          const secondaryTitle =
            isZh && item.titleZh && item.titleZh !== item.title ? item.title : null;
          const defAction = defaultActionFor(item, updateMods, disableIncompatible);
          const effective = modActions[item.fileName] ?? defAction;
          const overridden =
            modActions[item.fileName] !== undefined && modActions[item.fileName] !== defAction;
          return (
            <div
              key={item.fileName}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: row.size,
                transform: `translateY(${row.start}px)`,
              }}
              className="flex items-center justify-between gap-2.5 border-line/60 border-b px-3 text-sm last:border-b-0"
            >
              <div className="flex min-w-0 flex-1 items-center gap-2.5">
                {item.iconUrl ? (
                  <img
                    src={item.iconUrl}
                    alt=""
                    loading="lazy"
                    className="size-7 shrink-0 rounded-icon object-cover"
                  />
                ) : (
                  <span className="grid size-7 shrink-0 place-items-center rounded-icon bg-bg text-fg-muted">
                    <Package className="size-3.5" aria-hidden />
                  </span>
                )}
                <div className="min-w-0 flex-1">
                  <div className="flex items-baseline gap-1.5">
                    <Truncated className="block truncate font-medium" text={primaryTitle} />
                    {secondaryTitle && (
                      <span className="hidden truncate text-fg-muted text-xs sm:inline">
                        {secondaryTitle}
                      </span>
                    )}
                  </div>
                  <div className="flex items-center gap-2 text-fg-muted text-xs tabular-nums">
                    {(item.currentVersion || item.targetVersion) && (
                      <span className="truncate">
                        {item.currentVersion ?? "—"}
                        {item.targetVersion ? ` → ${item.targetVersion}` : ""}
                      </span>
                    )}
                    {item.delivery === "manual" && (
                      <span className="shrink-0 text-accent">{t("upgrade.manualBadge")}</span>
                    )}
                  </div>
                </div>
              </div>

              <div className="flex shrink-0 items-center gap-2">
                <StatusBadge status={item.status} />
                {item.delivery === "manual" && item.pageUrl && (
                  <button
                    type="button"
                    aria-label={t("upgrade.openDownloadPage")}
                    title={t("upgrade.openDownloadPage")}
                    onClick={() => {
                      if (item.pageUrl) void openUrl(item.pageUrl);
                    }}
                    className="grid size-6 place-items-center rounded-control border border-line text-fg-muted transition-colors duration-(--motion-press) hover:text-fg"
                  >
                    <ExternalLink className="size-3" aria-hidden />
                  </button>
                )}
                <ActionButton
                  item={item}
                  action={effective}
                  overridden={overridden}
                  onChange={(next) => onAction(item.fileName, next)}
                />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/**
 * 实例版本切换与升级向导（懒加载）：
 * 1. 选择目标 Minecraft 版本与加载器（预检支持中途取消）；
 * 2. 联网预检已装模组兼容性与四类风险（补丁 / 降级毁档 / 自定义 Java / 受管整合包脱离），支持筛选、搜索与单条动作覆写；
 * 3. 选择「克隆副本后升级」或「原地升级（自动快照）」，支持自动补装新增前置与 CurseForge 受限补件认领。
 */
export default function UpgradeVersionDialog({
  instance,
  open,
  onClose,
}: {
  instance: InstanceSummary;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const currentLoader = useMemo(() => parseCurrentLoader(instance.loader), [instance.loader]);

  const [versionEntries, setVersionEntries] = useState<GameVersionEntry[]>([]);
  const [category, setCategory] = useState<GameVersionCategory>("release");
  const [gameVersion, setGameVersion] = useState(instance.gameVersion);
  const [loader, setLoader] = useState<LoaderChoice>(currentLoader?.kind ?? "vanilla");
  const [catalog, setCatalog] = useState<CatalogResult | null>(null);
  const [picked, setPicked] = useState<{ key: string; version: string } | null>(null);
  const [attempt, setAttempt] = useState(0);

  const [checking, setChecking] = useState(false);
  const [choice, setChoice] = useState<UpgradeChoice | null>(null);
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [modActions, setModActions] = useState<Record<string, ModUpgradeAction>>({});

  const [mode, setMode] = useState<UpgradeMode>("clone");
  const [cloneName, setCloneName] = useState("");
  const [inPlaceName, setInPlaceName] = useState("");
  const [includeWorlds, setIncludeWorlds] = useState(true);
  const [updateMods, setUpdateMods] = useState(true);
  const [installDeps, setInstallDeps] = useState(true);
  const [disableIncompatible, setDisableIncompatible] = useState(true);
  const [submitting, setSubmitting] = useState(false);

  const tokenRef = useRef<string | null>(null);
  const previewRequestRef = useRef<string | null>(null);

  const discardToken = (token: string | null) => {
    if (!token) return;
    commands
      .instancesUpgradeDiscard(token)
      .catch((error: unknown) => logError(error, "清理版本升级预检缓存"));
  };

  const cancelPreview = () => {
    const reqId = previewRequestRef.current;
    previewRequestRef.current = null;
    if (reqId) {
      commands
        .instancesUpgradeCancelPreview(reqId)
        .catch((error: unknown) => logError(error, "取消版本升级预检"));
    }
    setChecking(false);
  };

  const versionEntriesRef = useRef<GameVersionEntry[]>([]);
  versionEntriesRef.current = versionEntries;

  useEffect(() => {
    if (!open) return;
    const matched = versionEntriesRef.current.find((e) => e.id === instance.gameVersion);
    setCategory(matched?.category ?? "release");
    setGameVersion(instance.gameVersion);
    setLoader(currentLoader?.kind ?? "vanilla");
    setPicked(null);
    setCatalog(null);
    setChoice(null);
    setStatusFilter("all");
    setSearchQuery("");
    setModActions({});
    setMode("clone");
    setIncludeWorlds(true);
    setUpdateMods(true);
    setInstallDeps(true);
    setDisableIncompatible(true);
  }, [open, instance.id, instance.gameVersion, currentLoader]);

  useEffect(() => {
    if (!open || versionEntries.length > 0) return;
    let active = true;
    commands
      .catalogGameVersionEntries()
      .then(unwrap)
      .then((list) => {
        if (!active) return;
        setVersionEntries(list);
        const currentEntry = list.find((e) => e.id === instance.gameVersion);
        if (currentEntry) setCategory(currentEntry.category);
      })
      .catch((error: unknown) => {
        if (active) reportError(error, "读取游戏版本列表");
      });
    return () => {
      active = false;
    };
  }, [open, versionEntries.length, instance.gameVersion]);

  const versions = useMemo(
    () => versionEntries.filter((e) => e.category === category).map((e) => e.id),
    [versionEntries, category],
  );
  const effectiveLoader: LoaderChoice = gameVersion
    ? fallbackLoaderChoice(loader, gameVersion)
    : loader;

  const catalogKey = `${gameVersion}:${effectiveLoader}:${attempt}`;
  useEffect(() => {
    if (!open || !gameVersion || effectiveLoader === "vanilla") return;
    let active = true;
    commands
      .catalogLoaderVersions(gameVersion, effectiveLoader)
      .then(unwrap)
      .then((list) => {
        if (active) setCatalog({ key: catalogKey, versions: list ?? "unsupported" });
      })
      .catch((error: unknown) => {
        if (!active) return;
        setCatalog({ key: catalogKey, versions: "error" });
        reportError(error, "读取加载器版本列表");
      });
    return () => {
      active = false;
    };
  }, [open, gameVersion, effectiveLoader, catalogKey]);

  const catalogVersions = catalog?.key === catalogKey ? catalog.versions : undefined;
  const loaderVersions = Array.isArray(catalogVersions) ? catalogVersions : undefined;
  const pickedVersion = picked?.key === catalogKey ? picked.version : undefined;
  const loaderVersion = loaderVersions?.some((v) => v.version === pickedVersion)
    ? (pickedVersion ?? "")
    : ((loaderVersions?.find((v) => v.stable) ?? loaderVersions?.[0])?.version ?? "");

  const isVanilla = effectiveLoader === "vanilla";
  const targetLoader: LoaderRef | null = isVanilla
    ? null
    : loaderVersion
      ? { kind: effectiveLoader, version: loaderVersion }
      : null;

  const unchanged =
    gameVersion === instance.gameVersion &&
    (targetLoader?.kind ?? null) === (currentLoader?.kind ?? null) &&
    (targetLoader?.version ?? null) === (currentLoader?.version ?? null);

  const canPreview =
    !checking && !unchanged && gameVersion !== "" && (isVanilla || loaderVersion !== "");

  const close = () => {
    cancelPreview();
    discardToken(tokenRef.current);
    tokenRef.current = null;
    setChoice(null);
    onClose();
  };

  const resetPreview = () => {
    discardToken(tokenRef.current);
    tokenRef.current = null;
    setChoice(null);
    setStatusFilter("all");
    setSearchQuery("");
    setModActions({});
  };

  const runPreview = async () => {
    const reqId =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `req-${Date.now()}`;
    previewRequestRef.current = reqId;
    setChecking(true);
    discardToken(tokenRef.current);
    tokenRef.current = null;
    try {
      const next = unwrap(
        await commands.instancesUpgradePreview(instance.id, gameVersion, targetLoader, reqId),
      );
      if (previewRequestRef.current !== reqId) {
        discardToken(next.token);
        return;
      }
      previewRequestRef.current = null;
      tokenRef.current = next.token;
      setChoice(next);
      setStatusFilter("all");
      setSearchQuery("");
      setModActions({});
      if (next.preview.isLinked) {
        setMode("clone");
      }
      // 跨版本降级时，克隆默认不复制世界存档以防误开高版本存档造成损坏
      setIncludeWorlds(!next.preview.isDowngrade);

      const suffix = targetLoader
        ? `${gameVersion} ${LOADER_LABELS[targetLoader.kind]}`
        : gameVersion;
      setCloneName(`${instance.name} (${suffix})`);
      // 若原实例名直接包含旧版本号（例如自动生成的「1.20.1-Fabric 0.20.1」），原地升级默认建议同步替换
      let suggestedInPlace = instance.name;
      if (instance.gameVersion !== gameVersion && suggestedInPlace.includes(instance.gameVersion)) {
        suggestedInPlace = suggestedInPlace.replace(instance.gameVersion, gameVersion);
      }
      if (
        currentLoader &&
        targetLoader &&
        currentLoader.kind === targetLoader.kind &&
        currentLoader.version !== targetLoader.version &&
        suggestedInPlace.includes(currentLoader.version)
      ) {
        suggestedInPlace = suggestedInPlace.replace(currentLoader.version, targetLoader.version);
      }
      setInPlaceName(suggestedInPlace);
    } catch (error) {
      if (previewRequestRef.current === reqId) {
        reportError(error, "检查版本兼容性");
      }
    } finally {
      if (previewRequestRef.current === reqId) {
        previewRequestRef.current = null;
      }
      setChecking(false);
    }
  };

  const p = choice?.preview;

  const filteredMods = useMemo(() => {
    if (!p) return [];
    const q = searchQuery.trim().toLowerCase();
    return p.mods.filter((item) => {
      if (statusFilter !== "all" && item.status !== statusFilter) return false;
      if (!q) return true;
      return (
        item.title.toLowerCase().includes(q) ||
        (item.titleZh?.toLowerCase().includes(q) ?? false) ||
        item.fileName.toLowerCase().includes(q)
      );
    });
  }, [p, statusFilter, searchQuery]);

  const effectiveCounts = useMemo(() => {
    if (!p) return { upgrading: 0, disabling: 0, customCount: 0 };
    let upgrading = 0;
    let disabling = 0;
    let customCount = 0;
    for (const item of p.mods) {
      if (item.status === "disabled") continue;
      const def = defaultActionFor(item, updateMods, disableIncompatible);
      const eff = modActions[item.fileName] ?? def;
      if (modActions[item.fileName] !== undefined && modActions[item.fileName] !== def) {
        customCount += 1;
      }
      if (eff === "upgrade") upgrading += 1;
      else if (eff === "disable") disabling += 1;
    }
    return { upgrading, disabling, customCount };
  }, [p, updateMods, disableIncompatible, modActions]);

  const setModAction = (fileName: string, next: ModUpgradeAction) => {
    setModActions((prev) => ({ ...prev, [fileName]: next }));
  };

  const applyBatchAction = (action: ModUpgradeAction) => {
    setModActions((prev) => {
      const next = { ...prev };
      for (const item of filteredMods) {
        if (item.status === "disabled") continue;
        if (action === "upgrade" && item.status !== "upgradable") continue;
        next[item.fileName] = action;
      }
      return next;
    });
  };

  const hasFilteredUpgradable = filteredMods.some((item) => item.status === "upgradable");
  const hasFilteredEditable = filteredMods.some((item) => item.status !== "disabled");

  const submit = async () => {
    if (!choice) return;
    setSubmitting(true);
    try {
      const started = unwrap(
        await commands.instancesUpgradeStart(choice.token, {
          mode,
          cloneName: mode === "clone" ? cloneName.trim() || null : null,
          newName:
            mode === "inPlace" && inPlaceName.trim() !== "" && inPlaceName.trim() !== instance.name
              ? inPlaceName.trim()
              : null,
          includeWorlds,
          updateMods,
          disableIncompatible,
          installDependencies: installDeps,
          modActions,
        }),
      );
      tokenRef.current = null;
      if (mode === "clone") {
        rememberCopy(started.jobId, started.instanceId);
      }
      useGameStore.getState().showTasksFor(started.jobId);
      toast.success(
        t(mode === "clone" ? "upgrade.startedClone" : "upgrade.startedInPlace", {
          name: mode === "clone" ? cloneName.trim() || instance.name : instance.name,
        }),
      );
      setChoice(null);
      onClose();
    } catch (error) {
      reportError(error, "切换实例版本");
    } finally {
      setSubmitting(false);
    }
  };

  const loaderLabel = effectiveLoader === "vanilla" ? "" : LOADER_LABELS[effectiveLoader];
  const canSubmit = !!choice && !submitting && (mode === "inPlace" || cloneName.trim() !== "");

  const allChips: { key: StatusFilter; label: string; count: number; danger?: boolean }[] = p
    ? [
        { key: "all", label: t("upgrade.filter.all", { n: p.mods.length }), count: p.mods.length },
        {
          key: "upgradable",
          label: t("upgrade.summary.upgradable", { n: p.counts.upgradable }),
          count: p.counts.upgradable,
        },
        {
          key: "incompatible",
          label: t("upgrade.summary.incompatible", { n: p.counts.incompatible }),
          count: p.counts.incompatible,
          danger: p.counts.incompatible > 0,
        },
        {
          key: "compatible",
          label: t("upgrade.summary.compatible", { n: p.counts.compatible }),
          count: p.counts.compatible,
        },
        {
          key: "locked",
          label: t("upgrade.summary.locked", { n: p.counts.locked }),
          count: p.counts.locked,
        },
        {
          key: "unknown",
          label: t("upgrade.summary.unknown", { n: p.counts.unknown }),
          count: p.counts.unknown,
        },
        {
          key: "disabled",
          label: t("upgrade.summary.disabled", { n: p.counts.disabled }),
          count: p.counts.disabled,
        },
      ]
    : [];
  const filterChips = allChips.filter((chip) => chip.key === "all" || chip.count > 0);

  return (
    <Dialog
      open={open}
      onClose={close}
      wide
      title={t("upgrade.title")}
      footer={
        !choice ? (
          <>
            <Button variant="ghost" onClick={checking ? cancelPreview : close}>
              {checking ? t("upgrade.cancelCheck") : t("common.cancel")}
            </Button>
            <Button variant="primary" disabled={!canPreview} onClick={() => void runPreview()}>
              {checking ? t("upgrade.checking") : t("upgrade.check")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" disabled={submitting} onClick={resetPreview}>
              {t("upgrade.back")}
            </Button>
            <Button variant="primary" disabled={!canSubmit} onClick={() => void submit()}>
              {mode === "clone" ? t("upgrade.confirmClone") : t("upgrade.confirmInPlace")}
            </Button>
          </>
        )
      }
    >
      {!p ? (
        <div className="flex flex-col gap-4">
          <div className="flex items-center justify-between gap-3 rounded-panel border border-line bg-surface px-3.5 py-2.5 text-sm">
            <span className="text-fg-muted">{t("upgrade.current")}</span>
            <span className="font-medium tabular-nums">
              Minecraft {instance.gameVersion} · {formatLoader(currentLoader, t("create.vanilla"))}
            </span>
          </div>

          <div className="flex flex-col gap-2">
            <span className="font-medium text-sm">{t("create.gameVersion")}</span>
            <div
              className="flex flex-wrap gap-1.5"
              role="tablist"
              aria-label={t("create.gameVersion")}
            >
              {UPGRADE_CATEGORIES.map((item) => {
                const active = item.id === category;
                return (
                  <button
                    key={item.id}
                    type="button"
                    role="tab"
                    aria-selected={active}
                    disabled={checking}
                    onClick={() => {
                      setCategory(item.id);
                      const first = versionEntries.find((e) => e.category === item.id)?.id;
                      if (first) {
                        setPicked(null);
                        setCatalog(null);
                        setGameVersion(first);
                        setLoader((prev) => fallbackLoaderChoice(prev, first));
                      }
                    }}
                    className={`inline-flex h-7 items-center gap-1 rounded-control border px-2.5 text-xs transition-colors duration-(--motion-press) ${
                      active
                        ? "border-accent/45 bg-accent-soft font-medium text-accent"
                        : "border-line bg-surface text-fg-muted hover:bg-surface-hover hover:text-fg"
                    }`}
                  >
                    {active && <Check className="size-3 shrink-0" aria-hidden />}
                    <span>{t(item.labelKey)}</span>
                  </button>
                );
              })}
            </div>
            <Select
              label={t("create.gameVersion")}
              value={gameVersion}
              disabled={versions.length === 0 || checking}
              placeholder={t("common.loading")}
              searchable
              searchPlaceholder={t("create.searchVersion")}
              emptySearchText={t("create.noSearchMatch")}
              onChange={(version) => {
                setPicked(null);
                setCatalog(null);
                setGameVersion(version);
                setLoader((prev) => fallbackLoaderChoice(prev, version));
              }}
              options={versions.map((v) => ({ value: v, label: v }))}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="font-medium text-sm">{t("create.loader")}</span>
            <div
              className="flex flex-wrap gap-1.5"
              role="radiogroup"
              aria-label={t("create.loader")}
            >
              {UPGRADE_LOADERS.map((item) => {
                const selected = item.id === effectiveLoader;
                const compatible =
                  item.id === "vanilla" || !gameVersion || isLoaderCompatible(item.id, gameVersion);
                const label = item.id === "vanilla" ? t("create.vanilla") : LOADER_LABELS[item.id];
                return (
                  <button
                    key={item.id}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    disabled={!compatible || checking}
                    title={compatible ? label : t("create.loaderIncompatibleHint")}
                    onClick={() => {
                      setPicked(null);
                      setCatalog(null);
                      setLoader(item.id);
                    }}
                    className={`inline-flex h-8 items-center gap-1 rounded-control border px-2.5 text-xs transition-colors duration-(--motion-press) ${
                      selected
                        ? "border-accent/45 bg-accent-soft font-medium text-accent"
                        : compatible
                          ? "border-line bg-surface text-fg hover:bg-surface-hover"
                          : "cursor-not-allowed border-line/60 bg-surface/50 text-fg-muted opacity-45"
                    }`}
                  >
                    {selected && <Check className="size-3 shrink-0" aria-hidden />}
                    <span>{label}</span>
                  </button>
                );
              })}
            </div>
          </div>

          {!isVanilla && (
            <Field
              label={t("create.loaderVersion", { loader: loaderLabel })}
              hint={
                catalogVersions === "unsupported"
                  ? t("create.unsupportedLoader", { loader: loaderLabel, version: gameVersion })
                  : loaderVersions?.length === 0
                    ? t("create.noLoader", { loader: loaderLabel, version: gameVersion })
                    : undefined
              }
            >
              {catalogVersions === "error" ? (
                <Button onClick={() => setAttempt((n) => n + 1)}>{t("create.retryCatalog")}</Button>
              ) : catalogVersions === "unsupported" ? null : (
                <Select
                  label={t("create.loaderVersion", { loader: loaderLabel })}
                  value={loaderVersion}
                  disabled={!loaderVersions?.length || checking}
                  placeholder={loaderVersions ? "" : t("common.loading")}
                  searchable={(loaderVersions?.length ?? 0) > 8}
                  searchPlaceholder={t("create.searchLoaderVersion")}
                  emptySearchText={t("create.noSearchMatch")}
                  onChange={(version) => setPicked({ key: catalogKey, version })}
                  options={(loaderVersions ?? []).map((v) => ({
                    value: v.version,
                    label: v.stable ? v.version : t("create.beta", { version: v.version }),
                  }))}
                />
              )}
            </Field>
          )}

          {unchanged && <p className="text-fg-muted text-xs">{t("upgrade.unchangedHint")}</p>}
        </div>
      ) : (
        <div className="flex flex-col gap-3.5">
          <div className="flex flex-wrap items-center gap-2 rounded-panel border border-line bg-surface px-3.5 py-2.5 text-sm">
            <span className="text-fg-muted tabular-nums">
              {p.currentGameVersion} · {formatLoader(p.currentLoader, t("create.vanilla"))}
            </span>
            <ArrowRight className="size-4 shrink-0 text-fg-muted" aria-hidden />
            <span className="font-medium text-accent tabular-nums">
              {p.targetGameVersion} · {formatLoader(p.targetLoader, t("create.vanilla"))}
            </span>
          </div>

          {p.isDowngrade && (
            <div className="flex items-start gap-2.5 rounded-panel border border-danger/40 bg-danger/5 p-3 text-sm">
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />
              <p className="text-fg">{t("upgrade.risk.downgrade")}</p>
            </div>
          )}

          {!p.sameGameVersion && !p.isDowngrade && p.hasWorlds && (
            <div className="flex items-start gap-2.5 rounded-panel border border-line bg-surface p-3 text-sm">
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
              <p className="text-fg-muted">{t("upgrade.risk.worldMigration")}</p>
            </div>
          )}

          {p.hasPatches && (!p.sameGameVersion || !p.sameLoaderKind) && (
            <div className="flex items-start gap-2.5 rounded-panel border border-line bg-surface p-3 text-sm">
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
              <p className="text-fg-muted">
                {t("upgrade.risk.patches", { names: p.patchNames.join("、") })}
              </p>
            </div>
          )}

          {p.hasCustomJava && (
            <div className="flex items-start gap-2.5 rounded-panel border border-line bg-surface p-3 text-sm">
              <CircleAlert className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
              <p className="text-fg-muted">{t("upgrade.risk.customJava")}</p>
            </div>
          )}

          {p.managedPackName && (!p.sameGameVersion || !p.sameLoaderKind) && (
            <div className="flex items-start gap-2.5 rounded-panel border border-line bg-surface p-3 text-sm">
              <CircleAlert className="mt-0.5 size-4 shrink-0 text-fg-muted" aria-hidden />
              <p className="text-fg-muted">
                {t("upgrade.risk.managedPack", { name: p.managedPackName })}
              </p>
            </div>
          )}

          {p.lookupFailed && (
            <div className="flex items-start gap-2.5 rounded-panel border border-line p-3 text-sm">
              <CircleAlert className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />
              <p className="text-fg-muted">{t("upgrade.lookupFailed")}</p>
            </div>
          )}

          {p.manualCount > 0 && (
            <div className="flex items-start gap-2.5 rounded-panel border border-line bg-surface p-3 text-sm">
              <CircleAlert className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
              <p className="text-fg-muted">{t("upgrade.manualHint", { n: p.manualCount })}</p>
            </div>
          )}

          {p.mods.length > 0 ? (
            <div className="flex flex-col gap-2">
              <div className="flex flex-wrap items-center gap-1.5 text-xs tabular-nums">
                {filterChips.map((chip) => {
                  const active = statusFilter === chip.key;
                  return (
                    <button
                      key={chip.key}
                      type="button"
                      onClick={() => setStatusFilter(chip.key)}
                      className={`rounded-control border px-2.5 py-1 transition-colors duration-(--motion-press) ${
                        active
                          ? "border-accent/40 bg-accent-soft font-medium text-accent"
                          : chip.danger
                            ? "border-line bg-surface text-danger hover:border-danger/40"
                            : "border-line bg-surface text-fg-muted hover:text-fg"
                      }`}
                    >
                      {chip.label}
                    </button>
                  );
                })}
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <div className="relative min-w-44 flex-1">
                  <Search
                    className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-fg-muted"
                    aria-hidden
                  />
                  <input
                    type="search"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder={t("upgrade.filter.searchPlaceholder")}
                    className="h-8 w-full rounded-control border border-line bg-surface pr-3 pl-8 text-sm outline-none transition-colors duration-(--motion-press) focus:border-accent"
                  />
                </div>
                {hasFilteredEditable && (
                  <div className="flex shrink-0 items-center gap-1">
                    {hasFilteredUpgradable && (
                      <Button variant="ghost" size="sm" onClick={() => applyBatchAction("upgrade")}>
                        {t("upgrade.action.batchUpgrade")}
                      </Button>
                    )}
                    <Button variant="ghost" size="sm" onClick={() => applyBatchAction("keep")}>
                      {t("upgrade.action.batchKeep")}
                    </Button>
                    <Button variant="ghost" size="sm" onClick={() => applyBatchAction("disable")}>
                      {t("upgrade.action.batchDisable")}
                    </Button>
                  </div>
                )}
                {effectiveCounts.customCount > 0 && (
                  <Button variant="ghost" size="sm" onClick={() => setModActions({})}>
                    <RotateCcw className="size-3.5" aria-hidden />
                    {t("upgrade.action.reset", { n: effectiveCounts.customCount })}
                  </Button>
                )}
              </div>

              <ModCompatibilityList
                mods={filteredMods}
                updateMods={updateMods}
                disableIncompatible={disableIncompatible}
                modActions={modActions}
                onAction={setModAction}
              />
            </div>
          ) : (
            <p className="text-fg-muted text-sm">{t("upgrade.noMods")}</p>
          )}

          {p.counts.upgradable > 0 && (
            <div className="flex items-center justify-between gap-6">
              <div className="min-w-0">
                <p className="font-medium text-sm">{t("upgrade.option.updateMods")}</p>
                <p className="mt-0.5 text-fg-muted text-xs">
                  {t("upgrade.option.updateModsHint", { n: effectiveCounts.upgrading })}
                </p>
              </div>
              <Switch
                label={t("upgrade.option.updateMods")}
                checked={updateMods}
                onChange={setUpdateMods}
              />
            </div>
          )}

          {effectiveCounts.upgrading > 0 && p.targetLoader && (
            <div className="flex items-center justify-between gap-6">
              <div className="min-w-0">
                <p className="font-medium text-sm">{t("upgrade.option.installDeps")}</p>
                <p className="mt-0.5 text-fg-muted text-xs">
                  {t("upgrade.option.installDepsHint")}
                </p>
              </div>
              <Switch
                label={t("upgrade.option.installDeps")}
                checked={installDeps}
                onChange={setInstallDeps}
              />
            </div>
          )}

          {p.counts.incompatible > 0 && (
            <div className="flex items-center justify-between gap-6">
              <div className="min-w-0">
                <p className="font-medium text-sm">{t("upgrade.option.disableIncompatible")}</p>
                <p className="mt-0.5 text-fg-muted text-xs">
                  {t("upgrade.option.disableIncompatibleHint", { n: effectiveCounts.disabling })}
                </p>
              </div>
              <Switch
                label={t("upgrade.option.disableIncompatible")}
                checked={disableIncompatible}
                onChange={setDisableIncompatible}
              />
            </div>
          )}

          <div className="flex flex-col gap-2">
            <span className="font-medium text-sm">{t("upgrade.mode.label")}</span>
            <Segmented
              label={t("upgrade.mode.label")}
              value={mode}
              onChange={setMode}
              options={[
                {
                  value: "clone",
                  label: p.isLinked ? t("upgrade.mode.cloneLinked") : t("upgrade.mode.clone"),
                },
                {
                  value: "inPlace",
                  label: t("upgrade.mode.inPlace"),
                  disabled: p.isLinked,
                },
              ]}
            />
            <p className="text-fg-muted text-xs">
              {p.isLinked
                ? t("upgrade.mode.linkedOnlyHint")
                : mode === "clone"
                  ? t("upgrade.mode.cloneHint")
                  : t("upgrade.mode.inPlaceHint")}
            </p>
          </div>

          {mode === "inPlace" && (
            <Field label={t("upgrade.inPlaceName")}>
              <TextInput
                value={inPlaceName}
                maxLength={64}
                onChange={(e) => setInPlaceName(e.target.value)}
              />
            </Field>
          )}

          {mode === "clone" && (
            <div className="flex flex-col gap-3">
              <Field label={t("upgrade.cloneName")}>
                <TextInput
                  value={cloneName}
                  maxLength={64}
                  onChange={(e) => setCloneName(e.target.value)}
                />
              </Field>
              <div className="flex items-center justify-between gap-6">
                <div className="min-w-0">
                  <p className="font-medium text-sm">{t("copy.worlds")}</p>
                  <p className="mt-0.5 text-fg-muted text-xs">{t("copy.worldsHint")}</p>
                </div>
                <Switch
                  label={t("copy.worlds")}
                  checked={includeWorlds}
                  onChange={setIncludeWorlds}
                />
              </div>
            </div>
          )}
        </div>
      )}
    </Dialog>
  );
}
