import {
  Bold,
  Check,
  Code,
  Columns2,
  Copy,
  Heading2,
  Link2,
  ListTodo,
  NotebookPen,
  PencilLine,
  Save,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import RichText from "@/components/discover/RichText";
import { Button } from "@/components/ui/Button";
import { EmptyState } from "@/components/ui/EmptyState";
import { HoverHint } from "@/components/ui/HoverCard";
import { IconButton } from "@/components/ui/IconButton";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { Segmented } from "@/components/ui/Segmented";
import { Tag } from "@/components/ui/Tag";
import { unwrap } from "@/lib/api";
import { commands, type InstanceSummary } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { MAX_NOTES_BYTES, prepareNotesMarkdown } from "./notesMarkdown";

type Mode = "preview" | "edit" | "split";

/**
 * 实例级笔记（V1.1-B3，懒加载）：
 * - 每个实例独立读写 `instances/<ID>/Lumi/notes.md`（原子写、256 KB 上限、自动承接 Prism/MultiMC 导入的 `notes`）
 * - 支持「预览 / 编辑 / 分栏」三种模式，预览经 marked + DOMPurify 严格清洗防 XSS
 * - 支持常用 Markdown 快捷插入、空态场景模板、Ctrl+S 快捷保存与全文复制
 */
export function InstanceNotesCard({ instance }: { instance: InstanceSummary }) {
  const { t } = useTranslation();
  const [saved, setSaved] = useState("");
  const [draft, setDraft] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [mode, setMode] = useState<Mode>("preview");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    let cancelled = false;
    setLoaded(false);
    commands
      .instancesNotesGet(instance.id)
      .then(unwrap)
      .then((text) => {
        if (cancelled) return;
        setSaved(text);
        setDraft(text);
        // 已有笔记直接展示排版预览，空白笔记直接进入编辑态方便记录
        setMode(text.trim().length > 0 ? "preview" : "edit");
        setLoaded(true);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setSaved("");
        setDraft("");
        setMode("edit");
        setLoaded(true);
        reportError(error, "读取实例笔记");
      });
    return () => {
      cancelled = true;
    };
  }, [instance.id]);

  const normalizedDraft = useMemo(() => draft.replace(/\r\n?/g, "\n"), [draft]);
  const draftBytes = useMemo(
    () => new TextEncoder().encode(normalizedDraft).length,
    [normalizedDraft],
  );
  const tooLarge = draftBytes > MAX_NOTES_BYTES;
  const dirty = loaded && normalizedDraft !== saved;
  const preparedMarkdown = useMemo(() => prepareNotesMarkdown(draft), [draft]);

  const savingRef = useRef(false);
  const latestRef = useRef({
    id: instance.id,
    loaded: false,
    draft: "",
    normalizedDraft: "",
    saved: "",
    tooLarge: false,
  });
  latestRef.current = {
    id: instance.id,
    loaded,
    draft,
    normalizedDraft,
    saved,
    tooLarge,
  };

  const save = useCallback(
    async (silent = false) => {
      const snap = latestRef.current;
      if (
        !snap.loaded ||
        savingRef.current ||
        snap.tooLarge ||
        snap.normalizedDraft === snap.saved
      ) {
        return;
      }
      savingRef.current = true;
      setSaving(true);
      const payload = snap.draft;
      try {
        const stored = unwrap(await commands.instancesNotesSet(snap.id, payload));
        if (latestRef.current.id !== snap.id) return;
        setSaved(stored);
        // 若异步落盘期间用户又继续敲字，不覆盖新输入的草稿
        setDraft((current) => (current === payload ? stored : current));
        if (!silent) {
          toast.success(
            stored.length === 0 ? t("instanceNotes.clearedToast") : t("instanceNotes.savedToast"),
          );
        }
      } catch (error) {
        reportError(error, "保存实例笔记");
      } finally {
        savingRef.current = false;
        setSaving(false);
      }
    },
    [t],
  );

  // 停顿 800ms 自动静默保存，切换页签不丢字
  useEffect(() => {
    if (!dirty || tooLarge) return;
    const timer = window.setTimeout(() => {
      void save(true);
    }, 800);
    return () => window.clearTimeout(timer);
  }, [dirty, tooLarge, draft, save]);

  // 切走页签或切换实例时若有尚未落盘的合法草稿，立即静默落盘防止丢失
  useEffect(() => {
    const idForCleanup = instance.id;
    return () => {
      const snap = latestRef.current;
      if (
        snap.id === idForCleanup &&
        snap.loaded &&
        !snap.tooLarge &&
        snap.normalizedDraft !== snap.saved
      ) {
        void commands.instancesNotesSet(snap.id, snap.draft).catch(() => {});
      }
    };
  }, [instance.id]);

  // 编辑或分栏模式下支持 Ctrl+S / Cmd+S 立即保存
  useEffect(() => {
    if (!dirty) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void save(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [dirty, save]);

  const insertSnippet = (before: string, after = "", placeholder = "") => {
    const el = textareaRef.current;
    if (!el) {
      setDraft((prev) => `${prev}${before}${placeholder}${after}`);
      return;
    }
    const start = el.selectionStart ?? draft.length;
    const end = el.selectionEnd ?? draft.length;
    const selected = draft.slice(start, end) || placeholder;
    const next = `${draft.slice(0, start)}${before}${selected}${after}${draft.slice(end)}`;
    setDraft(next);
    requestAnimationFrame(() => {
      el.focus();
      const cursorStart = start + before.length;
      const cursorEnd = cursorStart + selected.length;
      el.setSelectionRange(cursorStart, cursorEnd);
    });
  };

  const applyTemplate = (key: "guide" | "server" | "config") => {
    const template = t(`instanceNotes.templates.${key}`);
    setDraft(template);
    if (mode === "preview") setMode("edit");
    requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const copyAll = () => {
    if (!draft.trim()) return;
    navigator.clipboard
      .writeText(draft)
      .then(() => toast.success(t("instanceNotes.copiedToast")))
      .catch((error: unknown) => reportError(error, "复制实例笔记"));
  };

  const modeOptions: { value: Mode; label: string }[] = [
    { value: "preview", label: t("instanceNotes.mode.preview") },
    { value: "edit", label: t("instanceNotes.mode.edit") },
    { value: "split", label: t("instanceNotes.mode.split") },
  ];

  const formatButtons = [
    {
      key: "heading",
      label: t("instanceNotes.toolbar.heading"),
      icon: Heading2,
      onClick: () => insertSnippet("## ", "", t("instanceNotes.toolbar.headingSample")),
    },
    {
      key: "bold",
      label: t("instanceNotes.toolbar.bold"),
      icon: Bold,
      onClick: () => insertSnippet("**", "**", t("instanceNotes.toolbar.boldSample")),
    },
    {
      key: "code",
      label: t("instanceNotes.toolbar.code"),
      icon: Code,
      onClick: () => insertSnippet("`", "`", "/gamemode creative"),
    },
    {
      key: "todo",
      label: t("instanceNotes.toolbar.todo"),
      icon: ListTodo,
      onClick: () => insertSnippet("- [ ] ", "", t("instanceNotes.toolbar.todoSample")),
    },
    {
      key: "link",
      label: t("instanceNotes.toolbar.link"),
      icon: Link2,
      onClick: () => insertSnippet("[", "](https://)", t("instanceNotes.toolbar.linkSample")),
    },
  ];

  if (!loaded) {
    return (
      <div className="flex h-full items-center justify-center rounded-panel border border-line bg-surface shadow-raised">
        <LightSpinner className="size-5 text-accent" />
      </div>
    );
  }

  const showEditor = mode === "edit" || mode === "split";
  const showPreview = mode === "preview" || mode === "split";

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      {/* 顶部工具栏：模式切换 + Markdown 快捷插入 + 保存/复制 */}
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <Segmented<Mode>
          label={t("instanceNotes.mode.label")}
          value={mode}
          onChange={setMode}
          options={modeOptions}
        />

        {showEditor && (
          <div className="flex items-center gap-1 rounded-control border border-line bg-surface p-0.5 shadow-raised">
            {formatButtons.map(({ key, label, icon: Icon, onClick }) => (
              <HoverHint key={key} hint={label}>
                <IconButton label={label} size="sm" onClick={onClick}>
                  <Icon className="size-3.5" aria-hidden />
                </IconButton>
              </HoverHint>
            ))}
          </div>
        )}

        <div className="ml-auto flex items-center gap-2">
          {tooLarge && <Tag tone="danger">{t("instanceNotes.tooLarge", { max: 256 })}</Tag>}
          {!tooLarge && dirty && <Tag tone="warning">{t("instanceNotes.unsaved")}</Tag>}
          {!dirty && saved.length > 0 && (
            <span className="inline-flex items-center gap-1 text-fg-muted text-xs">
              <Check className="size-3.5 text-success" aria-hidden />
              {t("instanceNotes.synced")}
            </span>
          )}
          {draft.trim().length > 0 && (
            <HoverHint hint={t("instanceNotes.copyAll")}>
              <IconButton label={t("instanceNotes.copyAll")} variant="secondary" onClick={copyAll}>
                <Copy className="size-4" aria-hidden />
              </IconButton>
            </HoverHint>
          )}
          {dirty && (
            <Button variant="ghost" size="sm" disabled={saving} onClick={() => setDraft(saved)}>
              {t("instanceNotes.revert")}
            </Button>
          )}
          <Button
            variant="primary"
            size="sm"
            disabled={!dirty || saving || tooLarge}
            onClick={() => void save(false)}
          >
            {saving ? (
              <LightSpinner className="size-3.5" />
            ) : (
              <Save className="size-3.5" aria-hidden />
            )}
            {t(saving ? "instanceNotes.saving" : "instanceNotes.save")}
          </Button>
        </div>
      </div>

      {/* 主体：预览 / 编辑 / 分栏 */}
      <div className="flex min-h-0 flex-1 gap-3">
        {showEditor && (
          <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
            <div className="flex shrink-0 items-center justify-between gap-2 border-line border-b px-4 py-2 text-fg-muted text-xs">
              <span className="inline-flex items-center gap-1.5 font-medium text-fg">
                <PencilLine className="size-3.5 text-accent" aria-hidden />
                {t("instanceNotes.editorTitle")}
              </span>
              <span className="tabular-nums">
                {draftBytes >= 1024
                  ? `${(draftBytes / 1024).toFixed(1)} / 256 KB`
                  : `${draft.length} ${t("instanceNotes.chars")}`}
                {" · "}
                {t("instanceNotes.shortcutHint")}
              </span>
            </div>

            {draft.length === 0 && (
              <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-line border-b bg-surface-2 px-4 py-2 text-xs">
                <span className="text-fg-muted">{t("instanceNotes.templateLabel")}</span>
                {(["guide", "server", "config"] as const).map((key) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => applyTemplate(key)}
                    className="rounded-tag border border-line bg-surface px-2 py-0.5 text-fg transition-colors duration-(--motion-press) hover:border-accent hover:text-accent"
                  >
                    {t(`instanceNotes.templateNames.${key}`)}
                  </button>
                ))}
              </div>
            )}

            <textarea
              ref={textareaRef}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              placeholder={t("instanceNotes.placeholder")}
              aria-label={t("instanceNotes.editorTitle")}
              spellCheck={false}
              className="min-h-0 flex-1 resize-none select-text bg-transparent px-4 py-3 font-mono text-fg text-sm leading-relaxed outline-none placeholder:text-fg-muted"
            />
          </section>
        )}

        {showPreview && (
          <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-panel border border-line bg-surface shadow-raised">
            {mode === "split" && (
              <div className="flex shrink-0 items-center justify-between gap-2 border-line border-b px-4 py-2 text-fg-muted text-xs">
                <span className="inline-flex items-center gap-1.5 font-medium text-fg">
                  <Columns2 className="size-3.5 text-accent" aria-hidden />
                  {t("instanceNotes.previewTitle")}
                </span>
                <span>{t("instanceNotes.safeHint")}</span>
              </div>
            )}
            {draft.trim().length === 0 ? (
              <div className="flex flex-1 items-center justify-center p-6">
                <EmptyState
                  icon={NotebookPen}
                  title={t("instanceNotes.emptyTitle")}
                  hint={t("instanceNotes.emptyHint")}
                  action={
                    <Button variant="secondary" onClick={() => setMode("edit")}>
                      <PencilLine className="size-4" aria-hidden />
                      {t("instanceNotes.startWriting")}
                    </Button>
                  }
                />
              </div>
            ) : (
              <div className="min-h-0 flex-1 select-text overflow-y-auto px-6 py-5">
                <RichText body={preparedMarkdown} format="markdown" />
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}

export default InstanceNotesCard;
