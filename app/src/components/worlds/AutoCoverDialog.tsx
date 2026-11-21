import { Check, Images, Lock, Unlock } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import emptyWorlds from "@/assets/art/empty-worlds.webp";
import { formatTime } from "@/components/HistoryDialog";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { EmptyState } from "@/components/ui/EmptyState";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Tag } from "@/components/ui/Tag";
import { unwrap } from "@/lib/api";
import { type AutoCoverView, commands } from "@/lib/bindings";
import { imageAt } from "@/lib/cover";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";

export interface AutoCoverTarget {
  folder: string;
  name: string;
}

/**
 * 自动封面精选弹窗：展示该世界最近保存的最多 5 张自动封面，
 * 点击任意一张即可锁定为固定封面（游玩时不再被新截屏覆盖）；随时可恢复自动更新。
 */
export function AutoCoverDialog({
  instanceId,
  world,
  onClose,
}: {
  instanceId: string;
  world: AutoCoverTarget | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const refreshRecent = useGameStore((s) => s.refreshRecentWorlds);
  const [items, setItems] = useState<AutoCoverView[] | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!world) return;
    try {
      setItems(unwrap(await commands.worldsAutoCoversList(instanceId, world.folder)));
    } catch (error) {
      reportError(error, "读取自动封面");
      setItems([]);
    }
  }, [instanceId, world]);

  useEffect(() => {
    if (!world) {
      setItems(null);
      return;
    }
    void load();
  }, [world, load]);

  const lock = async (file: string) => {
    if (!world || busy) return;
    setBusy(true);
    try {
      unwrap(await commands.worldsAutoCoverLock(instanceId, world.folder, file));
      await Promise.all([load(), refreshRecent()]);
      toast.success(t("background.autoLocked"));
    } catch (error) {
      reportError(error, "锁定自动封面");
    } finally {
      setBusy(false);
    }
  };

  const unlock = async () => {
    if (!world || busy) return;
    setBusy(true);
    try {
      unwrap(await commands.worldsAutoCoverUnlock(instanceId, world.folder));
      await Promise.all([load(), refreshRecent()]);
      toast.success(t("background.autoUnlocked"));
    } catch (error) {
      reportError(error, "恢复自动封面");
    } finally {
      setBusy(false);
    }
  };

  const hasLocked = items?.some((item) => item.locked) ?? false;

  return (
    <Dialog
      open={world !== null}
      onClose={onClose}
      wide
      title={t("background.autoTitle", { name: world?.name ?? "" })}
      footer={
        <>
          {hasLocked && (
            <Button variant="secondary" disabled={busy} onClick={() => void unlock()}>
              <Unlock className="size-4" aria-hidden />
              {t("background.autoUnlock")}
            </Button>
          )}
          <Button variant="ghost" onClick={onClose}>
            {t("dialog.close")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-fg-muted text-sm">{t("background.autoHint")}</p>
        {items === null ? (
          <div className="grid h-44 place-items-center">
            <LightSpinner className="size-6 text-accent" />
          </div>
        ) : items.length === 0 ? (
          <EmptyState
            icon={Images}
            illustration={emptyWorlds}
            title={t("background.autoEmpty")}
            hint={t("background.autoEmptyHint")}
          />
        ) : (
          <div className="grid grid-cols-2 gap-3">
            {items.map((item) => (
              <button
                key={item.file}
                type="button"
                disabled={busy}
                onClick={() => void lock(item.file)}
                className={`group relative flex flex-col overflow-hidden rounded-panel border text-left transition-colors duration-(--motion-press) ${
                  item.locked || item.active
                    ? "border-accent bg-accent-soft/40"
                    : "border-line bg-surface hover:border-accent/55 hover:bg-surface-hover"
                }`}
              >
                <div className="relative aspect-video w-full overflow-hidden bg-surface-2">
                  <img
                    src={imageAt(item.url, 480)}
                    alt=""
                    loading="lazy"
                    className="hover-zoom size-full object-cover transition-transform duration-(--motion-enter) ease-out group-hover:scale-[1.03]"
                  />
                  {item.locked ? (
                    <Tag
                      tone="hero"
                      className="absolute top-2 right-2 gap-1 bg-accent text-on-accent"
                    >
                      <Lock className="size-3" aria-hidden />
                      {t("background.autoLockedTag")}
                    </Tag>
                  ) : item.active ? (
                    <Tag tone="hero" className="absolute top-2 right-2 gap-1">
                      <Check className="size-3" aria-hidden />
                      {t("background.autoActiveTag")}
                    </Tag>
                  ) : null}
                </div>
                <div className="flex items-center justify-between gap-2 px-3 py-2 text-xs">
                  <span className="truncate text-fg-muted tabular-nums">
                    {item.takenAt ? formatTime(item.takenAt) : ""}
                  </span>
                  <span className="shrink-0 font-medium text-accent">
                    {item.locked ? t("background.autoPinned") : t("background.autoUse")}
                  </span>
                </div>
              </button>
            ))}
          </div>
        )}
      </div>
    </Dialog>
  );
}
