import { FolderOpen, Pin, Trash2, Unlink } from "lucide-react";
import { motion } from "motion/react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import type { InstanceSummary, WorldView } from "@/lib/bindings";
import { imageAt, useCoverAccent } from "@/lib/cover";
import { formatAgo } from "@/lib/format";
import { accentStyle } from "@/lib/themeEngine";
import { InstanceDefaultCover, resolveDefaultCover } from "./art/InstanceDefaultCover";
import { LoaderBadgeIcon } from "./art/LoaderBadgeIcon";
import { openInstanceFolder } from "./HomeHero";
import { instanceSubtitle } from "./InstanceCard";
import { instanceMenuItems } from "./instanceMenu";
import { PlayButton } from "./PlayButton";
import { Truncated } from "./ui/HoverCard";
import { Menu } from "./ui/Menu";
import { Tag } from "./ui/Tag";
import { WorldPicture } from "./WorldPicture";

/** 本地 `lumi-img://` 支持按宽度缩图，远程整合包官方图直接加载 */
export function resolveCoverSrc(url: string, width: number): string {
  return url.startsWith("https://") || url.startsWith("http://") ? url : imageAt(url, width);
}

/**
 * 实例的封面（4 级全幅高清链，告别「纯色单字」和「64×64 糊底邮票」）：
 * 1. 实例级自定义封面或整合包官方封面/图标（`instance.coverUrl`）→ 全幅铺满；
 * 2. 最近世界的高清截图、自动封面或世界自定义背景 → 全幅铺满；
 * 3. 无高清图时（含新实例与仅有 64×64 小图标的旧世界）→ 全幅微光矢量景深封面，
 *    若有世界 64×64 小图标且未手动指定预设，自动继承世界主色相（保留「随世界变色」）。
 */
export function InstanceCover({
  instance,
  world,
  width,
  className = "",
  bare = false,
  compact = false,
  shared = true,
}: {
  instance: InstanceSummary;
  world: WorldView | undefined;
  /** 进实例页时封面平滑变成横幅；同一个实例在页面上出现两次时（置顶一份、分组里一份）只给一份 */
  shared?: boolean;
  /** 只画背景：实例页横幅左侧有独立头像位，这里只铺全幅景深 */
  bare?: boolean;
  /** 列表小缩略图模式 */
  compact?: boolean;
  /** 要多宽的图（CSS 像素），后端按这个缩小 */
  width: number;
  /** 大小和定位（relative / absolute）由用的地方给 */
  className?: string;
}) {
  const hasHighResWorld = Boolean(world?.imageUrl && world.imageKind !== "icon");
  const iconAccent = useCoverAccent(
    !instance.coverUrl && !hasHighResWorld && world?.imageKind === "icon" ? world.imageUrl : null,
    !bare,
  );

  return (
    <motion.div
      layoutId={shared ? `cover-${instance.id}` : undefined}
      className={`overflow-hidden bg-accent-soft ${className}`}
    >
      {instance.coverUrl ? (
        <img
          src={resolveCoverSrc(instance.coverUrl, width)}
          alt=""
          aria-hidden
          loading="lazy"
          decoding="async"
          className="absolute inset-0 size-full object-cover"
        />
      ) : hasHighResWorld && world?.imageUrl ? (
        <WorldPicture
          url={world.imageUrl}
          kind={world.imageKind}
          fit={world.imageFit}
          width={width}
          figure={!bare}
          idleSampling={!bare}
        />
      ) : (
        <InstanceDefaultCover
          instance={instance}
          bare={bare}
          compact={compact}
          worldHue={iconAccent?.hue}
        />
      )}
    </motion.div>
  );
}

/** 实例库里的一张卡片（网格）或一行（列表）。每张卡片随封面/默认色板柔和变色。 */
export function InstanceTile({
  instance,
  world,
  view,
  shared = true,
  onDelete,
}: {
  instance: InstanceSummary;
  world: WorldView | undefined;
  view: "grid" | "list";
  /** 见 `InstanceCover` 的 `shared` */
  shared?: boolean;
  /** 从实例库卡片直接删除实例或移除外部关联实例 */
  onDelete?: (instance: InstanceSummary) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const isLocalCover = Boolean(instance.coverUrl && !instance.coverUrl.startsWith("http"));
  const sampled = useCoverAccent(isLocalCover ? instance.coverUrl : world?.imageUrl, true);
  const { palette, isCustomPreset } = resolveDefaultCover(instance);
  // 自定义预设优先用预设色板，其次是封面/世界取色，最后落回实例默认微光色板
  const accent = isCustomPreset
    ? { hue: palette.hue, chroma: palette.chroma }
    : (sampled ?? { hue: palette.hue, chroma: palette.chroma });
  const open = () => navigate(`/instances/${encodeURIComponent(instance.id)}`);
  const played = instance.lastPlayed
    ? t("instances.lastPlayed", { ago: formatAgo(instance.lastPlayed, t) })
    : t("instances.neverPlayed");

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(event) => {
        if (event.key === "Enter") open();
      }}
      style={accentStyle(accent.hue, accent.chroma)}
      className={`accent-scope group cursor-default overflow-hidden rounded-panel border border-line bg-surface shadow-raised transition-[background-color,border-color] duration-(--motion-press) hover:border-accent/40 hover:bg-surface-hover ${
        view === "grid" ? "flex flex-col p-1.5" : "flex items-center gap-4 p-2"
      }`}
    >
      <InstanceCover
        instance={instance}
        world={world}
        shared={shared}
        compact={view === "list"}
        width={view === "grid" ? 360 : 112}
        className={
          view === "grid"
            ? "relative aspect-video rounded-item"
            : "relative h-16 w-28 shrink-0 rounded-item"
        }
      />
      <div
        className={`flex min-w-0 flex-1 items-center ${view === "grid" ? "gap-2.5 px-1.5 pt-2.5 pb-1.5" : "gap-3"}`}
      >
        {view === "grid" && (
          <div
            title={instance.loader ?? t("create.vanilla")}
            className="grid size-9 shrink-0 place-items-center rounded-row border border-line/70 bg-bg/75"
          >
            <LoaderBadgeIcon
              loader={instance.loader}
              title={instance.loader ?? t("create.vanilla")}
              className="size-6"
            />
          </div>
        )}
        <div className="min-w-0 flex-1 space-y-0.5">
          <div className="flex min-w-0 items-center gap-1.5">
            {instance.pinnedAt !== null && (
              <Pin
                className="size-3.5 shrink-0 text-fg-muted"
                aria-label={t("instances.pinnedBadge")}
              />
            )}
            <Truncated as="p" className="truncate font-medium leading-tight" text={instance.name} />
            {/* 挂上的别家版本：是哪家的 */}
            {instance.linked && <Tag>{t(`launchers.${instance.linked}`)}</Tag>}
          </div>
          {view === "list" ? (
            <p className="flex min-w-0 items-center gap-1.5 text-fg-muted text-xs leading-tight tabular-nums">
              <span className="shrink-0">{instance.gameVersion}</span>
              {instance.loader && (
                <>
                  <span aria-hidden>·</span>
                  <LoaderBadgeIcon
                    loader={instance.loader}
                    title={instance.loader}
                    className="size-3.5 shrink-0"
                  />
                  <span className="truncate">{instance.loader}</span>
                </>
              )}
            </p>
          ) : (
            <p className="truncate text-fg-muted text-xs leading-tight tabular-nums">
              {instanceSubtitle(instance)}
            </p>
          )}
          <p className="truncate text-fg-muted text-xs leading-tight">
            {world ? `${world.name} · ${played}` : played}
          </p>
        </div>
        {/* 菜单弹层挂在页面最外层，但点击、回车还是会冒泡到卡片（打开实例页）：在这里拦住 */}
        <div
          className="shrink-0"
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => event.stopPropagation()}
        >
          <Menu
            label={t("instances.more", { name: instance.name })}
            items={[
              ...instanceMenuItems(instance, t),
              {
                key: "folder",
                label: t("detail.menu.openFolder"),
                icon: FolderOpen,
                onSelect: () => openInstanceFolder(instance.id),
              },
              ...(onDelete
                ? [
                    instance.linked
                      ? {
                          key: "delete",
                          label: t("detail.menu.unlink"),
                          icon: Unlink,
                          separated: true,
                          onSelect: () => onDelete(instance),
                        }
                      : {
                          key: "delete",
                          label: t("detail.menu.delete"),
                          icon: Trash2,
                          danger: true,
                          onSelect: () => onDelete(instance),
                        },
                  ]
                : []),
            ]}
          />
        </div>
        <PlayButton instanceId={instance.id} compact />
      </div>
    </div>
  );
}
