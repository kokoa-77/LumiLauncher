import {
  Accessibility,
  FolderOpen,
  Gamepad2,
  Keyboard,
  Languages,
  Link2,
  MessageSquare,
  Monitor,
  Plus,
  Search,
  Shirt,
  Trash2,
  Unlink,
  Volume2,
  Wrench,
  X,
} from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  commands,
  type InstanceSummary,
  type SharedPackEntry,
  type SharedServerEntry,
} from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { TextInput } from "../ui/Field";
import { HoverHint } from "../ui/HoverCard";
import { IconButton } from "../ui/IconButton";
import { LightSpinner } from "../ui/LightSpinner";
import { Select } from "../ui/Select";
import { Slider } from "../ui/Slider";
import { Switch } from "../ui/Switch";
import { Tag } from "../ui/Tag";
import {
  CATALOG_OPTION_KEYS,
  domKeyCodeToMcKey,
  domMouseButtonToMcKey,
  formatMcKey,
  MC_KEYBINDS_CATALOG,
  MC_OPTIONS_CATALOG,
  type McOptionSpec,
  type OptionsTabId,
} from "./mcOptionsCatalog";

type OptionMap = Record<string, string>;

const TABS: { id: OptionsTabId; labelKey: string; icon: typeof Monitor }[] = [
  { id: "skin", labelKey: "settings.syncPage.optionsEditor.tabSkin", icon: Shirt },
  { id: "video", labelKey: "settings.syncPage.optionsEditor.tabVideo", icon: Monitor },
  { id: "language", labelKey: "settings.syncPage.optionsEditor.tabLanguage", icon: Languages },
  { id: "audio", labelKey: "settings.syncPage.optionsEditor.tabAudio", icon: Volume2 },
  { id: "controls", labelKey: "settings.syncPage.optionsEditor.tabControls", icon: Gamepad2 },
  { id: "keybinds", labelKey: "settings.syncPage.optionsEditor.tabKeybinds", icon: Keyboard },
  { id: "chat", labelKey: "settings.syncPage.optionsEditor.tabChat", icon: MessageSquare },
  {
    id: "accessibility",
    labelKey: "settings.syncPage.optionsEditor.tabAccessibility",
    icon: Accessibility,
  },
  { id: "custom", labelKey: "settings.syncPage.optionsEditor.tabCustom", icon: Wrench },
];

/** 单个可视化选项行：左侧标题 + 一句话说明、右侧控件 + 单项同步链接按钮 */
function OptionRow({
  label,
  hint,
  synced,
  onToggleSync,
  control,
}: {
  label: string;
  hint?: string;
  synced: boolean;
  onToggleSync: () => void;
  control: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex min-h-14 items-center justify-between gap-4 py-3">
      <div className="min-w-0 flex-1">
        <p className={`text-sm ${synced ? "font-medium text-fg" : "text-fg-muted"}`}>{label}</p>
        {hint && <p className="mt-0.5 text-fg-muted text-xs">{hint}</p>}
      </div>
      <div className="flex shrink-0 items-center gap-3">
        <div className={synced ? "" : "opacity-60"}>{control}</div>
        <HoverHint
          hint={
            synced
              ? t("settings.syncPage.optionsEditor.syncOn")
              : t("settings.syncPage.optionsEditor.syncOff")
          }
        >
          <button
            type="button"
            aria-pressed={synced}
            aria-label={`${label} · ${
              synced
                ? t("settings.syncPage.optionsEditor.syncOn")
                : t("settings.syncPage.optionsEditor.syncOff")
            }`}
            onClick={onToggleSync}
            className={`grid size-8 shrink-0 place-items-center rounded-control border transition-colors duration-(--motion-press) ${
              synced
                ? "border-accent bg-accent-soft text-accent"
                : "border-line bg-surface-2 text-fg-muted hover:bg-surface-hover hover:text-fg"
            }`}
          >
            {synced ? (
              <Link2 className="size-3.5" aria-hidden />
            ) : (
              <Unlink className="size-3.5" aria-hidden />
            )}
          </button>
        </HoverHint>
      </div>
    </div>
  );
}

/** 从已有 Lumi 实例一键导入的小控件 */
function ImportPicker({
  instances,
  busy,
  onImport,
}: {
  instances: InstanceSummary[];
  busy: boolean;
  onImport: (instanceId: string) => void;
}) {
  const { t } = useTranslation();
  const [selectedId, setSelectedId] = useState(instances[0]?.id ?? "");

  useEffect(() => {
    if (instances.length > 0 && !instances.some((item) => item.id === selectedId)) {
      setSelectedId(instances[0]?.id ?? "");
    }
  }, [instances, selectedId]);

  if (instances.length === 0) return null;

  return (
    <div className="flex items-center gap-2">
      <div className="w-44">
        <Select
          label={t("settings.syncPage.sourceInstance")}
          value={selectedId}
          onChange={setSelectedId}
          options={instances.map((item) => ({ value: item.id, label: item.name }))}
        />
      </div>
      <Button
        variant="secondary"
        size="sm"
        disabled={busy || !selectedId}
        onClick={() => selectedId && onImport(selectedId)}
      >
        {busy && <LightSpinner className="size-3.5" />}
        {t("settings.syncPage.importFromInstance")}
      </Button>
    </div>
  );
}

/** 渲染单条目录配置项（滑块 / 开关 / 下拉） */
function CatalogControl({
  spec,
  rawValue,
  isZh,
  onChange,
}: {
  spec: McOptionSpec;
  rawValue: string | undefined;
  isZh: boolean;
  onChange: (nextRaw: string) => void;
}) {
  const label = isZh ? spec.label.zh : spec.label.en;
  const ctrl = spec.control;
  if (ctrl.kind === "bool") {
    const trueVal = ctrl.trueVal ?? "true";
    const falseVal = ctrl.falseVal ?? "false";
    const current = rawValue ?? spec.defaultValue;
    return (
      <Switch
        label={label}
        checked={current === trueVal}
        onChange={(checked) => onChange(checked ? trueVal : falseVal)}
      />
    );
  }
  if (ctrl.kind === "select") {
    const current = rawValue ?? spec.defaultValue;
    const normalized = ctrl.choices.some((c) => c.value === current)
      ? current
      : (ctrl.choices.find((c) => c.value.replace(/"/g, "") === current.replace(/"/g, ""))?.value ??
        spec.defaultValue);
    return (
      <div className="w-44">
        <Select
          label={label}
          value={normalized}
          onChange={onChange}
          options={ctrl.choices.map((c) => ({
            value: c.value,
            label: isZh ? c.label.zh : c.label.en,
          }))}
        />
      </div>
    );
  }
  const uiValue = ctrl.toUi(rawValue);
  const formatted = ctrl.format(uiValue, isZh);
  return (
    <div className="flex w-48 items-center gap-3">
      <Slider
        value={uiValue}
        min={ctrl.min}
        max={ctrl.max}
        step={ctrl.step}
        label={label}
        valueText={formatted}
        onChange={(v) => onChange(ctrl.fromUi(v))}
      />
      <span className="w-12 shrink-0 text-right text-xs tabular-nums">{formatted}</span>
    </div>
  );
}

/**
 * 共享游戏设置（options.txt）完整 9 栏可视化精细编辑器（D26c）：
 * 皮肤自定义 / 视频 / 语言 / 音乐和声音 / 控制 / 按键绑定（支持键盘/鼠标直录）/ 聊天 / 无障碍 / 自定义设置。
 */
export function SharedOptionsEditorDialog({
  open,
  onClose,
  instances,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  instances: InstanceSummary[];
  onSaved: () => void;
}) {
  const { t, i18n } = useTranslation();
  const isZh = i18n.language.toLowerCase().startsWith("zh");
  const [tab, setTab] = useState<OptionsTabId>("skin");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [importing, setImporting] = useState(false);
  const [draft, setDraft] = useState<OptionMap>({});
  const [recordingKey, setRecordingKey] = useState<string | null>(null);
  const [customQuery, setCustomQuery] = useState("");
  const [newKey, setNewKey] = useState("");
  const [newVal, setNewVal] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const data = unwrap(await commands.syncSharedOptionsGet());
      setDraft({ ...data });
    } catch (error) {
      reportError(error, "读取共享游戏设置");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) {
      setRecordingKey(null);
      void load();
    }
  }, [open, load]);

  // 键盘 / 鼠标直录模式（仅在点中某个键帽后激活）
  useEffect(() => {
    if (!recordingKey) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.code === "Escape") {
        setRecordingKey(null);
        return;
      }
      const mcKey = domKeyCodeToMcKey(event.code);
      if (mcKey) {
        setDraft((prev) => ({ ...prev, [recordingKey]: mcKey }));
        setRecordingKey(null);
      }
    };
    const onMouseDown = (event: MouseEvent) => {
      // 忽略点击键帽本身，其余点击捕获为鼠标键位（含中键/右键/侧键）
      const target = event.target as HTMLElement | null;
      if (target?.closest("[data-keybind-recorder]")) return;
      event.preventDefault();
      event.stopPropagation();
      const mcKey = domMouseButtonToMcKey(event.button);
      if (mcKey) {
        setDraft((prev) => ({ ...prev, [recordingKey]: mcKey }));
        setRecordingKey(null);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("mousedown", onMouseDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("mousedown", onMouseDown, true);
    };
  }, [recordingKey]);

  const setKey = (key: string, value: string) => {
    setDraft((prev) => ({ ...prev, [key]: value }));
  };

  const toggleKey = (key: string, defaultValue: string) => {
    setDraft((prev) => {
      if (key in prev) {
        const next = { ...prev };
        delete next[key];
        return next;
      }
      return { ...prev, [key]: defaultValue };
    });
  };

  const removeKey = (key: string) => {
    setDraft((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
  };

  const tabSpecs = useMemo(() => {
    const map: Record<string, McOptionSpec[]> = {};
    for (const spec of MC_OPTIONS_CATALOG) {
      const list = map[spec.tab] ?? [];
      list.push(spec);
      map[spec.tab] = list;
    }
    return map;
  }, []);

  const extraKeybinds = useMemo(
    () =>
      Object.entries(draft)
        .filter(([key]) => key.startsWith("key_") && !CATALOG_OPTION_KEYS.has(key))
        .sort(([a], [b]) => a.localeCompare(b)),
    [draft],
  );

  const customKeys = useMemo(() => {
    const q = customQuery.trim().toLowerCase();
    return Object.entries(draft)
      .filter(([key, val]) => {
        if (CATALOG_OPTION_KEYS.has(key) || key.startsWith("key_")) return false;
        if (!q) return true;
        return key.toLowerCase().includes(q) || val.toLowerCase().includes(q);
      })
      .sort(([a], [b]) => a.localeCompare(b));
  }, [draft, customQuery]);

  const tabSyncedCount = (id: OptionsTabId): number => {
    if (id === "keybinds") {
      return MC_KEYBINDS_CATALOG.filter((k) => k.key in draft).length + extraKeybinds.length;
    }
    if (id === "custom") {
      return Object.keys(draft).filter(
        (key) => !CATALOG_OPTION_KEYS.has(key) && !key.startsWith("key_"),
      ).length;
    }
    return (tabSpecs[id] ?? []).filter((item) => item.key in draft).length;
  };

  const syncCurrentTabAll = () => {
    if (tab === "custom") return;
    setDraft((prev) => {
      const next = { ...prev };
      if (tab === "keybinds") {
        for (const kb of MC_KEYBINDS_CATALOG) {
          if (!(kb.key in next)) next[kb.key] = kb.defaultValue;
        }
      } else {
        for (const item of tabSpecs[tab] ?? []) {
          if (!(item.key in next)) next[item.key] = item.defaultValue;
        }
      }
      return next;
    });
  };

  const unsyncCurrentTabAll = () => {
    setDraft((prev) => {
      const next = { ...prev };
      if (tab === "keybinds") {
        for (const kb of MC_KEYBINDS_CATALOG) delete next[kb.key];
        for (const [key] of extraKeybinds) delete next[key];
      } else if (tab === "custom") {
        for (const [key] of customKeys) delete next[key];
      } else {
        for (const item of tabSpecs[tab] ?? []) delete next[item.key];
      }
      return next;
    });
  };

  const handleAddCustom = () => {
    const k = newKey.trim();
    if (!k || k.includes(":")) return;
    setDraft((prev) => ({ ...prev, [k]: newVal.trim() }));
    setNewKey("");
    setNewVal("");
  };

  const handleImport = async (instanceId: string) => {
    setImporting(true);
    try {
      unwrap(await commands.syncImportFromInstance(instanceId, true, false, false, false));
      const picked = instances.find((item) => item.id === instanceId);
      toast.success(t("settings.syncPage.importDone", { name: picked?.name ?? instanceId }));
      await load();
      onSaved();
    } catch (error) {
      reportError(error, "从实例导入游戏设置");
    } finally {
      setImporting(false);
    }
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      unwrap(await commands.syncSharedOptionsSet(draft));
      toast.success(t("settings.syncPage.optionsEditor.saved"));
      onSaved();
      onClose();
    } catch (error) {
      reportError(error, "保存共享游戏设置");
    } finally {
      setSaving(false);
    }
  };

  const syncedCount = Object.keys(draft).length;
  const currentTabSynced = tabSyncedCount(tab);

  return (
    <Dialog
      open={open}
      onClose={() => {
        if (recordingKey) {
          setRecordingKey(null);
          return;
        }
        onClose();
      }}
      title={t("settings.syncPage.optionsEditor.title")}
      wider
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" disabled={loading || saving} onClick={() => void handleSave()}>
            {saving && <LightSpinner className="size-4" />}
            {t("notes.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3.5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="min-w-0 flex-1 text-fg-muted text-xs leading-relaxed">
            {t("settings.syncPage.optionsEditor.intro")}
          </p>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <ImportPicker
              instances={instances}
              busy={importing}
              onImport={(id) => void handleImport(id)}
            />
            {syncedCount > 0 && (
              <Button variant="secondary" size="sm" onClick={() => setDraft({})}>
                <Unlink className="size-3.5" aria-hidden />
                {t("settings.syncPage.optionsEditor.unsyncAll")}
              </Button>
            )}
          </div>
        </div>

        <div className="flex h-[460px] max-h-[62vh] overflow-hidden rounded-panel border border-line bg-surface">
          <nav
            aria-label={t("settings.syncPage.optionsEditor.title")}
            className="flex w-44 shrink-0 flex-col gap-0.5 overflow-y-auto border-line border-r bg-surface-2/40 p-2"
          >
            {TABS.map(({ id, labelKey, icon: Icon }) => {
              const active = tab === id;
              const count = tabSyncedCount(id);
              return (
                <button
                  key={id}
                  type="button"
                  aria-current={active ? "page" : undefined}
                  onClick={() => {
                    setRecordingKey(null);
                    setTab(id);
                  }}
                  className={`flex items-center justify-between gap-2 rounded-row px-3 py-2 text-left text-sm transition-colors duration-(--motion-press) ${
                    active
                      ? "bg-accent-soft font-medium text-accent"
                      : "text-fg-muted hover:bg-surface-hover hover:text-fg"
                  }`}
                >
                  <span className="flex min-w-0 items-center gap-2.5">
                    <Icon className="size-4 shrink-0" aria-hidden />
                    <span className="truncate">{t(labelKey)}</span>
                  </span>
                  {count > 0 && (
                    <span
                      className={`shrink-0 text-xs tabular-nums ${active ? "text-accent" : "text-fg-muted"}`}
                    >
                      {count}
                    </span>
                  )}
                </button>
              );
            })}
          </nav>

          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex shrink-0 items-center justify-between gap-3 border-line border-b px-5 py-2.5">
              <span className="font-medium text-sm">
                {t(TABS.find((item) => item.id === tab)?.labelKey ?? "")}
              </span>
              <div className="flex items-center gap-2">
                {tab !== "custom" && (
                  <Button variant="ghost" size="sm" onClick={syncCurrentTabAll}>
                    <Link2 className="size-3.5" aria-hidden />
                    {t("settings.syncPage.optionsEditor.syncTabAll")}
                  </Button>
                )}
                {currentTabSynced > 0 && (
                  <Button variant="ghost" size="sm" onClick={unsyncCurrentTabAll}>
                    <Unlink className="size-3.5" aria-hidden />
                    {t("settings.syncPage.optionsEditor.unsyncTabAll")}
                  </Button>
                )}
              </div>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto px-5 py-1.5">
              {loading ? (
                <div className="flex h-full items-center justify-center gap-2 text-fg-muted text-sm">
                  <LightSpinner className="size-4" />
                  {t("common.loading")}
                </div>
              ) : tab === "keybinds" ? (
                <div className="divide-y divide-line">
                  {MC_KEYBINDS_CATALOG.map((kb) => {
                    const synced = kb.key in draft;
                    const recording = recordingKey === kb.key;
                    const currentVal = draft[kb.key] ?? kb.defaultValue;
                    return (
                      <OptionRow
                        key={kb.key}
                        label={isZh ? kb.label.zh : kb.label.en}
                        synced={synced}
                        onToggleSync={() => toggleKey(kb.key, kb.defaultValue)}
                        control={
                          <div className="flex items-center gap-1.5" data-keybind-recorder>
                            <button
                              type="button"
                              onClick={() => setRecordingKey(recording ? null : kb.key)}
                              className={`inline-flex h-8 min-w-28 items-center justify-center rounded-control border px-3 font-mono text-xs transition-colors duration-(--motion-press) ${
                                recording
                                  ? "border-accent bg-accent-soft font-medium text-accent"
                                  : "border-line bg-surface-2 text-fg hover:bg-surface-hover"
                              }`}
                            >
                              {recording
                                ? t("settings.syncPage.optionsEditor.pressKey")
                                : formatMcKey(currentVal, isZh)}
                            </button>
                            {synced && currentVal !== "key.keyboard.unknown" && (
                              <IconButton
                                label={t("settings.syncPage.optionsEditor.keyClear")}
                                size="sm"
                                variant="ghost"
                                onClick={() => setKey(kb.key, "key.keyboard.unknown")}
                              >
                                <X className="size-3.5" aria-hidden />
                              </IconButton>
                            )}
                          </div>
                        }
                      />
                    );
                  })}
                  {extraKeybinds.map(([key, value]) => {
                    const recording = recordingKey === key;
                    return (
                      <OptionRow
                        key={key}
                        label={key.replace(/^key_/, "")}
                        hint={key}
                        synced
                        onToggleSync={() => removeKey(key)}
                        control={
                          <div className="flex items-center gap-1.5" data-keybind-recorder>
                            <button
                              type="button"
                              onClick={() => setRecordingKey(recording ? null : key)}
                              className={`inline-flex h-8 min-w-28 items-center justify-center rounded-control border px-3 font-mono text-xs transition-colors duration-(--motion-press) ${
                                recording
                                  ? "border-accent bg-accent-soft font-medium text-accent"
                                  : "border-line bg-surface-2 text-fg hover:bg-surface-hover"
                              }`}
                            >
                              {recording
                                ? t("settings.syncPage.optionsEditor.pressKey")
                                : formatMcKey(value, isZh)}
                            </button>
                          </div>
                        }
                      />
                    );
                  })}
                </div>
              ) : tab === "custom" ? (
                <div className="flex flex-col gap-3 py-2">
                  <div className="relative">
                    <Search
                      className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3 size-3.5 text-fg-muted"
                      aria-hidden
                    />
                    <TextInput
                      value={customQuery}
                      placeholder={t("settings.syncPage.optionsEditor.customSearch")}
                      aria-label={t("settings.syncPage.optionsEditor.customSearch")}
                      className="h-8 pl-8 text-xs"
                      onChange={(event) => setCustomQuery(event.target.value)}
                    />
                  </div>
                  <form
                    className="flex items-center gap-2"
                    onSubmit={(event) => {
                      event.preventDefault();
                      handleAddCustom();
                    }}
                  >
                    <div className="flex-1">
                      <TextInput
                        value={newKey}
                        placeholder={t("settings.syncPage.optionsEditor.customAddKey")}
                        aria-label={t("settings.syncPage.optionsEditor.customAddKey")}
                        className="h-8 font-mono text-xs"
                        spellCheck={false}
                        onChange={(event) => setNewKey(event.target.value)}
                      />
                    </div>
                    <div className="w-36">
                      <TextInput
                        value={newVal}
                        placeholder={t("settings.syncPage.optionsEditor.customAddVal")}
                        aria-label={t("settings.syncPage.optionsEditor.customAddVal")}
                        className="h-8 font-mono text-xs"
                        spellCheck={false}
                        onChange={(event) => setNewVal(event.target.value)}
                      />
                    </div>
                    <Button
                      type="submit"
                      variant="secondary"
                      size="sm"
                      disabled={!newKey.trim() || newKey.includes(":")}
                    >
                      <Plus className="size-3.5" aria-hidden />
                      {t("settings.syncPage.optionsEditor.customAdd")}
                    </Button>
                  </form>
                  {customKeys.length === 0 ? (
                    <p className="py-8 text-center text-fg-muted text-sm leading-relaxed">
                      {t("settings.syncPage.optionsEditor.emptyKeybinds")}
                    </p>
                  ) : (
                    <div className="divide-y divide-line">
                      {customKeys.map(([key, value]) => (
                        <div key={key} className="flex items-center gap-3 py-2">
                          <span className="min-w-0 flex-1 truncate font-mono text-fg text-xs">
                            {key}
                          </span>
                          <div className="w-44 shrink-0">
                            <TextInput
                              value={value}
                              aria-label={key}
                              className="h-8 font-mono text-xs"
                              spellCheck={false}
                              onChange={(event) => setKey(key, event.target.value)}
                            />
                          </div>
                          <IconButton
                            label={`${key} · ${t("settings.syncPage.optionsEditor.removeKey")}`}
                            variant="ghost"
                            onClick={() => removeKey(key)}
                          >
                            <Trash2 className="size-3.5" aria-hidden />
                          </IconButton>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              ) : (
                <div className="divide-y divide-line">
                  {(tabSpecs[tab] ?? []).map((spec) => (
                    <OptionRow
                      key={spec.key}
                      label={isZh ? spec.label.zh : spec.label.en}
                      hint={isZh ? spec.hint.zh : spec.hint.en}
                      synced={spec.key in draft}
                      onToggleSync={() => toggleKey(spec.key, spec.defaultValue)}
                      control={
                        <CatalogControl
                          spec={spec}
                          rawValue={draft[spec.key]}
                          isZh={isZh}
                          onChange={(nextRaw) => setKey(spec.key, nextRaw)}
                        />
                      }
                    />
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </Dialog>
  );
}

/** 共享多人服务器列表（servers.dat）可视化管理弹窗 */
export function SharedServersEditorDialog({
  open,
  onClose,
  instances,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  instances: InstanceSummary[];
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [servers, setServers] = useState<SharedServerEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState(false);
  const [name, setName] = useState("");
  const [address, setAddress] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setServers(unwrap(await commands.syncSharedServersList()));
    } catch (error) {
      reportError(error, "读取共享服务器列表");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  const handleAdd = async () => {
    const trimmedAddr = address.trim();
    if (!trimmedAddr) return;
    setBusy(true);
    try {
      const next = unwrap(
        await commands.syncSharedServersAdd(name.trim() || trimmedAddr, trimmedAddr),
      );
      setServers(next);
      setName("");
      setAddress("");
      toast.success(t("settings.syncPage.serversEditor.added"));
      onChanged();
    } catch (error) {
      reportError(error, "添加共享服务器");
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (targetAddress: string) => {
    setBusy(true);
    try {
      const next = unwrap(await commands.syncSharedServersRemove(targetAddress));
      setServers(next);
      toast.success(t("settings.syncPage.serversEditor.removed"));
      onChanged();
    } catch (error) {
      reportError(error, "移除共享服务器");
    } finally {
      setBusy(false);
    }
  };

  const handleImport = async (instanceId: string) => {
    setImporting(true);
    try {
      unwrap(await commands.syncImportFromInstance(instanceId, false, true, false, false));
      const picked = instances.find((item) => item.id === instanceId);
      toast.success(t("settings.syncPage.importDone", { name: picked?.name ?? instanceId }));
      await load();
      onChanged();
    } catch (error) {
      reportError(error, "从实例导入服务器列表");
    } finally {
      setImporting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("settings.syncPage.serversEditor.title")}
      wide
      footer={
        <Button variant="primary" onClick={onClose}>
          {t("java.presets.done")}
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <ImportPicker
            instances={instances}
            busy={importing}
            onImport={(id) => void handleImport(id)}
          />
        </div>

        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void handleAdd();
          }}
        >
          <div className="min-w-36 flex-1">
            <TextInput
              value={name}
              placeholder={t("settings.syncPage.serversEditor.namePlaceholder")}
              aria-label={t("settings.syncPage.serversEditor.namePlaceholder")}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <div className="min-w-48 flex-1">
            <TextInput
              value={address}
              className="font-mono text-xs"
              placeholder={t("settings.syncPage.serversEditor.addressPlaceholder")}
              aria-label={t("settings.syncPage.serversEditor.addressPlaceholder")}
              spellCheck={false}
              onChange={(event) => setAddress(event.target.value)}
            />
          </div>
          <Button type="submit" variant="secondary" disabled={busy || !address.trim()}>
            <Plus className="size-3.5" aria-hidden />
            {t("settings.syncPage.serversEditor.add")}
          </Button>
        </form>

        <div className="max-h-72 overflow-y-auto rounded-panel border border-line bg-surface">
          {loading ? (
            <div className="flex items-center justify-center gap-2 py-10 text-fg-muted text-sm">
              <LightSpinner className="size-4" />
              {t("common.loading")}
            </div>
          ) : servers.length === 0 ? (
            <p className="px-4 py-8 text-center text-fg-muted text-sm">
              {t("settings.syncPage.serversEditor.empty")}
            </p>
          ) : (
            <div className="divide-y divide-line">
              {servers.map((server) => (
                <div
                  key={server.address}
                  className="flex items-center justify-between gap-3 px-4 py-2.5"
                >
                  <div className="min-w-0">
                    <p className="truncate font-medium text-sm">{server.name}</p>
                    <p className="truncate font-mono text-fg-muted text-xs">{server.address}</p>
                  </div>
                  <IconButton
                    label={`${server.name} · ${t("settings.syncPage.serversEditor.remove")}`}
                    variant="ghost"
                    disabled={busy}
                    onClick={() => void handleRemove(server.address)}
                  >
                    <Trash2 className="size-4" aria-hidden />
                  </IconButton>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}

/** 共享通用资源包 / 通用光影包可视化管理弹窗 */
export function SharedPacksEditorDialog({
  kind,
  onClose,
  onChanged,
}: {
  kind: "resourcePacks" | "shaderPacks" | null;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [packs, setPacks] = useState<SharedPackEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [busyName, setBusyName] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!kind) return;
    setLoading(true);
    try {
      setPacks(unwrap(await commands.syncSharedPacksList(kind)));
    } catch (error) {
      reportError(error, "读取共享包列表");
    } finally {
      setLoading(false);
    }
  }, [kind]);

  useEffect(() => {
    if (kind) void load();
  }, [kind, load]);

  const openFolder = async () => {
    if (!kind) return;
    try {
      unwrap(await commands.syncOpenSharedFolder(kind));
    } catch (error) {
      reportError(error, "打开共享文件夹");
    }
  };

  const handleRemove = async (packName: string) => {
    if (!kind) return;
    setBusyName(packName);
    try {
      const next = unwrap(await commands.syncSharedPacksRemove(kind, packName));
      setPacks(next);
      toast.success(t("settings.syncPage.packsEditor.removed", { name: packName }));
      onChanged();
    } catch (error) {
      reportError(error, "移除共享包");
    } finally {
      setBusyName(null);
    }
  };

  const title =
    kind === "shaderPacks"
      ? t("settings.syncPage.packsEditor.titleShader")
      : t("settings.syncPage.packsEditor.titleResource");

  return (
    <Dialog
      open={kind !== null}
      onClose={onClose}
      title={title}
      wide
      footer={
        <>
          <Button variant="secondary" onClick={() => void openFolder()}>
            <FolderOpen className="size-3.5" aria-hidden />
            {t("settings.syncPage.openFolder")}
          </Button>
          <Button variant="primary" onClick={onClose}>
            {t("java.presets.done")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-fg-muted text-xs leading-relaxed">
          {t("settings.syncPage.packsEditor.hint")}
        </p>

        <div className="max-h-72 overflow-y-auto rounded-panel border border-line bg-surface">
          {loading ? (
            <div className="flex items-center justify-center gap-2 py-10 text-fg-muted text-sm">
              <LightSpinner className="size-4" />
              {t("common.loading")}
            </div>
          ) : packs.length === 0 ? (
            <p className="px-4 py-8 text-center text-fg-muted text-sm">
              {t("settings.syncPage.packsEditor.empty")}
            </p>
          ) : (
            <div className="divide-y divide-line">
              {packs.map((pack) => (
                <div
                  key={pack.name}
                  className="flex items-center justify-between gap-3 px-4 py-2.5"
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-medium text-sm">{pack.name}</span>
                    {pack.isDir && <Tag>{t("settings.syncPage.packsEditor.folderTag")}</Tag>}
                    {pack.bytes !== null && (
                      <span className="shrink-0 text-fg-muted text-xs tabular-nums">
                        {formatSize(pack.bytes)}
                      </span>
                    )}
                  </div>
                  <IconButton
                    label={`${pack.name} · ${t("settings.syncPage.packsEditor.remove")}`}
                    variant="ghost"
                    disabled={busyName === pack.name}
                    onClick={() => void handleRemove(pack.name)}
                  >
                    <Trash2 className="size-4" aria-hidden />
                  </IconButton>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}

/** 共享命令记录（command_history.txt）可视化管理弹窗（D26c） */
export function SharedCommandsEditorDialog({
  open,
  onClose,
  instances,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  instances: InstanceSummary[];
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [lines, setLines] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [importing, setImporting] = useState(false);
  const [input, setInput] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setLines(unwrap(await commands.syncSharedCommandsGet()));
    } catch (error) {
      reportError(error, "读取共享命令记录");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  const persist = async (nextLines: string[]) => {
    setSaving(true);
    try {
      const saved = unwrap(await commands.syncSharedCommandsSet(nextLines));
      setLines(saved);
      toast.success(t("settings.syncPage.commandsEditor.saved"));
      onChanged();
    } catch (error) {
      reportError(error, "保存共享命令记录");
    } finally {
      setSaving(false);
    }
  };

  const handleAdd = () => {
    const trimmed = input.trim();
    if (!trimmed) return;
    setInput("");
    void persist([...lines.filter((item) => item !== trimmed), trimmed]);
  };

  const handleImport = async (instanceId: string) => {
    setImporting(true);
    try {
      unwrap(await commands.syncImportFromInstance(instanceId, false, false, true, false));
      const picked = instances.find((item) => item.id === instanceId);
      toast.success(t("settings.syncPage.importDone", { name: picked?.name ?? instanceId }));
      await load();
      onChanged();
    } catch (error) {
      reportError(error, "从实例导入命令记录");
    } finally {
      setImporting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("settings.syncPage.commandsEditor.title")}
      wide
      footer={
        <>
          {lines.length > 0 && (
            <Button variant="ghost" disabled={saving} onClick={() => void persist([])}>
              {t("settings.syncPage.commandsEditor.clear")}
            </Button>
          )}
          <Button variant="primary" onClick={onClose}>
            {t("java.presets.done")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="min-w-0 flex-1 text-fg-muted text-xs leading-relaxed">
            {t("settings.syncPage.commandsEditor.hint")}
          </p>
          <ImportPicker
            instances={instances}
            busy={importing}
            onImport={(id) => void handleImport(id)}
          />
        </div>

        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            handleAdd();
          }}
        >
          <div className="flex-1">
            <TextInput
              value={input}
              className="font-mono text-xs"
              placeholder={t("settings.syncPage.commandsEditor.placeholder")}
              aria-label={t("settings.syncPage.commandsEditor.placeholder")}
              spellCheck={false}
              onChange={(event) => setInput(event.target.value)}
            />
          </div>
          <Button type="submit" variant="secondary" disabled={saving || !input.trim()}>
            <Plus className="size-3.5" aria-hidden />
            {t("settings.syncPage.commandsEditor.add")}
          </Button>
        </form>

        <div className="max-h-72 overflow-y-auto rounded-panel border border-line bg-surface">
          {loading ? (
            <div className="flex items-center justify-center gap-2 py-10 text-fg-muted text-sm">
              <LightSpinner className="size-4" />
              {t("common.loading")}
            </div>
          ) : lines.length === 0 ? (
            <p className="px-4 py-8 text-center text-fg-muted text-sm">
              {t("settings.syncPage.commandsEditor.empty")}
            </p>
          ) : (
            <div className="divide-y divide-line">
              {[...lines].reverse().map((cmd) => (
                <div key={cmd} className="flex items-center justify-between gap-3 px-4 py-2">
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{cmd}</span>
                  <IconButton
                    label={`${cmd} · ${t("settings.syncPage.serversEditor.remove")}`}
                    variant="ghost"
                    disabled={saving}
                    onClick={() => void persist(lines.filter((item) => item !== cmd))}
                  >
                    <Trash2 className="size-3.5" aria-hidden />
                  </IconButton>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}

/** 共享创造模式物品栏（hotbar.nbt）管理弹窗（D26c） */
export function SharedHotbarEditorDialog({
  open,
  onClose,
  hotbarBytes,
  instances,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  hotbarBytes: number | null;
  instances: InstanceSummary[];
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [importing, setImporting] = useState(false);
  const [clearing, setClearing] = useState(false);

  const handleImport = async (instanceId: string) => {
    setImporting(true);
    try {
      unwrap(await commands.syncImportFromInstance(instanceId, false, false, false, true));
      const picked = instances.find((item) => item.id === instanceId);
      toast.success(t("settings.syncPage.importDone", { name: picked?.name ?? instanceId }));
      onChanged();
    } catch (error) {
      reportError(error, "从实例导入创造模式物品栏");
    } finally {
      setImporting(false);
    }
  };

  const handleClear = async () => {
    setClearing(true);
    try {
      unwrap(await commands.syncSharedHotbarClear());
      toast.success(t("settings.syncPage.hotbarEditor.cleared"));
      onChanged();
    } catch (error) {
      reportError(error, "清空共享创造模式物品栏");
    } finally {
      setClearing(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("settings.syncPage.hotbarEditor.title")}
      footer={
        <>
          {hotbarBytes !== null && (
            <Button variant="ghost" disabled={clearing} onClick={() => void handleClear()}>
              {t("settings.syncPage.hotbarEditor.clear")}
            </Button>
          )}
          <Button variant="primary" onClick={onClose}>
            {t("java.presets.done")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-fg-muted text-xs leading-relaxed">
          {t("settings.syncPage.hotbarEditor.hint")}
        </p>
        <div className="rounded-panel border border-line bg-surface px-4 py-3 text-sm">
          {hotbarBytes !== null
            ? t("settings.syncPage.hotbarEditor.statusSaved", { size: formatSize(hotbarBytes) })
            : t("settings.syncPage.hotbarEditor.statusEmpty")}
        </div>
        <ImportPicker
          instances={instances}
          busy={importing}
          onImport={(id) => void handleImport(id)}
        />
      </div>
    </Dialog>
  );
}
