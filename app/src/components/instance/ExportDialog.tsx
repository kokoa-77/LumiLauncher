import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import { commands, type ExportPreview, type InstanceSummary } from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { logError, reportError, userMessage } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Checkbox } from "../ui/Checkbox";
import { Dialog } from "../ui/Dialog";
import { Field, TextInput } from "../ui/Field";
import { HoverHint } from "../ui/HoverCard";

/** 一组可以勾的：小标题右边一个全选框，下面一行一项（名字 + 大小），太多时自己滚动 */
function CheckList({
  title,
  items,
  selected,
  onChange,
  empty,
}: {
  title: string;
  items: { key: string; label: string; size: number }[];
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  empty: string;
}) {
  const { t } = useTranslation();
  const all = items.length > 0 && items.every((i) => selected.has(i.key));
  const some = items.some((i) => selected.has(i.key));
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center gap-3 px-1">
        <Checkbox
          checked={all}
          mixed={some && !all}
          disabled={items.length === 0}
          label={t("export.selectAll", { title })}
          onChange={(on) => onChange(new Set(on ? items.map((i) => i.key) : []))}
        />
        <h3 className="font-medium text-fg-muted text-xs">{title}</h3>
      </div>
      {items.length === 0 ? (
        <p className="px-1 text-fg-muted text-xs">{empty}</p>
      ) : (
        <ul className="max-h-40 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
          {items.map((item) => (
            <li key={item.key} className="flex items-center gap-3 px-3 py-2 text-sm">
              <Checkbox
                checked={selected.has(item.key)}
                label={item.label}
                onChange={(on) => {
                  const next = new Set(selected);
                  if (on) next.add(item.key);
                  else next.delete(item.key);
                  onChange(next);
                }}
              />
              <span className="min-w-0 flex-1 truncate">{item.label}</span>
              <span className="shrink-0 text-fg-muted text-xs tabular-nums">
                {formatSize(item.size)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * 导出整合包：打开时列出会导出什么（联网查一次 Modrinth，一两秒）；
 * 名字、版本，配置和其他文件一项一项勾（地图缓存默认不勾），世界一个一个勾（默认不带）。
 * 点「导出」后端弹「另存为」，选好就在后台导出，任务中心有进度，导出完右下角提示「打开所在文件夹」。
 */
export function ExportDialog({
  instance,
  open,
  onClose,
}: {
  instance: InstanceSummary;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState(instance.name);
  const [version, setVersion] = useState("1.0.0");
  const [entries, setEntries] = useState<Set<string>>(new Set());
  const [worlds, setWorlds] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    setPreview(null);
    setError(null);
    commands
      .instancesExportPreview(instance.id)
      .then(unwrap)
      .then((found) => {
        if (!alive) return;
        setPreview(found);
        setName(found.name);
        setEntries(new Set(found.entries.filter((e) => e.defaultOn).map((e) => e.name)));
        setWorlds(new Set());
      })
      .catch((caught: unknown) => {
        logError(caught, "列出要导出的东西");
        if (alive) setError(userMessage(caught));
      });
    return () => {
      alive = false;
    };
  }, [open, instance.id, attempt]);

  const size =
    (preview?.bundledBytes ?? 0) +
    (preview?.entries ?? [])
      .filter((e) => entries.has(e.name))
      .reduce((n, e) => n + (e.size ?? 0), 0) +
    (preview?.worlds ?? [])
      .filter((w) => worlds.has(w.folder))
      .reduce((n, w) => n + (w.size ?? 0), 0);

  const submit = async () => {
    setBusy(true);
    try {
      const jobId = unwrap(
        await commands.instancesExport(instance.id, {
          name: name.trim(),
          version: version.trim(),
          summary: null,
          entries: [...entries],
          worlds: [...worlds],
        }),
      );
      if (jobId) {
        useGameStore.getState().showTasksFor(jobId);
        onClose();
      }
    } catch (caught) {
      reportError(caught, "导出整合包");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("export.title")}
      wide
      footer={
        <>
          {preview && (
            <span className="mr-auto self-center text-fg-muted text-xs tabular-nums">
              {t("export.size", { size: formatSize(size) })}
            </span>
          )}
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" disabled={!preview || busy} onClick={() => void submit()}>
            {t("export.submit")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        <p className="text-fg-muted text-sm">{t("export.intro")}</p>
        {error ? (
          <div className="flex items-center justify-between gap-4 rounded-panel border border-line bg-surface px-4 py-3">
            <p className="min-w-0 flex-1 text-danger text-sm">{error}</p>
            <Button variant="secondary" size="sm" onClick={() => setAttempt((n) => n + 1)}>
              {t("export.retry")}
            </Button>
          </div>
        ) : !preview ? (
          <p className="py-6 text-center text-fg-muted text-sm">{t("export.loading")}</p>
        ) : (
          <>
            <div className="grid grid-cols-[1fr_9rem] gap-3">
              <Field label={t("export.name")}>
                <TextInput value={name} maxLength={64} onChange={(e) => setName(e.target.value)} />
              </Field>
              <Field label={t("export.version")}>
                <TextInput
                  value={version}
                  maxLength={32}
                  onChange={(e) => setVersion(e.target.value)}
                />
              </Field>
            </div>
            <section className="flex flex-col gap-1.5 rounded-panel border border-line bg-surface px-4 py-3 text-sm">
              <p>
                {t("export.linked", { n: preview.linked })}
                {preview.bundled.length > 0 && (
                  <>
                    {" · "}
                    <HoverHint
                      hint={preview.bundled.join("\n")}
                      className="cursor-default underline decoration-dotted underline-offset-4"
                    >
                      {t("export.bundled", {
                        n: preview.bundled.length,
                        size: formatSize(preview.bundledBytes ?? 0),
                      })}
                    </HoverHint>
                  </>
                )}
              </p>
              {preview.bundled.length > 0 && (
                <p className="text-fg-muted text-xs">{t("export.bundledHint")}</p>
              )}
              {preview.disabled > 0 && (
                <p className="text-fg-muted text-xs">
                  {t("export.disabled", { n: preview.disabled })}
                </p>
              )}
            </section>
            <CheckList
              title={t("export.entries")}
              items={preview.entries.map((e) => ({
                key: e.name,
                label: e.folder ? `${e.name}/` : e.name,
                size: e.size ?? 0,
              }))}
              selected={entries}
              onChange={setEntries}
              empty={t("export.entriesNone")}
            />
            <CheckList
              title={t("export.worlds")}
              items={preview.worlds.map((w) => ({
                key: w.folder,
                label: w.name === w.folder ? w.name : `${w.name}（${w.folder}）`,
                size: w.size ?? 0,
              }))}
              selected={worlds}
              onChange={setWorlds}
              empty={t("export.worldsNone")}
            />
          </>
        )}
      </div>
    </Dialog>
  );
}
