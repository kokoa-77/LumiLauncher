import {
  ChevronDown,
  ChevronUp,
  Coffee,
  FolderOpen,
  Gamepad2,
  Globe,
  Info,
  type LucideIcon,
  Pencil,
  X,
} from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import {
  commands,
  type InstanceOverrides,
  type InstanceSettingsView,
  type InstanceSummary,
  type InstanceSyncExclusions,
  type JavaEntry,
} from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { type SettingsSection, useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";
import { openInstanceFolder } from "../HomeHero";
import { EnvVarsEditor } from "../settings/EnvVars";
import { JavaArgsRows } from "../settings/JavaArgs";
import { MemoryPicker } from "../settings/MemorySettings";
import { ArgsInput, Group, Row, StackedRow } from "../settings/parts";
import { SettingsShell } from "../settings/SettingsShell";
import { WindowPicker } from "../settings/WindowPicker";
import { Button } from "../ui/Button";
import { TextInput } from "../ui/Field";
import { HoverHint } from "../ui/HoverCard";
import { Switch } from "../ui/Switch";
import { Tag } from "../ui/Tag";
import { JavaField, JavaPickerDialog } from "./JavaPicker";

export type InstanceSection = "general" | "game" | "java";

const SECTIONS: { id: InstanceSection; icon: LucideIcon }[] = [
  { id: "general", icon: Info },
  { id: "game", icon: Gamepad2 },
  { id: "java", icon: Coffee },
];

/**
 * 实例里可以单独设的一组（照 XMCL 的做法）：控件一直摆着，没改过时显示的就是全局现在的值，
 * 标题旁边灰色「跟着全局」；改了任何一项，这一组就变成这个实例自己的，标签变成主色「本实例」，点 × 改回跟着全局。
 */
function ScopedGroup({
  title,
  own,
  onReset,
  children,
}: {
  title: string;
  /** 这一组是这个实例自己的（改过） */
  own: boolean;
  onReset: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <Group
      title={title}
      badge={
        own ? (
          <span className="inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-tag bg-accent-soft py-0.5 pr-0.5 pl-2 text-accent text-xs">
            <Pencil className="size-3" aria-hidden />
            {t("instanceSettings.ownTag")}
            <HoverHint hint={t("instanceSettings.resetToGlobal")}>
              <button
                type="button"
                aria-label={`${title}：${t("instanceSettings.resetToGlobal")}`}
                onClick={onReset}
                className="grid size-4 place-items-center rounded-icon transition-colors duration-(--motion-press) hover:bg-accent/15"
              >
                <X className="size-3" aria-hidden />
              </button>
            </HoverHint>
          </span>
        ) : (
          <Tag className="gap-1">
            <Globe className="size-3" aria-hidden />
            {t("instanceSettings.followTag")}
          </Tag>
        )
      }
    >
      {children}
    </Group>
  );
}

/**
 * 实例设置弹窗（验收时从页签改成弹窗，和全局设置同一个外壳）：实例页横幅的齿轮打开。
 * 通用（名字、文件夹）、游戏（内存、游戏窗口）、Java（用哪个 Java、启动参数、环境变量）。
 * 默认每组都跟着全局（显示的就是全局现在的值，全局改了这里跟着变）；在这里改了哪一组，那一组就只对这个实例生效。
 * 每改一项马上保存，下次启动生效。
 */
export function InstanceSettingsDialog({
  instance,
  section,
  onSection,
  onClose,
}: {
  instance: InstanceSummary;
  /** 开着哪一栏；关着是 null */
  section: InstanceSection | null;
  onSection: (section: InstanceSection) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const open = section !== null;
  const settings = useAppStore((s) => s.settings);
  const openSettings = useAppStore((s) => s.openSettings);
  const refreshInstances = useGameStore((s) => s.refreshInstances);
  const [view, setView] = useState<InstanceSettingsView | null>(null);
  const [javas, setJavas] = useState<JavaEntry[] | null>(null);
  // 自动时这个实例这次会给多少（按它的模组数量算）
  const [autoMb, setAutoMb] = useState<number | undefined>(undefined);
  const [pickingJava, setPickingJava] = useState(false);
  const [syncDetailOpen, setSyncDetailOpen] = useState(false);

  const refreshAuto = useCallback(() => {
    commands
      .memoryCheck(instance.id)
      .then(unwrap)
      .then((check) => setAutoMb(check?.auto ? check.wantedMb : undefined))
      .catch((error: unknown) => reportError(error, "读实例要的内存"));
  }, [instance.id]);

  useEffect(() => {
    if (!open) return;
    commands
      .instancesSettingsGet(instance.id)
      .then(unwrap)
      .then(setView)
      .catch((error: unknown) => reportError(error, "读取实例设置"));
    commands
      .javaList()
      .then(unwrap)
      .then((overview) => setJavas(overview.entries))
      .catch((error: unknown) => reportError(error, "读取 Java 列表"));
    refreshAuto();
  }, [open, instance.id, refreshAuto]);

  /** 关掉实例设置，打开全局设置的那一栏 */
  const openGlobal = (target: SettingsSection) => () => {
    onClose();
    openSettings(target);
  };

  const current = section ?? "general";
  let page: ReactNode = <p className="text-fg-muted text-sm">{t("common.loading")}</p>;

  if (view && settings) {
    const game = settings.game;
    const overrides = view.overrides;
    const save = async (name: string, next: InstanceOverrides) => {
      try {
        const saved = unwrap(await commands.instancesSettingsSet(instance.id, name, next));
        setView(saved);
        if (saved.name !== instance.name) void refreshInstances();
        if (next.memory !== overrides.memory) refreshAuto();
      } catch (error) {
        reportError(error, "保存实例设置");
      }
    };
    const change = (patch: Partial<InstanceOverrides>) =>
      void save(view.name, { ...overrides, ...patch });

    if (current === "general") {
      page = (
        <Group>
          <Row
            label={t("instanceSettings.name")}
            hint={t("instanceSettings.nameHint")}
            control={
              <div className="w-64">
                <TextInput
                  key={view.name}
                  defaultValue={view.name}
                  maxLength={64}
                  aria-label={t("instanceSettings.name")}
                  onBlur={(event) => {
                    const name = event.target.value.trim();
                    if (name && name !== view.name) void save(name, overrides);
                    else event.target.value = view.name;
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") event.currentTarget.blur();
                  }}
                />
              </div>
            }
          />
          <Row
            label={t("instanceSettings.folder")}
            hint={t("instanceSettings.folderHint")}
            control={
              <Button variant="secondary" onClick={() => openInstanceFolder(instance.id)}>
                <FolderOpen className="size-4" aria-hidden />
                {t("instanceSettings.openFolder")}
              </Button>
            }
          />
          <Row
            label={t("instanceSettings.syncLabel")}
            hint={
              instance.linked
                ? t("instanceSettings.syncLinkedHint")
                : t("instanceSettings.syncHint")
            }
            control={
              <Switch
                label={t("instanceSettings.syncLabel")}
                checked={!instance.linked && !overrides.syncDisabled}
                disabled={Boolean(instance.linked)}
                onChange={(participate) => change({ syncDisabled: !participate })}
              />
            }
          />
          {!instance.linked &&
            !overrides.syncDisabled &&
            (() => {
              const exclusions: InstanceSyncExclusions = {
                options: overrides.syncExclusions?.options ?? false,
                servers: overrides.syncExclusions?.servers ?? false,
                resourcePacks: overrides.syncExclusions?.resourcePacks ?? false,
                shaderPacks: overrides.syncExclusions?.shaderPacks ?? false,
                commandHistory: overrides.syncExclusions?.commandHistory ?? false,
                hotbar: overrides.syncExclusions?.hotbar ?? false,
              };
              const skippedCount = Object.values(exclusions).filter(Boolean).length;
              const items: { key: keyof InstanceSyncExclusions; labelKey: string }[] = [
                { key: "options", labelKey: "settings.syncPage.options" },
                { key: "servers", labelKey: "settings.syncPage.servers" },
                { key: "resourcePacks", labelKey: "settings.syncPage.resourcePacks" },
                { key: "shaderPacks", labelKey: "settings.syncPage.shaderPacks" },
                { key: "commandHistory", labelKey: "settings.syncPage.commandHistory" },
                { key: "hotbar", labelKey: "settings.syncPage.hotbar" },
              ];
              return (
                <>
                  <Row
                    label={t("instanceSettings.syncExclusionsLabel")}
                    hint={
                      skippedCount > 0
                        ? `${t("instanceSettings.syncExclusionsHint")} · ${t("instanceSettings.syncExclusionsCount", { n: skippedCount })}`
                        : t("instanceSettings.syncExclusionsHint")
                    }
                    control={
                      <Button
                        variant="secondary"
                        size="sm"
                        aria-expanded={syncDetailOpen}
                        onClick={() => setSyncDetailOpen((v) => !v)}
                      >
                        {syncDetailOpen ? (
                          <ChevronUp className="size-3.5" aria-hidden />
                        ) : (
                          <ChevronDown className="size-3.5" aria-hidden />
                        )}
                        {syncDetailOpen
                          ? t("settings.advanced.collapse")
                          : t("settings.advanced.expand")}
                      </Button>
                    }
                  />
                  {syncDetailOpen && (
                    <div className="grid grid-cols-2 gap-x-6 gap-y-2.5 px-4 py-3">
                      {items.map(({ key, labelKey }) => {
                        const included = !exclusions[key];
                        return (
                          <div key={key} className="flex items-center justify-between gap-3">
                            <span className="truncate text-sm">{t(labelKey)}</span>
                            <Switch
                              label={t(labelKey)}
                              checked={included}
                              onChange={(nextIncluded) =>
                                change({
                                  syncExclusions: { ...exclusions, [key]: !nextIncluded },
                                })
                              }
                            />
                          </div>
                        );
                      })}
                    </div>
                  )}
                </>
              );
            })()}
        </Group>
      );
    } else if (current === "game") {
      page = (
        <>
          <ScopedGroup
            title={t("memory.settings.group")}
            own={overrides.memory !== null}
            onReset={() => change({ memory: null })}
          >
            <MemoryPicker
              value={overrides.memory ?? { auto: game.memoryAuto, maxMemoryMb: game.maxMemoryMb }}
              autoMb={autoMb}
              onChange={(memory) => change({ memory })}
            />
          </ScopedGroup>
          <ScopedGroup
            title={t("window.group")}
            own={overrides.window !== null}
            onReset={() => change({ window: null })}
          >
            <WindowPicker
              value={overrides.window ?? game.window}
              onChange={(window) => change({ window })}
            />
          </ScopedGroup>
          <ScopedGroup
            title={t("settings.advanced.windowTitle")}
            own={overrides.customWindowTitle !== null}
            onReset={() => change({ customWindowTitle: null })}
          >
            <StackedRow
              label={t("settings.advanced.windowTitle")}
              hint={t("settings.advanced.windowTitleHint")}
            >
              <ArgsInput
                value={overrides.customWindowTitle ?? game.customWindowTitle}
                placeholder={t("settings.advanced.windowTitlePlaceholder")}
                label={t("settings.advanced.windowTitle")}
                onSave={(customWindowTitle) => change({ customWindowTitle })}
              />
            </StackedRow>
          </ScopedGroup>
          <ScopedGroup
            title={t("settings.advanced.preLaunchHook")}
            own={overrides.hooks !== null}
            onReset={() => change({ hooks: null })}
          >
            <StackedRow
              label={t("settings.advanced.preLaunchHook")}
              hint={t("settings.advanced.preLaunchHookHint")}
            >
              <ArgsInput
                value={(overrides.hooks ?? game.hooks).preLaunch}
                placeholder={t("settings.advanced.hookPlaceholder")}
                label={t("settings.advanced.preLaunchHook")}
                onSave={(preLaunch) =>
                  change({ hooks: { ...(overrides.hooks ?? game.hooks), preLaunch } })
                }
              />
            </StackedRow>
            <StackedRow
              label={t("settings.advanced.postExitHook")}
              hint={t("settings.advanced.postExitHookHint")}
            >
              <ArgsInput
                value={(overrides.hooks ?? game.hooks).postExit}
                placeholder={t("settings.advanced.hookPlaceholder")}
                label={t("settings.advanced.postExitHook")}
                onSave={(postExit) =>
                  change({ hooks: { ...(overrides.hooks ?? game.hooks), postExit } })
                }
              />
            </StackedRow>
          </ScopedGroup>
        </>
      );
    } else {
      const required = view.requiredJava;
      const chosenJava = javas?.find((j) => j.id === overrides.javaId) ?? null;
      const autoJava = javas?.find((j) => j.id === view.autoJavaId) ?? null;
      // 框里显示的：实例选了就是选的（找不到了就显示自动会用的），没选就是自动会用的
      const shownJava = chosenJava ?? autoJava;
      let javaTip: { text: string; warn: boolean } | null = null;
      if (javas && overrides.javaId && !chosenJava) {
        javaTip = { text: t("instanceSettings.java.missing"), warn: true };
      } else if (chosenJava && required !== null && chosenJava.major < required) {
        javaTip = {
          text: t("instanceSettings.java.tooOld", {
            version: instance.gameVersion,
            major: required,
          }),
          warn: true,
        };
      } else if (javas && !shownJava && required !== null) {
        javaTip = {
          text: t("instanceSettings.java.willDownload", { major: required }),
          warn: false,
        };
      }
      const own = (key: keyof InstanceOverrides) => overrides[key] !== null;
      page = (
        <>
          <ScopedGroup
            title={t("instanceSettings.java.group")}
            own={own("javaId")}
            onReset={() => change({ javaId: null })}
          >
            <div className="flex flex-col gap-2 px-4 py-3">
              <JavaField
                entry={shownJava}
                required={required}
                onOpen={() => setPickingJava(true)}
              />
              {javaTip && (
                <p className={`px-1 text-xs ${javaTip.warn ? "text-warning" : "text-fg-muted"}`}>
                  {javaTip.text}
                </p>
              )}
            </div>
            <JavaPickerDialog
              open={pickingJava}
              onClose={() => setPickingJava(false)}
              entries={javas}
              autoEntry={autoJava}
              chosenId={overrides.javaId}
              required={required}
              onPick={(javaId) => change({ javaId })}
              onEntries={setJavas}
            />
          </ScopedGroup>
          <ScopedGroup
            title={t("java.args.group")}
            own={own("launch")}
            onReset={() => change({ launch: null })}
          >
            <JavaArgsRows
              value={
                overrides.launch ?? {
                  gcPreset: game.gcPreset,
                  jvmArgs: game.jvmArgs,
                  gameArgs: game.gameArgs,
                }
              }
              onChange={(launch) => change({ launch })}
            />
          </ScopedGroup>
          <ScopedGroup
            title={t("env.group")}
            own={own("envVars")}
            onReset={() => change({ envVars: null })}
          >
            {/* 改回跟着全局时换个 key，重新读一遍全局的 */}
            <EnvVarsEditor
              key={own("envVars") ? "own" : "global"}
              value={overrides.envVars ?? game.envVars}
              onChange={(envVars) => change({ envVars })}
            />
          </ScopedGroup>
        </>
      );
    }
  }

  return (
    <SettingsShell
      open={open}
      onClose={onClose}
      title={t("instanceSettings.title")}
      subtitle={view?.name ?? instance.name}
      sections={SECTIONS.map(({ id, icon }) => ({
        id,
        icon,
        label: t(`instanceSettings.section.${id}`),
      }))}
      current={current}
      onSection={onSection}
      headerAction={
        current === "general" ? null : (
          <Button variant="secondary" size="sm" onClick={openGlobal(current)}>
            <Globe className="size-3.5" aria-hidden />
            {t("instanceSettings.openGlobal")}
          </Button>
        )
      }
    >
      {page}
    </SettingsShell>
  );
}
