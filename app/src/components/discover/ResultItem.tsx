import { openUrl } from "@tauri-apps/plugin-opener";
import {
  BookOpen,
  Clock,
  Copy,
  Download,
  ExternalLink,
  Globe,
  Monitor,
  Server,
  Server as ServerIcon,
  UserRound,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ServerStatusLine, useServerStatus } from "@/components/ServerStatus";
import { HoverHint, MoreTag, Truncated } from "@/components/ui/HoverCard";
import { Menu, type MenuItem } from "@/components/ui/Menu";
import { Tag } from "@/components/ui/Tag";
import { useSoftCover } from "@/lib/cover";
import { type Entry, formatCount, formatSince, mcmodSearchUrl, tagsOf } from "@/lib/discover";
import { reportError } from "@/lib/report";
import { FavoriteButton } from "./FavoriteButton";
import { InstallButton } from "./InstallButton";
import { ProjectName } from "./ProjectName";
import type { InstallKind, InstallState } from "./useInstall";

export const open = (url: string, what: string) => {
  openUrl(url).catch((error: unknown) => reportError(error, what));
};

/** 「⋯」菜单：在各平台打开、打开作者主页、复制链接、在 MC百科 搜索 */
export function useEntryMenu(entry: Entry): MenuItem[] {
  const { t } = useTranslation();
  const { hit, sources, pages } = entry;
  const firstPage = sources.map((s) => pages[s]).find(Boolean);
  return [
    ...sources.flatMap((source) => {
      const url = pages[source];
      return url
        ? [
            {
              key: source,
              label: t("discover.openOn", { source: t(`discover.sourceName.${source}`) }),
              icon: ExternalLink,
              onSelect: () => open(url, "打开项目页"),
            },
          ]
        : [];
    }),
    ...(hit.authorUrl
      ? [
          {
            key: "author",
            label: t("discover.openAuthor", { name: hit.author }),
            icon: UserRound,
            onSelect: () => hit.authorUrl && open(hit.authorUrl, "打开作者主页"),
          },
        ]
      : []),
    ...(firstPage
      ? [
          {
            key: "copy",
            label: t("discover.copyLink"),
            icon: Copy,
            onSelect: () => {
              navigator.clipboard
                .writeText(firstPage)
                .then(() => toast.success(t("discover.copied")))
                .catch((error: unknown) => reportError(error, "复制链接"));
            },
          },
        ]
      : []),
    ...(hit.server
      ? [
          {
            key: "address",
            label: t("discover.copyAddress"),
            icon: ServerIcon,
            onSelect: () => {
              const address = hit.server?.address ?? "";
              navigator.clipboard
                .writeText(address)
                .then(() => toast.success(t("discover.addressCopied", { address })))
                .catch((error: unknown) => reportError(error, "复制服务器地址"));
            },
          },
        ]
      : []),
    {
      key: "mcmod",
      label: t("discover.mcmod"),
      icon: BookOpen,
      onSelect: () => open(mcmodSearchUrl(hit.title), "打开 MC百科"),
    },
  ];
}

/** 作者名：有主页时可以点（用浏览器打开） */
export function AuthorLink({ entry, className = "" }: { entry: Entry; className?: string }) {
  const { t } = useTranslation();
  const { author, authorUrl } = entry.hit;
  if (!author) return null;
  if (!authorUrl) return <span className={`truncate ${className}`}>{author}</span>;
  return (
    <HoverHint
      hint={t("discover.openAuthor", { name: author })}
      className={`inline-flex min-w-0 max-w-full ${className}`}
    >
      <button
        type="button"
        onClick={(event) => {
          event.stopPropagation();
          open(authorUrl, "打开作者主页");
        }}
        className="truncate text-left transition-colors duration-(--motion-press) hover:text-accent hover:underline"
      >
        {author}
      </button>
    </HoverHint>
  );
}

/** 项目图标；没有图标时是名字的第一个字 */
export function ProjectIcon({ entry, className }: { entry: Entry; className: string }) {
  const { iconUrl, title } = entry.hit;
  return iconUrl ? (
    <img
      src={iconUrl}
      alt=""
      loading="lazy"
      decoding="async"
      className={`object-cover ${className}`}
    />
  ) : (
    <div
      className={`grid place-items-center bg-accent-soft font-semibold text-accent ${className}`}
    >
      {Array.from(title.trim())[0]?.toUpperCase() ?? "?"}
    </div>
  );
}

interface ResultItemProps {
  entry: Entry;
  kind: InstallKind;
  state: InstallState;
  /** 没有可以装进去的实例 */
  disabled: boolean;
  /** 进详情页 */
  onOpen: () => void;
  onInstall: () => void;
  /** 收藏夹里的项目：显示它是哪一类 */
  showKind?: boolean;
}

/** 服务器现在的样子：自己测的在线人数和延迟，连不上的写「连不上」 */
function ServerLive({ address }: { address: string }) {
  const status = useServerStatus(address);
  return <ServerStatusLine status={status} address={address} className="shrink-0" />;
}

/** 下载量和多久前更新。卡片里上下两行（底栏左边不空着），列表里排成一行 */
function Meta({ entry, stacked = false }: { entry: Entry; stacked?: boolean }) {
  const { t } = useTranslation();
  const since = formatSince(entry.hit.updated, t);
  return (
    <span
      className={`flex min-w-0 text-fg-muted text-xs ${stacked ? "flex-col gap-0.5" : "items-center gap-3"}`}
    >
      {entry.hit.server ? (
        <ServerLive address={entry.hit.server.address} />
      ) : (
        <HoverHint
          className="inline-flex shrink-0 items-center gap-1 tabular-nums"
          // 两个平台合成一条时显示的是加起来的总数，悬停看各自多少
          hint={
            entry.sources.length > 1
              ? t("discover.downloadsSplit", {
                  parts: entry.sources
                    .map(
                      (source) =>
                        `${t(`discover.sourceName.${source}`)} ${formatCount(entry.counts[source])}`,
                    )
                    .join(" + "),
                })
              : null
          }
        >
          <Download className="size-3.5" aria-hidden />
          {formatCount(entry.hit.downloads)}
        </HoverHint>
      )}
      {since && (
        <span className="inline-flex min-w-0 items-center gap-1 truncate">
          <Clock className="size-3.5 shrink-0" aria-hidden />
          {since}
        </span>
      )}
    </span>
  );
}

export const SIDE_ICON = { client: Monitor, server: Server, both: Globe, either: Globe } as const;

/**
 * 标签：全部同一种样式（不用颜色区分），只占一行，放不下的显示「+N」。
 * 装在哪一端用小图标区分：显示器 = 客户端，服务器 = 服务端，地球 = 两端。
 */
export function Tags({
  entry,
  kind,
  showKind,
  max = 2,
  tone = "neutral",
}: {
  entry: Entry;
  kind: InstallKind;
  showKind?: boolean;
  /** 分类标签最多显示几个 */
  max?: number;
  tone?: "neutral" | "hero";
}) {
  const { t } = useTranslation();
  const all = tagsOf(entry.hit, t, Number.POSITIVE_INFINITY);
  const shown = all.slice(0, max);
  const hidden = all.slice(max);
  const side = entry.hit.side;
  const SideIcon = side ? SIDE_ICON[side] : null;
  return (
    <div className="flex min-w-0 flex-nowrap items-center gap-1.5 overflow-hidden">
      {showKind && <Tag tone={tone}>{t(`kind.${kind}`)}</Tag>}
      {kind === "map" && entry.hit.manualOnly && (
        <Tag tone={tone} className="gap-1">
          <ExternalLink className="size-3 shrink-0" aria-hidden />
          {t("discover.manualTag")}
        </Tag>
      )}
      {entry.hit.server?.gameVersions?.[0] && (
        <Tag tone={tone}>{entry.hit.server.gameVersions?.[0]}</Tag>
      )}
      {side && SideIcon && (
        <HoverHint hint={t(`discover.side.${side}`)} className="inline-flex shrink-0">
          <Tag tone={tone} className="gap-1">
            <SideIcon className="size-3 shrink-0" aria-hidden />
            {t(`discover.sideShort.${side}`)}
          </Tag>
        </HoverHint>
      )}
      {shown.map((tag) => (
        <Tag key={tag} tone={tone}>
          {tag}
        </Tag>
      ))}
      <MoreTag items={hidden} tone={tone} />
    </div>
  );
}

/** 名字后面的平台小标（两个平台都有时两个都标） */
function Sources({ entry }: { entry: Entry }) {
  const { t } = useTranslation();
  return (
    <span className="flex shrink-0 gap-1">
      {entry.sources.map((s) => (
        <span
          key={s}
          className="rounded-tag border border-line px-1.5 text-2xs text-fg-muted leading-[18px]"
        >
          {t(`discover.sourceName.${s}`)}
        </span>
      ))}
    </span>
  );
}

/** 卡片上面的图：先铺图标虚化成的一片颜色，作者的展示图下好了再淡入（不会先白一块） */
function CardImage({ entry }: { entry: Entry }) {
  const { hit } = entry;
  const soft = useSoftCover(hit.iconUrl, true, true);
  const [loaded, setLoaded] = useState(false);
  return (
    <div className="relative aspect-[2/1] overflow-hidden rounded-item bg-accent-soft">
      {soft && (
        <img src={soft} alt="" className="absolute inset-0 size-full object-cover opacity-80" />
      )}
      {hit.galleryUrl && (
        <img
          src={hit.galleryUrl}
          alt=""
          loading="lazy"
          decoding="async"
          onLoad={() => setLoaded(true)}
          className={`hover-zoom absolute inset-0 size-full object-cover transition-[opacity,scale] duration-(--motion-page) ease-out group-hover:scale-[1.03] ${
            loaded ? "opacity-100" : "opacity-0"
          }`}
        />
      )}
    </div>
  );
}

/**
 * 精美模式的大图卡片：上面是作者放的展示图（没有时是图标虚化成的一片颜色），
 * 下面左边对齐：图标 + 名字 + 平台、作者，两行简介，一行标签，底栏左边下载量和更新时间、右边按钮。
 * 简介固定两行、标签固定一行，所以同一排的卡片一样高。整张卡片点了进详情页。
 */
export function ResultCard({
  entry,
  kind,
  state,
  disabled,
  onOpen,
  onInstall,
  showKind,
}: ResultItemProps) {
  const { t } = useTranslation();
  const menu = useEntryMenu(entry);
  const { hit } = entry;
  return (
    // 卡片里还有按钮，不能整张做成 <button>
    <article
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(event) => {
        if (event.key === "Enter") onOpen();
      }}
      className="group flex h-full cursor-default flex-col rounded-panel border border-line bg-surface p-1.5 shadow-raised transition-[border-color] duration-(--motion-press) hover:border-accent/40"
    >
      <CardImage entry={entry} />
      <div className="flex flex-1 flex-col gap-2.5 px-2.5 pt-3 pb-1.5">
        <div className="flex min-w-0 items-center gap-3">
          <ProjectIcon entry={entry} className="size-11 shrink-0 rounded-row" />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-2">
              <ProjectName
                className="min-w-0 truncate font-medium"
                title={hit.title}
                titleZh={hit.titleZh}
              />
              <Sources entry={entry} />
            </div>
            <AuthorLink entry={entry} className="block max-w-full text-fg-muted text-xs" />
          </div>
        </div>
        <Truncated
          as="p"
          className="line-clamp-2 h-10 text-fg-muted text-sm leading-5"
          text={hit.description}
        />
        <Tags entry={entry} kind={kind} showKind={showKind} />
        <div className="mt-auto flex items-end gap-2 pt-1">
          <Meta entry={entry} stacked />
          <div className="ml-auto flex shrink-0 items-center gap-1">
            <FavoriteButton entry={entry} kind={kind} />
            <Menu label={t("discover.menu")} items={menu} />
            <InstallButton state={state} kind={kind} disabled={disabled} onClick={onInstall} />
          </div>
        </div>
      </div>
    </article>
  );
}

/** 流畅模式（或者手动选「列表」）：一行一个，没有大图，老电脑上也轻 */
export function ResultRow({
  entry,
  kind,
  state,
  disabled,
  onOpen,
  onInstall,
  showKind,
}: ResultItemProps) {
  const { t } = useTranslation();
  const menu = useEntryMenu(entry);
  const { hit } = entry;
  return (
    // 行里还有按钮，不能整行做成 <button>
    <article
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(event) => {
        if (event.key === "Enter") onOpen();
      }}
      className="flex cursor-default items-center gap-4 rounded-panel border border-line bg-surface p-3 shadow-raised transition-[border-color] duration-(--motion-press) hover:border-accent/40"
    >
      <ProjectIcon entry={entry} className="size-12 shrink-0 rounded-row" />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <ProjectName
            className="min-w-0 truncate font-medium"
            title={hit.title}
            titleZh={hit.titleZh}
          />
          <Sources entry={entry} />
          <AuthorLink entry={entry} className="min-w-0 shrink text-fg-muted text-xs" />
        </div>
        <Truncated as="p" className="line-clamp-1 text-fg-muted text-sm" text={hit.description} />
        <div className="flex min-w-0 items-center gap-4">
          <Tags entry={entry} kind={kind} showKind={showKind} max={3} />
          <span className="ml-auto shrink-0">
            <Meta entry={entry} />
          </span>
        </div>
      </div>
      <FavoriteButton entry={entry} kind={kind} />
      <Menu label={t("discover.menu")} items={menu} />
      <InstallButton state={state} kind={kind} disabled={disabled} onClick={onInstall} />
    </article>
  );
}
