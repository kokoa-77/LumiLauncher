import { Pin } from "lucide-react";
import { motion } from "motion/react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import type { InstanceSummary, WorldView } from "@/lib/bindings";
import { useCoverAccent } from "@/lib/cover";
import { formatAgo } from "@/lib/format";
import { accentStyle } from "@/lib/themeEngine";
import { instanceSubtitle } from "./InstanceCard";
import { instanceMenuItems } from "./instanceMenu";
import { PlayButton } from "./PlayButton";
import { Truncated } from "./ui/HoverCard";
import { Menu } from "./ui/Menu";
import { Tag } from "./ui/Tag";
import { WorldPicture } from "./WorldPicture";

/**
 * 实例的封面：最近那个世界的背景图（截图、自动封面、自己选的图）；没有存档时是名字的第一个字。
 * 世界只有 64×64 小图标时：背景是它缩成 8×8 再放大的一片顺滑颜色，正中间再放一个按像素显示的清楚图标，
 * 不把小图标直接放大成马赛克。
 */
export function InstanceCover({
  instance,
  world,
  width,
  className = "",
  bare = false,
  shared = true,
}: {
  instance: InstanceSummary;
  world: WorldView | undefined;
  /** 进实例页时封面平滑变成横幅；同一个实例在页面上出现两次时（置顶一份、分组里一份）只给一份 */
  shared?: boolean;
  /** 只画背景：只有小图标的世界、完整显示的自选图（立绘）只铺它的颜色，没有图时什么字都不写（实例页横幅把图标放到名字左边） */
  bare?: boolean;
  /** 要多宽的图（CSS 像素），后端按这个缩小 */
  width: number;
  /** 大小和定位（relative / absolute）由用的地方给 */
  className?: string;
}) {
  const letter = Array.from(instance.name.trim())[0]?.toUpperCase() ?? "?";
  const iconOnly = world?.imageKind === "icon";
  return (
    <motion.div
      layoutId={shared ? `cover-${instance.id}` : undefined}
      className={`overflow-hidden bg-accent-soft ${className}`}
    >
      {world?.imageUrl ? (
        <>
          <WorldPicture
            url={world.imageUrl}
            kind={world.imageKind}
            fit={world.imageFit}
            width={width}
            figure={!bare}
            idleSampling={!bare}
          />
          {iconOnly && !bare && (
            <img
              src={world.imageUrl}
              alt=""
              loading="lazy"
              className="-translate-x-1/2 -translate-y-1/2 absolute top-1/2 left-1/2 aspect-square h-[56%] max-h-24 rounded-row shadow-overlay ring-1 ring-hero-chip-line [image-rendering:pixelated]"
            />
          )}
        </>
      ) : bare ? null : (
        <span className="absolute inset-0 grid place-items-center font-semibold text-3xl text-accent">
          {letter}
        </span>
      )}
    </motion.div>
  );
}

/** 实例库里的一张卡片（网格）或一行（列表）。每张卡片用自己世界的颜色（随实例变色）。 */
export function InstanceTile({
  instance,
  world,
  view,
  shared = true,
}: {
  instance: InstanceSummary;
  world: WorldView | undefined;
  view: "grid" | "list";
  /** 见 `InstanceCover` 的 `shared` */
  shared?: boolean;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const color = useCoverAccent(world?.imageUrl, true);
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
      style={color ? accentStyle(color.hue, color.chroma) : undefined}
      className={`accent-scope group cursor-default overflow-hidden rounded-panel border border-line bg-surface shadow-raised transition-[background-color,border-color] duration-(--motion-press) hover:border-accent/40 hover:bg-surface-hover ${
        view === "grid" ? "flex flex-col p-1.5" : "flex items-center gap-4 p-2"
      }`}
    >
      <InstanceCover
        instance={instance}
        world={world}
        shared={shared}
        width={view === "grid" ? 360 : 112}
        className={
          view === "grid"
            ? "relative aspect-video rounded-item"
            : "relative h-16 w-28 shrink-0 rounded-item"
        }
      />
      <div
        className={`flex min-w-0 flex-1 items-center gap-3 ${view === "grid" ? "px-2 pt-3 pb-1.5" : ""}`}
      >
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-1.5">
            {instance.pinnedAt !== null && (
              <Pin
                className="size-3.5 shrink-0 text-fg-muted"
                aria-label={t("instances.pinnedBadge")}
              />
            )}
            <Truncated as="p" className="truncate font-medium" text={instance.name} />
            {/* 挂上的别家版本：是哪家的 */}
            {instance.linked && <Tag>{t(`launchers.${instance.linked}`)}</Tag>}
          </div>
          <p className="truncate text-fg-muted text-xs">{instanceSubtitle(instance)}</p>
          <p className="truncate text-fg-muted text-xs">
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
            items={instanceMenuItems(instance, t)}
          />
        </div>
        <PlayButton instanceId={instance.id} compact />
      </div>
    </div>
  );
}
