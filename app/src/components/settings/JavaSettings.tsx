import { Check, Download, Plus, RefreshCw, Trash2, X } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { type CommandResult, unwrap } from "@/lib/api";
import { commands, type JavaEntry, type JavaOverview, type Settings } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Truncated } from "../ui/HoverCard";
import { IconButton } from "../ui/IconButton";
import { Tag } from "../ui/Tag";
import { EnvVarsEditor } from "./EnvVars";
import { JavaArgsRows } from "./JavaArgs";
import { Group, type UpdateSettings } from "./parts";

/** 写了「哪些游戏版本用它」的 Java 大版本 */
export const KNOWN_MAJORS = new Set([8, 16, 17, 21, 25]);
/** 游戏要用的几个大版本：电脑上没有也列出来，能「安装推荐版本」 */
const RECOMMENDED_MAJORS = [25, 21, 17, 8];

/** 按大版本分组，从新到旧；游戏要用的几个大版本没有也留一组（空的） */
export function byMajor(entries: JavaEntry[]): [number, JavaEntry[]][] {
  const groups = new Map<number, JavaEntry[]>(RECOMMENDED_MAJORS.map((major) => [major, []]));
  for (const entry of entries) {
    groups.set(entry.major, [...(groups.get(entry.major) ?? []), entry]);
  }
  return [...groups.entries()].sort(([a], [b]) => b - a);
}

/**
 * 设置 →「Java」（D17h）：找到的、下载的、手动添加的 Java，按大版本分组，每组选一个默认（默认「自动挑选」）；
 * 下面是启动参数（Java 参数 + 参数预设、游戏参数）和环境变量（D17i 验收时改）。
 * 界面只拿 Java 的 ID，路径只用来显示。
 */
export function JavaSettings({ settings, update }: { settings: Settings; update: UpdateSettings }) {
  const { t } = useTranslation();
  const [overview, setOverview] = useState<JavaOverview | null>(null);
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState<JavaEntry | null>(null);
  // 正在「安装推荐版本」的大版本 → 任务 ID（进度从任务中心的数据里读）
  const [installing, setInstalling] = useState<Record<number, string>>({});
  const jobs = useGameStore((s) => s.jobs);

  const run = useCallback(
    async (action: () => Promise<CommandResult<JavaOverview | null>>, context: string) => {
      setBusy(true);
      try {
        const next = unwrap(await action());
        if (next) setOverview(next);
        return true;
      } catch (error) {
        reportError(error, context);
        return false;
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  useEffect(() => {
    void run(() => commands.javaList(), "读取 Java 列表");
  }, [run]);

  // 下载任务结束：提示一下、刷新列表
  useEffect(() => {
    const finished = Object.entries(installing).filter(([, id]) => {
      const job = jobs[id];
      return job !== undefined && job.state !== "running";
    });
    if (finished.length === 0) return;
    setInstalling((current) => {
      const next = { ...current };
      for (const [major] of finished) delete next[Number(major)];
      return next;
    });
    for (const [major, id] of finished) {
      const job = jobs[id];
      if (job?.state === "succeeded") toast.success(t("java.list.installed", { major }));
      else if (job?.state === "failed") toast.error(job.error ?? t("java.list.installFailed"));
    }
    void run(() => commands.javaList(), "刷新 Java 列表");
  }, [jobs, installing, run, t]);

  const install = async (major: number) => {
    try {
      const id = unwrap(await commands.javaInstall(major));
      setInstalling((current) => ({ ...current, [major]: id }));
    } catch (error) {
      reportError(error, "安装推荐的 Java");
    }
  };

  /** 下载进度（0–100）；还不知道总量时为 null */
  const percent = (major: number) => {
    const progress = jobs[installing[major] ?? ""]?.progress;
    if (!progress?.total || progress.done === null) return null;
    return Math.min(99, Math.floor((progress.done / progress.total) * 100));
  };

  const groups = overview ? byMajor(overview.entries) : [];

  return (
    <>
      {overview?.systemIgnored && (
        <p className="rounded-row bg-warning-soft px-3 py-2 text-sm text-warning">
          {t("java.list.systemIgnored")}
        </p>
      )}
      <Group
        title={t("java.list.title")}
        action={
          <>
            <IconButton
              label={t("java.list.rescan")}
              size="sm"
              disabled={busy}
              onClick={() => void run(() => commands.javaList(), "重新扫描 Java")}
            >
              <RefreshCw className={`size-4 ${busy ? "animate-spin" : ""}`} aria-hidden />
            </IconButton>
            <Button
              size="sm"
              variant="secondary"
              disabled={busy}
              onClick={() => void run(() => commands.javaAdd(), "添加 Java")}
            >
              <Plus className="size-3.5" aria-hidden />
              {t("java.list.add")}
            </Button>
          </>
        }
      >
        {!overview && <p className="px-4 py-6 text-fg-muted text-sm">{t("java.list.loading")}</p>}
        {groups.map(([major, entries]) => {
          const chosen = entries.some((e) => e.chosen);
          const auto = entries.find((e) => e.auto);
          const hasRecommended = entries.some((e) => e.origin === "downloaded");
          const downloading = installing[major] !== undefined;
          const pct = percent(major);
          return (
            <section key={major} className="flex flex-col gap-1 px-2 pt-3 pb-2">
              <div className="flex min-h-8 items-center gap-2 px-2 pb-1">
                <h4 className="flex min-w-0 flex-1 items-baseline gap-2">
                  <span className="font-medium">{t("java.list.major", { major })}</span>
                  {KNOWN_MAJORS.has(major) && (
                    <span className="truncate text-fg-muted text-xs">
                      {t(`java.uses.${major}`)}
                    </span>
                  )}
                </h4>
                {RECOMMENDED_MAJORS.includes(major) && !hasRecommended && (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={downloading || !overview}
                    onClick={() => void install(major)}
                  >
                    <Download className="size-3.5" aria-hidden />
                    {downloading
                      ? pct === null
                        ? t("java.list.installing")
                        : t("java.list.installingPct", { pct })
                      : t("java.list.install")}
                  </Button>
                )}
              </div>
              {entries.length === 0 ? (
                <p className="px-2 pb-1 text-fg-muted text-xs">{t("java.list.none", { major })}</p>
              ) : (
                <div role="radiogroup" aria-label={t("java.list.major", { major })}>
                  <Choice
                    selected={!chosen}
                    disabled={busy}
                    onSelect={() =>
                      void run(() => commands.javaSetDefault(major, null), "选择默认的 Java")
                    }
                    title={t("java.list.auto")}
                    detail={
                      auto
                        ? t("java.list.autoDetail", {
                            version: auto.version,
                            vendor: auto.vendor ?? t("java.list.unknownVendor"),
                          })
                        : undefined
                    }
                  />
                  {entries.map((entry) => (
                    <Choice
                      key={entry.id}
                      selected={entry.chosen}
                      disabled={busy}
                      onSelect={() =>
                        void run(() => commands.javaSetDefault(major, entry.id), "选择默认的 Java")
                      }
                      title={entry.version}
                      vendor={entry.vendor}
                      detail={entry.location}
                      tags={
                        <>
                          <Tag>{t(`java.origin.${entry.origin}`)}</Tag>
                          {!entry.is64bit && <Tag tone="warning">{t("java.list.bit32")}</Tag>}
                        </>
                      }
                      action={
                        entry.origin === "downloaded" ? (
                          <IconButton
                            label={t("java.list.delete")}
                            size="sm"
                            danger
                            disabled={busy}
                            onClick={() => setDeleting(entry)}
                          >
                            <Trash2 className="size-4" aria-hidden />
                          </IconButton>
                        ) : entry.origin === "manual" ? (
                          <IconButton
                            label={t("java.list.remove")}
                            size="sm"
                            disabled={busy}
                            onClick={() =>
                              void run(() => commands.javaRemove(entry.id), "移除手动添加的 Java")
                            }
                          >
                            <X className="size-4" aria-hidden />
                          </IconButton>
                        ) : null
                      }
                    />
                  ))}
                </div>
              )}
            </section>
          );
        })}
      </Group>
      <p className="-mt-3 px-1 text-fg-muted text-xs">{t("java.list.note")}</p>

      <Group title={t("java.args.group")}>
        <JavaArgsRows
          value={{
            gcPreset: settings.game.gcPreset,
            jvmArgs: settings.game.jvmArgs,
            gameArgs: settings.game.gameArgs,
          }}
          onChange={({ gcPreset, jvmArgs, gameArgs }) =>
            update((s) => ({ ...s, game: { ...s.game, gcPreset, jvmArgs, gameArgs } }))
          }
        />
      </Group>

      <Group title={t("env.group")}>
        <EnvVarsEditor
          value={settings.game.envVars}
          onChange={(envVars) => update((s) => ({ ...s, game: { ...s.game, envVars } }))}
        />
      </Group>

      <Dialog
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        title={t("java.delete.title", { version: deleting?.version ?? "" })}
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleting(null)}>
              {t("java.delete.cancel")}
            </Button>
            <Button
              variant="danger"
              disabled={busy}
              onClick={() => {
                const id = deleting?.id;
                if (!id) return;
                void run(() => commands.javaDelete(id), "删除下载的 Java").then((ok) => {
                  if (ok) setDeleting(null);
                });
              }}
            >
              {t("java.delete.confirm")}
            </Button>
          </>
        }
      >
        <p className="text-fg-muted text-sm">{t("java.delete.body")}</p>
      </Dialog>
    </>
  );
}

/** 列表里的一项：整行能点（选它），选中的是主色浅底 + 主色字 + 勾；右边可以有自己的按钮（实例的「选择 Java」也用） */
export function Choice({
  selected,
  disabled,
  onSelect,
  title,
  vendor,
  detail,
  tags,
  action,
}: {
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
  title: string;
  vendor?: string | null;
  detail?: string;
  tags?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <button
        type="button"
        role="radio"
        aria-checked={selected}
        disabled={disabled && !selected}
        onClick={() => {
          if (!selected) onSelect();
        }}
        className={`flex min-w-0 flex-1 items-center gap-3 rounded-row px-2 py-2 text-left transition-colors duration-(--motion-press) ${
          selected ? "bg-accent-soft text-accent" : "hover:bg-surface-hover"
        }`}
      >
        <span className="grid size-4 shrink-0 place-items-center" aria-hidden>
          {selected && <Check className="size-4" />}
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="font-medium text-sm tabular-nums">{title}</span>
            {vendor && (
              <span className={`text-xs ${selected ? "" : "text-fg-muted"}`}>{vendor}</span>
            )}
            {tags}
          </span>
          {detail && (
            <Truncated
              className={`mt-0.5 block truncate text-xs ${selected ? "" : "text-fg-muted"}`}
              text={detail}
            />
          )}
        </span>
      </button>
      {action && <div className="shrink-0 pr-1">{action}</div>}
    </div>
  );
}
