import {
  ArrowUpCircle,
  Copy,
  FolderOpen,
  Globe,
  Image as ImageIcon,
  Layers,
  MonitorUp,
  MoreHorizontal,
  NotebookPen,
  PackageCheck,
  PackageOpen,
  ScrollText,
  Share,
  Wrench,
} from "lucide-react";
import { motion } from "motion/react";
import { lazy, Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { CoverEmblem, resolveDefaultCover } from "@/components/art/InstanceDefaultCover";
import { LoaderBadgeIcon } from "@/components/art/LoaderBadgeIcon";
import { LumiIcon } from "@/components/art/LumiIcon";
import { openInstanceFolder } from "@/components/HomeHero";
import { InstanceCover } from "@/components/InstanceTile";
import { ContentTab } from "@/components/instance/ContentTab";
import { CopyDialog, DeleteDialog } from "@/components/instance/CopyDeleteDialogs";
import {
  type InstanceSection,
  InstanceSettingsDialog,
} from "@/components/instance/InstanceSettingsDialog";
import { LogTab } from "@/components/instance/LogTab";
import { RepairDialog } from "@/components/instance/RepairDialog";
import { ScreenshotsTab } from "@/components/instance/ScreenshotsTab";
import { WorldsTab } from "@/components/instance/WorldsTab";
import { PlayButton } from "@/components/PlayButton";
import { EmptyState } from "@/components/ui/EmptyState";
import { IconButton } from "@/components/ui/IconButton";
import { Menu } from "@/components/ui/Menu";
import { Tabs } from "@/components/ui/Tabs";
import { unwrap } from "@/lib/api";
import { type ContentKind, commands } from "@/lib/bindings";
import { imageAt, useCoverAccent, useWorldAccent } from "@/lib/cover";
import { PAGE } from "@/lib/layout";
import { duration, ease } from "@/lib/motion";
import { useGameStore } from "@/stores/game";

const ExportDialog = lazy(() => import("@/components/instance/ExportDialog"));
const InstanceNotesCard = lazy(() => import("@/components/instance/InstanceNotesCard"));
const UpgradeVersionDialog = lazy(() => import("@/components/instance/UpgradeVersionDialog"));
const UpdatePackDialog = lazy(() => import("@/components/instance/UpdatePackDialog"));

/** 实例详情页静默检测整合包新版的会话级缓存（键包含当前版本，更新/回滚后自动失效） */
const packUpdateOfferCache = new Map<string, string | null>();

type Tab = "content" | "worlds" | "screenshots" | "log" | "notes";
const TABS: Tab[] = ["content", "worlds", "screenshots", "log", "notes"];
/** 横幅「⋯」菜单打开的弹窗 */
type Action = "export" | "copy" | "delete" | "repair" | "upgrade" | "packUpdate";
const KINDS: ContentKind[] = ["mod", "shader", "resourcePack"];

const TAB_ICON = {
  content: Layers,
  worlds: Globe,
  screenshots: ImageIcon,
  log: ScrollText,
  notes: NotebookPen,
} as const;

/**
 * 实例详情：上面是横幅（最近那个世界的截图、实例设置、「⋯」更多操作、开始游戏），下面四个页签——
 * 内容（模组 / 光影包 / 资源包）、世界（单人存档 + 多人服务器）、截图、日志。
 * 齿轮打开实例设置弹窗（和全局设置同一个外壳）；「⋯」里是打开文件夹、导出整合包、复制实例、删除实例。
 * 页签和内容类型记在地址里：从资源中心装完东西按「后退」，回到的还是原来那一页。
 */
export function InstanceDetailPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { id = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const instance = useGameStore((s) => s.instances.find((i) => i.id === id));
  const world = useGameStore((s) => s.recentWorlds?.find((r) => r.instanceId === id)?.world);
  const recentLoaded = useGameStore((s) => s.recentWorlds !== null);
  const [action, setAction] = useState<Action | null>(null);
  const [settingsSection, setSettingsSection] = useState<InstanceSection | null>(null);
  const [packUpdateVersion, setPackUpdateVersion] = useState<string | null>(null);
  // 整套界面换成这个实例最近那个世界的颜色（招牌 2）
  const color = useCoverAccent(world?.imageUrl);
  useWorldAccent(recentLoaded ? color : undefined);

  const packCacheKey =
    instance && !instance.linked && instance.packSource && instance.packProjectId
      ? `${instance.id}:${instance.packVersionId ?? instance.packVersion ?? ""}`
      : null;

  useEffect(() => {
    if (!packCacheKey || !instance) {
      setPackUpdateVersion(null);
      return;
    }
    if (packUpdateOfferCache.has(packCacheKey)) {
      setPackUpdateVersion(packUpdateOfferCache.get(packCacheKey) ?? null);
      return;
    }
    let cancelled = false;
    setPackUpdateVersion(null);
    commands
      .modpackUpdateVersions(instance.id)
      .then(unwrap)
      .then((list) => {
        if (cancelled) return;
        const candidate =
          list.find((v) => v.release && v.gameVersions.includes(instance.gameVersion)) ??
          list.find((v) => v.recommended) ??
          list[0];
        const isNewer =
          Boolean(candidate) &&
          candidate?.id !== instance.packVersionId &&
          candidate?.versionNumber !== instance.packVersion;
        const nextVersion = isNewer && candidate ? candidate.versionNumber : null;
        packUpdateOfferCache.set(packCacheKey, nextVersion);
        setPackUpdateVersion(nextVersion);
      })
      .catch(() => {
        if (!cancelled) setPackUpdateVersion(null);
      });
    return () => {
      cancelled = true;
    };
  }, [packCacheKey, instance]);

  const tabParam = params.get("tab") as Tab | null;
  const tab: Tab = tabParam && TABS.includes(tabParam) ? tabParam : "content";
  const kindParam = params.get("kind") as ContentKind | null;
  const kind: ContentKind = kindParam && KINDS.includes(kindParam) ? kindParam : "mod";

  const setParam = (key: string, value: string, fallback: string) => {
    const next = new URLSearchParams(params);
    if (value === fallback) next.delete(key);
    else next.set(key, value);
    setParams(next, { replace: true });
  };

  if (!instance) {
    return (
      <div className={PAGE}>
        <EmptyState icon={PackageOpen} title={t("detail.missing")} hint={t("detail.missingHint")} />
      </div>
    );
  }

  const iconOnly = !instance.coverUrl && world?.imageKind === "icon";
  // 完整显示的自选图（立绘）：横幅太扁放不下，横幅只铺它的颜色，立绘的上半身当名字左边的头像
  const figure =
    !instance.coverUrl && world?.imageKind === "custom" && world.imageFit === "contain";
  const onHero = Boolean(instance.coverUrl || (world?.imageUrl && !iconOnly && !figure));
  const { emblem, palette } = resolveDefaultCover(instance);

  return (
    <div className={`${PAGE} flex h-full flex-col gap-5`}>
      {/*
        横幅：最近那个世界的截图铺满（从实例库的卡片放大过来），遮罩从左往右淡，字压在左边、图露在右边。
        只有小图标的世界铺它的颜色，图标放到名字左边当头像；没有存档时头像是名字的第一个字。
        右边三个按钮一样高（48 像素）：实例设置、「⋯」更多操作 + 中号的开始游戏。
      */}
      <header className="relative flex h-36 shrink-0 items-center overflow-hidden rounded-panel">
        <InstanceCover
          instance={instance}
          world={world}
          width={1100}
          bare
          className="absolute inset-0"
        />
        {onHero && <div className="absolute inset-0 bg-hero-tint" />}
        <div className="absolute inset-0 bg-linear-to-r from-hero-scrim from-10% via-hero-scrim/75 via-40% to-hero-scrim/0" />
        <div className="relative flex w-full items-center gap-4 px-6 text-on-hero">
          {(iconOnly || figure || !onHero) && (
            <div
              className="grid size-16 shrink-0 place-items-center overflow-hidden rounded-row bg-surface shadow-raised ring-1 ring-hero-chip-line"
              style={
                !iconOnly && !figure
                  ? {
                      background: `linear-gradient(135deg, ${palette.deep} 0%, ${palette.mid} 100%)`,
                    }
                  : undefined
              }
            >
              {iconOnly && world?.imageUrl ? (
                <img
                  src={world.imageUrl}
                  alt=""
                  className="size-full object-cover [image-rendering:pixelated]"
                />
              ) : figure && world?.imageUrl ? (
                <img
                  src={imageAt(world.imageUrl, 128)}
                  alt=""
                  className="size-full object-cover object-top"
                />
              ) : (
                <CoverEmblem
                  emblem={emblem}
                  accent={palette.accent}
                  glow={palette.glow}
                  className="size-10"
                />
              )}
            </div>
          )}
          <div className="min-w-0 flex-1 select-text">
            <h1 className="truncate font-semibold text-3xl leading-tight">{instance.name}</h1>
            <div className="mt-0.5 flex min-w-0 flex-wrap items-center gap-1.5 text-on-hero-muted text-sm tabular-nums">
              <LoaderBadgeIcon
                loader={instance.loader}
                title={instance.loader ?? t("create.vanilla")}
                className="size-4 shrink-0"
              />
              <span className="truncate">
                {[
                  instance.gameVersion,
                  instance.loader,
                  instance.linked &&
                    t("detail.linkedFrom", { launcher: t(`launchers.${instance.linked}`) }),
                  world?.name,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
              {packUpdateVersion && (
                <button
                  type="button"
                  onClick={() => setAction("packUpdate")}
                  className="inline-flex shrink-0 items-center gap-1 rounded-tag bg-hero-chip px-2 py-0.5 font-medium text-on-hero text-xs ring-1 ring-hero-chip-line ring-inset backdrop-blur-md transition-colors duration-(--motion-press) hover:bg-hero-chip-hover"
                >
                  <ArrowUpCircle className="size-3.5 text-accent" aria-hidden />
                  <span>{t("packUpdate.updatePill", { version: packUpdateVersion })}</span>
                </button>
              )}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <IconButton
              label={t("detail.settings")}
              variant="hero"
              size="cta"
              pressed={settingsSection !== null}
              onClick={() => setSettingsSection("general")}
            >
              <LumiIcon name="settings" className="size-[18px]" />
            </IconButton>
            <Menu
              label={t("detail.more")}
              trigger="hero"
              size="cta"
              icon={MoreHorizontal}
              items={[
                ...(!instance.linked && instance.pack
                  ? [
                      {
                        key: "packUpdate",
                        label: t("detail.menu.packUpdate"),
                        icon: PackageCheck,
                        onSelect: () => setAction("packUpdate"),
                      },
                    ]
                  : []),
                {
                  key: "upgrade",
                  label: t("detail.menu.upgrade"),
                  icon: ArrowUpCircle,
                  onSelect: () => setAction("upgrade"),
                },
                ...(!instance.linked && !instance.pack
                  ? [
                      {
                        key: "packUpdate",
                        label: t("detail.menu.packUpdate"),
                        icon: PackageCheck,
                        onSelect: () => setAction("packUpdate"),
                      },
                    ]
                  : []),
                {
                  key: "folder",
                  label: t("detail.menu.openFolder"),
                  icon: FolderOpen,
                  onSelect: () => openInstanceFolder(instance.id),
                },
                {
                  key: "export",
                  label: t("detail.menu.export"),
                  icon: Share,
                  onSelect: () => setAction("export"),
                },
                {
                  key: "copy",
                  label: t(instance.linked ? "detail.menu.copyLinked" : "detail.menu.copy"),
                  icon: Copy,
                  onSelect: () => setAction("copy"),
                },
                {
                  key: "shortcut",
                  label: t("shortcut.menu"),
                  icon: MonitorUp,
                  onSelect: () => void useGameStore.getState().createShortcut(instance.id),
                },
                ...(instance.linked
                  ? []
                  : [
                      {
                        key: "repair",
                        label: t("diagnostics.files.action"),
                        icon: Wrench,
                        onSelect: () => setAction("repair"),
                      },
                    ]),
              ]}
            />
            <PlayButton instanceId={instance.id} large size="md" />
          </div>
        </div>
      </header>

      <Tabs
        label={t("detail.tabs")}
        value={tab}
        onChange={(value) => setParam("tab", value, "content")}
        options={TABS.map((value) => ({
          value,
          label: t(`detail.tab.${value}`),
          icon: TAB_ICON[value],
        }))}
      />

      <motion.div
        key={tab}
        role="tabpanel"
        className="min-h-0 flex-1"
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0, transition: { duration: duration.enter, ease: ease.out } }}
      >
        {tab === "content" && (
          <ContentTab
            instance={instance}
            kind={kind}
            onKind={(value) => setParam("kind", value, "mod")}
          />
        )}
        {tab === "worlds" && <WorldsTab instance={instance} />}
        {tab === "screenshots" && <ScreenshotsTab instance={instance} />}
        {tab === "log" && (
          <LogTab
            instance={instance}
            recovery={{
              onJava: () => setSettingsSection("java"),
              onMemory: () => setSettingsSection("game"),
              onMods: () => {
                const next = new URLSearchParams(params);
                for (const key of ["tab", "kind", "record"]) next.delete(key);
                setParams(next, { replace: true });
              },
              onRepair: instance.linked ? undefined : () => setAction("repair"),
            }}
          />
        )}
        {tab === "notes" && (
          <Suspense fallback={null}>
            <InstanceNotesCard instance={instance} />
          </Suspense>
        )}
      </motion.div>

      <InstanceSettingsDialog
        instance={instance}
        section={settingsSection}
        onSection={setSettingsSection}
        onUpgrade={() => setAction("upgrade")}
        onDelete={() => setAction("delete")}
        onClose={() => setSettingsSection(null)}
      />
      <RepairDialog
        instanceId={instance.id}
        open={action === "repair"}
        onClose={() => setAction(null)}
      />
      {action === "export" && (
        <Suspense fallback={null}>
          <ExportDialog instance={instance} open onClose={() => setAction(null)} />
        </Suspense>
      )}
      <CopyDialog instance={instance} open={action === "copy"} onClose={() => setAction(null)} />
      {action === "upgrade" && (
        <Suspense fallback={null}>
          <UpgradeVersionDialog instance={instance} open onClose={() => setAction(null)} />
        </Suspense>
      )}
      {action === "packUpdate" && (
        <Suspense fallback={null}>
          <UpdatePackDialog instance={instance} open onClose={() => setAction(null)} />
        </Suspense>
      )}
      <DeleteDialog
        instance={instance}
        open={action === "delete"}
        onClose={() => setAction(null)}
        onDeleted={() => navigate("/instances", { replace: true })}
      />
    </div>
  );
}
