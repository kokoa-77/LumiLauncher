import {
  Archive,
  Compass,
  FolderOpen,
  Globe,
  ListTodo,
  Map as MapIcon,
  MonitorUp,
  NotebookPen,
  Play,
  RefreshCw,
  Share2,
} from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import emptyWorlds from "@/assets/art/empty-worlds.webp";
import { useBackgroundItems } from "@/components/backgroundItems";
import { ServerIcon, ServerStatusLine, useServerStatus } from "@/components/ServerStatus";
import { Button } from "@/components/ui/Button";
import { EmptyState } from "@/components/ui/EmptyState";
import { HoverHint } from "@/components/ui/HoverCard";
import { IconButton } from "@/components/ui/IconButton";
import { Menu } from "@/components/ui/Menu";
import { SectionTitle } from "@/components/ui/SectionTitle";
import { Tag } from "@/components/ui/Tag";
import { VirtualGrid } from "@/components/VirtualGrid";
import { WorldPicture } from "@/components/WorldPicture";
import { AutoCoverDialog, type AutoCoverTarget } from "@/components/worlds/AutoCoverDialog";
import { BackupsDialog, type BackupTarget } from "@/components/worlds/BackupsDialog";
import { NotesDialog, type NotesTarget } from "@/components/worlds/NotesDialog";
import type { ShareCardTarget } from "@/components/worlds/ShareCardDialog";
import { unwrap } from "@/lib/api";
import {
  commands,
  type InstanceSummary,
  type ServerEntryView,
  type WorldList,
  type WorldView,
} from "@/lib/bindings";
import { worldFacts } from "@/lib/format";
import { logError, reportError } from "@/lib/report";
import { isActive, useGameStore } from "@/stores/game";

const ShareCardDialog = lazy(async () => ({
  default: (await import("@/components/worlds/ShareCardDialog")).ShareCardDialog,
}));

/**
 * 世界的封面：背景图（截图、自动封面、自己选的图，见 `WorldPicture`）；只有小图标时，
 * 图标放中间、后面是它的颜色；什么都没有时是名字的第一个字
 */
function WorldThumb({ world }: { world: WorldView }) {
  const { t } = useTranslation();
  const iconOnly = world.imageKind === "icon";
  const letter = Array.from(world.name.trim())[0]?.toUpperCase() ?? "?";
  return (
    <div className="relative aspect-video overflow-hidden bg-accent-soft">
      {world.imageUrl ? (
        <>
          <WorldPicture
            url={world.imageUrl}
            kind={world.imageKind}
            fit={world.imageFit}
            width={360}
            className="hover-zoom transition-[scale] duration-(--motion-page) ease-out group-hover:scale-[1.04]"
          />
          {iconOnly && (
            <img
              src={world.imageUrl}
              alt=""
              loading="lazy"
              className="-translate-x-1/2 -translate-y-1/2 absolute top-1/2 left-1/2 aspect-square h-1/2 rounded-row shadow-overlay ring-1 ring-hero-chip-line [image-rendering:pixelated]"
            />
          )}
        </>
      ) : (
        <span className="absolute inset-0 grid place-items-center font-semibold text-2xl text-accent">
          {letter}
        </span>
      )}
      {/* 世界笔记：压在图上，不让卡片有高有矮 */}
      {world.note && (
        <Tag tone="hero" className="absolute bottom-2 left-2 max-w-[calc(100%-1rem)] gap-1.5">
          {world.note.kind === "todo" && <ListTodo className="size-3.5 shrink-0" aria-hidden />}
          <span className="truncate">
            {world.note.kind === "todo"
              ? t("notes.next", { text: world.note.text })
              : world.note.text}
          </span>
        </Tag>
      )}
    </div>
  );
}

function WorldCard({
  world,
  instanceId,
  instanceVersion,
  busy,
  onEnter,
  onOpenFolder,
  onBackups,
  onNotes,
  onAutoCovers,
  onShareCard,
  onShortcut,
}: {
  world: WorldView;
  instanceId: string;
  /** 实例的游戏版本：存档是别的版本存的（地图常见），进去时游戏会先问要不要备份 */
  instanceVersion: string;
  busy: boolean;
  onEnter: () => void;
  onOpenFolder: () => void;
  onBackups: () => void;
  onNotes: () => void;
  onAutoCovers: () => void;
  onShareCard: () => void;
  onShortcut: () => void;
}) {
  const { t } = useTranslation();
  const otherVersion = world.version && world.version !== instanceVersion ? world.version : null;
  const background = useBackgroundItems(instanceId, world, onAutoCovers);
  const facts = [
    otherVersion ? t("detail.worlds.savedIn", { version: otherVersion }) : null,
    ...worldFacts(world, t),
  ].filter(Boolean);
  const factsLine = facts.join(" · ");
  const factsHint = otherVersion
    ? t("detail.worlds.otherVersionHint", {
        version: otherVersion,
        current: instanceVersion,
      })
    : factsLine;
  return (
    <article className="group flex flex-col overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
      <WorldThumb world={world} />
      <div className="flex items-center gap-2 p-3">
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-sm">{world.name}</p>
          <HoverHint hint={factsHint} className="block truncate text-fg-muted text-xs">
            {factsLine}
          </HoverHint>
        </div>
        <Menu
          label={t("detail.worlds.more", { name: world.name })}
          items={[
            { key: "notes", label: t("notes.menu"), icon: NotebookPen, onSelect: onNotes },
            { key: "shareCard", label: t("shareCard.menu"), icon: Share2, onSelect: onShareCard },
            { key: "backups", label: t("backup.menu"), icon: Archive, onSelect: onBackups },
            { key: "shortcut", label: t("shortcut.menu"), icon: MonitorUp, onSelect: onShortcut },
            {
              key: "folder",
              label: t("detail.worlds.openFolder"),
              icon: FolderOpen,
              onSelect: onOpenFolder,
            },
            ...background,
          ]}
        />
        <HoverHint hint={busy ? t("detail.worlds.running") : null}>
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            aria-label={t("detail.worlds.enterWorld", { name: world.name })}
            onClick={onEnter}
          >
            <Play className="size-3 fill-current" aria-hidden />
            {t("detail.worlds.enter")}
          </Button>
        </HoverHint>
      </div>
    </article>
  );
}

function ServerRow({
  server,
  busy,
  refresh,
  onEnter,
}: {
  server: ServerEntryView;
  busy: boolean;
  /** 点了「重新测一下」的次数 */
  refresh: number;
  onEnter: () => void;
}) {
  const { t } = useTranslation();
  const status = useServerStatus(server.address, refresh);
  return (
    <div className="flex items-center gap-3 rounded-item px-2 py-2 transition-colors duration-(--motion-press) hover:bg-surface-hover">
      <ServerIcon status={status} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm">{server.name || server.address}</p>
        <ServerStatusLine status={status} address={server.address} className="max-w-full" />
      </div>
      <Button
        variant="outline"
        size="sm"
        disabled={busy}
        aria-label={t("detail.servers.enterServer", { name: server.name || server.address })}
        onClick={onEnter}
      >
        {t("detail.servers.enter")}
      </Button>
    </div>
  );
}

/**
 * 「世界」页签：左边是单人世界（截图卡片，点「进入」直接进这个存档），
 * 右边是多人游戏列表（游戏里的服务器列表，点「进入」直接连这个服务器）。
 * 游戏在运行时「进入」都按不了。1.20 以前的版本不支持直接进，会先停在标题画面，上面写明。
 */
export function WorldsTab({ instance }: { instance: InstanceSummary }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const launch = useGameStore((s) => s.launch);
  const running = useGameStore((s) => isActive(s.sessions[instance.id]));
  // 游戏退出后天数、时长、截图都会变：跟着最近世界一起重读
  const recentWorlds = useGameStore((s) => s.recentWorlds);
  const [worlds, setWorlds] = useState<WorldList | null>(null);
  const [servers, setServers] = useState<ServerEntryView[] | null>(null);
  const [backupsOf, setBackupsOf] = useState<BackupTarget | null>(null);
  const [notesOf, setNotesOf] = useState<NotesTarget | null>(null);
  const [autoCoverOf, setAutoCoverOf] = useState<AutoCoverTarget | null>(null);
  const [shareOf, setShareOf] = useState<ShareCardTarget | null>(null);
  const [refresh, setRefresh] = useState(0);
  const createShortcut = useGameStore((s) => s.createShortcut);

  useEffect(() => {
    void recentWorlds;
    let cancelled = false;
    commands
      .worldsList(instance.id)
      .then(unwrap)
      .then((list) => !cancelled && setWorlds(list))
      .catch((error: unknown) => {
        if (cancelled) return;
        setWorlds({ quickPlay: true, worlds: [] });
        reportError(error, "读取实例的存档");
      });
    commands
      .serversList(instance.id)
      .then(unwrap)
      .then((list) => !cancelled && setServers(list))
      .catch((error: unknown) => {
        if (cancelled) return;
        setServers([]);
        logError(error, "读取实例的服务器列表");
      });
    return () => {
      cancelled = true;
    };
  }, [instance.id, recentWorlds]);

  const openFolder = (folder: string) => {
    commands
      .worldsOpenFolder(instance.id, folder)
      .then(unwrap)
      .catch((error: unknown) => reportError(error, "打开存档文件夹"));
  };

  const find = (kind: "map" | "server") =>
    navigate(`/discover?from=instance&instance=${encodeURIComponent(instance.id)}&kind=${kind}`);

  if (worlds === null) return null;
  const list = worlds.worlds;

  return (
    <div className="grid h-full min-h-0 grid-cols-[minmax(0,1fr)_288px] gap-6">
      <section className="flex min-h-0 flex-col gap-3">
        <div className="flex h-9 shrink-0 items-center gap-3">
          <SectionTitle title={t("detail.worlds.title")} count={list.length} />
          <Button variant="secondary" className="ml-auto" onClick={() => find("map")}>
            <MapIcon className="size-4" aria-hidden />
            {t("detail.worlds.findMaps")}
          </Button>
        </div>
        {!worlds.quickPlay && (
          <p className="shrink-0 text-fg-muted text-xs">
            {t("home.world.noQuickPlay", { version: instance.gameVersion })}
          </p>
        )}
        {list.length === 0 ? (
          <EmptyState
            icon={Globe}
            illustration={emptyWorlds}
            title={t("detail.worlds.empty")}
            hint={t("detail.worlds.emptyHint")}
            action={
              <Button variant="outline" onClick={() => find("map")}>
                <MapIcon className="size-4" aria-hidden />
                {t("detail.worlds.findMaps")}
              </Button>
            }
          />
        ) : (
          <VirtualGrid
            className="flex-1"
            items={list}
            minWidth={220}
            estimate={(width) => (width * 9) / 16 + 60}
            getKey={(world) => world.folder}
            render={(world) => (
              <WorldCard
                world={world}
                instanceId={instance.id}
                instanceVersion={instance.gameVersion}
                busy={running}
                onEnter={() => void launch(instance.id, world.folder)}
                onOpenFolder={() => openFolder(world.folder)}
                onBackups={() => setBackupsOf({ folder: world.folder, name: world.name })}
                onNotes={() =>
                  setNotesOf({ folder: world.folder, name: world.name, seed: world.seed })
                }
                onAutoCovers={() => setAutoCoverOf({ folder: world.folder, name: world.name })}
                onShareCard={() => setShareOf({ instance, world })}
                onShortcut={() => void createShortcut(instance.id, world.folder)}
              />
            )}
          />
        )}
      </section>

      <aside className="flex min-h-0 flex-col self-start rounded-panel border border-line bg-surface shadow-raised max-h-full">
        <header className="flex items-center gap-2 px-4 pt-4 pb-2">
          <SectionTitle
            title={t("detail.servers.title")}
            count={servers && servers.length > 0 ? servers.length : null}
          />
          {servers && servers.length > 0 && (
            <IconButton
              label={t("server.refresh")}
              size="sm"
              className="ml-auto"
              onClick={() => setRefresh((n) => n + 1)}
            >
              <RefreshCw className="size-4" aria-hidden />
            </IconButton>
          )}
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto px-2">
          {servers?.length === 0 && (
            <div className="px-2 pt-1 pb-3">
              <p className="text-sm">{t("detail.servers.empty")}</p>
              <p className="mt-1 text-fg-muted text-xs">{t("detail.servers.emptyHint")}</p>
            </div>
          )}
          {servers?.map((server) => (
            <ServerRow
              key={server.address}
              server={server}
              busy={running}
              refresh={refresh}
              onEnter={() => void launch(instance.id, undefined, server.address)}
            />
          ))}
        </div>
        <footer className="border-line border-t p-2">
          <Button
            variant="ghost"
            size="sm"
            className="w-full justify-start"
            onClick={() => find("server")}
          >
            <Compass className="size-4" aria-hidden />
            {t("detail.servers.find")}
          </Button>
        </footer>
      </aside>
      <BackupsDialog
        instanceId={instance.id}
        world={backupsOf}
        onClose={() => setBackupsOf(null)}
      />
      <NotesDialog instanceId={instance.id} world={notesOf} onClose={() => setNotesOf(null)} />
      <AutoCoverDialog
        instanceId={instance.id}
        world={autoCoverOf}
        onClose={() => setAutoCoverOf(null)}
      />
      {shareOf && (
        <Suspense fallback={null}>
          <ShareCardDialog target={shareOf} onClose={() => setShareOf(null)} />
        </Suspense>
      )}
    </div>
  );
}
