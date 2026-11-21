import {
  AppWindow,
  Boxes,
  FolderOpen,
  ListTodo,
  MonitorUp,
  MoreHorizontal,
  NotebookPen,
  Package,
  Share2,
  Shuffle,
} from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { unwrap } from "@/lib/api";
import {
  commands,
  type InstanceSummary,
  type RecentServer,
  type RecentWorld,
} from "@/lib/bindings";
import { worldFacts } from "@/lib/format";
import { HOME_STAGE, HOME_STAGE_CONTENT } from "@/lib/layout";
import { reportError } from "@/lib/report";
import { isActive, useGameStore } from "@/stores/game";
import { useBackgroundItems } from "./backgroundItems";
import { PlayButton } from "./PlayButton";
import { ServerIcon, ServerStatusLine, useServerStatus } from "./ServerStatus";
import { Stagger, StaggerItem } from "./Stagger";
import { Truncated } from "./ui/HoverCard";
import { Menu, type MenuItem } from "./ui/Menu";
import { Tag } from "./ui/Tag";
import { type HomeTarget, WorldPicker } from "./WorldPicker";

const NotesDialog = lazy(async () => ({
  default: (await import("./worlds/NotesDialog")).NotesDialog,
}));
const AutoCoverDialog = lazy(async () => ({
  default: (await import("./worlds/AutoCoverDialog")).AutoCoverDialog,
}));
const ShareCardDialog = lazy(async () => ({
  default: (await import("./worlds/ShareCardDialog")).ShareCardDialog,
}));

/** 在资源管理器里打开实例文件夹（路径由后端解析，界面只传 ID） */
export function openInstanceFolder(instanceId: string): void {
  commands
    .instancesOpenFolder(instanceId)
    .then(unwrap)
    .catch((error: unknown) => reportError(error, "打开实例文件夹"));
}

export type HomeHeroTarget =
  | { kind: "world"; entry: RecentWorld }
  | { kind: "server"; entry: RecentServer };

/**
 * 首页「继续你的世界 / 多人服务器」（招牌 1）：
 * 左下角是目标标题与关键事实（世界天数或服务器在线人数/延迟），右下角一个胶囊主按钮和一个「⋯」。
 */
export function HomeHero({
  instance,
  target,
  worlds,
  servers,
  instances,
  onPick,
}: {
  instance: InstanceSummary;
  target: HomeHeroTarget;
  worlds: RecentWorld[];
  servers: RecentServer[];
  instances: InstanceSummary[];
  onPick: (next: HomeTarget) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const launch = useGameStore((s) => s.launch);
  const running = useGameStore((s) => isActive(s.sessions[instance.id]));
  const [picking, setPicking] = useState(false);
  const [notesOpen, setNotesOpen] = useState(false);
  const [notesLoaded, setNotesLoaded] = useState(false);
  const [autoCoverOpen, setAutoCoverOpen] = useState(false);
  const [autoCoverLoaded, setAutoCoverLoaded] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [shareLoaded, setShareLoaded] = useState(false);

  const world = target.kind === "world" ? target.entry.world : null;
  const server = target.kind === "server" ? target.entry : null;
  const quickPlay = target.entry.quickPlay;
  const canPick = worlds.length + servers.length > 1;
  const serverStatus = useServerStatus(server?.address);

  const openNotes = () => {
    setNotesLoaded(true);
    setNotesOpen(true);
  };
  const openAutoCovers = () => {
    setAutoCoverLoaded(true);
    setAutoCoverOpen(true);
  };
  const openShareCard = () => {
    setShareLoaded(true);
    setShareOpen(true);
  };
  const background = useBackgroundItems(instance.id, world, world ? openAutoCovers : undefined);

  const currentTarget: HomeTarget =
    target.kind === "world"
      ? { kind: "world", instanceId: instance.id }
      : { kind: "server", instanceId: instance.id, address: target.entry.address };

  const items: MenuItem[] = [
    ...(quickPlay && !running
      ? [
          {
            key: "title",
            label: t("home.world.titleScreen"),
            icon: AppWindow,
            onSelect: () => void launch(instance.id),
          },
        ]
      : []),
    {
      key: "manage",
      label: t("home.manage"),
      icon: Package,
      onSelect: () => navigate(`/instances/${encodeURIComponent(instance.id)}`),
    },
    ...(canPick
      ? [
          {
            key: "pick",
            label: t("home.world.pick"),
            icon: Shuffle,
            onSelect: () => setPicking(true),
          },
        ]
      : []),
    ...(world
      ? [
          {
            key: "notes",
            label: t("notes.menu"),
            icon: NotebookPen,
            onSelect: openNotes,
          },
          {
            key: "shareCard",
            label: t("shareCard.menu"),
            icon: Share2,
            onSelect: openShareCard,
          },
          {
            key: "shortcut",
            label: t("shortcut.menu"),
            icon: MonitorUp,
            onSelect: () => void useGameStore.getState().createShortcut(instance.id, world.folder),
          },
          ...background,
        ]
      : []),
    {
      key: "folder",
      label: t("home.world.openFolder"),
      icon: FolderOpen,
      onSelect: () => openInstanceFolder(instance.id),
    },
    {
      key: "all",
      label: t("home.world.allInstances"),
      icon: Boxes,
      onSelect: () => navigate("/instances"),
    },
  ];

  const staggerKey = world
    ? `world:${instance.id}:${world.folder}`
    : `server:${instance.id}:${server?.address ?? ""}`;

  return (
    <section className={`relative ${HOME_STAGE}`}>
      <div className={HOME_STAGE_CONTENT}>
        <Stagger key={staggerKey} className="min-w-0 flex-1 text-on-hero">
          {world && world.imageKind === "icon" && world.imageUrl && (
            <StaggerItem>
              {/* 没有截图的世界：小图标按像素清清楚楚地放在名字上面，背后是它放大后的颜色 */}
              <img
                src={world.imageUrl}
                alt=""
                className="mb-5 size-20 rounded-panel shadow-overlay ring-1 ring-hero-chip-line [image-rendering:pixelated]"
              />
            </StaggerItem>
          )}
          {server && (
            <StaggerItem>
              <ServerIcon
                status={serverStatus}
                className="mb-4 size-14 rounded-panel shadow-overlay ring-1 ring-hero-chip-line"
              />
            </StaggerItem>
          )}
          <StaggerItem>
            <Truncated
              as="p"
              className="truncate text-on-hero-muted text-sm"
              text={
                world
                  ? t("home.world.continue", { instance: instance.name })
                  : t("home.server.continue", { instance: instance.name })
              }
            />
          </StaggerItem>
          <StaggerItem className="mt-1 flex min-w-0 items-center gap-3">
            <Truncated
              as="h1"
              className="truncate font-semibold text-hero tracking-[-0.02em]"
              text={world ? world.name : server?.name || server?.address || ""}
            />
            {canPick && (
              <WorldPicker
                worlds={worlds}
                servers={servers}
                instances={instances}
                current={currentTarget}
                open={picking}
                onOpenChange={setPicking}
                onPick={onPick}
              />
            )}
          </StaggerItem>
          {world?.note && (
            <StaggerItem className="mt-2 flex min-w-0">
              {/* 世界笔记：第一条没做完的待办，点一下打开笔记 */}
              <button
                type="button"
                className="inline-flex min-w-0 items-center gap-2 rounded-control bg-hero-chip px-3 py-1.5 text-on-hero text-sm ring-1 ring-hero-chip-line ring-inset transition-colors duration-(--motion-press) hover:bg-hero-chip-hover"
                onClick={openNotes}
              >
                {world.note.kind === "todo" && <ListTodo className="size-4 shrink-0" aria-hidden />}
                <span className="truncate">
                  {world.note.kind === "todo"
                    ? t("notes.next", { text: world.note.text })
                    : world.note.text}
                </span>
              </button>
            </StaggerItem>
          )}
          {world && (
            <StaggerItem className="mt-3 flex flex-wrap gap-2">
              {worldFacts(world, t).map((fact) => (
                <Tag key={fact} tone="hero" className="px-2.5 py-1 text-sm tabular-nums">
                  {fact}
                </Tag>
              ))}
            </StaggerItem>
          )}
          {server && (
            <StaggerItem className="mt-3 flex flex-wrap items-center gap-2">
              <Tag tone="hero" className="px-2.5 py-1 text-sm">
                <ServerStatusLine status={serverStatus} address={server.address} tone="hero" />
              </Tag>
              <Tag tone="hero" className="px-2.5 py-1 text-sm">
                {server.address}
              </Tag>
            </StaggerItem>
          )}
          {!quickPlay && (
            <StaggerItem>
              <p className="mt-3 text-on-hero-muted text-sm">
                {t(world ? "home.world.noQuickPlay" : "home.server.noQuickPlay", {
                  version: instance.gameVersion,
                })}
              </p>
            </StaggerItem>
          )}
        </Stagger>
        <div className="flex shrink-0 items-center gap-3">
          {world ? (
            <PlayButton
              instanceId={instance.id}
              world={quickPlay ? world.folder : undefined}
              label={quickPlay ? t("home.world.resume") : undefined}
              sub={quickPlay ? t("home.world.resumeSub", { world: world.name }) : undefined}
              large
            />
          ) : server ? (
            <PlayButton
              instanceId={instance.id}
              server={quickPlay ? server.address : undefined}
              label={quickPlay ? t("home.server.join") : undefined}
              sub={
                quickPlay
                  ? t("home.server.joinSub", { server: server.name || server.address })
                  : undefined
              }
              large
            />
          ) : null}
          <Menu
            label={t("home.world.more")}
            items={items}
            up
            trigger="hero"
            size="lg"
            icon={MoreHorizontal}
          />
        </div>
      </div>
      {world && notesLoaded && (
        <Suspense fallback={null}>
          <NotesDialog
            instanceId={instance.id}
            world={notesOpen ? { folder: world.folder, name: world.name, seed: world.seed } : null}
            onClose={() => setNotesOpen(false)}
          />
        </Suspense>
      )}
      {world && autoCoverLoaded && (
        <Suspense fallback={null}>
          <AutoCoverDialog
            instanceId={instance.id}
            world={autoCoverOpen ? { folder: world.folder, name: world.name } : null}
            onClose={() => setAutoCoverOpen(false)}
          />
        </Suspense>
      )}
      {world && shareLoaded && (
        <Suspense fallback={null}>
          <ShareCardDialog
            target={shareOpen ? { instance, world } : null}
            onClose={() => setShareOpen(false)}
          />
        </Suspense>
      )}
    </section>
  );
}
