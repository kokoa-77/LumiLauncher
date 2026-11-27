import { Check } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  commands,
  type GameVersionCategory,
  type GameVersionEntry,
  type LoaderKind,
  type LoaderVersion,
} from "@/lib/bindings";
import { fallbackLoaderChoice, isLoaderCompatible, LOADER_LABELS } from "@/lib/loaders";
import { reportError } from "@/lib/report";
import { LoaderBadgeIcon } from "./art/LoaderBadgeIcon";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { Field, TextInput } from "./ui/Field";
import { Select } from "./ui/Select";

type LoaderChoice = "vanilla" | LoaderKind;

interface CatalogResult {
  key: string;
  versions: LoaderVersion[] | "unsupported" | "error";
}

const CATEGORIES: { id: GameVersionCategory; labelKey: string }[] = [
  { id: "release", labelKey: "create.category.release" },
  { id: "snapshot", labelKey: "create.category.snapshot" },
  { id: "aprilFools", labelKey: "create.category.aprilFools" },
  { id: "ancient", labelKey: "create.category.ancient" },
];

const LOADER_CHOICES: { id: LoaderChoice; labelKey?: string }[] = [
  { id: "vanilla", labelKey: "create.vanilla" },
  { id: "neoforge" },
  { id: "fabric" },
  { id: "forge" },
  { id: "quilt" },
  { id: "legacyfabric" },
  { id: "babric" },
];

function orderCategoryEntries(
  entries: GameVersionEntry[],
  category: GameVersionCategory,
): GameVersionEntry[] {
  const list = entries.filter((entry) => entry.category === category);
  if (category === "release") {
    const idx = list.findIndex((entry) => entry.isLatestRelease);
    if (idx > 0) {
      const latest = list[idx];
      if (latest) return [latest, ...list.slice(0, idx), ...list.slice(idx + 1)];
    }
  } else if (category === "snapshot") {
    const idx = list.findIndex((entry) => entry.isLatestSnapshot);
    if (idx > 0) {
      const latest = list[idx];
      if (latest) return [latest, ...list.slice(0, idx), ...list.slice(idx + 1)];
    }
  }
  return list;
}

/** 新建实例向导 v2：全版本矩阵（正式/快照/愚人节/远古）、可搜索版本清单与全加载器兼容筛选。 */
export function CreateInstanceDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<GameVersionEntry[]>([]);
  const [category, setCategory] = useState<GameVersionCategory>("release");
  const [pickedGameVersion, setPickedGameVersion] = useState<string | null>(null);
  const [loader, setLoader] = useState<LoaderChoice>("vanilla");
  const [catalog, setCatalog] = useState<CatalogResult | null>(null);
  const [picked, setPicked] = useState<{ key: string; version: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  /** 手动改过的名字；null 表示用自动名字（跟着版本变） */
  const [customName, setCustomName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open || entries.length > 0) return;
    let current = true;
    commands
      .catalogGameVersionEntries()
      .then(unwrap)
      .then((list) => {
        if (current) setEntries(list);
      })
      .catch((error: unknown) => {
        if (current) reportError(error, "读取游戏版本列表");
      });
    return () => {
      current = false;
    };
  }, [open, entries.length]);

  const categoryCounts = useMemo(() => {
    const counts: Record<GameVersionCategory, number> = {
      release: 0,
      snapshot: 0,
      aprilFools: 0,
      ancient: 0,
    };
    for (const entry of entries) counts[entry.category] += 1;
    return counts;
  }, [entries]);

  const visibleEntries = useMemo(
    () => orderCategoryEntries(entries, category),
    [entries, category],
  );

  const gameVersion =
    pickedGameVersion && visibleEntries.some((v) => v.id === pickedGameVersion)
      ? pickedGameVersion
      : (visibleEntries[0]?.id ?? "");

  const effectiveLoader: LoaderChoice = gameVersion
    ? fallbackLoaderChoice(loader, gameVersion)
    : loader;

  const key = `${gameVersion}:${effectiveLoader}:${attempt}`;
  useEffect(() => {
    if (!open || !gameVersion || effectiveLoader === "vanilla") return;
    let current = true;
    commands
      .catalogLoaderVersions(gameVersion, effectiveLoader)
      .then(unwrap)
      .then((list) => {
        if (current) setCatalog({ key, versions: list ?? "unsupported" });
      })
      .catch((error: unknown) => {
        if (!current) return;
        setCatalog({ key, versions: "error" });
        reportError(error, "读取加载器版本列表");
      });
    return () => {
      current = false;
    };
  }, [open, gameVersion, effectiveLoader, key]);

  // 按请求标识匹配，切换游戏或加载器时不会拿上一组的版本去创建。
  const catalogVersions = catalog?.key === key ? catalog.versions : undefined;
  const loaderVersions = Array.isArray(catalogVersions) ? catalogVersions : undefined;
  const pickedVersion = picked?.key === key ? picked.version : undefined;
  const loaderVersion = loaderVersions?.some((v) => v.version === pickedVersion)
    ? (pickedVersion ?? "")
    : ((loaderVersions?.find((v) => v.stable) ?? loaderVersions?.[0])?.version ?? "");
  const isVanilla = effectiveLoader === "vanilla";
  const loaderLabel = isVanilla ? "" : LOADER_LABELS[effectiveLoader];
  const autoName =
    loaderLabel && gameVersion
      ? `${gameVersion}-${loaderLabel}${loaderVersion ? ` ${loaderVersion}` : ""}`
      : gameVersion;
  const name = customName ?? autoName;
  const canCreate =
    !busy && name.trim() !== "" && gameVersion !== "" && (isVanilla || loaderVersion !== "");

  const close = () => {
    setCategory("release");
    setLoader("vanilla");
    setPickedGameVersion(null);
    setPicked(null);
    onClose();
  };

  const selectGameVersion = (nextVersion: string) => {
    setPicked(null);
    setCatalog(null);
    setPickedGameVersion(nextVersion);
    setLoader((prev) => fallbackLoaderChoice(prev, nextVersion));
  };

  const selectCategory = (nextCategory: GameVersionCategory) => {
    if (nextCategory === category) return;
    const nextList = orderCategoryEntries(entries, nextCategory);
    const first = nextList[0]?.id ?? "";
    setCategory(nextCategory);
    setPickedGameVersion(first || null);
    setPicked(null);
    setCatalog(null);
    if (first) setLoader((prev) => fallbackLoaderChoice(prev, first));
  };

  const create = async () => {
    setBusy(true);
    try {
      unwrap(
        await commands.instancesCreate(
          name.trim(),
          gameVersion,
          isVanilla ? null : { kind: effectiveLoader, version: loaderVersion },
        ),
      );
      toast.success(t("create.started", { name: name.trim() }));
      close();
      setCustomName(null);
    } catch (error) {
      reportError(error, "新建实例");
    } finally {
      setBusy(false);
    }
  };

  const latestLabel = t("create.latestBadge");

  return (
    <Dialog
      open={open}
      onClose={close}
      title={t("create.title")}
      wide
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" disabled={!canCreate} onClick={() => void create()}>
            {t("create.confirm")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("create.name")} hint={t("create.nameHint")}>
          <TextInput
            value={name}
            placeholder={autoName}
            maxLength={48}
            onChange={(event) => setCustomName(event.target.value)}
            // 清空后离开输入框：恢复自动名字
            onBlur={() => {
              if (customName?.trim() === "") setCustomName(null);
            }}
          />
        </Field>

        <div className="flex flex-col gap-2">
          <span className="font-medium text-sm">{t("create.gameVersion")}</span>
          <div
            className="flex flex-wrap gap-1.5"
            role="tablist"
            aria-label={t("create.gameVersion")}
          >
            {CATEGORIES.map((item) => {
              const active = item.id === category;
              const count = categoryCounts[item.id];
              return (
                <button
                  key={item.id}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  onClick={() => selectCategory(item.id)}
                  className={`inline-flex h-8 items-center gap-1.5 rounded-control border px-3 text-sm transition-colors duration-(--motion-press) ${
                    active
                      ? "border-accent/45 bg-accent-soft font-medium text-accent"
                      : "border-line bg-surface text-fg-muted hover:bg-surface-hover hover:text-fg"
                  }`}
                >
                  {active && <Check className="size-3.5 shrink-0" aria-hidden />}
                  <span>{t(item.labelKey)}</span>
                  {entries.length > 0 && (
                    <span className="text-xs tabular-nums opacity-70">{count}</span>
                  )}
                </button>
              );
            })}
          </div>
          <Select
            label={t("create.gameVersion")}
            value={gameVersion}
            disabled={visibleEntries.length === 0}
            placeholder={t("common.loading")}
            searchable
            searchPlaceholder={t("create.searchVersion")}
            emptySearchText={t("create.noSearchMatch")}
            onChange={selectGameVersion}
            options={visibleEntries.map((v) => ({
              value: v.id,
              label:
                (category === "release" && v.isLatestRelease) ||
                (category === "snapshot" && v.isLatestSnapshot)
                  ? `${v.id} · ${latestLabel}`
                  : v.id,
            }))}
          />
        </div>

        <div className="flex flex-col gap-2">
          <span className="font-medium text-sm">{t("create.loader")}</span>
          <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-label={t("create.loader")}>
            {LOADER_CHOICES.map((choice) => {
              const selected = choice.id === effectiveLoader;
              const compatible =
                choice.id === "vanilla" ||
                !gameVersion ||
                isLoaderCompatible(choice.id, gameVersion);
              const label =
                choice.id === "vanilla" ? t("create.vanilla") : LOADER_LABELS[choice.id];
              return (
                <button
                  key={choice.id}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  disabled={!compatible}
                  title={compatible ? label : t("create.loaderIncompatibleHint")}
                  onClick={() => {
                    setPicked(null);
                    setCatalog(null);
                    setLoader(choice.id);
                  }}
                  className={`inline-flex h-8 items-center gap-1.5 rounded-control border px-2.5 text-sm transition-colors duration-(--motion-press) ${
                    selected
                      ? "border-accent/45 bg-accent-soft font-medium text-accent"
                      : compatible
                        ? "border-line bg-surface text-fg hover:bg-surface-hover"
                        : "cursor-not-allowed border-line/60 bg-surface/50 text-fg-muted opacity-45"
                  }`}
                >
                  {selected ? (
                    <Check className="size-3.5 shrink-0" aria-hidden />
                  ) : (
                    <LoaderBadgeIcon
                      loader={choice.id === "vanilla" ? null : choice.id}
                      className="size-3.5 shrink-0"
                    />
                  )}
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
                  : loaderVersions?.length
                    ? t("create.loaderHint")
                    : undefined
            }
          >
            {catalogVersions === "error" ? (
              <Button onClick={() => setAttempt((n) => n + 1)}>{t("create.retryCatalog")}</Button>
            ) : catalogVersions === "unsupported" ? null : (
              <Select
                label={t("create.loaderVersion", { loader: loaderLabel })}
                value={loaderVersion}
                disabled={!loaderVersions?.length}
                placeholder={loaderVersions ? "" : t("common.loading")}
                searchable={(loaderVersions?.length ?? 0) > 8}
                searchPlaceholder={t("create.searchLoaderVersion")}
                emptySearchText={t("create.noSearchMatch")}
                onChange={(version) => setPicked({ key, version })}
                options={(loaderVersions ?? []).map((v) => ({
                  value: v.version,
                  label: v.stable ? v.version : t("create.beta", { version: v.version }),
                }))}
              />
            )}
          </Field>
        )}
      </div>
    </Dialog>
  );
}
