import { FolderOpen, Image as ImageIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import emptyWorlds from "@/assets/art/empty-worlds.webp";
import { Lightbox } from "@/components/Lightbox";
import { Button } from "@/components/ui/Button";
import { EmptyState } from "@/components/ui/EmptyState";
import { SectionTitle } from "@/components/ui/SectionTitle";
import { VirtualGrid } from "@/components/VirtualGrid";
import { unwrap } from "@/lib/api";
import { commands, type InstanceSummary, type ScreenshotView } from "@/lib/bindings";
import { imageAt } from "@/lib/cover";
import { formatAgo } from "@/lib/format";
import { getUiLanguage } from "@/lib/i18n";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";

/**
 * 「截图」页签：这个实例的游戏截图（新的在前），网格虚拟滚动；点一张铺满窗口看大图，左右键切换。
 * 游戏退出后跟着重读一次（玩的时候可能又截了几张）。
 */
export function ScreenshotsTab({ instance }: { instance: InstanceSummary }) {
  const { t } = useTranslation();
  const recentWorlds = useGameStore((s) => s.recentWorlds);
  const [shots, setShots] = useState<ScreenshotView[] | null>(null);
  const [viewing, setViewing] = useState<number | null>(null);

  useEffect(() => {
    void recentWorlds;
    let cancelled = false;
    commands
      .instancesScreenshots(instance.id)
      .then(unwrap)
      .then((list) => !cancelled && setShots(list))
      .catch((error: unknown) => {
        if (cancelled) return;
        setShots([]);
        reportError(error, "读取截图");
      });
    return () => {
      cancelled = true;
    };
  }, [instance.id, recentWorlds]);

  const big = useMemo(
    () =>
      (shots ?? []).map((shot) => ({
        url: imageAt(shot.url, 1600),
        title: shot.takenAt
          ? `${shot.name} · ${new Date(shot.takenAt).toLocaleString(getUiLanguage())}`
          : shot.name,
      })),
    [shots],
  );

  const openFolder = () => {
    commands
      .instancesOpenScreenshots(instance.id)
      .then(unwrap)
      .catch((error: unknown) => reportError(error, "打开截图文件夹"));
  };

  if (shots === null) return null;

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex h-9 shrink-0 items-center gap-3">
        <SectionTitle title={t("detail.screenshots.title")} count={shots.length} />
        <Button variant="secondary" className="ml-auto" onClick={openFolder}>
          <FolderOpen className="size-4" aria-hidden />
          {t("detail.screenshots.openFolder")}
        </Button>
      </div>
      {shots.length === 0 ? (
        <EmptyState
          icon={ImageIcon}
          illustration={emptyWorlds}
          title={t("detail.screenshots.empty")}
          hint={t("detail.screenshots.emptyHint")}
        />
      ) : (
        <VirtualGrid
          className="flex-1"
          items={shots}
          minWidth={200}
          estimate={(width) => (width * 9) / 16}
          getKey={(shot) => shot.name}
          render={(shot, index) => (
            <button
              type="button"
              onClick={() => setViewing(index)}
              aria-label={t("detail.screenshots.view", { name: shot.name })}
              title={shot.takenAt ? formatAgo(shot.takenAt, t) : shot.name}
              className="group relative block aspect-video w-full overflow-hidden rounded-item border border-line bg-surface-2"
            >
              <img
                src={imageAt(shot.url, 320)}
                alt=""
                loading="lazy"
                decoding="async"
                className="hover-zoom size-full object-cover transition-[scale] duration-(--motion-page) ease-out group-hover:scale-[1.04]"
              />
            </button>
          )}
        />
      )}
      <Lightbox
        images={big}
        index={viewing}
        onIndex={setViewing}
        onClose={() => setViewing(null)}
      />
    </div>
  );
}
