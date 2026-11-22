import { Boxes, FolderMinus, Pencil, Pin, Plus, Search } from "lucide-react";
import { AnimatePresence, LayoutGroup, motion } from "motion/react";
import { memo, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";
import { groupNames } from "@/components/GroupDialog";
import { InstanceTile } from "@/components/InstanceTile";
import { NewInstanceDialog } from "@/components/NewInstanceDialog";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { EmptyState } from "@/components/ui/EmptyState";
import { TextInput } from "@/components/ui/Field";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Menu } from "@/components/ui/Menu";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { SectionTitle } from "@/components/ui/SectionTitle";
import { Segmented } from "@/components/ui/Segmented";
import type { InstanceSummary, JobSnapshot, WorldView } from "@/lib/bindings";
import { jobProgressLabel, jobTitle } from "@/lib/jobLabels";
import { PAGE } from "@/lib/layout";
import { duration, ease, spring } from "@/lib/motion";
import { useGameStore } from "@/stores/game";

type View = "grid" | "list";
const VIEW_KEY = "mcl.instancesView";

function savedView(): View {
  try {
    return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

/** 正在导入的整合包：在新实例会出现的地方先放一张进度卡 */
function ImportingCard({ job }: { job: JobSnapshot }) {
  const { t } = useTranslation();
  const title = jobTitle(job, t);
  const done = job.progress.done ?? 0;
  const total = job.progress.total ?? 0;
  return (
    <li className="flex flex-col gap-2 rounded-panel border border-line bg-surface p-4 shadow-raised">
      <div className="flex items-center gap-2 text-sm">
        <LightSpinner className="size-4 shrink-0 text-accent" />
        <span className="truncate font-medium">{title}</span>
        <span className="ml-auto shrink-0 text-fg-muted text-xs">
          {jobProgressLabel(job.progress.message, t)}
        </span>
      </div>
      <ProgressBar value={total > 0 ? done / total : 0} label={title} />
    </li>
  );
}

interface Section {
  key: string;
  /** 没有标题：一个分组、一个置顶都没有时整页就是一块 */
  title: string | null;
  /** 是分组的话是分组名（标题旁边能改名、解散） */
  group: string | null;
  pinned: boolean;
  items: InstanceSummary[];
}

/**
 * 分块：置顶的在最上面（先置顶的在前），然后每个分组一块（按名字），最后是没分组的「其他」。
 * 置顶像收藏栏：置顶的实例在最上面多一份，原来的分组里还有它（不然置顶一下分组就空了）。
 */
function sectionsOf(
  all: InstanceSummary[],
  shown: InstanceSummary[],
  names: { other: string; all: string },
): Section[] {
  const organized = all.some((i) => i.pinnedAt !== null || i.group !== null);
  if (!organized) return [{ key: "all", title: null, group: null, pinned: false, items: shown }];
  const pinned = shown
    .filter((i) => i.pinnedAt !== null)
    .sort((a, b) => (a.pinnedAt ?? 0) - (b.pinnedAt ?? 0));
  const sections: Section[] = [];
  if (pinned.length > 0)
    sections.push({ key: "pinned", title: null, group: null, pinned: true, items: pinned });
  for (const group of groupNames(shown)) {
    sections.push({
      key: `group:${group}`,
      title: group,
      group,
      pinned: false,
      items: shown.filter((i) => i.group === group),
    });
  }
  const loose = shown.filter((i) => i.group === null);
  // 一个分组都没有（只置顶了几个）时，下面这块就是全部实例
  const hasGroups = all.some((i) => i.group !== null);
  if (loose.length > 0)
    sections.push({
      key: "other",
      title: hasGroups ? names.other : names.all,
      group: null,
      pinned: false,
      items: loose,
    });
  return sections;
}

const TileList = memo(function TileList({
  items,
  view,
  worldOf,
  pinnedSection,
  layoutKey,
}: {
  items: InstanceSummary[];
  view: View;
  worldOf: Map<string, WorldView | undefined>;
  /** 是「置顶」那一块：进实例页的封面动画给这一份 */
  pinnedSection: boolean;
  layoutKey: string;
}) {
  return (
    <motion.ul
      layout
      layoutDependency={layoutKey}
      className={
        view === "grid"
          ? "grid grid-cols-[repeat(auto-fill,minmax(250px,1fr))] gap-4"
          : "flex flex-col gap-2"
      }
    >
      <AnimatePresence mode="popLayout" initial={false}>
        {items.map((instance) => (
          <motion.li
            key={instance.id}
            layout
            layoutDependency={layoutKey}
            initial={{ opacity: 0, scale: 0.96 }}
            animate={{ opacity: 1, scale: 1, transition: spring.gentle }}
            exit={{
              opacity: 0,
              scale: 0.96,
              transition: { duration: duration.press, ease: ease.in },
            }}
          >
            <InstanceTile
              instance={instance}
              world={worldOf.get(instance.id)}
              view={view}
              shared={pinnedSection || instance.pinnedAt === null}
            />
          </motion.li>
        ))}
      </AnimatePresence>
    </motion.ul>
  );
});

/** 分组改名、解散（分组标题旁边的「⋮」） */
function GroupActions({
  renaming,
  dissolving,
  onClose,
}: {
  renaming: string | null;
  dissolving: string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const renameGroup = useGameStore((s) => s.renameGroup);
  const [name, setName] = useState("");
  useEffect(() => {
    if (renaming) setName(renaming);
  }, [renaming]);
  const typed = name.trim();
  return (
    <>
      <Dialog
        open={renaming !== null}
        onClose={onClose}
        title={t("instances.group.renameTitle", { name: renaming ?? "" })}
        footer={
          <>
            <Button variant="ghost" onClick={onClose}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={!typed}
              onClick={() => {
                if (renaming && typed && typed !== renaming) void renameGroup(renaming, typed);
                onClose();
              }}
            >
              {t("instances.group.save")}
            </Button>
          </>
        }
      >
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (renaming && typed && typed !== renaming) void renameGroup(renaming, typed);
            onClose();
          }}
        >
          <TextInput
            value={name}
            onChange={(event) => setName(event.target.value)}
            aria-label={t("instances.group.rename")}
            maxLength={24}
            autoFocus
          />
        </form>
      </Dialog>
      <Dialog
        open={dissolving !== null}
        onClose={onClose}
        title={t("instances.group.dissolveTitle", { name: dissolving ?? "" })}
        footer={
          <>
            <Button variant="ghost" onClick={onClose}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                if (dissolving) void renameGroup(dissolving, null);
                onClose();
              }}
            >
              {t("instances.group.dissolve")}
            </Button>
          </>
        }
      >
        <p className="text-fg-muted text-sm">{t("instances.group.dissolveBody")}</p>
      </Dialog>
    </>
  );
}

/** 实例库：网格或列表，每张卡片是那个实例最近世界的截图；输入即筛选，卡片平滑挪位置。 */
export function InstancesPage() {
  const { t } = useTranslation();
  const instances = useGameStore((s) => s.instances);
  const loaded = useGameStore((s) => s.instancesLoaded);
  const jobs = useGameStore((s) => s.jobs);
  const recent = useGameStore((s) => s.recentWorlds);
  const [params, setParams] = useSearchParams();
  const [creating, setCreating] = useState(() => params.get("new") === "1");
  const [query, setQuery] = useState("");
  const [view, setView] = useState<View>(savedView);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dissolving, setDissolving] = useState<string | null>(null);
  const importing = Object.values(jobs).filter(
    (j) => j.key.startsWith("modpack-import:") && j.state === "running",
  );
  const worldOf = useMemo(
    () => new Map((recent ?? []).map((r) => [r.instanceId, r.world])),
    [recent],
  );
  const q = query.trim().toLowerCase();
  const shown = useMemo(
    () =>
      q
        ? instances.filter(
            (i) =>
              i.name.toLowerCase().includes(q) ||
              (i.pack ?? "").toLowerCase().includes(q) ||
              (i.group ?? "").toLowerCase().includes(q),
          )
        : instances,
    [instances, q],
  );
  const sections = useMemo(
    () =>
      sectionsOf(instances, shown, {
        other: t("instances.ungrouped"),
        all: t("instances.allInstances"),
      }),
    [instances, shown, t],
  );
  // 导入进度和封面颜色会刷新，只有列表结构改变时才需要重新测量位移。
  const importingCount = importing.length;
  const layoutKey = useMemo(
    () =>
      JSON.stringify([
        view,
        importingCount,
        sections.map((section) => [section.key, section.items.map((instance) => instance.id)]),
      ]),
    [view, importingCount, sections],
  );

  // 命令面板「新建实例」带过来的 ?new=1：打开窗口后从地址里去掉，返回时不会再弹
  useEffect(() => {
    if (params.get("new") !== "1") return;
    setCreating(true);
    const next = new URLSearchParams(params);
    next.delete("new");
    setParams(next, { replace: true });
  }, [params, setParams]);

  const changeView = (next: View) => {
    setView(next);
    try {
      localStorage.setItem(VIEW_KEY, next);
    } catch {
      // 记不住就每次用网格，不影响使用
    }
  };

  return (
    <div className={`${PAGE} flex flex-col gap-6`}>
      {/* 和资源中心一样：标题那一排放这一页最重要的按钮，下面一排是搜索，视图切换在最右 */}
      <div className="flex flex-col gap-4">
        <header className="flex items-center gap-3">
          <h1 className="mr-auto font-semibold text-2xl">{t("instances.title")}</h1>
          <Button variant="primary" onClick={() => setCreating(true)}>
            <Plus className="size-4" aria-hidden />
            {t("instances.create")}
          </Button>
        </header>
        <div className="flex flex-wrap items-center gap-3">
          <div className="relative min-w-56 max-w-96 flex-1">
            <Search
              className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3.5 size-4 text-fg-muted"
              aria-hidden
            />
            <TextInput
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("instances.search")}
              aria-label={t("instances.search")}
              className="pl-10"
            />
          </div>
          <div className="ml-auto">
            <Segmented<View>
              label={t("instances.view")}
              value={view}
              onChange={changeView}
              options={[
                { value: "grid", label: t("instances.grid") },
                { value: "list", label: t("instances.list") },
              ]}
            />
          </div>
        </div>
      </div>
      {importing.length > 0 && (
        <ul className="flex flex-col gap-2">
          {importing.map((job) => (
            <ImportingCard key={job.id} job={job} />
          ))}
        </ul>
      )}
      {loaded && instances.length === 0 && importing.length === 0 ? (
        <EmptyState
          icon={Boxes}
          illustration="empty-library"
          title={t("instances.empty")}
          hint={t("instances.emptyHint")}
        />
      ) : shown.length === 0 && q ? (
        <EmptyState
          icon={Search}
          title={t("instances.noMatch", { query: query.trim() })}
          hint={t("instances.noMatchHint")}
        />
      ) : (
        <LayoutGroup>
          <div className="flex flex-col gap-6">
            {sections.map((section) => (
              <motion.section
                key={section.key}
                layout="position"
                layoutDependency={layoutKey}
                className="flex flex-col gap-3"
              >
                {(section.title || section.pinned) && (
                  <div className="flex h-8 items-center gap-2">
                    {section.pinned && <Pin className="size-4 text-fg-muted" aria-hidden />}
                    <SectionTitle
                      title={section.pinned ? t("instances.pinnedSection") : (section.title ?? "")}
                      count={section.items.length}
                    />
                    {section.group && (
                      <Menu
                        label={t("instances.group.more", { name: section.group })}
                        items={[
                          {
                            key: "rename",
                            label: t("instances.group.rename"),
                            icon: Pencil,
                            onSelect: () => setRenaming(section.group),
                          },
                          {
                            key: "dissolve",
                            label: t("instances.group.dissolve"),
                            icon: FolderMinus,
                            onSelect: () => setDissolving(section.group),
                          },
                        ]}
                      />
                    )}
                  </div>
                )}
                <TileList
                  items={section.items}
                  view={view}
                  worldOf={worldOf}
                  pinnedSection={section.pinned}
                  layoutKey={layoutKey}
                />
              </motion.section>
            ))}
          </div>
        </LayoutGroup>
      )}
      <NewInstanceDialog open={creating} onClose={() => setCreating(false)} />
      <GroupActions
        renaming={renaming}
        dissolving={dissolving}
        onClose={() => {
          setRenaming(null);
          setDissolving(null);
        }}
      />
    </div>
  );
}
