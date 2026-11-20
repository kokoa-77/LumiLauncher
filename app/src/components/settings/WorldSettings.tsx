import { AlertTriangle, Check, Download, RefreshCw, Trash2, Upload } from "lucide-react";
import { motion } from "motion/react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  type BackupDirInfo,
  type BackupUsage,
  commands,
  type Settings,
  type StorageReport,
  type UnusedVersion,
} from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { spring } from "@/lib/motion";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Truncated } from "../ui/HoverCard";
import { LightSpinner } from "../ui/LightSpinner";
import { Segmented } from "../ui/Segmented";
import { Switch } from "../ui/Switch";
import { Group, Row, type UpdateSettings } from "./parts";

const KEEP_CHOICES = ["3", "5", "10"] as const;

interface StorageSegment {
  key: "instances" | "sharedGame" | "java" | "backups" | "cache";
  bytes: number;
  color: string;
}

/**
 * 设置「存储与备份」一栏（D17j / D26a）：
 * - 游戏目录（含 `!` 路径防炸警示）
 * - 存储占用分段条、一键安全清缓存、引用感知闲置版本清理
 * - 世界自动备份开关、保留份数、备份占用与全部删除
 * - 偏好设置导出 / 导入（剥离本机路径，绝不含账号凭据）
 */
export function WorldSettings({
  settings,
  update,
}: {
  settings: Settings;
  update: UpdateSettings;
}) {
  const { t } = useTranslation();
  const gameDir = useAppStore((s) => s.gameDir);
  const chooseGameDir = useAppStore((s) => s.chooseGameDir);
  const backupsVersion = useGameStore((s) => s.backupsVersion);
  const bumpBackups = useGameStore((s) => s.bumpBackups);

  const [usage, setUsage] = useState<BackupUsage | null>(null);
  const [backupDir, setBackupDir] = useState<BackupDirInfo | null>(null);
  const [migratingBackupDir, setMigratingBackupDir] = useState(false);
  const [report, setReport] = useState<StorageReport | null>(null);
  const [scanning, setScanning] = useState(false);
  const [cleaningCache, setCleaningCache] = useState(false);

  const [unusedChecking, setUnusedChecking] = useState(false);
  const [unusedList, setUnusedList] = useState<UnusedVersion[] | null>(null);
  const [selectedUnused, setSelectedUnused] = useState<Set<string>>(() => new Set());
  const [cleaningUnused, setCleaningUnused] = useState(false);

  const [confirming, setConfirming] = useState(false);
  const [clearing, setClearing] = useState(false);

  const [profileBusy, setProfileBusy] = useState(false);

  const refreshStorage = useCallback(async () => {
    setScanning(true);
    try {
      setReport(unwrap(await commands.storageScan()));
    } catch (error) {
      reportError(error, "统计磁盘占用");
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    void backupsVersion;
    let cancelled = false;
    commands
      .backupsUsage()
      .then(unwrap)
      .then((value) => !cancelled && setUsage(value))
      .catch((error: unknown) => reportError(error, "统计备份占用"));
    commands
      .backupsDirInfo()
      .then((info) => !cancelled && setBackupDir(info))
      .catch((error: unknown) => reportError(error, "读取备份目录"));
    void refreshStorage();
    return () => {
      cancelled = true;
    };
  }, [backupsVersion, refreshStorage]);

  const keep = String(settings.worlds.keepBackups);
  const keepOptions = KEEP_CHOICES.includes(keep as (typeof KEEP_CHOICES)[number])
    ? KEEP_CHOICES
    : ([...KEEP_CHOICES, keep] as string[]);
  const size = formatSize(usage?.bytes ?? 0);

  const clearBackups = async () => {
    setClearing(true);
    try {
      const freed = unwrap(await commands.backupsClear()) ?? 0;
      toast.success(t("settings.backupCleared", { size: formatSize(freed) }));
      bumpBackups();
      setConfirming(false);
    } catch (error) {
      reportError(error, "删除所有世界备份");
    } finally {
      setClearing(false);
    }
  };

  const cleanCache = async () => {
    setCleaningCache(true);
    try {
      const result = unwrap(await commands.storageCleanCache());
      toast.success(
        t("settings.storage.cleanCacheDone", { size: formatSize(result.freedBytes ?? 0) }),
      );
      await refreshStorage();
    } catch (error) {
      reportError(error, "清理缓存");
    } finally {
      setCleaningCache(false);
    }
  };

  const checkUnusedVersions = async () => {
    setUnusedChecking(true);
    try {
      const list = unwrap(await commands.storageUnusedVersions());
      if (list.length === 0) {
        toast.info(t("settings.storage.unusedNone"));
        return;
      }
      setUnusedList(list);
      setSelectedUnused(new Set(list.map((item) => item.id)));
    } catch (error) {
      reportError(error, "检查闲置游戏版本");
    } finally {
      setUnusedChecking(false);
    }
  };

  const cleanUnusedVersions = async () => {
    const ids = Array.from(selectedUnused);
    if (ids.length === 0) return;
    setCleaningUnused(true);
    try {
      const result = unwrap(await commands.storageCleanUnusedVersions(ids));
      toast.success(
        t("settings.storage.unusedCleaned", {
          n: result.removedCount,
          size: formatSize(result.freedBytes ?? 0),
        }),
      );
      setUnusedList(null);
      await refreshStorage();
    } catch (error) {
      reportError(error, "清理闲置游戏版本");
    } finally {
      setCleaningUnused(false);
    }
  };

  const chooseBackupDir = async () => {
    setMigratingBackupDir(true);
    try {
      const result = unwrap(await commands.backupsDirChoose());
      if (result) {
        setBackupDir(result.info);
        useAppStore.setState({ settings: await commands.settingsGet() });
        toast.success(
          result.movedFiles > 0
            ? t("settings.storage.backupDirChanged", { n: result.movedFiles })
            : t("settings.storage.backupDirChangedEmpty"),
        );
        bumpBackups();
      }
    } catch (error) {
      reportError(error, "更换世界备份目录");
    } finally {
      setMigratingBackupDir(false);
    }
  };

  const resetBackupDir = async () => {
    setMigratingBackupDir(true);
    try {
      const result = unwrap(await commands.backupsDirReset());
      setBackupDir(result.info);
      useAppStore.setState({ settings: await commands.settingsGet() });
      toast.success(
        result.movedFiles > 0
          ? t("settings.storage.backupDirResetDone", { n: result.movedFiles })
          : t("settings.storage.backupDirResetDoneEmpty"),
      );
      bumpBackups();
    } catch (error) {
      reportError(error, "恢复默认世界备份目录");
    } finally {
      setMigratingBackupDir(false);
    }
  };

  const exportSettings = async () => {
    setProfileBusy(true);
    try {
      const saved = unwrap(await commands.settingsExport());
      if (saved) toast.success(t("settings.storage.exported"));
    } catch (error) {
      reportError(error, "导出偏好设置");
    } finally {
      setProfileBusy(false);
    }
  };

  const importSettings = async () => {
    setProfileBusy(true);
    try {
      const imported = unwrap(await commands.settingsImport());
      if (imported) {
        useAppStore.setState({ settings: imported });
        toast.success(t("settings.storage.imported"));
      }
    } catch (error) {
      reportError(error, "导入偏好设置");
    } finally {
      setProfileBusy(false);
    }
  };

  const segments: StorageSegment[] = report
    ? [
        { key: "instances", bytes: report.instancesBytes ?? 0, color: "bg-accent" },
        { key: "sharedGame", bytes: report.sharedGameBytes ?? 0, color: "bg-accent/55" },
        { key: "java", bytes: report.javaBytes ?? 0, color: "bg-fg-muted/65" },
        { key: "backups", bytes: report.backupsBytes ?? 0, color: "bg-fg-muted/35" },
        { key: "cache", bytes: report.cacheBytes ?? 0, color: "bg-warning" },
      ]
    : [];
  const totalBytes = report?.totalBytes ?? 0;
  const pathHasBang = report?.pathHasBang || (gameDir?.path ?? "").includes("!");
  const cacheBytes = report?.cacheBytes ?? 0;
  const cacheFiles = report?.cacheFiles ?? 0;

  let offsetRatio = 0;
  const barSegments = segments.map((seg) => {
    const ratio = totalBytes > 0 ? seg.bytes / totalBytes : 0;
    const x = offsetRatio;
    offsetRatio += ratio;
    return { ...seg, x, ratio };
  });

  const unusedTotalBytes = (unusedList ?? []).reduce((sum, item) => sum + (item.bytes ?? 0), 0);

  return (
    <>
      <Group title={t("settings.storage.dirGroup")}>
        <Row
          label={t("settings.gameDir")}
          hint={gameDir?.locked ? t("settings.gameDirLocked") : t("settings.gameDirHint")}
          control={
            <div className="flex min-w-0 items-center gap-3">
              <Truncated
                className="max-w-60 select-text truncate text-fg-muted text-xs"
                text={gameDir?.path ?? ""}
              />
              <Button
                variant="secondary"
                disabled={gameDir?.locked ?? true}
                onClick={() => {
                  void chooseGameDir().then((changed) => {
                    if (changed) {
                      void useGameStore.getState().refreshInstances();
                      void refreshStorage();
                    }
                  });
                }}
              >
                {t("settings.gameDirChange")}
              </Button>
            </div>
          }
        />
        {pathHasBang && (
          <div className="flex items-start gap-2.5 bg-warning-soft px-4 py-3 text-warning text-xs leading-relaxed">
            <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
            <p>{t("settings.storage.pathBangWarning")}</p>
          </div>
        )}
      </Group>

      <Group title={t("settings.storage.usageGroup")}>
        <div className="flex flex-col gap-3 px-4 py-3.5">
          <div className="flex items-center justify-between gap-3">
            <span className="font-medium text-sm tabular-nums">
              {report
                ? t("settings.storage.total", { size: formatSize(totalBytes) })
                : t("settings.storage.scanning")}
            </span>
            <Button
              variant="ghost"
              size="sm"
              disabled={scanning}
              onClick={() => void refreshStorage()}
            >
              {scanning ? (
                <LightSpinner className="size-3.5" />
              ) : (
                <RefreshCw className="size-3.5" aria-hidden />
              )}
              {t("settings.storage.refresh")}
            </Button>
          </div>
          {report && (
            <>
              <div
                role="img"
                aria-label={t("settings.storage.total", { size: formatSize(totalBytes) })}
                className="relative h-2.5 overflow-hidden rounded-control bg-surface-hover"
              >
                {barSegments.map((seg) => (
                  <motion.div
                    key={seg.key}
                    className={`absolute inset-0 origin-left ${seg.color}`}
                    initial={false}
                    animate={{ x: `${seg.x * 100}%`, scaleX: seg.ratio }}
                    transition={spring.gentle}
                  />
                ))}
              </div>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 text-fg-muted text-xs">
                {segments.map((seg) => (
                  <span key={seg.key} className="inline-flex items-center gap-1.5 tabular-nums">
                    <span className={`size-2 rounded-tag ${seg.color}`} aria-hidden />
                    {t(`settings.storage.${seg.key}`)} {formatSize(seg.bytes)}
                  </span>
                ))}
              </div>
            </>
          )}
        </div>
        <Row
          label={t("settings.storage.cleanCache")}
          hint={t("settings.storage.cleanCacheHint")}
          control={
            <div className="flex items-center gap-3">
              <span className="text-fg-muted text-xs tabular-nums">
                {cacheFiles > 0
                  ? t("settings.storage.cleanCacheCount", {
                      size: formatSize(cacheBytes),
                      n: cacheFiles,
                    })
                  : t("settings.storage.cleanCacheEmpty")}
              </span>
              <Button
                variant="secondary"
                disabled={cleaningCache || cacheFiles === 0}
                onClick={() => void cleanCache()}
              >
                {cleaningCache ? (
                  <LightSpinner className="size-4" />
                ) : (
                  <Trash2 className="size-3.5" aria-hidden />
                )}
                {t("settings.storage.cleanCache")}
              </Button>
            </div>
          }
        />
        <Row
          label={t("settings.storage.unusedLabel")}
          hint={t("settings.storage.unusedHint")}
          control={
            <Button
              variant="secondary"
              disabled={unusedChecking}
              onClick={() => void checkUnusedVersions()}
            >
              {unusedChecking && <LightSpinner className="size-4" />}
              {unusedChecking
                ? t("settings.storage.unusedChecking")
                : t("settings.storage.unusedCheck")}
            </Button>
          }
        />
      </Group>

      <Group title={t("settings.storage.worldsGroup")}>
        <Row
          label={t("settings.autoBackup")}
          hint={t("settings.autoBackupHint")}
          control={
            <Switch
              label={t("settings.autoBackup")}
              checked={settings.worlds.autoBackup}
              onChange={(autoBackup) =>
                update((s) => ({ ...s, worlds: { ...s.worlds, autoBackup } }))
              }
            />
          }
        />
        <Row
          label={t("settings.keepBackups")}
          hint={t("settings.keepBackupsHint")}
          control={
            <Segmented<string>
              label={t("settings.keepBackups")}
              value={keep}
              options={keepOptions.map((value) => ({ value, label: value }))}
              onChange={(value) =>
                update((s) => ({ ...s, worlds: { ...s.worlds, keepBackups: Number(value) } }))
              }
            />
          }
        />
        <Row
          label={t("settings.storage.backupDir")}
          hint={t("settings.storage.backupDirHint")}
          control={
            <div className="flex min-w-0 items-center gap-2">
              <Truncated
                text={backupDir?.path ?? ""}
                className="max-w-52 select-text truncate text-fg-muted text-xs"
              >
                {backupDir?.custom ? backupDir.path : t("settings.storage.backupDirDefault")}
              </Truncated>
              {backupDir?.custom && (
                <Button
                  variant="ghost"
                  disabled={migratingBackupDir}
                  onClick={() => void resetBackupDir()}
                >
                  {t("settings.storage.backupDirReset")}
                </Button>
              )}
              <Button
                variant="secondary"
                disabled={migratingBackupDir}
                onClick={() => void chooseBackupDir()}
              >
                {migratingBackupDir && <LightSpinner className="size-4" />}
                {migratingBackupDir
                  ? t("settings.storage.backupDirMigrating")
                  : t("settings.storage.backupDirChange")}
              </Button>
            </div>
          }
        />
        <Row
          label={t("settings.backupUsage")}
          control={
            <div className="flex items-center gap-3">
              <span className="text-fg-muted text-sm tabular-nums">
                {usage && usage.count > 0
                  ? t("settings.backupUsageValue", { size, n: usage.count })
                  : t("settings.backupUsageNone")}
              </span>
              <Button
                variant="secondary"
                disabled={!usage || usage.count === 0}
                onClick={() => setConfirming(true)}
              >
                {t("settings.backupClear")}
              </Button>
            </div>
          }
        />
      </Group>

      <Group title={t("settings.storage.profileGroup")}>
        <Row
          label={t("settings.storage.profileLabel")}
          hint={t("settings.storage.profileHint")}
          control={
            <div className="flex items-center gap-2">
              <Button
                variant="secondary"
                disabled={profileBusy}
                onClick={() => void exportSettings()}
              >
                <Download className="size-3.5" aria-hidden />
                {t("settings.storage.export")}
              </Button>
              <Button
                variant="outline"
                disabled={profileBusy}
                onClick={() => void importSettings()}
              >
                <Upload className="size-3.5" aria-hidden />
                {t("settings.storage.import")}
              </Button>
            </div>
          }
        />
      </Group>

      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={t("settings.backupClearTitle")}
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              {t("common.cancel")}
            </Button>
            <Button variant="danger" disabled={clearing} onClick={() => void clearBackups()}>
              {t("settings.backupClear")}
            </Button>
          </>
        }
      >
        <p className="text-fg-muted text-sm">{t("settings.backupClearBody", { size })}</p>
      </Dialog>

      <Dialog
        open={unusedList !== null}
        onClose={() => setUnusedList(null)}
        title={t("settings.storage.unusedDialogTitle")}
        footer={
          <>
            <Button variant="ghost" onClick={() => setUnusedList(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="danger"
              disabled={cleaningUnused || selectedUnused.size === 0}
              onClick={() => void cleanUnusedVersions()}
            >
              {cleaningUnused && <LightSpinner className="size-4" />}
              {t("settings.storage.unusedDialogConfirm", { n: selectedUnused.size })}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3">
          <p className="text-fg-muted text-sm">
            {t("settings.storage.unusedDialogIntro", {
              n: unusedList?.length ?? 0,
              size: formatSize(unusedTotalBytes),
            })}
          </p>
          <div className="max-h-64 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
            {(unusedList ?? []).map((item) => {
              const checked = selectedUnused.has(item.id);
              return (
                <button
                  key={item.id}
                  type="button"
                  role="checkbox"
                  aria-checked={checked}
                  onClick={() =>
                    setSelectedUnused((prev) => {
                      const next = new Set(prev);
                      if (next.has(item.id)) next.delete(item.id);
                      else next.add(item.id);
                      return next;
                    })
                  }
                  className="flex w-full items-center gap-3 px-3.5 py-2.5 text-left text-sm transition-colors duration-(--motion-press) hover:bg-surface-hover"
                >
                  <span
                    className={`grid size-4 shrink-0 place-items-center rounded-tag border transition-colors duration-(--motion-press) ${
                      checked
                        ? "border-accent bg-accent text-on-accent"
                        : "border-line bg-surface-2"
                    }`}
                    aria-hidden
                  >
                    {checked && <Check className="size-3" />}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{item.id}</span>
                  <span className="shrink-0 text-fg-muted text-xs tabular-nums">
                    {formatSize(item.bytes ?? 0)}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      </Dialog>
    </>
  );
}
