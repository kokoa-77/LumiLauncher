import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Check,
  CircleAlert,
  Copy,
  ExternalLink,
  FileArchive,
  FolderOpen,
  Info,
  ScrollText,
  Search,
  Upload,
} from "lucide-react";
import { useDeferredValue, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";
import { toast } from "sonner";
import { LogLines, NO_LINES } from "@/components/LogDrawer";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { EmptyState } from "@/components/ui/EmptyState";
import { TextInput } from "@/components/ui/Field";
import { HoverHint } from "@/components/ui/HoverCard";
import { IconButton } from "@/components/ui/IconButton";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { SectionTitle } from "@/components/ui/SectionTitle";
import { Segmented } from "@/components/ui/Segmented";
import { Tag } from "@/components/ui/Tag";
import { VirtualList } from "@/components/VirtualList";
import { unwrap } from "@/lib/api";
import {
  commands,
  type InstanceSummary,
  type LaunchRecord,
  type LogLine,
  type MclogsUpload,
  type RecordLog,
} from "@/lib/bindings";
import { formatDuration } from "@/lib/format";
import {
  countLevels,
  filterLines,
  LEVEL_GROUPS,
  type LevelFilter,
  type LevelGroup,
  lineText,
  memoryText,
  outcomeTone,
  recordSeconds,
  recordTime,
} from "@/lib/records";
import { reportError } from "@/lib/report";
import { isActive, useGameStore } from "@/stores/game";
import { CrashAdvice, type RecoveryActions } from "./CrashAdvice";

type View = "output" | "crash" | "mods";

/**
 * 「日志」页签（D17c）：这个实例最近 20 次启动，每一次都有完整的游戏输出、用的 Java 和内存、
 * 当时装着的模组、怎么结束的、崩溃报告。左边挑哪一次，右边看那一次；正在运行的那一次实时滚动。
 */
export function LogTab({
  instance,
  recovery,
}: {
  instance: InstanceSummary;
  recovery: RecoveryActions;
}) {
  const { t } = useTranslation();
  const [records, setRecords] = useState<LaunchRecord[] | null>(null);
  // 从崩溃提示条的「查看日志」过来时，地址里带着是哪一次
  const [params] = useSearchParams();
  const wanted = params.get("record");
  const [chosen, setChosen] = useState<string | null>(wanted);
  useEffect(() => {
    if (wanted) setChosen(wanted);
  }, [wanted]);
  const version = useGameStore((s) => s.recordsVersion[instance.id] ?? 0);
  const session = useGameStore((s) => s.sessions[instance.id]);
  const liveId = isActive(session) ? (session?.recordId ?? null) : null;

  // 打开时、写完一次记录时、开始新的一次时重新读
  useEffect(() => {
    void version;
    let cancelled = false;
    commands
      .launchRecords(instance.id)
      .then(unwrap)
      .then((list) => !cancelled && setRecords(list))
      .catch((error: unknown) => {
        if (cancelled) return;
        setRecords([]);
        reportError(error, "读取启动记录");
      });
    return () => {
      cancelled = true;
    };
  }, [instance.id, version, liveId]);

  // 开始了新的一次：跳到它
  useEffect(() => {
    if (liveId) setChosen(liveId);
  }, [liveId]);

  if (records === null) return null;
  if (records.length === 0) return <LiveOnly instance={instance} />;
  const selected = records.find((r) => r.id === chosen) ?? records[0];
  if (!selected) return null;

  return (
    <div className="flex h-full min-h-0 gap-4">
      <aside className="flex w-60 shrink-0 flex-col overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
        <SectionTitle
          title={t("detail.logTab.recent")}
          count={records.length}
          className="shrink-0 border-line border-b px-4 py-3"
        />
        <div className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto p-1.5">
          {records.map((record) => (
            <RecordRow
              key={record.id}
              record={record}
              active={record.id === selected.id}
              onClick={() => setChosen(record.id)}
            />
          ))}
        </div>
      </aside>
      <RecordDetail
        key={selected.id}
        instance={instance}
        record={selected}
        live={selected.id === liveId}
        recovery={recovery}
      />
    </div>
  );
}

function RecordRow({
  record,
  active,
  onClick,
}: {
  record: LaunchRecord;
  active: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const seconds = recordSeconds(record);
  const detail =
    record.outcome === "failed"
      ? (record.error ?? t("detail.logTab.outcome.failed"))
      : record.outcome === "running"
        ? t("detail.logTab.playingFor", { time: formatDuration(seconds, t) })
        : [
            t("detail.logTab.played", { time: formatDuration(seconds, t) }),
            record.reachedWorld ? t("detail.logTab.reachedWorld") : null,
          ]
            .filter(Boolean)
            .join(" · ");
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={`flex w-full flex-col gap-1 rounded-row px-3 py-2.5 text-left transition-colors duration-(--motion-press) ${
        active ? "bg-accent-soft" : "hover:bg-surface-hover"
      }`}
    >
      <span className="flex items-center gap-2">
        <span
          className={`min-w-0 flex-1 truncate font-medium text-sm ${active ? "text-accent" : "text-fg"}`}
        >
          {recordTime(record.startedAt ?? 0, t)}
        </span>
        <Tag tone={outcomeTone[record.outcome]}>{t(`detail.logTab.outcome.${record.outcome}`)}</Tag>
      </span>
      <span className="truncate text-fg-muted text-xs">{detail}</span>
    </button>
  );
}

/** 右边：一次启动的详情 */
function RecordDetail({
  instance,
  record,
  live,
  recovery,
}: {
  instance: InstanceSummary;
  record: LaunchRecord;
  live: boolean;
  recovery: RecoveryActions;
}) {
  const { t } = useTranslation();
  const [view, setView] = useState<View>("output");
  const [exporting, setExporting] = useState(false);
  const [uploadOpen, setUploadOpen] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [uploaded, setUploaded] = useState<MclogsUpload | null>(null);
  const [query, setQuery] = useState("");
  const [levels, setLevels] = useState<LevelFilter>(() => new Set());
  const [saved, setSaved] = useState<RecordLog | null>(null);
  const [crash, setCrash] = useState<string | null>(null);
  const liveLines = useGameStore((s) => s.logs[instance.id] ?? NO_LINES);
  const liveLoaded = useGameStore((s) => instance.id in s.logs);
  const loadLog = useGameStore((s) => s.loadLog);

  // 正在运行的这一次看内存里的实时输出；结束了的读文件
  useEffect(() => {
    if (live) {
      if (!liveLoaded) void loadLog(instance.id);
      return;
    }
    let cancelled = false;
    commands
      .launchRecordLog(instance.id, record.id)
      .then(unwrap)
      .then((log) => !cancelled && setSaved(log))
      .catch((error: unknown) => {
        if (!cancelled) reportError(error, "读取这次启动的输出");
      });
    return () => {
      cancelled = true;
    };
  }, [live, liveLoaded, loadLog, instance.id, record.id]);

  useEffect(() => {
    if (view !== "crash" || crash !== null) return;
    commands
      .launchRecordCrash(instance.id, record.id)
      .then(unwrap)
      .then((text) => setCrash(text ?? ""))
      .catch((error: unknown) => reportError(error, "读取崩溃报告"));
  }, [view, crash, instance.id, record.id]);

  const lines: LogLine[] = live ? liveLines : (saved?.lines ?? NO_LINES);
  const deferredQuery = useDeferredValue(query);
  const shown = useMemo(
    () => filterLines(lines, deferredQuery, levels),
    [lines, deferredQuery, levels],
  );
  const counts = useMemo(() => countLevels(lines), [lines]);
  const narrowed = shown.length !== lines.length;
  const disabledMods = record.mods.filter((m) => !m.enabled).length;

  const copy = () => {
    const text = view === "crash" ? (crash ?? "") : shown.map(lineText).join("\n");
    navigator.clipboard
      .writeText(text)
      .then(() =>
        toast.success(
          view === "crash"
            ? t("detail.logTab.crashCopied")
            : t("detail.logTab.copied", { n: shown.length }),
        ),
      )
      .catch((error: unknown) => reportError(error, "复制日志"));
  };

  const reveal = () => {
    commands
      .launchRecordReveal(instance.id, record.id)
      .then(unwrap)
      .catch((error: unknown) => reportError(error, "打开启动记录所在的文件夹"));
  };

  // 概要：版本、Java（悬停看路径）、内存、直接进哪里
  const facts: { text: string; title?: string }[] = [
    { text: [record.gameVersion, record.loader].filter(Boolean).join(" · ") },
    ...(record.javaVersion
      ? [
          {
            text: t("detail.logTab.java", { version: record.javaVersion }),
            title: record.javaPath ?? undefined,
          },
        ]
      : []),
    ...(record.memoryMb
      ? [{ text: t("detail.logTab.memory", { memory: memoryText(record.memoryMb) }) }]
      : []),
    ...(record.quickPlay?.kind === "world"
      ? [{ text: t("detail.logTab.quickWorld", { name: record.quickPlay.folder }) }]
      : record.quickPlay?.kind === "server"
        ? [{ text: t("detail.logTab.quickServer", { address: record.quickPlay.address }) }]
        : []),
  ];

  const exportDiagnostic = async () => {
    setExporting(true);
    try {
      if (unwrap(await commands.launchRecordDiagnosticExport(instance.id, record.id))) {
        toast.success(t("diagnostics.exported"));
      }
    } catch (error) {
      reportError(error, t("diagnostics.export"));
    } finally {
      setExporting(false);
    }
  };

  const confirmUpload = async () => {
    setUploading(true);
    try {
      const result = unwrap(await commands.launchRecordUploadLog(instance.id, record.id));
      setUploaded(result);
    } catch (error) {
      reportError(error, t("detail.logTab.upload"));
    } finally {
      setUploading(false);
    }
  };

  const copyUploadedUrl = () => {
    if (!uploaded) return;
    navigator.clipboard
      .writeText(uploaded.url)
      .then(() => toast.success(t("detail.logTab.uploadCopied")))
      .catch((error: unknown) => reportError(error, "复制分享链接"));
  };

  const openUploadedUrl = () => {
    if (!uploaded) return;
    openUrl(uploaded.url).catch((error: unknown) =>
      reportError(error, t("detail.logTab.uploadOpen")),
    );
  };

  const canUpload = !live && (record.logLines > 0 || record.hasCrashReport);
  const canDiagnose =
    (record.outcome === "crashed" || record.outcome === "failed") &&
    record.errorCode !== "job.cancelled";

  const problem =
    record.outcome === "crashed"
      ? record.exitCode !== null
        ? t("detail.logTab.crashedCode", { code: record.exitCode })
        : t("detail.logTab.crashed")
      : record.outcome === "failed"
        ? (record.error ?? t("detail.logTab.outcome.failed"))
        : record.outcome === "interrupted"
          ? t("detail.logTab.interrupted")
          : null;

  const views: { value: View; label: string }[] = [
    {
      value: "output",
      label: t("detail.logTab.output", {
        n: live ? lines.length : (saved?.total ?? record.logLines),
      }),
    },
    ...(record.hasCrashReport
      ? [{ value: "crash" as const, label: t("detail.logTab.crashReport") }]
      : []),
    { value: "mods", label: t("detail.logTab.mods", { n: record.mods.length }) },
  ];

  return (
    <section className="flex min-w-0 flex-1 flex-col gap-3">
      <div className="flex min-h-9 shrink-0 flex-wrap items-center gap-2">
        {facts.map((fact) => (
          <Tag key={fact.text}>
            <HoverHint hint={fact.title} className="max-w-64 truncate">
              {fact.text}
            </HoverHint>
          </Tag>
        ))}
        <div className="ml-auto flex items-center gap-2">
          {canUpload && (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                setUploaded(null);
                setUploadOpen(true);
              }}
            >
              <Upload className="size-4" aria-hidden />
              {t("detail.logTab.upload")}
            </Button>
          )}
          {!live && (
            <Button
              variant="secondary"
              size="sm"
              disabled={exporting}
              aria-description={t("diagnostics.exportHint")}
              onClick={() => void exportDiagnostic()}
            >
              <FileArchive className="size-4" aria-hidden />
              {t(exporting ? "diagnostics.exporting" : "diagnostics.export")}
            </Button>
          )}
          <IconButton
            label={view === "crash" ? t("detail.logTab.copyCrash") : t("detail.logTab.copyOutput")}
            variant="secondary"
            disabled={view === "mods" || (view === "output" ? shown.length === 0 : !crash)}
            onClick={copy}
          >
            <Copy className="size-4" aria-hidden />
          </IconButton>
          <IconButton label={t("detail.logTab.reveal")} variant="secondary" onClick={reveal}>
            <FolderOpen className="size-4" aria-hidden />
          </IconButton>
        </div>
      </div>

      {canDiagnose && (
        <CrashAdvice instanceId={instance.id} recordId={record.id} actions={recovery} />
      )}

      {problem && (
        <p
          className={`flex shrink-0 items-start gap-2 text-sm ${
            record.outcome === "interrupted" ? "text-fg-muted" : "text-danger"
          }`}
        >
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
          <span className="min-w-0 flex-1">
            {problem}
            {record.hasCrashReport && view !== "crash" && (
              <button
                type="button"
                onClick={() => setView("crash")}
                className="ml-2 rounded-control px-1.5 text-accent transition-colors duration-(--motion-press) hover:bg-accent-soft"
              >
                {t("detail.logTab.seeCrashReport")}
              </button>
            )}
          </span>
        </p>
      )}

      {/* 窗口窄的时候（最小 960）错误 / 警告 / 信息换到第二行 */}
      <div className="flex min-h-9 shrink-0 flex-wrap items-center gap-2">
        <Segmented<View>
          label={t("detail.logTab.view")}
          value={view}
          onChange={setView}
          options={views}
        />
        {view === "output" && (
          <>
            <div className="relative min-w-40 max-w-64 flex-1">
              <Search
                className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3.5 size-4 text-fg-muted"
                aria-hidden
              />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t("detail.logTab.search")}
                aria-label={t("detail.logTab.search")}
                className="h-9 w-full rounded-control border border-line bg-surface pr-4 pl-10 text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus:border-accent"
              />
            </div>
            <LevelChips value={levels} counts={counts} onChange={setLevels} />
            {narrowed && (
              <span className="ml-auto shrink-0 text-fg-muted text-xs">
                {t("detail.logTab.shown", { n: shown.length, total: lines.length })}
              </span>
            )}
          </>
        )}
        {view === "mods" && disabledMods > 0 && (
          <span className="ml-auto shrink-0 text-fg-muted text-xs">
            {t("detail.logTab.modsDisabled", { n: disabledMods })}
          </span>
        )}
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
        {view === "output" && (
          <>
            {!live && (saved?.skipped ?? 0) > 0 && (
              <p className="flex shrink-0 items-center gap-2 border-line border-b px-4 py-2 text-fg-muted text-xs">
                <Info className="size-3.5 shrink-0" aria-hidden />
                {t("detail.logTab.skipped", { n: saved?.skipped ?? 0 })}
              </p>
            )}
            {record.omittedLines > 0 && (
              <p className="flex shrink-0 items-center gap-2 border-line border-b px-4 py-2 text-fg-muted text-xs">
                <Info className="size-3.5 shrink-0" aria-hidden />
                {t("detail.logTab.omitted", { n: record.omittedLines })}
              </p>
            )}
            {shown.length > 0 ? (
              <LogLines lines={shown} className="min-h-0 flex-1 px-4 py-2" />
            ) : (
              <p className="m-auto p-8 text-center text-fg-muted text-sm">
                {lines.length === 0
                  ? live
                    ? t("detail.logTab.waiting")
                    : t("detail.logTab.noOutput")
                  : t("detail.logTab.noMatch")}
              </p>
            )}
          </>
        )}
        {view === "crash" && (
          <pre className="min-h-0 flex-1 select-text overflow-auto whitespace-pre-wrap break-all px-4 py-3 font-mono text-xs leading-5">
            {crash ?? ""}
          </pre>
        )}
        {view === "mods" &&
          (record.mods.length === 0 ? (
            <p className="m-auto p-8 text-center text-fg-muted text-sm">
              {t("detail.logTab.noMods")}
            </p>
          ) : (
            <VirtualList
              className="min-h-0 flex-1"
              items={record.mods}
              estimate={52}
              getKey={(mod) => mod.fileName}
              itemClassName="px-2"
              render={(mod) => (
                <div className="flex items-center gap-3 rounded-row px-3 py-2">
                  <div className="min-w-0 flex-1">
                    <p className={`truncate text-sm ${mod.enabled ? "text-fg" : "text-fg-muted"}`}>
                      {mod.title}
                      {mod.version && (
                        <span className="ml-2 text-fg-muted text-xs">{mod.version}</span>
                      )}
                    </p>
                    <p className="truncate text-fg-muted text-xs">{mod.fileName}</p>
                  </div>
                  {!mod.enabled && <Tag>{t("detail.logTab.modOff")}</Tag>}
                </div>
              )}
            />
          ))}
      </div>

      <Dialog
        open={uploadOpen}
        onClose={() => !uploading && setUploadOpen(false)}
        title={uploaded ? t("detail.logTab.uploadDoneTitle") : t("detail.logTab.uploadTitle")}
        footer={
          uploaded ? (
            <>
              <Button variant="secondary" onClick={openUploadedUrl}>
                <ExternalLink className="size-4" aria-hidden />
                {t("detail.logTab.uploadOpen")}
              </Button>
              <Button variant="primary" onClick={() => setUploadOpen(false)}>
                {t("java.presets.done")}
              </Button>
            </>
          ) : (
            <>
              <Button variant="ghost" disabled={uploading} onClick={() => setUploadOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button variant="primary" disabled={uploading} onClick={() => void confirmUpload()}>
                {uploading && <LightSpinner className="size-4" />}
                {uploading ? t("detail.logTab.uploading") : t("detail.logTab.uploadConfirm")}
              </Button>
            </>
          )
        }
      >
        {uploaded ? (
          <div className="flex items-center gap-2">
            <TextInput
              readOnly
              value={uploaded.url}
              aria-label={t("detail.logTab.uploadDoneTitle")}
              className="font-mono text-xs"
              onFocus={(event) => event.currentTarget.select()}
            />
            <IconButton
              label={t("detail.logTab.copy")}
              variant="secondary"
              onClick={copyUploadedUrl}
            >
              <Copy className="size-4" aria-hidden />
            </IconButton>
          </div>
        ) : (
          <p className="text-fg-muted text-sm leading-relaxed">{t("detail.logTab.uploadBody")}</p>
        )}
      </Dialog>
    </section>
  );
}

/**
 * 按级别筛：错误、警告、信息三个按钮，一个都没按是全都看；按一个只看那一种，按几个一起看。
 * 按下的是「选中」的样子（主色浅底 + 主色字 + 勾），后面的数字是这一种有几行。
 */
function LevelChips({
  value,
  counts,
  onChange,
}: {
  value: LevelFilter;
  counts: Record<LevelGroup, number>;
  onChange: (value: LevelFilter) => void;
}) {
  const { t } = useTranslation();
  const toggle = (group: LevelGroup) => {
    const next = new Set(value);
    if (next.has(group)) next.delete(group);
    else next.add(group);
    onChange(next);
  };
  return (
    <fieldset className="flex shrink-0 items-center gap-2">
      <legend className="sr-only">{t("detail.logTab.level")}</legend>
      {LEVEL_GROUPS.map((group) => {
        const active = value.has(group);
        return (
          <button
            key={group}
            type="button"
            aria-pressed={active}
            onClick={() => toggle(group)}
            className={`inline-flex h-9 items-center gap-1.5 rounded-control border px-3.5 text-sm shadow-raised transition-colors duration-(--motion-press) ${
              active
                ? "border-transparent bg-accent-soft font-medium text-accent"
                : "border-line bg-surface text-fg hover:bg-surface-hover"
            }`}
          >
            {active && <Check className="size-3.5" aria-hidden />}
            {t(`detail.logTab.levels.${group}`)}
            <span className={`text-xs ${active ? "text-accent" : "text-fg-muted"}`}>
              {counts[group]}
            </span>
          </button>
        );
      })}
    </fieldset>
  );
}

/** 还没有任何启动记录（D17c 以前玩的、或者记录没开成）：只显示内存里这一次的日志 */
function LiveOnly({ instance }: { instance: InstanceSummary }) {
  const { t } = useTranslation();
  const lines = useGameStore((s) => s.logs[instance.id] ?? NO_LINES);
  const loaded = useGameStore((s) => instance.id in s.logs);
  const loadLog = useGameStore((s) => s.loadLog);

  useEffect(() => {
    if (!loaded) void loadLog(instance.id);
  }, [loaded, loadLog, instance.id]);

  if (lines.length === 0) {
    return (
      <EmptyState
        icon={ScrollText}
        title={t("detail.logTab.empty")}
        hint={t("detail.logTab.emptyHint")}
      />
    );
  }
  return (
    <div className="h-full min-h-0 overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
      <LogLines lines={lines} className="h-full px-4 py-2" />
    </div>
  );
}
