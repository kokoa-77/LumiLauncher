import {
  CalendarDays,
  CircleAlert,
  CircleCheck,
  Clock,
  Download,
  ExternalLink,
  FileText,
  Globe,
  Heart,
  Images,
  Layers,
  Scale,
  Users,
} from "lucide-react";
import { lazy, type ReactNode, Suspense, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation, useParams, useSearchParams } from "react-router";
import { FavoriteButton } from "@/components/discover/FavoriteButton";
import { InstallButton } from "@/components/discover/InstallButton";
import {
  AuthorLink,
  open,
  ProjectIcon,
  SIDE_ICON,
  Tags,
  useEntryMenu,
} from "@/components/discover/ResultItem";
import {
  type InstallKind,
  useInstall,
  useInstalledContent,
} from "@/components/discover/useInstall";
import { Lightbox } from "@/components/Lightbox";
import { ServerStatusLine, useServerStatus } from "@/components/ServerStatus";
import { EmptyState } from "@/components/ui/EmptyState";
import { MoreTag, Truncated } from "@/components/ui/HoverCard";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Menu } from "@/components/ui/Menu";
import { Tabs } from "@/components/ui/Tabs";
import { Tag } from "@/components/ui/Tag";
import { unwrap } from "@/lib/api";
import { type ContentSource, commands, type ProjectDetail } from "@/lib/bindings";
import { useSoftCover } from "@/lib/cover";
import {
  type Entry,
  findInstalled,
  formatCount,
  formatSince,
  groupVersions,
  isContentKind,
  pickTarget,
  supportOf,
  toEntry,
} from "@/lib/discover";
import { PAGE } from "@/lib/layout";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";

// 介绍渲染要 Markdown 解析和 HTML 清洗，第一次打开详情页才加载
const RichText = lazy(() => import("@/components/discover/RichText"));
const ProjectVersionsTab = lazy(async () => ({
  default: (await import("@/components/discover/ProjectVersionsTab")).ProjectVersionsTab,
}));

type Tab = "about" | "versions" | "gallery";

/** 右边信息栏的一张卡片：上面一个小标题，`dense` 时里面是一行行能点的（作者、链接） */
function Card({
  title,
  dense = false,
  children,
}: {
  title: string;
  dense?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      className={`flex flex-col rounded-panel border border-line bg-surface shadow-raised ${
        dense ? "gap-0.5 p-2" : "gap-3 p-4"
      }`}
    >
      <h3 className={`font-medium text-sm ${dense ? "px-2 pt-1.5 pb-1" : ""}`}>{title}</h3>
      {children}
    </section>
  );
}

/** 兼容性里的一行：左边小字说明，下面一排标签 */
function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-fg-muted text-xs">{label}</span>
      <div className="flex flex-wrap gap-1.5">{children}</div>
    </div>
  );
}

/** 服务器现在的样子（D17k）：在线人数、延迟，鼠标停上去看版本和简介 */
function ServerLiveFact({ address }: { address: string }) {
  const { t } = useTranslation();
  const status = useServerStatus(address);
  return (
    <div className="flex items-center gap-2.5 text-sm">
      <Users className="size-4 shrink-0 text-fg-muted" aria-hidden />
      <span className="shrink-0 text-fg-muted">{t("discover.project.online")}</span>
      <ServerStatusLine status={status} address={address} className="ml-auto min-w-0" />
    </div>
  );
}

function Fact({ icon: Icon, label, value }: { icon: typeof Clock; label: string; value: string }) {
  return (
    <div className="flex items-center gap-2.5 text-sm">
      <Icon className="size-4 shrink-0 text-fg-muted" aria-hidden />
      <span className="shrink-0 text-fg-muted">{label}</span>
      <Truncated className="ml-auto min-w-0 truncate text-right" text={value} />
    </div>
  );
}

/**
 * 项目详情：页头是作者放的展示图（没有时是图标放大成的一片颜色），图标、名字、作者、安装按钮压在图的下沿；
 * 下面是「介绍 / 图片」两页和右边的信息栏（下载量、更新时间、许可证、外部链接）。
 * 从列表点进来时先用列表里的信息把页头画出来，详情读到了再补全，不让你对着空白等。
 */
export function ProjectPage() {
  const { t } = useTranslation();
  const location = useLocation();
  const [params] = useSearchParams();
  const { source, projectId = "" } = useParams<{ source: ContentSource; projectId: string }>();
  const instances = useGameStore((s) => s.instances);
  const passed = (location.state as { entry?: Entry } | null)?.entry;

  const [detail, setDetail] = useState<ProjectDetail | null>(null);
  const [failed, setFailed] = useState(false);
  const [tab, setTab] = useState<Tab>("about");
  const [viewing, setViewing] = useState<number | null>(null);

  const kindParam = params.get("kind");
  const kind: InstallKind =
    kindParam === "modpack" ||
    kindParam === "shader" ||
    kindParam === "resourcePack" ||
    kindParam === "map" ||
    kindParam === "server"
      ? kindParam
      : "mod";
  const instanceId = pickTarget(instances, params.get("instance"));
  const instance = instances.find((i) => i.id === instanceId);
  const installed = useInstalledContent(instanceId, isContentKind(kind) ? [kind] : []);
  const installer = useInstall(instanceId, instance?.name ?? "");

  useEffect(() => {
    if (source !== "modrinth" && source !== "curseForge") return;
    let cancelled = false;
    setFailed(false);
    commands
      .discoverProject(source, projectId)
      .then(unwrap)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        reportError(error, "读取项目详情");
        setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [source, projectId]);

  // 列表里带过来的信息（合并过两个平台）优先，详情里有更全的再补上
  const entry: Entry | null = useMemo(() => {
    if (passed) {
      if (!detail) return passed;
      return {
        ...passed,
        hit: {
          ...detail.hit,
          author: passed.hit.author || detail.hit.author,
          authorUrl: passed.hit.author ? passed.hit.authorUrl : detail.hit.authorUrl,
          server: passed.hit.server ?? null,
          titleZh: detail.hit.titleZh ?? passed.hit.titleZh ?? null,
        },
      };
    }
    return detail ? toEntry(detail.hit) : null;
  }, [passed, detail]);

  const menu = useEntryMenu(entry ?? toEntry(emptyHit(source, projectId)));
  const banner = detail?.gallery[0]?.url ?? entry?.hit.galleryUrl ?? null;
  // 标题栏「现在在哪」的最后一级显示项目名（直接打开、刷新后也有）
  const setPageTitle = useAppStore((s) => s.setPageTitle);
  const title = entry ? (entry.hit.titleZh ?? entry.hit.title) : null;
  useEffect(() => {
    setPageTitle(title);
    return () => setPageTitle(null);
  }, [title, setPageTitle]);
  const soft = useSoftCover(entry?.hit.iconUrl, !banner);

  if (!entry) {
    return (
      <div className={`${PAGE} flex flex-col gap-6`}>
        {failed ? (
          <EmptyState
            icon={ExternalLink}
            title={t("discover.project.loadFailed")}
            hint={t("discover.project.loadFailedHint")}
          />
        ) : (
          <div className="grid h-64 place-items-center">
            <LightSpinner className="size-6 text-accent" />
          </div>
        )}
      </div>
    );
  }

  const { hit } = entry;
  const since = formatSince(hit.updated, t);
  const published = formatSince(detail?.published, t);
  const gallery = detail?.gallery ?? [];
  const versions = groupVersions(detail?.gameVersions ?? []);
  const supportsVersionsTab = isContentKind(kind);
  const installedItem =
    supportsVersionsTab && entry ? (findInstalled(entry, installed[kind] ?? []) ?? null) : null;
  const support =
    detail && instance && kind !== "modpack"
      ? supportOf(detail.gameVersions, detail.loaders, kind, instance)
      : null;
  const SideIcon = hit.side ? SIDE_ICON[hit.side] : null;

  return (
    <div className={`${PAGE} flex flex-col gap-6`}>
      <header className="relative isolate flex min-h-60 flex-col justify-end overflow-hidden rounded-panel border border-line">
        {banner ? (
          <img src={banner} alt="" className="-z-10 absolute inset-0 size-full object-cover" />
        ) : soft ? (
          <img src={soft} alt="" className="-z-10 absolute inset-0 size-full object-cover" />
        ) : (
          <div className="-z-10 absolute inset-0 bg-accent-soft" />
        )}
        <div className="-z-10 absolute inset-0 bg-hero-tint" />
        <div className="-z-10 absolute inset-0 bg-linear-to-t from-hero-scrim from-15% via-hero-scrim/60 to-transparent" />
        <div className="flex flex-wrap items-end gap-5 p-6 text-on-hero">
          <ProjectIcon
            entry={entry}
            className="size-20 shrink-0 rounded-panel shadow-overlay ring-1 ring-hero-chip-line"
          />
          <div className="min-w-0 flex-1">
            <h1 className="truncate font-semibold text-3xl leading-tight">
              {hit.titleZh ?? hit.title}
              {hit.titleZh && (
                <span className="ml-3 font-normal text-lg text-on-hero-muted">{hit.title}</span>
              )}
            </h1>
            <div className="mt-1 flex items-center gap-2 text-on-hero-muted text-sm">
              <AuthorLink entry={entry} />
            </div>
            <Truncated
              as="p"
              className="mt-2 line-clamp-2 max-w-3xl text-on-hero-muted text-sm"
              text={hit.description}
            />
            <div className="mt-3">
              <Tags entry={entry} kind={kind} max={6} tone="hero" />
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <FavoriteButton entry={entry} kind={kind} tone="hero" />
            <Menu label={t("discover.menu")} items={menu} trigger="hero" />
            <InstallButton
              state={installer.stateOf(entry, kind, installed)}
              kind={kind}
              disabled={kind !== "modpack" && !instanceId}
              onClick={() => void installer.start(entry, kind)}
            />
          </div>
        </div>
      </header>

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_280px]">
        <section className="flex min-w-0 flex-col gap-4">
          <Tabs<Tab>
            label={t("discover.project.tabs")}
            value={tab}
            onChange={setTab}
            options={[
              { value: "about", label: t("discover.project.about"), icon: FileText },
              ...(supportsVersionsTab
                ? [
                    {
                      value: "versions" as const,
                      label: t("discover.project.versionsTab"),
                      icon: Layers,
                    },
                  ]
                : []),
              {
                value: "gallery",
                label: t("discover.project.gallery", { n: gallery.length }),
                icon: Images,
              },
            ]}
          />
          {tab === "about" ? (
            <div className="rounded-panel border border-line bg-surface p-6 shadow-raised">
              {detail ? (
                detail.body.trim() ? (
                  <Suspense fallback={<LightSpinner className="size-5 text-accent" />}>
                    <RichText body={detail.body} format={detail.bodyFormat} />
                  </Suspense>
                ) : (
                  <p className="text-fg-muted text-sm">
                    {hit.description || t("discover.project.noBody")}
                  </p>
                )
              ) : failed ? (
                <p className="text-fg-muted text-sm">{t("discover.project.loadFailedHint")}</p>
              ) : (
                <LightSpinner className="size-5 text-accent" />
              )}
            </div>
          ) : tab === "versions" &&
            supportsVersionsTab &&
            (source === "modrinth" || source === "curseForge") ? (
            <Suspense
              fallback={
                <div className="grid h-48 place-items-center rounded-panel border border-line bg-surface shadow-raised">
                  <LightSpinner className="size-6 text-accent" />
                </div>
              }
            >
              <ProjectVersionsTab
                source={source}
                projectId={projectId}
                kind={kind}
                instance={instance}
                installedItem={installedItem}
                gameVersions={detail?.gameVersions ?? []}
                loaders={detail?.loaders ?? []}
              />
            </Suspense>
          ) : gallery.length === 0 ? (
            <EmptyState
              icon={Heart}
              title={t("discover.project.noGallery")}
              hint={t("discover.project.noGalleryHint")}
            />
          ) : (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
              {gallery.map((image, i) => (
                <button
                  key={image.url}
                  type="button"
                  onClick={() => setViewing(i)}
                  className="group relative aspect-video overflow-hidden rounded-item border border-line bg-surface-2"
                >
                  <img
                    src={image.thumbUrl}
                    alt={image.title ?? ""}
                    loading="lazy"
                    decoding="async"
                    className="hover-zoom size-full object-cover transition-[scale] duration-(--motion-page) ease-out group-hover:scale-[1.04]"
                  />
                  {image.title && (
                    <span className="absolute inset-x-0 bottom-0 truncate bg-linear-to-t from-hero-scrim to-transparent px-3 pt-6 pb-2 text-left text-on-hero text-xs">
                      {image.title}
                    </span>
                  )}
                </button>
              ))}
            </div>
          )}
        </section>

        <aside className="flex flex-col gap-4">
          {detail && (detail.gameVersions.length > 0 || detail.loaders.length > 0 || hit.side) && (
            <Card title={t("discover.project.compat")}>
              {support && instance && (
                <div
                  className={`flex items-start gap-2 rounded-row px-3 py-2 text-xs ${
                    support === "yes"
                      ? "bg-accent-soft text-accent"
                      : "bg-warning-soft text-warning"
                  }`}
                >
                  {support === "yes" ? (
                    <CircleCheck className="mt-px size-3.5 shrink-0" aria-hidden />
                  ) : (
                    <CircleAlert className="mt-px size-3.5 shrink-0" aria-hidden />
                  )}
                  <span className="min-w-0">
                    {t(`discover.project.support.${support}`, {
                      name: instance.name,
                      version: instance.gameVersion,
                    })}
                  </span>
                </div>
              )}
              {versions.shown.length > 0 && (
                <Row label={t("discover.project.versions")}>
                  {versions.shown.map((v) => (
                    <Tag
                      key={v}
                      className={instance && v === instance.gameVersion ? "text-fg" : ""}
                    >
                      {v}
                    </Tag>
                  ))}
                  <MoreTag items={versions.hidden} />
                </Row>
              )}
              {detail.loaders.length > 0 && (
                <Row label={t("discover.project.loaders")}>
                  {detail.loaders.map((l) => (
                    <Tag key={l}>{t(`discover.loader.${l}`, { defaultValue: l })}</Tag>
                  ))}
                </Row>
              )}
              {hit.side && SideIcon && (
                <Row label={t("discover.project.environment")}>
                  <Tag className="gap-1">
                    <SideIcon className="size-3 shrink-0" aria-hidden />
                    {t(`discover.side.${hit.side}`)}
                  </Tag>
                </Row>
              )}
            </Card>
          )}

          {hit.server && (
            <Card title={t("discover.project.serverInfo")}>
              <Fact icon={Globe} label={t("discover.project.address")} value={hit.server.address} />
              <ServerLiveFact address={hit.server.address} />
              {(hit.server.gameVersions?.length ?? 0) > 0 && (
                <Row label={t("discover.project.versions")}>
                  {(hit.server.gameVersions ?? []).slice(0, 6).map((v) => (
                    <Tag key={v}>{v}</Tag>
                  ))}
                </Row>
              )}
              <p className="text-fg-muted text-xs">
                {hit.server.modpackVersion
                  ? t("discover.project.serverNeedsPack")
                  : t("discover.project.serverVanilla")}
              </p>
            </Card>
          )}
          <Card title={t("discover.project.info")}>
            <Fact
              icon={Download}
              label={t("discover.project.downloads")}
              value={formatCount(hit.downloads)}
            />
            {detail?.followers != null && (
              <Fact
                icon={Heart}
                label={t("discover.project.followers")}
                value={formatCount(detail?.followers)}
              />
            )}
            {since && <Fact icon={Clock} label={t("discover.project.updated")} value={since} />}
            {published && (
              <Fact icon={CalendarDays} label={t("discover.project.published")} value={published} />
            )}
            {detail?.license && (
              <Fact
                icon={Scale}
                label={t("discover.project.license")}
                value={
                  detail.license === "All Rights Reserved"
                    ? t("discover.project.arr")
                    : detail.license
                }
              />
            )}
            <div className="flex flex-wrap gap-1.5 pt-1">
              {entry.sources.map((s) => (
                <Tag key={s}>{t(`discover.sourceName.${s}`)}</Tag>
              ))}
            </div>
          </Card>

          {detail && detail.members.length > 0 && (
            <Card title={t("discover.project.team")} dense>
              {detail.members.slice(0, 8).map((member) => (
                <button
                  key={member.name}
                  type="button"
                  disabled={!member.url}
                  onClick={() => member.url && open(member.url, "打开作者主页")}
                  className="flex items-center gap-2.5 rounded-item px-2 py-1.5 text-left transition-colors duration-(--motion-press) hover:bg-surface-hover disabled:hover:bg-transparent"
                >
                  {member.avatarUrl ? (
                    <img
                      src={member.avatarUrl}
                      alt=""
                      loading="lazy"
                      className="size-8 shrink-0 rounded-icon bg-surface-2 object-cover"
                    />
                  ) : (
                    <span className="grid size-8 shrink-0 place-items-center rounded-icon bg-accent-soft font-medium text-accent text-xs">
                      {Array.from(member.name)[0]?.toUpperCase() ?? "?"}
                    </span>
                  )}
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm">{member.name}</span>
                    <span className="block truncate text-fg-muted text-xs">
                      {member.role
                        ? t(`discover.project.role.${member.role}`, { defaultValue: member.role })
                        : t("discover.project.author")}
                    </span>
                  </span>
                  {member.url && (
                    <ExternalLink className="size-3.5 shrink-0 text-fg-muted" aria-hidden />
                  )}
                </button>
              ))}
            </Card>
          )}

          {detail && detail.links.length > 0 && (
            <Card title={t("discover.project.linksTitle")} dense>
              {detail.links.map((link) => (
                <button
                  key={link.url}
                  type="button"
                  onClick={() => open(link.url, "打开项目链接")}
                  className="flex items-center gap-2.5 rounded-item px-2 py-2 text-left text-sm transition-colors duration-(--motion-press) hover:bg-surface-hover"
                >
                  <ExternalLink className="size-4 shrink-0 text-fg-muted" aria-hidden />
                  <span className="truncate">{t(`discover.project.link.${link.kind}`)}</span>
                </button>
              ))}
            </Card>
          )}
        </aside>
      </div>

      <Lightbox
        images={gallery}
        index={viewing}
        onIndex={setViewing}
        onClose={() => setViewing(null)}
      />
      {installer.dialogs}
    </div>
  );
}

/** 详情还没读到、也没从列表带信息过来时，菜单先用一个空的占位 */
function emptyHit(source: ContentSource | undefined, projectId: string) {
  return {
    projectId,
    slug: "",
    title: "",
    description: "",
    author: "",
    iconUrl: null,
    downloads: 0,
    displayCategories: [],
    source: source ?? "modrinth",
    side: null,
    pageUrl: null,
  };
}
