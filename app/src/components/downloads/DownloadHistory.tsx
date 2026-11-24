import type { TFunction } from "i18next";
import {
  ChevronDown,
  Copy,
  Download,
  FolderOpen,
  History,
  RotateCw,
  Search,
  SearchX,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { DownloadRestArt } from "@/components/art/DownloadRestArt";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { commands, type JobSnapshot, type JobSource } from "@/lib/bindings";
import { getUiLanguage } from "@/lib/i18n";
import { jobProgressLabel, jobTitle } from "@/lib/jobLabels";
import {
  canRetry,
  filterJobs,
  formatClock,
  instanceOf,
  type JobFilter,
  jobAmount,
  jobDiagnostics,
  jobEta,
  jobFraction,
  jobSpeed,
  stateTone,
} from "@/lib/jobs";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { EmptyState } from "../ui/EmptyState";
import { IconButton } from "../ui/IconButton";
import { ProgressBar } from "../ui/ProgressBar";
import { Select } from "../ui/Select";
import { Tabs } from "../ui/Tabs";
import { Tag } from "../ui/Tag";
import { VirtualList } from "../VirtualList";
import { JobIcon } from "./JobIcon";

type Tab = "running" | "history";
const SOURCES: JobSource[] = ["modrinth", "curseForge", "game", "java", "local", "other"];
const STATES: Exclude<JobFilter["state"], "all">[] = [
  "succeeded",
  "failed",
  "interrupted",
  "cancelled",
];
/** 启动前的准备不进下载历史（后端也不存） */
const isTransient = (job: JobSnapshot) => job.key.startsWith("launch:");

/** 用了多久：「42 秒」「3 分钟」 */
function took(job: JobSnapshot, t: TFunction) {
  if (job.finishedAt == null) return null;
  const seconds = Math.max(0, Math.round((job.finishedAt - (job.startedAt ?? 0)) / 1000));
  const text =
    seconds < 60
      ? t("downloads.seconds", { n: seconds })
      : t("downloads.minutes", { n: Math.round(seconds / 60) });
  return t("downloads.took", { duration: text });
}

/** 名字后面的小圆点 + 状态（「● 已完成」），颜色跟着状态 */
function Dot({ tone, children }: { tone: string; children: string }) {
  return (
    <span className={`flex shrink-0 items-center gap-1.5 text-xs ${tone}`}>
      <span aria-hidden className="size-1.5 rounded-full bg-current" />
      {children}
    </span>
  );
}

/**
 * 下载历史里的一条记录：
 * 第一行名字、来源标签、「● 状态」，实例删了再加「● 实例已删除」；
 * 第二行「状态 · 时间 · 目标实例：某某 · 用了多久」；
 * 进行中的有进度条、速度、还剩多久，失败的写原因。右边：重试 / 取消、打开实例、复制诊断信息、展开每一步、删除这条。
 */
function DownloadRow({
  job,
  expanded,
  onToggle,
  onRetried,
  onRemoved,
  onOpenInstance,
}: {
  job: JobSnapshot;
  expanded: boolean;
  onToggle: () => void;
  onRetried: (id: string) => void;
  onRemoved: (id: string) => void;
  onOpenInstance: (id: string) => void;
}) {
  const { t } = useTranslation();
  const cancel = useGameStore((s) => s.cancelJob);
  const retryJob = useGameStore((s) => s.retryJob);
  const instanceId = instanceOf(job);
  const instance = useGameStore((s) => s.instances.find((i) => i.id === instanceId));
  const instancesLoaded = useGameStore((s) => s.instancesLoaded);
  const settings = useAppStore((s) => s.settings);
  const version = useAppStore((s) => s.info?.version ?? null);
  const [retrying, setRetrying] = useState(false);
  const running = job.state === "running";
  const title = jobTitle(job, t);
  const fraction = jobFraction(job.progress);
  const tone = stateTone(job.state);
  const pace = running
    ? [jobAmount(job.progress, t), jobSpeed(job, t), jobEta(job, t)].filter(Boolean).join(" · ")
    : "";
  const meta = [
    t(`tasks.state.${job.state}`),
    formatClock(job.startedAt ?? 0),
    instanceId ? t("downloads.target", { name: instance?.name ?? instanceId }) : null,
    took(job, t),
  ]
    .filter(Boolean)
    .join(" · ");
  const via = job.fallbacks > 0 ? t("downloads.fallbacks", { n: job.fallbacks }) : null;

  const retry = async () => {
    setRetrying(true);
    if (await retryJob(job.id)) {
      toast.success(t("downloads.retried", { title }));
      onRetried(job.id);
    }
    setRetrying(false);
  };

  const copy = () => {
    const text = jobDiagnostics(
      job,
      { appVersion: version, instanceName: instance?.name ?? null, settings },
      t,
    );
    navigator.clipboard
      .writeText(text)
      .then(() => toast.success(t("downloads.copied")))
      .catch((error: unknown) => reportError(error, "复制诊断信息"));
  };

  const remove = async () => {
    try {
      if (await commands.jobsRemove(job.id)) onRemoved(job.id);
    } catch (error) {
      reportError(error, "删除下载记录");
    }
  };

  return (
    <div className="flex gap-3 border-line border-b py-3">
      <JobIcon job={job} />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <p className="min-w-0 truncate font-medium text-sm">{title}</p>
          <Tag>{t(`downloads.source.${job.source}`)}</Tag>
          {running ? (
            <span className="flex shrink-0 items-center gap-1 text-fg-muted text-xs">
              <LightSpinner className="size-3.5" />
              {t("tasks.state.running")}
            </span>
          ) : (
            <Dot tone={tone}>{t(`tasks.state.${job.state}`)}</Dot>
          )}
          {instanceId && instancesLoaded && !instance && (
            <Dot tone="text-fg-muted">{t("downloads.instanceGone")}</Dot>
          )}
        </div>
        <p className="truncate text-fg-muted text-xs tabular-nums">{meta}</p>
        {via && <p className="truncate text-fg-muted text-xs">{via}</p>}
        {running && (
          <div className="flex flex-col gap-1 pt-0.5">
            {fraction !== null && <ProgressBar value={fraction} label={title} />}
            <p className="flex gap-2 text-fg-muted text-xs">
              <span className="min-w-0 flex-1 truncate">
                {jobProgressLabel(job.progress.message, t) ?? t("tasks.running")}
              </span>
              {pace && <span className="shrink-0 tabular-nums">{pace}</span>}
            </p>
          </div>
        )}
        {job.state === "failed" && job.error && (
          <p className="line-clamp-2 text-danger text-xs">{job.error}</p>
        )}
        {expanded &&
          (job.steps.length === 0 ? (
            <p className="text-fg-muted text-xs">{t("downloads.noSteps")}</p>
          ) : (
            <ol className="mt-1 flex flex-col gap-1 rounded-row bg-surface-2 px-3 py-2 text-xs">
              {job.steps.map((step, index) => (
                <li key={`${step.at}-${index}`} className="flex gap-3">
                  <span className="shrink-0 text-fg-muted tabular-nums">
                    {new Date(step.at ?? 0).toLocaleTimeString(getUiLanguage())}
                  </span>
                  <span className="min-w-0 break-words">{jobProgressLabel(step.label, t)}</span>
                </li>
              ))}
            </ol>
          ))}
      </div>
      <div className="flex shrink-0 items-start gap-1.5">
        {running && (
          <Button variant="danger" size="sm" onClick={() => void cancel(job.id)}>
            <X className="size-3.5" aria-hidden />
            {t("tasks.cancel")}
          </Button>
        )}
        {canRetry(job) && (
          <Button variant="outline" size="sm" disabled={retrying} onClick={() => void retry()}>
            <RotateCw className="size-3.5" aria-hidden />
            {t("downloads.retry")}
          </Button>
        )}
        {instance && (
          <Button variant="secondary" size="sm" onClick={() => onOpenInstance(instance.id)}>
            <FolderOpen className="size-3.5" aria-hidden />
            {t("downloads.openInstance")}
          </Button>
        )}
        <IconButton label={t("downloads.copy")} size="sm" onClick={() => void copy()}>
          <Copy className="size-3.5" aria-hidden />
        </IconButton>
        <IconButton
          label={expanded ? t("downloads.collapse") : t("downloads.details")}
          size="sm"
          onClick={onToggle}
          pressed={expanded}
        >
          <ChevronDown
            className={`size-4 transition-transform duration-(--motion-toggle) ${expanded ? "rotate-180" : ""}`}
            aria-hidden
          />
        </IconButton>
        {!running && (
          <IconButton label={t("downloads.remove")} size="sm" danger onClick={() => void remove()}>
            <Trash2 className="size-3.5" aria-hidden />
          </IconButton>
        )}
      </div>
    </div>
  );
}

/**
 * 下载历史：标题栏「下载」面板底下点开。两个页签「正在进行 / 历史记录」，
 * 下面一排搜索、所有来源、所有状态，最右边「清空历史」；历史关掉启动器再打开还在（后端存在应用数据目录）。
 * 条目可能有两百条，用虚拟滚动；展开的那条高度会自己量。
 */
export function DownloadHistory() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const open = useGameStore((s) => s.historyOpen);
  const setOpen = useGameStore((s) => s.setHistoryOpen);
  const live = useGameStore((s) => s.jobs);
  const instances = useGameStore((s) => s.instances);
  const [past, setPast] = useState<JobSnapshot[]>([]);
  const [tab, setTab] = useState<Tab>("running");
  const [filter, setFilter] = useState<JobFilter>({ query: "", source: "all", state: "all" });
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    commands
      .jobsHistory()
      .then((list) => {
        if (cancelled) return;
        setPast(list);
        // 有正在进行的先看它们，没有就看历史
        setTab(list.some((j) => j.state === "running") ? "running" : "history");
      })
      .catch((error: unknown) => reportError(error, "读取下载历史"));
    return () => {
      cancelled = true;
    };
  }, [open]);

  // 读到的历史 + 这次打开后的实时变化（同一个任务以实时的为准）
  const all = useMemo(() => {
    const byId = new Map(past.map((job) => [job.id, job]));
    for (const job of Object.values(live)) {
      if (!isTransient(job)) byId.set(job.id, job);
    }
    return [...byId.values()].sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0));
  }, [past, live]);
  const nameOf = (id: string) => instances.find((i) => i.id === id)?.name;
  const runningJobs = all.filter((j) => j.state === "running");
  const finished = all.filter((j) => j.state !== "running");
  const shown =
    tab === "running"
      ? filterJobs(runningJobs, { ...filter, state: "all" }, nameOf, t)
      : filterJobs(finished, filter, nameOf, t);

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  /** 从列表里拿掉一条（重试后被新任务接替、或者删掉了） */
  const drop = (id: string) => {
    setPast((prev) => prev.filter((j) => j.id !== id));
    useGameStore.setState((s) => {
      const { [id]: _, ...jobs } = s.jobs;
      return { jobs };
    });
  };

  const clear = async () => {
    try {
      await commands.jobsClearHistory();
      setPast((prev) => prev.filter((j) => j.state === "running"));
      useGameStore.setState((s) => ({
        jobs: Object.fromEntries(Object.entries(s.jobs).filter(([, j]) => j.state === "running")),
      }));
      toast.success(t("downloads.cleared"));
    } catch (error) {
      reportError(error, "清空下载历史");
    }
  };

  const openInstance = (id: string) => {
    setOpen(false);
    navigate(`/instances/${encodeURIComponent(id)}`);
  };

  const records = tab === "running" ? runningJobs : finished;
  const empty =
    records.length > 0 || filter.query.trim()
      ? { icon: SearchX, title: t("downloads.noMatch"), hint: t("downloads.noMatchHint") }
      : tab === "running"
        ? {
            icon: Download,
            title: t("tasks.empty"),
            hint: t("tasks.emptyHint"),
            illustration: <DownloadRestArt className="h-24 w-36" />,
          }
        : {
            icon: History,
            title: t("downloads.emptyHistory"),
            hint: t("downloads.emptyHistoryHint"),
          };

  return (
    <Dialog open={open} onClose={() => setOpen(false)} wider title={t("downloads.title")}>
      <div className="flex flex-col gap-3">
        <Tabs<Tab>
          label={t("downloads.tabsLabel")}
          value={tab}
          onChange={setTab}
          options={[
            {
              value: "running",
              icon: Download,
              label: `${t("downloads.tabs.running")} ${runningJobs.length}`,
            },
            { value: "history", icon: History, label: t("downloads.tabs.history") },
          ]}
        />
        <div className="flex items-center gap-2">
          <div className="relative min-w-40 flex-1">
            <Search
              className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3.5 size-4 text-fg-muted"
              aria-hidden
            />
            <input
              value={filter.query}
              onChange={(event) => setFilter((f) => ({ ...f, query: event.target.value }))}
              placeholder={t("downloads.search")}
              aria-label={t("downloads.search")}
              className="h-9 w-full rounded-control border border-line bg-surface pr-4 pl-10 text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus:border-accent"
            />
          </div>
          <div className="w-36 shrink-0">
            <Select
              label={t("downloads.sourceFilter")}
              value={filter.source}
              onChange={(source) =>
                setFilter((f) => ({ ...f, source: source as JobFilter["source"] }))
              }
              options={[
                { value: "all", label: t("downloads.allSources") },
                ...SOURCES.map((s) => ({ value: s, label: t(`downloads.source.${s}`) })),
              ]}
            />
          </div>
          {tab === "history" && (
            <>
              <div className="w-32 shrink-0">
                <Select
                  label={t("downloads.stateFilter")}
                  value={filter.state}
                  onChange={(state) =>
                    setFilter((f) => ({ ...f, state: state as JobFilter["state"] }))
                  }
                  options={[
                    { value: "all", label: t("downloads.allStates") },
                    ...STATES.map((s) => ({ value: s, label: t(`tasks.state.${s}`) })),
                  ]}
                />
              </div>
              <Button
                variant="secondary"
                disabled={finished.length === 0}
                onClick={() => void clear()}
              >
                <Trash2 className="size-4" aria-hidden />
                {t("downloads.clear")}
              </Button>
            </>
          )}
        </div>
        {shown.length === 0 ? (
          <EmptyState
            icon={empty.icon}
            illustration={empty.illustration}
            title={empty.title}
            hint={empty.hint}
          />
        ) : (
          <VirtualList
            items={shown}
            estimate={96}
            getKey={(job) => job.id}
            className="h-[min(56vh,560px)]"
            render={(job) => (
              <DownloadRow
                job={job}
                expanded={expanded.has(job.id)}
                onToggle={() => toggle(job.id)}
                onRetried={drop}
                onRemoved={drop}
                onOpenInstance={openInstance}
              />
            )}
          />
        )}
      </div>
    </Dialog>
  );
}
