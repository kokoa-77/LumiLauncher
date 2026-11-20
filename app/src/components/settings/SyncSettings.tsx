import { Pencil, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  commands,
  type Settings,
  type SyncOverview,
  type SyncPreview,
  type SyncStrategy,
} from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { LightSpinner } from "../ui/LightSpinner";
import { Select } from "../ui/Select";
import { Switch } from "../ui/Switch";
import { Tag } from "../ui/Tag";
import { Group, Row, type UpdateSettings } from "./parts";
import {
  SharedCommandsEditorDialog,
  SharedHotbarEditorDialog,
  SharedOptionsEditorDialog,
  SharedPacksEditorDialog,
  SharedServersEditorDialog,
} from "./SyncEditors";

const STRATEGIES: { id: SyncStrategy; labelKey: string; hintKey: string }[] = [
  {
    id: "merge",
    labelKey: "settings.syncPage.strategyMerge",
    hintKey: "settings.syncPage.strategyMergeHint",
  },
  {
    id: "overwriteFromShared",
    labelKey: "settings.syncPage.strategyOverwrite",
    hintKey: "settings.syncPage.strategyOverwriteHint",
  },
  {
    id: "pullFromInstance",
    labelKey: "settings.syncPage.strategyPull",
    hintKey: "settings.syncPage.strategyPullHint",
  },
];

/**
 * 设置「实例同步」一栏（D26b / D26c 黄金 6 项）：纯本机跨实例同步。
 * - 清爽中性引导 + 总开关 + 空库一键从已有实例导入
 * - 黄金 6 项统一「编辑 + 开关」行（游戏设置 / 多人服务器 / 通用资源包 / 通用光影包 / 命令记录 / 创造模式物品栏）
 * - 完整保留 Lumi 独有的游戏退出自动增量回写与差异预览/冲突处理弹窗
 */
export function SyncSettings({ settings, update }: { settings: Settings; update: UpdateSettings }) {
  const { t } = useTranslation();
  const sync = settings.sync;
  const allInstances = useGameStore((s) => s.instances);
  const lumiInstances = useMemo(() => allInstances.filter((item) => !item.linked), [allInstances]);

  const [overview, setOverview] = useState<SyncOverview | null>(null);
  const [seedInstanceId, setSeedInstanceId] = useState<string>("");
  const [seeding, setSeeding] = useState(false);

  const [optionsEditorOpen, setOptionsEditorOpen] = useState(false);
  const [serversEditorOpen, setServersEditorOpen] = useState(false);
  const [commandsEditorOpen, setCommandsEditorOpen] = useState(false);
  const [hotbarEditorOpen, setHotbarEditorOpen] = useState(false);
  const [packsEditorKind, setPacksEditorKind] = useState<"resourcePacks" | "shaderPacks" | null>(
    null,
  );

  const [previewBusy, setPreviewBusy] = useState(false);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [strategy, setStrategy] = useState<SyncStrategy>("merge");
  const [sourceInstanceId, setSourceInstanceId] = useState<string>("");
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    if (lumiInstances.length > 0 && !lumiInstances.some((item) => item.id === seedInstanceId)) {
      setSeedInstanceId(lumiInstances[0]?.id ?? "");
    }
  }, [lumiInstances, seedInstanceId]);

  const refreshOverview = useCallback(async () => {
    try {
      setOverview(unwrap(await commands.syncOverview()));
    } catch (error) {
      reportError(error, "读取实例同步状态");
    }
  }, []);

  useEffect(() => {
    void refreshOverview();
  }, [refreshOverview]);

  const setSync = (patch: Partial<Settings["sync"]>) =>
    update((s) => ({ ...s, sync: { ...s.sync, ...patch } }));

  const seedFromInstance = async () => {
    if (!seedInstanceId) return;
    setSeeding(true);
    try {
      unwrap(await commands.syncImportFromInstance(seedInstanceId, true, true, true, true));
      const picked = lumiInstances.find((item) => item.id === seedInstanceId);
      toast.success(t("settings.syncPage.importDone", { name: picked?.name ?? seedInstanceId }));
      await refreshOverview();
    } catch (error) {
      reportError(error, "从实例导入到共享库");
    } finally {
      setSeeding(false);
    }
  };

  const openPreview = async () => {
    setPreviewBusy(true);
    try {
      const next = unwrap(await commands.syncPreview());
      setPreview(next);
      setStrategy("merge");
      setSourceInstanceId(next.instances[0]?.instanceId ?? "");
    } catch (error) {
      reportError(error, "预览跨实例同步差异");
    } finally {
      setPreviewBusy(false);
    }
  };

  const applySync = async () => {
    setApplying(true);
    try {
      const result = unwrap(
        await commands.syncApply(
          strategy,
          strategy === "pullFromInstance" ? sourceInstanceId || null : null,
        ),
      );
      if (result.updatedInstances === 0) {
        toast.info(t("settings.syncPage.applyNoChange"));
      } else {
        toast.success(
          t("settings.syncPage.applyDone", {
            instances: result.updatedInstances,
            options: result.optionsWritten,
            servers: result.serversMerged,
            packs: result.packsSynced,
          }),
        );
      }
      setPreview(null);
      await refreshOverview();
    } catch (error) {
      reportError(error, "执行跨实例同步");
    } finally {
      setApplying(false);
    }
  };

  const totalSharedPacks =
    (overview?.sharedResourcePacks ?? 0) + (overview?.sharedShaderPacks ?? 0);
  const sharedIsEmpty =
    overview !== null &&
    overview.sharedOptionsKeys === 0 &&
    overview.sharedServers === 0 &&
    totalSharedPacks === 0 &&
    overview.sharedCommands === 0 &&
    overview.sharedHotbarBytes === null;

  return (
    <>
      <p className="text-fg-muted text-sm leading-relaxed">{t("settings.syncPage.intro")}</p>

      <Group>
        <Row
          label={t("settings.syncPage.enabled")}
          hint={t("settings.syncPage.enabledHint")}
          control={
            <Switch
              label={t("settings.syncPage.enabled")}
              checked={sync.enabled}
              onChange={(enabled) => setSync({ enabled })}
            />
          }
        />
        {overview && (
          <div className="px-4 py-2.5 text-fg-muted text-xs tabular-nums">
            {t("settings.syncPage.summary", {
              participating: overview.participatingInstances,
              options: overview.sharedOptionsKeys,
              servers: overview.sharedServers,
              packs: totalSharedPacks,
            })}
            {overview.detachedInstances > 0
              ? ` · ${t("settings.syncPage.summaryDetached", { detached: overview.detachedInstances })}`
              : ""}
          </div>
        )}
      </Group>

      {sharedIsEmpty && lumiInstances.length > 0 && (
        <Group>
          <div className="flex flex-wrap items-center justify-between gap-4 px-4 py-3.5">
            <div className="min-w-0 flex-1">
              <p className="font-medium text-sm">{t("settings.syncPage.emptySeedTitle")}</p>
              <p className="mt-0.5 text-fg-muted text-xs">{t("settings.syncPage.emptySeedHint")}</p>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <div className="w-44">
                <Select
                  label={t("settings.syncPage.sourceInstance")}
                  value={seedInstanceId}
                  onChange={setSeedInstanceId}
                  options={lumiInstances.map((item) => ({ value: item.id, label: item.name }))}
                />
              </div>
              <Button
                variant="secondary"
                disabled={seeding || !seedInstanceId}
                onClick={() => void seedFromInstance()}
              >
                {seeding && <LightSpinner className="size-3.5" />}
                {t("settings.syncPage.emptySeedAction")}
              </Button>
            </div>
          </div>
        </Group>
      )}

      <Group title={t("settings.syncPage.assetsGroup")}>
        <Row
          label={t("settings.syncPage.options")}
          hint={`${t("settings.syncPage.optionsHint")}${
            overview
              ? ` · ${t("settings.syncPage.countOptions", { n: overview.sharedOptionsKeys })}`
              : ""
          }`}
          control={
            <div className="flex items-center gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setOptionsEditorOpen(true)}
                aria-label={`${t("settings.syncPage.edit")} ${t("settings.syncPage.options")}`}
              >
                <Pencil className="size-3.5" aria-hidden />
                {t("settings.syncPage.edit")}
              </Button>
              <Switch
                label={t("settings.syncPage.options")}
                checked={sync.options}
                onChange={(options) => setSync({ options })}
              />
            </div>
          }
        />
        <Row
          label={t("settings.syncPage.servers")}
          hint={`${t("settings.syncPage.serversHint")}${
            overview
              ? ` · ${t("settings.syncPage.countServers", { n: overview.sharedServers })}`
              : ""
          }`}
          control={
            <div className="flex items-center gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setServersEditorOpen(true)}
                aria-label={`${t("settings.syncPage.edit")} ${t("settings.syncPage.servers")}`}
              >
                <Pencil className="size-3.5" aria-hidden />
                {t("settings.syncPage.edit")}
              </Button>
              <Switch
                label={t("settings.syncPage.servers")}
                checked={sync.servers}
                onChange={(servers) => setSync({ servers })}
              />
            </div>
          }
        />
        <Row
          label={t("settings.syncPage.resourcePacks")}
          hint={`${t("settings.syncPage.resourcePacksHint")}${
            overview
              ? ` · ${t("settings.syncPage.countPacks", { n: overview.sharedResourcePacks })}`
              : ""
          }`}
          control={
            <div className="flex items-center gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setPacksEditorKind("resourcePacks")}
                aria-label={`${t("settings.syncPage.edit")} ${t("settings.syncPage.resourcePacks")}`}
              >
                <Pencil className="size-3.5" aria-hidden />
                {t("settings.syncPage.edit")}
              </Button>
              <Switch
                label={t("settings.syncPage.resourcePacks")}
                checked={sync.resourcePacks}
                onChange={(resourcePacks) => setSync({ resourcePacks })}
              />
            </div>
          }
        />
        <Row
          label={t("settings.syncPage.shaderPacks")}
          hint={`${t("settings.syncPage.shaderPacksHint")}${
            overview
              ? ` · ${t("settings.syncPage.countPacks", { n: overview.sharedShaderPacks })}`
              : ""
          }`}
          control={
            <div className="flex items-center gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setPacksEditorKind("shaderPacks")}
                aria-label={`${t("settings.syncPage.edit")} ${t("settings.syncPage.shaderPacks")}`}
              >
                <Pencil className="size-3.5" aria-hidden />
                {t("settings.syncPage.edit")}
              </Button>
              <Switch
                label={t("settings.syncPage.shaderPacks")}
                checked={sync.shaderPacks}
                onChange={(shaderPacks) => setSync({ shaderPacks })}
              />
            </div>
          }
        />
        <Row
          label={t("settings.syncPage.commandHistory")}
          hint={`${t("settings.syncPage.commandHistoryHint")}${
            overview
              ? ` · ${t("settings.syncPage.countCommands", { n: overview.sharedCommands })}`
              : ""
          }`}
          control={
            <div className="flex items-center gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setCommandsEditorOpen(true)}
                aria-label={`${t("settings.syncPage.edit")} ${t("settings.syncPage.commandHistory")}`}
              >
                <Pencil className="size-3.5" aria-hidden />
                {t("settings.syncPage.edit")}
              </Button>
              <Switch
                label={t("settings.syncPage.commandHistory")}
                checked={Boolean(sync.commandHistory)}
                onChange={(commandHistory) => setSync({ commandHistory })}
              />
            </div>
          }
        />
        <Row
          label={t("settings.syncPage.hotbar")}
          hint={`${t("settings.syncPage.hotbarHint")}${
            overview
              ? ` · ${
                  overview.sharedHotbarBytes !== null
                    ? t("settings.syncPage.hotbarSaved", {
                        size: formatSize(overview.sharedHotbarBytes),
                      })
                    : t("settings.syncPage.hotbarEmpty")
                }`
              : ""
          }`}
          control={
            <div className="flex items-center gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setHotbarEditorOpen(true)}
                aria-label={`${t("settings.syncPage.edit")} ${t("settings.syncPage.hotbar")}`}
              >
                <Pencil className="size-3.5" aria-hidden />
                {t("settings.syncPage.edit")}
              </Button>
              <Switch
                label={t("settings.syncPage.hotbar")}
                checked={Boolean(sync.hotbar)}
                onChange={(hotbar) => setSync({ hotbar })}
              />
            </div>
          }
        />
      </Group>

      <Group title={t("settings.syncPage.actionsGroup")}>
        <Row
          label={t("settings.syncPage.previewLabel")}
          hint={t("settings.syncPage.previewHint")}
          control={
            <Button variant="secondary" disabled={previewBusy} onClick={() => void openPreview()}>
              {previewBusy ? (
                <LightSpinner className="size-4" />
              ) : (
                <RefreshCw className="size-3.5" aria-hidden />
              )}
              {previewBusy
                ? t("settings.syncPage.previewBusy")
                : t("settings.syncPage.previewAction")}
            </Button>
          }
        />
      </Group>

      <SharedOptionsEditorDialog
        open={optionsEditorOpen}
        onClose={() => setOptionsEditorOpen(false)}
        instances={lumiInstances}
        onSaved={() => void refreshOverview()}
      />

      <SharedServersEditorDialog
        open={serversEditorOpen}
        onClose={() => setServersEditorOpen(false)}
        instances={lumiInstances}
        onChanged={() => void refreshOverview()}
      />

      <SharedPacksEditorDialog
        kind={packsEditorKind}
        onClose={() => setPacksEditorKind(null)}
        onChanged={() => void refreshOverview()}
      />

      <SharedCommandsEditorDialog
        open={commandsEditorOpen}
        onClose={() => setCommandsEditorOpen(false)}
        instances={lumiInstances}
        onChanged={() => void refreshOverview()}
      />

      <SharedHotbarEditorDialog
        open={hotbarEditorOpen}
        onClose={() => setHotbarEditorOpen(false)}
        hotbarBytes={overview?.sharedHotbarBytes ?? null}
        instances={lumiInstances}
        onChanged={() => void refreshOverview()}
      />

      <Dialog
        open={preview !== null}
        onClose={() => setPreview(null)}
        title={t("settings.syncPage.dialogTitle")}
        wide
        footer={
          <>
            <Button variant="ghost" onClick={() => setPreview(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={
                applying ||
                !preview ||
                preview.instances.length === 0 ||
                (strategy === "pullFromInstance" && !sourceInstanceId)
              }
              onClick={() => void applySync()}
            >
              {applying && <LightSpinner className="size-4" />}
              {applying ? t("settings.syncPage.applyBusy") : t("settings.syncPage.applyAction")}
            </Button>
          </>
        }
      >
        {preview && preview.instances.length === 0 ? (
          <p className="text-fg-muted text-sm">{t("settings.syncPage.dialogEmpty")}</p>
        ) : (
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <p className="font-medium text-xs text-fg-muted">
                {t("settings.syncPage.strategyLabel")}
              </p>
              <div className="flex flex-col gap-1.5">
                {STRATEGIES.map((item) => {
                  const selected = strategy === item.id;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      role="radio"
                      aria-checked={selected}
                      onClick={() => setStrategy(item.id)}
                      className={`flex flex-col items-start gap-0.5 rounded-row border px-3.5 py-2.5 text-left transition-colors duration-(--motion-press) ${
                        selected
                          ? "border-accent bg-accent-soft"
                          : "border-line bg-surface hover:bg-surface-hover"
                      }`}
                    >
                      <span
                        className={`text-sm font-medium ${selected ? "text-accent" : "text-fg"}`}
                      >
                        {t(item.labelKey)}
                      </span>
                      <span className="text-fg-muted text-xs">{t(item.hintKey)}</span>
                    </button>
                  );
                })}
              </div>
              {strategy === "pullFromInstance" && preview && preview.instances.length > 0 && (
                <div className="mt-1 flex items-center justify-between gap-3">
                  <span className="text-fg-muted text-xs">
                    {t("settings.syncPage.sourceInstance")}
                  </span>
                  <div className="w-60">
                    <Select
                      label={t("settings.syncPage.sourceInstance")}
                      value={sourceInstanceId}
                      onChange={setSourceInstanceId}
                      options={preview.instances.map((inst) => ({
                        value: inst.instanceId,
                        label: inst.instanceName,
                      }))}
                    />
                  </div>
                </div>
              )}
            </div>

            {preview && (
              <div className="max-h-56 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
                {preview.instances.map((inst) => {
                  const parts: string[] = [];
                  if (inst.optionsConflicts > 0) {
                    parts.push(t("settings.syncPage.diffOptions", { n: inst.optionsConflicts }));
                  }
                  if (inst.serversDiff > 0) {
                    parts.push(t("settings.syncPage.diffServers", { n: inst.serversDiff }));
                  }
                  if (inst.packsPending > 0) {
                    parts.push(t("settings.syncPage.diffPacks", { n: inst.packsPending }));
                  }
                  if (inst.commandsDiff > 0) {
                    parts.push(t("settings.syncPage.diffCommands", { n: inst.commandsDiff }));
                  }
                  if (inst.hotbarDiff) {
                    parts.push(t("settings.syncPage.diffHotbar"));
                  }
                  return (
                    <div
                      key={inst.instanceId}
                      className="flex items-center justify-between gap-3 px-3.5 py-2.5"
                    >
                      <div className="min-w-0">
                        <p className="truncate font-medium text-sm">{inst.instanceName}</p>
                        <p className="text-fg-muted text-xs tabular-nums">
                          {parts.length > 0 ? parts.join(" · ") : t("settings.syncPage.diffInSync")}
                        </p>
                      </div>
                      {inst.willBackupFirst && (
                        <Tag tone="accent">{t("settings.syncPage.backupBadge")}</Tag>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </Dialog>
    </>
  );
}
