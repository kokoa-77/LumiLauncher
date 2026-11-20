import { Check, ChevronDown, Download, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type JavaEntry } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { byMajor, Choice, KNOWN_MAJORS } from "../settings/JavaSettings";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Truncated } from "../ui/HoverCard";
import { Tag } from "../ui/Tag";

/**
 * 实例设置里「用哪个 Java」的框（D17i 第三轮验收时改）：占满一行，显示这个实例现在会用的 Java——
 * 版本、厂商、路径；跟着全局时就是自动挑出来的那个。整个框点哪里都打开「选择 Java」。
 */
export function JavaField({
  entry,
  required,
  onOpen,
}: {
  /** 现在会用的 Java；电脑上一个合适的都没有时为空 */
  entry: JavaEntry | null;
  required: number | null;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      onClick={onOpen}
      aria-haspopup="dialog"
      className="flex h-9 w-full min-w-0 items-center gap-2.5 rounded-control border border-line bg-surface px-4 text-left text-sm shadow-raised transition-colors duration-(--motion-press) hover:bg-surface-hover"
    >
      {entry ? (
        <>
          <span className="shrink-0 font-medium tabular-nums">{entry.version}</span>
          <span className="shrink-0 text-fg-muted text-xs">
            {entry.vendor ?? t("java.list.unknownVendor")}
          </span>
          <Truncated
            text={entry.location}
            className="min-w-0 flex-1 truncate font-mono text-fg-muted text-xs"
          />
        </>
      ) : (
        <span className="min-w-0 flex-1 truncate text-fg-muted">
          {required === null
            ? t("instanceSettings.java.noneFound")
            : t("instanceSettings.java.noneFoundMajor", { major: required })}
        </span>
      )}
      <ChevronDown className="size-4 shrink-0 text-fg-muted" aria-hidden />
    </button>
  );
}

/**
 * 「选择 Java」：和全局「Java」一栏长一个样子，按大版本分组。最上面「自动挑选（推荐）」= 跟着全局；
 * 这个版本要的那组标「适合这个版本」，还没有启动器下载的那份时能「安装推荐版本」（装好直接选上）；
 * 底部「添加 Java」选一个 java.exe（自动认版本），加好直接选上。点哪一行就用哪个。
 */
export function JavaPickerDialog({
  open,
  onClose,
  entries,
  autoEntry,
  chosenId,
  required,
  onPick,
  onEntries,
}: {
  open: boolean;
  onClose: () => void;
  entries: JavaEntry[] | null;
  /** 跟着全局时会用的那个 */
  autoEntry: JavaEntry | null;
  /** 实例选的；跟着全局是 null */
  chosenId: string | null;
  required: number | null;
  /** 选了一个（null = 改回跟着全局） */
  onPick: (id: string | null) => void;
  /** 添加、下载后列表变了 */
  onEntries: (entries: JavaEntry[]) => void;
}) {
  const { t } = useTranslation();
  const jobs = useGameStore((s) => s.jobs);
  const [busy, setBusy] = useState(false);
  // 正在「安装推荐版本」的任务
  const [installJob, setInstallJob] = useState<string | null>(null);

  const pick = (id: string | null) => {
    onPick(id);
    onClose();
  };
  // 下载完的那一下要用最新的 pick / onEntries（它们每次渲染都是新的，不放进下面的依赖里）
  const latest = useRef({ pick, onEntries, required });
  latest.current = { pick, onEntries, required };

  // 推荐版本下载完：刷新列表，选上刚下载的那份
  useEffect(() => {
    if (!installJob) return;
    const job = jobs[installJob];
    if (!job || job.state === "running") return;
    setInstallJob(null);
    if (job.state !== "succeeded") {
      if (job.state === "failed") toast.error(job.error ?? t("java.list.installFailed"));
      return;
    }
    const { required: major } = latest.current;
    toast.success(t("java.list.installed", { major }));
    commands
      .javaList()
      .then(unwrap)
      .then((overview) => {
        latest.current.onEntries(overview.entries);
        const downloaded = overview.entries.find(
          (e) => e.major === major && e.origin === "downloaded",
        );
        if (downloaded) latest.current.pick(downloaded.id);
      })
      .catch((error: unknown) => reportError(error, "刷新 Java 列表"));
  }, [jobs, installJob, t]);

  const install = async (major: number) => {
    try {
      setInstallJob(unwrap(await commands.javaInstall(major)));
    } catch (error) {
      reportError(error, "安装推荐的 Java");
    }
  };

  const add = async () => {
    setBusy(true);
    try {
      const before = new Set((entries ?? []).map((e) => e.id));
      const overview = unwrap(await commands.javaAdd());
      if (!overview) return;
      onEntries(overview.entries);
      const added = overview.entries.find((e) => !before.has(e.id));
      if (added) pick(added.id);
    } catch (error) {
      reportError(error, "添加 Java");
    } finally {
      setBusy(false);
    }
  };

  const progress = installJob ? jobs[installJob]?.progress : undefined;
  const pct =
    progress?.total && progress.done !== null
      ? Math.min(99, Math.floor((progress.done / progress.total) * 100))
      : null;
  const groups = entries ? byMajor(entries) : [];

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("instanceSettings.java.pickTitle")}
      wide
      footer={
        <>
          <Button
            variant="secondary"
            className="mr-auto"
            disabled={busy}
            onClick={() => void add()}
          >
            <Plus className="size-4" aria-hidden />
            {t("java.list.add")}
          </Button>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
        </>
      }
    >
      <div
        role="radiogroup"
        aria-label={t("instanceSettings.java.pickTitle")}
        className="flex flex-col gap-3"
      >
        <div className="rounded-panel border border-line bg-surface p-2">
          <Choice
            selected={chosenId === null}
            disabled={false}
            onSelect={() => pick(null)}
            title={t("instanceSettings.java.auto")}
            detail={
              autoEntry
                ? t("java.list.autoDetail", {
                    version: autoEntry.version,
                    vendor: autoEntry.vendor ?? t("java.list.unknownVendor"),
                  })
                : required === null
                  ? undefined
                  : t("instanceSettings.java.noneFoundMajor", { major: required })
            }
          />
        </div>
        {!entries && <p className="px-1 text-fg-muted text-sm">{t("java.list.loading")}</p>}
        {groups.length > 0 && (
          <div className="divide-y divide-line rounded-panel border border-line bg-surface">
            {groups.map(([major, list]) => {
              const fits = major === required;
              const hasDownloaded = list.some((e) => e.origin === "downloaded");
              return (
                <section key={major} className="flex flex-col gap-1 px-2 pt-3 pb-2">
                  <div className="flex min-h-8 items-center gap-2 px-2 pb-1">
                    <h4 className="flex min-w-0 flex-1 items-center gap-2">
                      <span className="font-medium">{t("java.list.major", { major })}</span>
                      {fits ? (
                        <Tag tone="accent" className="gap-1">
                          <Check className="size-3" aria-hidden />
                          {t("instanceSettings.java.fits")}
                        </Tag>
                      ) : (
                        KNOWN_MAJORS.has(major) && (
                          <span className="truncate text-fg-muted text-xs">
                            {t(`java.uses.${major}`)}
                          </span>
                        )
                      )}
                    </h4>
                    {fits && !hasDownloaded && (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={installJob !== null}
                        onClick={() => void install(major)}
                      >
                        <Download className="size-3.5" aria-hidden />
                        {installJob
                          ? pct === null
                            ? t("java.list.installing")
                            : t("java.list.installingPct", { pct })
                          : t("java.list.install")}
                      </Button>
                    )}
                  </div>
                  {list.length === 0 ? (
                    <p className="px-2 pb-1 text-fg-muted text-xs">
                      {t("java.list.none", { major })}
                    </p>
                  ) : (
                    list.map((entry) => (
                      <Choice
                        key={entry.id}
                        selected={entry.id === chosenId}
                        disabled={false}
                        onSelect={() => pick(entry.id)}
                        title={entry.version}
                        vendor={entry.vendor}
                        detail={entry.location}
                        tags={
                          <>
                            <Tag>{t(`java.origin.${entry.origin}`)}</Tag>
                            {!entry.is64bit && <Tag tone="warning">{t("java.list.bit32")}</Tag>}
                          </>
                        }
                      />
                    ))
                  )}
                </section>
              );
            })}
          </div>
        )}
      </div>
    </Dialog>
  );
}
