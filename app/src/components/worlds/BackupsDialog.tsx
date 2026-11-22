import { Archive, FolderOpen, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { formatTime } from "@/components/HistoryDialog";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { EmptyState } from "@/components/ui/EmptyState";
import { HoverHint } from "@/components/ui/HoverCard";
import { Menu } from "@/components/ui/Menu";
import { Tag } from "@/components/ui/Tag";
import { unwrap } from "@/lib/api";
import { type BackupInfo, commands } from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { isActive, useGameStore } from "@/stores/game";

export interface BackupTarget {
  folder: string;
  name: string;
}

/**
 * 一个世界的备份：列出每一份（怎么来的、什么时候、多大），能恢复、删除、在文件夹里找到；
 * 「现在备份」手动备一份。游戏开着、正在备份或恢复这个世界时按钮都不能点。
 */
export function BackupsDialog({
  instanceId,
  world,
  onClose,
}: {
  instanceId: string;
  /** 看哪个世界的备份；null 时弹窗关着 */
  world: BackupTarget | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const settings = useAppStore((s) => s.settings);
  const openSettings = useAppStore((s) => s.openSettings);
  const backupsVersion = useGameStore((s) => s.backupsVersion);
  const gameRunning = useGameStore((s) => isActive(s.sessions[instanceId]));
  const job = useGameStore((s) =>
    world
      ? Object.values(s.jobs).find(
          (j) => j.key === `backup:${instanceId}:${world.folder}` && j.state === "running",
        )
      : undefined,
  );
  const [list, setList] = useState<BackupInfo[] | null>(null);
  const [restoring, setRestoring] = useState<BackupInfo | null>(null);
  const [deleting, setDeleting] = useState<BackupInfo | null>(null);

  const folder = world?.folder;
  useEffect(() => {
    void backupsVersion;
    if (!folder) return;
    let cancelled = false;
    commands
      .backupsList(instanceId, folder)
      .then(unwrap)
      .then((value) => !cancelled && setList(value))
      .catch((error: unknown) => {
        if (!cancelled) setList([]);
        reportError(error, "读取世界备份");
      });
    return () => {
      cancelled = true;
    };
  }, [instanceId, folder, backupsVersion]);

  const busy = gameRunning || !!job;
  const name = world?.name ?? "";

  const backupNow = async () => {
    if (!world) return;
    try {
      unwrap(await commands.backupsCreate(instanceId, world.folder));
      toast.success(t("backup.started", { name }));
    } catch (error) {
      reportError(error, "备份世界");
    }
  };

  const restore = async (backup: BackupInfo) => {
    if (!world) return;
    setRestoring(null);
    try {
      unwrap(await commands.backupsRestore(instanceId, world.folder, backup.id));
      toast.success(t("backup.restoreStarted", { name }));
    } catch (error) {
      reportError(error, "恢复世界");
    }
  };

  const remove = async (backup: BackupInfo) => {
    if (!world) return;
    setDeleting(null);
    try {
      unwrap(await commands.backupsDelete(instanceId, world.folder, backup.id));
      toast.success(t("backup.deleted"));
      setList((current) => current?.filter((b) => b.id !== backup.id) ?? null);
    } catch (error) {
      reportError(error, "删除世界备份");
    }
  };

  const reveal = (backup: BackupInfo) => {
    if (!world) return;
    commands
      .backupsReveal(instanceId, world.folder, backup.id)
      .then(unwrap)
      .catch((error: unknown) => reportError(error, "打开备份所在的文件夹"));
  };

  const total = job?.progress.total ?? 0;
  const progress = total > 0 ? Math.round(((job?.progress.done ?? 0) / total) * 100) : null;

  return (
    <>
      <Dialog
        open={world !== null}
        onClose={onClose}
        wide
        title={t("backup.title", { name })}
        footer={
          <HoverHint hint={gameRunning ? t("backup.gameRunning") : null}>
            <Button variant="primary" disabled={busy} onClick={() => void backupNow()}>
              <Archive className="size-4" aria-hidden />
              {t("backup.now")}
            </Button>
          </HoverHint>
        }
      >
        <div className="flex flex-col gap-3">
          <div className="flex items-center gap-3 rounded-row bg-surface-2 px-3 py-2">
            <p className="min-w-0 flex-1 text-fg-muted text-sm">
              {job
                ? `${t("backup.running")}${progress !== null ? ` ${progress}%` : ""}`
                : gameRunning
                  ? t("backup.gameRunning")
                  : settings?.worlds.autoBackup
                    ? t("backup.autoOn", { n: settings.worlds.keepBackups })
                    : t("backup.autoOff")}
            </p>
            <Button variant="secondary" size="sm" onClick={() => openSettings("worlds")}>
              {t("backup.openSettings")}
            </Button>
          </div>
          {list?.length === 0 ? (
            <EmptyState
              icon={Archive}
              illustration="empty-worlds"
              title={t("backup.empty")}
              hint={t("backup.emptyHint")}
            />
          ) : (
            <ul className="flex flex-col divide-y divide-line">
              {list?.map((backup) => (
                <li key={backup.id} className="flex items-center gap-3 py-2.5">
                  <Tag>{t(`backup.reason.${backup.reason}`)}</Tag>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm">{formatTime(backup.at ?? 0)}</p>
                    <p className="text-fg-muted text-xs">{formatSize(backup.size ?? 0)}</p>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => setRestoring(backup)}
                  >
                    {t("backup.restore")}
                  </Button>
                  <Menu
                    label={t("backup.title", { name })}
                    items={[
                      {
                        key: "reveal",
                        label: t("backup.reveal"),
                        icon: FolderOpen,
                        onSelect: () => reveal(backup),
                      },
                      {
                        key: "delete",
                        label: t("backup.delete"),
                        icon: Trash2,
                        danger: true,
                        onSelect: () => setDeleting(backup),
                      },
                    ]}
                  />
                </li>
              ))}
            </ul>
          )}
        </div>
      </Dialog>

      <Dialog
        open={restoring !== null}
        onClose={() => setRestoring(null)}
        title={t("backup.restoreTitle", { name, time: formatTime(restoring?.at ?? 0) })}
        footer={
          <>
            <Button variant="ghost" onClick={() => setRestoring(null)}>
              {t("common.cancel")}
            </Button>
            <Button variant="primary" onClick={() => restoring && void restore(restoring)}>
              {t("backup.restore")}
            </Button>
          </>
        }
      >
        <p className="text-fg-muted text-sm">{t("backup.restoreBody")}</p>
      </Dialog>

      <Dialog
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        title={t("backup.deleteTitle", { time: formatTime(deleting?.at ?? 0) })}
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleting(null)}>
              {t("common.cancel")}
            </Button>
            <Button variant="danger" onClick={() => deleting && void remove(deleting)}>
              {t("backup.delete")}
            </Button>
          </>
        }
      >
        <p className="text-fg-muted text-sm">{t("backup.deleteBody")}</p>
      </Dialog>
    </>
  );
}
