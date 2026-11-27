import {
  AlertTriangle,
  Archive,
  Box,
  Check,
  ChevronDown,
  ChevronRight,
  FileCode2,
  Folder,
  FolderArchive,
  Globe,
  Info,
  Layers,
  Loader2,
  Palette,
  RotateCcw,
  Search,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import {
  commands,
  type ExportFormat,
  type ExportGroup,
  type ExportItemView,
  type ExportMatchReport,
  type ExportPreview,
  type InstanceSummary,
} from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { logError, reportError, userMessage } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Checkbox } from "../ui/Checkbox";
import { Dialog } from "../ui/Dialog";
import { Field, TextInput } from "../ui/Field";
import { Segmented } from "../ui/Segmented";

type ViewMode = "semantic" | "tree";

const FORMATS: ExportFormat[] = ["modrinth", "curseForge", "multiMc", "fullOffline"];

/** 语义层展示顺序（隐私组单独置底为保护卡片） */
const SEMANTIC_GROUPS: Exclude<ExportGroup, "privacy">[] = [
  "mods",
  "disabledMods",
  "modConfigs",
  "gameOptions",
  "resourcePacks",
  "shaderPacks",
  "worlds",
  "other",
];

const GROUP_ICON: Record<Exclude<ExportGroup, "privacy">, typeof Box> = {
  mods: Box,
  disabledMods: Box,
  modConfigs: SlidersHorizontal,
  gameOptions: Settings2,
  resourcePacks: Palette,
  shaderPacks: Sparkles,
  worlds: Globe,
  other: FolderArchive,
};

interface TreeDirNode {
  kind: "dir";
  path: string;
  name: string;
  children: TreeNode[];
  itemKeys: string[];
  totalSize: number;
}

interface TreeLeafNode {
  kind: "leaf";
  path: string;
  name: string;
  item: ExportItemView;
}

type TreeNode = TreeDirNode | TreeLeafNode;

function buildFileTree(items: ExportItemView[]): TreeNode[] {
  interface MutableDir {
    path: string;
    name: string;
    dirs: Map<string, MutableDir>;
    leaves: TreeLeafNode[];
  }

  const root: MutableDir = { path: "", name: "", dirs: new Map(), leaves: [] };

  for (const item of items) {
    const parts = item.key.split("/").filter(Boolean);
    if (parts.length <= 1) {
      root.leaves.push({
        kind: "leaf",
        path: item.key,
        name: item.folder ? `${item.label.replace(/\/$/, "")}/` : item.label,
        item,
      });
      continue;
    }
    let cursor = root;
    let prefix = "";
    for (let i = 0; i < parts.length - 1; i++) {
      const seg = parts[i] ?? "";
      prefix = prefix ? `${prefix}/${seg}` : seg;
      let next = cursor.dirs.get(seg);
      if (!next) {
        next = { path: prefix, name: seg, dirs: new Map(), leaves: [] };
        cursor.dirs.set(seg, next);
      }
      cursor = next;
    }
    const leafName = item.folder
      ? `${item.label.replace(/\/$/, "")}/`
      : (parts[parts.length - 1] ?? item.label);
    cursor.leaves.push({
      kind: "leaf",
      path: item.key,
      name: leafName,
      item,
    });
  }

  const finalize = (dir: MutableDir): TreeNode[] => {
    const dirs: TreeDirNode[] = Array.from(dir.dirs.values())
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((child) => {
        const children = finalize(child);
        const itemKeys: string[] = [];
        let totalSize = 0;
        for (const node of children) {
          if (node.kind === "dir") {
            itemKeys.push(...node.itemKeys);
            totalSize += node.totalSize;
          } else {
            itemKeys.push(node.item.key);
            totalSize += node.item.size ?? 0;
          }
        }
        return {
          kind: "dir",
          path: child.path,
          name: child.name,
          children,
          itemKeys,
          totalSize,
        };
      });
    const leaves = [...dir.leaves].sort((a, b) => a.name.localeCompare(b.name));
    return [...dirs, ...leaves];
  };

  return finalize(root);
}

function isLinkedInFormat(
  item: ExportItemView,
  format: ExportFormat,
  modrinthSet: Set<string>,
  curseforgeSet: Set<string>,
): boolean {
  if (!item.isContentFile || item.disabled) return false;
  if (format === "modrinth") return item.modrinthLinked || modrinthSet.has(item.key);
  if (format === "curseForge") return item.curseforgeLinked || curseforgeSet.has(item.key);
  return false;
}

/** 语义层分组折叠行：三态勾选 + 展开逐项微调 */
function SemanticGroupRow({
  group,
  items,
  selected,
  format,
  modrinthSet,
  curseforgeSet,
  expanded,
  onToggleExpand,
  onToggleKeys,
}: {
  group: Exclude<ExportGroup, "privacy">;
  items: ExportItemView[];
  selected: Set<string>;
  format: ExportFormat;
  modrinthSet: Set<string>;
  curseforgeSet: Set<string>;
  expanded: boolean;
  onToggleExpand: () => void;
  onToggleKeys: (keys: string[], on: boolean) => void;
}) {
  const { t } = useTranslation();
  const Icon = GROUP_ICON[group];
  const selectedCount = items.reduce((n, item) => n + (selected.has(item.key) ? 1 : 0), 0);
  const all = items.length > 0 && selectedCount === items.length;
  const mixed = selectedCount > 0 && selectedCount < items.length;
  const groupBytes = items.reduce(
    (sum, item) =>
      sum +
      (selected.has(item.key) && !isLinkedInFormat(item, format, modrinthSet, curseforgeSet)
        ? (item.size ?? 0)
        : 0),
    0,
  );
  const title = t(`export.groups.${group}`);

  return (
    <div className="flex flex-col">
      <div className="flex items-center gap-3 px-3.5 py-2.5 transition-colors hover:bg-surface-hover">
        <Checkbox
          checked={all}
          mixed={mixed}
          label={t("export.selectAll", { title })}
          onChange={(on) =>
            onToggleKeys(
              items.map((i) => i.key),
              on,
            )
          }
        />
        <button
          type="button"
          onClick={onToggleExpand}
          className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
        >
          <Icon className="h-4 w-4 shrink-0 text-fg-muted" />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="font-medium text-fg text-sm">{title}</span>
              <span className="rounded-tag bg-surface-2 px-1.5 py-0.5 text-2xs text-fg-muted tabular-nums">
                {selectedCount}/{items.length}
              </span>
            </div>
            <p className="truncate text-fg-muted text-xs">{t(`export.groups.${group}Hint`)}</p>
          </div>
          <span className="shrink-0 text-fg-muted text-xs tabular-nums">
            {formatSize(groupBytes)}
          </span>
          {expanded ? (
            <ChevronDown className="h-4 w-4 shrink-0 text-fg-muted" />
          ) : (
            <ChevronRight className="h-4 w-4 shrink-0 text-fg-muted" />
          )}
        </button>
      </div>
      {expanded && (
        <ul className="max-h-44 divide-y divide-line overflow-y-auto border-line border-t bg-surface-2/60">
          {items.map((item) => {
            const linked = isLinkedInFormat(item, format, modrinthSet, curseforgeSet);
            return (
              <li key={item.key} className="flex items-center gap-2.5 py-1.5 pr-3.5 pl-10 text-xs">
                <Checkbox
                  checked={selected.has(item.key)}
                  label={item.key}
                  onChange={(on) => onToggleKeys([item.key], on)}
                />
                <span className="min-w-0 flex-1 truncate text-fg" title={item.key}>
                  {group === "modConfigs" || group === "other" ? item.key : item.label}
                </span>
                {linked && (
                  <span className="shrink-0 rounded-tag bg-accent-soft px-1.5 py-0.5 text-2xs text-accent">
                    {format === "modrinth" ? t("export.tagModrinth") : t("export.tagCurseForge")}
                  </span>
                )}
                {item.disabled && (
                  <span className="shrink-0 rounded-tag bg-surface px-1.5 py-0.5 text-2xs text-fg-muted">
                    {t("export.tagDisabled")}
                  </span>
                )}
                <span className="shrink-0 text-fg-muted tabular-nums">
                  {formatSize(item.size ?? 0)}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** 文件树微调层：递归渲染目录与文件节点 */
function TreeBranch({
  nodes,
  depth,
  selected,
  expandedDirs,
  forceExpand,
  format,
  modrinthSet,
  curseforgeSet,
  onToggleDir,
  onToggleKeys,
}: {
  nodes: TreeNode[];
  depth: number;
  selected: Set<string>;
  expandedDirs: Set<string>;
  forceExpand: boolean;
  format: ExportFormat;
  modrinthSet: Set<string>;
  curseforgeSet: Set<string>;
  onToggleDir: (path: string) => void;
  onToggleKeys: (keys: string[], on: boolean) => void;
}) {
  const { t } = useTranslation();

  return (
    <>
      {nodes.map((node) => {
        if (node.kind === "dir") {
          const selectedCount = node.itemKeys.reduce((n, k) => n + (selected.has(k) ? 1 : 0), 0);
          const all = node.itemKeys.length > 0 && selectedCount === node.itemKeys.length;
          const mixed = selectedCount > 0 && selectedCount < node.itemKeys.length;
          const open = forceExpand || expandedDirs.has(node.path);
          return (
            <div key={`dir:${node.path}`}>
              <div
                className="flex items-center gap-2 py-1.5 pr-3 text-xs transition-colors hover:bg-surface-hover"
                style={{ paddingLeft: `${depth * 16 + 10}px` }}
              >
                <button
                  type="button"
                  onClick={() => onToggleDir(node.path)}
                  aria-label={node.name}
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded-inner text-fg-muted hover:bg-surface-2 hover:text-fg"
                >
                  {open ? (
                    <ChevronDown className="h-3.5 w-3.5" />
                  ) : (
                    <ChevronRight className="h-3.5 w-3.5" />
                  )}
                </button>
                <Checkbox
                  checked={all}
                  mixed={mixed}
                  label={node.path}
                  onChange={(on) => onToggleKeys(node.itemKeys, on)}
                />
                <button
                  type="button"
                  onClick={() => onToggleDir(node.path)}
                  className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                >
                  <Folder className="h-3.5 w-3.5 shrink-0 text-fg-muted" />
                  <span className="truncate font-medium text-fg">{node.name}/</span>
                  <span className="shrink-0 text-2xs text-fg-muted tabular-nums">
                    {selectedCount}/{node.itemKeys.length}
                  </span>
                </button>
                <span className="shrink-0 text-fg-muted tabular-nums">
                  {formatSize(node.totalSize)}
                </span>
              </div>
              {open && (
                <TreeBranch
                  nodes={node.children}
                  depth={depth + 1}
                  selected={selected}
                  expandedDirs={expandedDirs}
                  forceExpand={forceExpand}
                  format={format}
                  modrinthSet={modrinthSet}
                  curseforgeSet={curseforgeSet}
                  onToggleDir={onToggleDir}
                  onToggleKeys={onToggleKeys}
                />
              )}
            </div>
          );
        }

        const { item } = node;
        const linked = isLinkedInFormat(item, format, modrinthSet, curseforgeSet);
        return (
          <div
            key={`leaf:${item.key}`}
            className="flex items-center gap-2 py-1.5 pr-3 text-xs transition-colors hover:bg-surface-hover"
            style={{ paddingLeft: `${depth * 16 + 34}px` }}
          >
            <Checkbox
              checked={selected.has(item.key)}
              label={item.key}
              onChange={(on) => onToggleKeys([item.key], on)}
            />
            {item.folder ? (
              <Folder className="h-3.5 w-3.5 shrink-0 text-fg-muted" />
            ) : (
              <FileCode2 className="h-3.5 w-3.5 shrink-0 text-fg-muted" />
            )}
            <span className="min-w-0 flex-1 truncate text-fg" title={item.key}>
              {node.name}
            </span>
            {linked && (
              <span className="shrink-0 rounded-tag bg-accent-soft px-1.5 py-0.5 text-2xs text-accent">
                {format === "modrinth" ? t("export.tagModrinth") : t("export.tagCurseForge")}
              </span>
            )}
            {item.group === "privacy" && (
              <span className="shrink-0 rounded-tag bg-warning-soft px-1.5 py-0.5 text-2xs text-warning">
                {t("export.tagPrivacy")}
              </span>
            )}
            {item.disabled && (
              <span className="shrink-0 rounded-tag bg-surface-2 px-1.5 py-0.5 text-2xs text-fg-muted">
                {t("export.tagDisabled")}
              </span>
            )}
            <span className="shrink-0 text-fg-muted tabular-nums">
              {formatSize(item.size ?? 0)}
            </span>
          </div>
        );
      })}
    </>
  );
}

/**
 * 导出整合包（V1.1-A4 全面升级）：
 * 1. 四大格式：Modrinth `.mrpack` / CurseForge `.zip` / MultiMC·Prism `.zip` / 完整离线包 `.zip`；
 * 2. 两段式非阻塞加载：本地毫秒级扫描出树，后台异步并行查双平台外链，实时刷新外链数与体积；
 * 3. 两层视图共享勾选状态：默认「语义勾选 + 隐私隔离护盾」，一键切「文件树微调」逐文件掌控。
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
  const [token, setToken] = useState<string | null>(null);
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [format, setFormat] = useState<ExportFormat>("modrinth");
  const [name, setName] = useState(instance.name);
  const [version, setVersion] = useState("1.0.0");
  const [author, setAuthor] = useState("");
  const [summary, setSummary] = useState("");
  const [recommendedRam, setRecommendedRam] = useState("");
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  const [viewMode, setViewMode] = useState<ViewMode>("semantic");

  const [matching, setMatching] = useState(false);
  const [modrinthSet, setModrinthSet] = useState<Set<string>>(new Set());
  const [curseforgeSet, setCurseforgeSet] = useState<Set<string>>(new Set());
  const [matchReport, setMatchReport] = useState<ExportMatchReport | null>(null);

  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const [privacyOpen, setPrivacyOpen] = useState(false);
  const [treeQuery, setTreeQuery] = useState("");
  const [treeSelectedOnly, setTreeSelectedOnly] = useState(false);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set(["mods", "config"]));

  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const activeTokenRef = useRef<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    setToken(null);
    setPreview(null);
    setError(null);
    setMatching(false);
    setModrinthSet(new Set());
    setCurseforgeSet(new Set());
    setMatchReport(null);
    setExpandedGroups(new Set());
    setPrivacyOpen(false);
    setTreeQuery("");
    setTreeSelectedOnly(false);

    commands
      .instancesExportPreview(instance.id)
      .then(unwrap)
      .then((choice) => {
        if (!alive) {
          void commands.instancesExportDiscard(choice.token);
          return;
        }
        activeTokenRef.current = choice.token;
        setToken(choice.token);
        setPreview(choice.preview);
        setName(choice.preview.name);
        setRecommendedRam(
          choice.preview.recommendedMemoryMb ? String(choice.preview.recommendedMemoryMb) : "",
        );
        setSelectedKeys(new Set(choice.preview.items.filter((i) => i.defaultOn).map((i) => i.key)));

        const hasContent = choice.preview.items.some((i) => i.isContentFile && !i.disabled);
        if (!hasContent) return;

        setMatching(true);
        commands
          .instancesExportMatch(choice.token)
          .then(unwrap)
          .then((report) => {
            if (!alive) return;
            setMatchReport(report);
            setModrinthSet(new Set(report.modrinthLinked));
            setCurseforgeSet(new Set(report.curseforgeLinked));
          })
          .catch((caught: unknown) => {
            logError(caught, "匹配导出外链");
          })
          .finally(() => {
            if (alive) setMatching(false);
          });
      })
      .catch((caught: unknown) => {
        logError(caught, "扫描要导出的内容");
        if (alive) setError(userMessage(caught));
      });

    return () => {
      alive = false;
      const toDiscard = activeTokenRef.current;
      activeTokenRef.current = null;
      if (toDiscard) {
        void commands.instancesExportDiscard(toDiscard);
      }
    };
  }, [open, instance.id, attempt]);

  const itemsByGroup = useMemo(() => {
    const map: Record<ExportGroup, ExportItemView[]> = {
      mods: [],
      disabledMods: [],
      resourcePacks: [],
      shaderPacks: [],
      modConfigs: [],
      gameOptions: [],
      worlds: [],
      privacy: [],
      other: [],
    };
    for (const item of preview?.items ?? []) {
      map[item.group].push(item);
    }
    return map;
  }, [preview]);

  const stats = useMemo(() => {
    let linked = 0;
    let bundled = 0;
    let bundledBytes = 0;
    for (const item of preview?.items ?? []) {
      if (!selectedKeys.has(item.key)) continue;
      if (isLinkedInFormat(item, format, modrinthSet, curseforgeSet)) {
        linked += 1;
      } else {
        bundled += 1;
        bundledBytes += item.size ?? 0;
      }
    }
    return { linked, bundled, bundledBytes };
  }, [preview, selectedKeys, format, modrinthSet, curseforgeSet]);

  const filteredTreeNodes = useMemo(() => {
    const q = treeQuery.trim().toLowerCase();
    const filtered = (preview?.items ?? []).filter((item) => {
      if (treeSelectedOnly && !selectedKeys.has(item.key)) return false;
      if (!q) return true;
      return item.key.toLowerCase().includes(q) || item.label.toLowerCase().includes(q);
    });
    return buildFileTree(filtered);
  }, [preview, treeQuery, treeSelectedOnly, selectedKeys]);

  const toggleKeys = (keys: string[], on: boolean) => {
    setSelectedKeys((prev) => {
      const next = new Set(prev);
      for (const k of keys) {
        if (on) next.add(k);
        else next.delete(k);
      }
      return next;
    });
  };

  const resetDefaults = () => {
    if (!preview) return;
    setSelectedKeys(new Set(preview.items.filter((i) => i.defaultOn).map((i) => i.key)));
  };

  const parsedRam = useMemo(() => {
    const cleaned = recommendedRam.trim();
    if (!cleaned) return null;
    const n = Number.parseInt(cleaned, 10);
    return Number.isFinite(n) && n >= 512 && n <= 65536 ? n : null;
  }, [recommendedRam]);

  const submit = async () => {
    if (!preview || selectedKeys.size === 0) return;
    setBusy(true);
    try {
      const jobId = unwrap(
        await commands.instancesExport(instance.id, token, {
          format,
          name: name.trim(),
          version: version.trim(),
          author: author.trim() || null,
          summary: summary.trim() || null,
          recommendedMemoryMb: parsedRam,
          selectedKeys: [...selectedKeys],
        }),
      );
      if (jobId) {
        activeTokenRef.current = null;
        useGameStore.getState().showTasksFor(jobId);
        onClose();
      }
    } catch (caught) {
      reportError(caught, "导出整合包");
    } finally {
      setBusy(false);
    }
  };

  const privacyItems = itemsByGroup.privacy;
  const privacySelectedCount = privacyItems.reduce(
    (n, item) => n + (selectedKeys.has(item.key) ? 1 : 0),
    0,
  );
  const privacyExcludedBytes = privacyItems.reduce(
    (sum, item) => sum + (selectedKeys.has(item.key) ? 0 : (item.size ?? 0)),
    0,
  );
  const formatUsesLinks = format === "modrinth" || format === "curseForge";
  const hasMatchWarning =
    (format === "modrinth" && Boolean(matchReport?.modrinthError)) ||
    (format === "curseForge" && Boolean(matchReport?.curseforgeError));

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("export.title")}
      wide
      className="max-w-2xl"
      footer={
        <>
          {preview && (
            <div className="mr-auto flex min-w-0 flex-col justify-center">
              <span className="truncate font-medium text-fg text-xs tabular-nums">
                {selectedKeys.size === 0
                  ? t("export.emptySelection")
                  : t("export.size", { size: formatSize(stats.bundledBytes) })}
              </span>
              {selectedKeys.size > 0 && (
                <span className="truncate text-2xs text-fg-muted tabular-nums">
                  {formatUsesLinks
                    ? t("export.summaryLinked", { linked: stats.linked, bundled: stats.bundled })
                    : t("export.summaryBundledOnly", { bundled: stats.bundled })}
                </span>
              )}
            </div>
          )}
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={!preview || busy || selectedKeys.size === 0}
            onClick={() => void submit()}
          >
            {t("export.submit")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-fg-muted text-sm">{t("export.intro")}</p>

        {error ? (
          <div className="flex items-center justify-between gap-4 rounded-panel border border-line bg-surface px-4 py-3">
            <p className="min-w-0 flex-1 text-danger text-sm">{error}</p>
            <Button variant="secondary" size="sm" onClick={() => setAttempt((n) => n + 1)}>
              {t("export.retry")}
            </Button>
          </div>
        ) : !preview ? (
          <div className="flex items-center justify-center gap-2.5 py-8 text-fg-muted text-sm">
            <Loader2 className="h-4 w-4 animate-spin text-accent" />
            <span>{t("export.loading")}</span>
          </div>
        ) : (
          <>
            {/* 1. 四大格式选择 */}
            <section className="flex flex-col gap-2">
              <div className="flex items-center justify-between px-0.5">
                <h3 className="font-medium text-fg-muted text-xs">{t("export.formatLabel")}</h3>
                <span className="text-2xs text-fg-muted">
                  {preview.gameVersion}
                  {preview.loader ? ` · ${preview.loader}` : ""}
                </span>
              </div>
              <div className="grid grid-cols-2 gap-2">
                {FORMATS.map((fmt) => {
                  const active = format === fmt;
                  return (
                    <button
                      key={fmt}
                      type="button"
                      aria-pressed={active}
                      onClick={() => setFormat(fmt)}
                      className={`flex flex-col gap-1 rounded-panel border p-3 text-left transition-colors ${
                        active
                          ? "border-accent bg-accent-soft/45"
                          : "border-line bg-surface hover:bg-surface-hover"
                      }`}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <span className="truncate font-medium text-fg text-sm">
                            {t(`export.formats.${fmt}.name`)}
                          </span>
                          <span className="shrink-0 rounded-tag bg-surface-2 px-1.5 py-0.5 font-mono text-2xs text-fg-muted">
                            {t(`export.formats.${fmt}.ext`)}
                          </span>
                          {fmt === "modrinth" && (
                            <span className="shrink-0 rounded-tag bg-accent-soft px-1.5 py-0.5 font-medium text-2xs text-accent">
                              {t("export.recommended")}
                            </span>
                          )}
                        </div>
                        {active && <Check className="h-4 w-4 shrink-0 text-accent" />}
                      </div>
                      <p className="text-fg-muted text-xs leading-snug">
                        {t(`export.formats.${fmt}.desc`)}
                      </p>
                      {fmt === "multiMc" && (preview.patchCount > 0 || preview.hasNotes) && (
                        <div className="mt-0.5 flex flex-wrap gap-1.5">
                          {preview.patchCount > 0 && (
                            <span className="rounded-tag bg-surface-2 px-1.5 py-0.5 text-2xs text-fg-muted">
                              {t("export.multiMcMeta", { patches: preview.patchCount })}
                            </span>
                          )}
                          {preview.hasNotes && (
                            <span className="rounded-tag bg-surface-2 px-1.5 py-0.5 text-2xs text-fg-muted">
                              {t("export.multiMcNotes")}
                            </span>
                          )}
                        </div>
                      )}
                    </button>
                  );
                })}
              </div>

              {format === "curseForge" && !preview.curseforgeAvailable && (
                <div className="flex items-start gap-2 rounded-panel border border-line bg-surface-2 px-3 py-2 text-fg-muted text-xs">
                  <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-accent" />
                  <span>{t("export.cfUnavailable")}</span>
                </div>
              )}

              {format === "fullOffline" && (
                <div className="flex items-start gap-2 rounded-panel border border-line bg-warning-soft px-3 py-2 text-warning text-xs">
                  <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span>{t("export.offlineNotice")}</span>
                </div>
              )}
            </section>

            {/* 2. 元数据 */}
            <section className="flex flex-col gap-2.5">
              <div className="grid grid-cols-[1fr_8.5rem] gap-2.5">
                <Field label={t("export.name")}>
                  <TextInput
                    value={name}
                    maxLength={64}
                    onChange={(e) => setName(e.target.value)}
                  />
                </Field>
                <Field label={t("export.version")}>
                  <TextInput
                    value={version}
                    maxLength={32}
                    onChange={(e) => setVersion(e.target.value)}
                  />
                </Field>
              </div>
              <div className="grid grid-cols-[9rem_1fr_8.5rem] gap-2.5">
                <Field label={t("export.author")}>
                  <TextInput
                    value={author}
                    maxLength={48}
                    placeholder={t("export.authorPlaceholder")}
                    onChange={(e) => setAuthor(e.target.value)}
                  />
                </Field>
                <Field label={t("export.summary")}>
                  <TextInput
                    value={summary}
                    maxLength={120}
                    placeholder={t("export.summaryPlaceholder")}
                    onChange={(e) => setSummary(e.target.value)}
                  />
                </Field>
                <Field label={t("export.recommendedRam")}>
                  <TextInput
                    type="number"
                    min={512}
                    max={65536}
                    step={512}
                    value={recommendedRam}
                    placeholder={t("export.recommendedRamPlaceholder")}
                    onChange={(e) => setRecommendedRam(e.target.value)}
                  />
                </Field>
              </div>
            </section>

            {/* 3. 后台匹配实时状态条 */}
            {formatUsesLinks && (matching || hasMatchWarning) && (
              <div className="flex items-center gap-2 rounded-panel border border-line bg-surface px-3.5 py-2 text-xs">
                {matching ? (
                  <>
                    <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-accent" />
                    <span className="text-fg-muted">{t("export.matching")}</span>
                  </>
                ) : (
                  <>
                    <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-warning" />
                    <span className="text-warning">{t("export.matchPartialWarn")}</span>
                  </>
                )}
              </div>
            )}

            {/* 4. 两层内容选择（语义勾选 / 文件树微调） */}
            <section className="flex flex-col gap-3">
              <Segmented
                value={viewMode}
                onChange={setViewMode}
                label={t("export.tabs.label")}
                fullWidth
                options={[
                  { value: "semantic", label: t("export.tabs.semantic") },
                  { value: "tree", label: t("export.tabs.tree") },
                ]}
              />

              {viewMode === "semantic" ? (
                <div className="flex flex-col gap-3">
                  <div className="divide-y divide-line overflow-hidden rounded-panel border border-line bg-surface">
                    {SEMANTIC_GROUPS.filter((g) => itemsByGroup[g].length > 0).map((group) => (
                      <SemanticGroupRow
                        key={group}
                        group={group}
                        items={itemsByGroup[group]}
                        selected={selectedKeys}
                        format={format}
                        modrinthSet={modrinthSet}
                        curseforgeSet={curseforgeSet}
                        expanded={expandedGroups.has(group)}
                        onToggleExpand={() =>
                          setExpandedGroups((prev) => {
                            const next = new Set(prev);
                            if (next.has(group)) next.delete(group);
                            else next.add(group);
                            return next;
                          })
                        }
                        onToggleKeys={toggleKeys}
                      />
                    ))}
                    {SEMANTIC_GROUPS.every((g) => itemsByGroup[g].length === 0) && (
                      <div className="flex items-center gap-2 px-4 py-4 text-fg-muted text-xs">
                        <Archive className="h-4 w-4 shrink-0" />
                        <span>{t("export.treeEmpty")}</span>
                      </div>
                    )}
                  </div>

                  {/* 隐私与个人缓存保护卡片 */}
                  <div className="overflow-hidden rounded-panel border border-line bg-surface">
                    <button
                      type="button"
                      onClick={() => privacyItems.length > 0 && setPrivacyOpen((v) => !v)}
                      disabled={privacyItems.length === 0}
                      className="flex w-full items-center gap-3 px-3.5 py-2.5 text-left transition-colors enabled:hover:bg-surface-hover"
                    >
                      <ShieldCheck className="h-4 w-4 shrink-0 text-accent" />
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="font-medium text-fg text-sm">
                            {t("export.privacyTitle")}
                          </span>
                          {privacyItems.length > 0 && (
                            <span
                              className={`rounded-tag px-1.5 py-0.5 text-2xs tabular-nums ${
                                privacySelectedCount > 0
                                  ? "bg-warning-soft text-warning"
                                  : "bg-accent-soft text-accent"
                              }`}
                            >
                              {privacySelectedCount > 0
                                ? t("export.privacyIncluded", {
                                    included: privacySelectedCount,
                                    total: privacyItems.length,
                                  })
                                : t("export.privacyExcluded", {
                                    n: privacyItems.length,
                                    size: formatSize(privacyExcludedBytes),
                                  })}
                            </span>
                          )}
                        </div>
                        <p className="text-fg-muted text-xs">
                          {privacyItems.length > 0
                            ? t("export.privacyHint")
                            : t("export.privacyClean")}
                        </p>
                      </div>
                      {privacyItems.length > 0 &&
                        (privacyOpen ? (
                          <ChevronDown className="h-4 w-4 shrink-0 text-fg-muted" />
                        ) : (
                          <ChevronRight className="h-4 w-4 shrink-0 text-fg-muted" />
                        ))}
                    </button>
                    {privacyOpen && privacyItems.length > 0 && (
                      <ul className="max-h-44 divide-y divide-line overflow-y-auto border-line border-t bg-surface-2/60">
                        {privacyItems.map((item) => (
                          <li
                            key={item.key}
                            className="flex items-center gap-2.5 py-1.5 pr-3.5 pl-4 text-xs"
                          >
                            <Checkbox
                              checked={selectedKeys.has(item.key)}
                              label={item.key}
                              onChange={(on) => toggleKeys([item.key], on)}
                            />
                            <span
                              className="min-w-0 flex-1 truncate font-mono text-fg"
                              title={item.key}
                            >
                              {item.folder ? `${item.key}/` : item.key}
                            </span>
                            <span className="shrink-0 text-fg-muted tabular-nums">
                              {formatSize(item.size ?? 0)}
                            </span>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                </div>
              ) : (
                <div className="flex flex-col gap-2.5">
                  <div className="flex flex-wrap items-center gap-2">
                    <div className="relative min-w-44 flex-1">
                      <Search className="pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-fg-muted" />
                      <input
                        type="text"
                        value={treeQuery}
                        onChange={(e) => setTreeQuery(e.target.value)}
                        placeholder={t("export.treeSearch")}
                        className="h-8 w-full rounded-control border border-line bg-surface pr-3 pl-8 text-fg text-xs outline-none placeholder:text-fg-muted focus:border-accent"
                      />
                    </div>
                    <button
                      type="button"
                      aria-pressed={treeSelectedOnly}
                      onClick={() => setTreeSelectedOnly((v) => !v)}
                      className={`inline-flex h-8 items-center gap-1.5 rounded-control border px-2.5 text-xs transition-colors ${
                        treeSelectedOnly
                          ? "border-accent bg-accent-soft text-accent"
                          : "border-line bg-surface text-fg-muted hover:bg-surface-hover hover:text-fg"
                      }`}
                    >
                      <Layers className="h-3.5 w-3.5" />
                      <span>{t("export.treeSelectedOnly")}</span>
                    </button>
                    <Button variant="ghost" size="sm" onClick={resetDefaults}>
                      <RotateCcw className="h-3.5 w-3.5" />
                      <span>{t("export.treeReset")}</span>
                    </Button>
                  </div>

                  <div className="max-h-72 overflow-y-auto rounded-panel border border-line bg-surface py-1">
                    {filteredTreeNodes.length === 0 ? (
                      <p className="py-6 text-center text-fg-muted text-xs">
                        {t("export.treeEmpty")}
                      </p>
                    ) : (
                      <TreeBranch
                        nodes={filteredTreeNodes}
                        depth={0}
                        selected={selectedKeys}
                        expandedDirs={expandedDirs}
                        forceExpand={Boolean(treeQuery.trim()) || treeSelectedOnly}
                        format={format}
                        modrinthSet={modrinthSet}
                        curseforgeSet={curseforgeSet}
                        onToggleDir={(dirPath) =>
                          setExpandedDirs((prev) => {
                            const next = new Set(prev);
                            if (next.has(dirPath)) next.delete(dirPath);
                            else next.add(dirPath);
                            return next;
                          })
                        }
                        onToggleKeys={toggleKeys}
                      />
                    )}
                  </div>
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </Dialog>
  );
}

export default ExportDialog;
