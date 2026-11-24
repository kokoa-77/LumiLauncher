// 下载中心用的小工具：任务属于哪个实例、做完能不能直接开始游戏、进度、速度、剩余时间怎么写，
// 下载历史怎么筛，诊断信息怎么拼。
import type { TFunction } from "i18next";
import type { JobProgress, JobSnapshot, JobSource, JobState, Settings } from "./bindings";
import { formatDateTime, formatSize } from "./format";
import { getUiLanguage } from "./i18n";
import { jobProgressLabel, jobTitle } from "./jobLabels";

/** 键名里第二段是实例 ID 的任务（`content:<实例>:…`、`drop:<实例>:…`…） */
const WITH_INSTANCE = new Set(["content", "drop", "map-install", "launch"]);
/** 做完以后能直接「开始游戏」的任务（装了东西进实例） */
const PLAYABLE_AFTER = new Set(["content", "drop", "map-install"]);

/** 任务属于哪个实例（按键名认）；看不出来是 null */
export function jobInstance(key: string): string | null {
  const [kind, id] = key.split(":");
  return kind && id && WITH_INSTANCE.has(kind) ? id : null;
}

/** 任务属于哪个实例：后端记下的优先（新建实例、导入整合包做完才知道），没有再按键名认 */
export function instanceOf(job: Pick<JobSnapshot, "key" | "instanceId">): string | null {
  return job.instanceId ?? jobInstance(job.key);
}

/** 做完以后能不能直接「开始游戏」 */
export function playableAfter(key: string): boolean {
  return PLAYABLE_AFTER.has(key.split(":")[0] ?? "") && jobInstance(key) !== null;
}

/** 失败、取消或者启动器关掉时没做完（已中断），而且后端记了怎么重来 */
export function canRetry(job: Pick<JobSnapshot, "state" | "retryable">): boolean {
  return (
    job.retryable &&
    (job.state === "failed" || job.state === "cancelled" || job.state === "interrupted")
  );
}

/** 状态的颜色（docs/DESIGN.md 第 5 节的颜色角色）：做完了是主色，失败是红色，取消、中断、进行中是灰色 */
export function stateTone(state: JobState): "text-accent" | "text-danger" | "text-fg-muted" {
  if (state === "succeeded") return "text-accent";
  if (state === "failed") return "text-danger";
  return "text-fg-muted";
}

/** 0–1；总量不知道时是 null */
export function jobFraction(progress: JobProgress): number | null {
  const { done, total } = progress;
  if (!total || total <= 0) return null;
  return Math.min(1, Math.max(0, (done ?? 0) / total));
}

/** 「3 / 5 个」「120.4 MB / 380.0 MB」；总量不知道时是 null */
export function jobAmount(progress: JobProgress, t: TFunction): string | null {
  const { done, total, unit } = progress;
  if (!total) return null;
  if (unit === "bytes") return `${formatSize(done ?? 0)} / ${formatSize(total)}`;
  return t("tasks.items", { done: done ?? 0, total });
}

/** 进行中的任务优先留在面板里，已经结束的按时间补齐。 */
export function recentDownloadJobs(jobs: JobSnapshot[], limit: number) {
  const ordered = [...jobs].sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0));
  const running = ordered.filter((job) => job.state === "running");
  const finished = ordered.filter((job) => job.state !== "running");
  return { running, visible: [...running, ...finished].slice(0, limit) };
}

/** 正在跑的任务合起来的进度（只算知道总量的）；一个都不知道时是 null */
export function overallFraction(jobs: JobSnapshot[]): number | null {
  const known = jobs.map((j) => jobFraction(j.progress)).filter((f): f is number => f !== null);
  if (known.length === 0) return null;
  return known.reduce((sum, f) => sum + f, 0) / known.length;
}

/** 「2.3 MB/s」「12 个/秒」；还没算出速度时是 null */
export function jobSpeed(
  job: Pick<JobSnapshot, "speed" | "progress">,
  t: TFunction,
): string | null {
  const { speed } = job;
  if (speed == null) return null;
  if (job.progress.unit === "bytes") return t("downloads.speed", { size: formatSize(speed) });
  return t("downloads.itemSpeed", { n: speed < 10 ? speed.toFixed(1) : Math.round(speed) });
}

/** 「还剩约 3 分钟」；不知道时是 null */
export function jobEta(job: Pick<JobSnapshot, "eta">, t: TFunction): string | null {
  const { eta } = job;
  if (eta == null || !Number.isFinite(eta)) return null;
  if (eta < 5) return t("downloads.eta.almost");
  if (eta < 60) return t("downloads.eta.seconds", { n: Math.ceil(eta / 5) * 5 });
  const minutes = Math.round(eta / 60);
  if (minutes < 60) return t("downloads.eta.minutes", { n: minutes });
  const hours = Math.floor(minutes / 60);
  return t("downloads.eta.hours", { h: hours, m: minutes % 60 });
}

/** 「11月15日 14:03」 / “Nov 15, 14:03” */
export function formatClock(ms: number): string {
  return formatDateTime(ms);
}

/** 下载历史的筛选条件 */
export interface JobFilter {
  query: string;
  source: JobSource | "all";
  state: Exclude<JobState, "running"> | "all";
}

/**
 * 按关键词、来源、状态筛。关键词比对名字、失败原因、实例名、做过的步骤（不分大小写）。
 * `instanceName` 把实例 ID 换成名字（实例删了就用 ID）。
 */
export function filterJobs(
  jobs: JobSnapshot[],
  filter: JobFilter,
  instanceName: (id: string) => string | undefined,
  t?: TFunction,
): JobSnapshot[] {
  const query = filter.query.trim().toLowerCase();
  return jobs.filter((job) => {
    if (filter.source !== "all" && job.source !== filter.source) return false;
    if (filter.state !== "all" && job.state !== filter.state) return false;
    if (!query) return true;
    const instance = instanceOf(job);
    const haystack = [
      job.title,
      job.error ?? "",
      instance ? (instanceName(instance) ?? instance) : "",
      ...job.steps.map((s) => s.label),
      ...(t ? [jobTitle(job, t), ...job.steps.map((s) => jobProgressLabel(s.label, t))] : []),
    ]
      .join("\n")
      .toLowerCase();
    return haystack.includes(query);
  });
}

/** 拼诊断信息用的环境 */
export interface DiagnosticsContext {
  appVersion: string | null;
  instanceName: string | null;
  settings: Settings | null;
}

/**
 * 「复制诊断信息」的文字：任务本身、每一步、当时的下载设置。给人看也给我们查问题，不含路径和密钥
 * （界面本来就拿不到这些）。
 */
export function jobDiagnostics(job: JobSnapshot, ctx: DiagnosticsContext, t: TFunction): string {
  const d = (key: string, values: Record<string, unknown> = {}) =>
    t(`downloads.diag.${key}`, values);
  const lines = [
    d("title", { title: job.title }),
    d("state", {
      state: t(`tasks.state.${job.state}`),
      error: job.error ? `（${job.error}）` : "",
    }),
    d("source", { source: t(`downloads.source.${job.source}`) }),
  ];
  const instance = instanceOf(job);
  if (instance) lines.push(d("instance", { name: ctx.instanceName ?? instance }));
  const startedAt = job.startedAt ?? 0;
  const language = getUiLanguage();
  lines.push(d("started", { time: new Date(startedAt).toLocaleString(language) }));
  if (job.finishedAt != null) {
    const seconds = Math.max(0, Math.round((job.finishedAt - startedAt) / 1000));
    lines.push(
      d("finished", {
        time: new Date(job.finishedAt).toLocaleString(language),
        seconds,
      }),
    );
  }
  if (job.fallbacks > 0) {
    lines.push(d("via", { n: job.fallbacks }));
  }
  const amount = jobAmount(job.progress, t);
  if (amount) lines.push(d("progress", { amount }));
  if (job.progress.message) {
    lines.push(
      d("message", { message: jobProgressLabel(job.progress.message, t) ?? job.progress.message }),
    );
  }
  if (job.steps.length > 0) {
    lines.push(d("steps"));
    for (const step of job.steps) {
      lines.push(`  ${new Date(step.at ?? 0).toLocaleTimeString(language)}  ${step.label}`);
    }
  }
  lines.push(d("id", { id: job.id, key: job.key }));
  if (ctx.appVersion) lines.push(d("version", { version: ctx.appVersion }));
  if (ctx.settings) {
    const download = ctx.settings.download;
    lines.push(
      d("downloadSettings", {
        proxy: t(`settings.proxyMode.${download.proxy.mode}`),
        concurrency: download.maxConcurrency,
        doh: download.dohEnabled ? t("downloads.diag.on") : t("downloads.diag.off"),
      }),
    );
  }
  return lines.join("\n");
}
