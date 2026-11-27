import {
  ChevronDown,
  ChevronUp,
  Coffee,
  Download,
  Gamepad2,
  HardDrive,
  Info,
  type LucideIcon,
  Palette,
  RefreshCw,
} from "lucide-react";
import { type ReactNode, useMemo, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import type {
  AfterLaunchBehavior,
  EffectsMode,
  GameLanguage,
  Settings,
  ThemeMode,
} from "@/lib/bindings";
import { resolveEffects } from "@/lib/effects";
import { getSoundEnabled, setSoundEnabled, subscribeSoundEnabled } from "@/lib/sound";
import { type SettingsSection, useAppStore } from "@/stores/app";
import { AboutSettings } from "./settings/AboutSettings";
import { AccentPicker } from "./settings/AccentPicker";
import { DownloadSettings } from "./settings/DownloadSettings";
import { JavaSettings } from "./settings/JavaSettings";
import { LanguagePicker } from "./settings/LanguagePicker";
import { LinkedRoots } from "./settings/LinkedRoots";
import { MemorySettings } from "./settings/MemorySettings";
import { ArgsInput, Group, Row, StackedRow } from "./settings/parts";
import { SettingsShell, type ShellSearchItem } from "./settings/SettingsShell";
import { SyncSettings } from "./settings/SyncSettings";
import { WindowPicker } from "./settings/WindowPicker";
import { WorldSettings } from "./settings/WorldSettings";
import { Button } from "./ui/Button";
import { Segmented } from "./ui/Segmented";
import { Select } from "./ui/Select";
import { Switch } from "./ui/Switch";

const SECTIONS: { id: SettingsSection; icon: LucideIcon }[] = [
  { id: "appearance", icon: Palette },
  { id: "game", icon: Gamepad2 },
  { id: "sync", icon: RefreshCw },
  { id: "java", icon: Coffee },
  { id: "worlds", icon: HardDrive },
  { id: "download", icon: Download },
  { id: "about", icon: Info },
];

const GAME_LANGUAGES: GameLanguage[] = [
  "followLauncher",
  "gameDefault",
  "zhCn",
  "zhTw",
  "enUs",
  "jaJp",
];

/**
 * 设置弹窗：右上角齿轮或 Ctrl + , 打开。首页留在后面，关掉就回来，不跳页。
 * 只放整个启动器的设置；某个实例自己的内存、Java、参数、窗口在实例设置弹窗里（实例页横幅的齿轮），两个弹窗同一个外壳。
 */
export function SettingsDialog() {
  const { t } = useTranslation();
  const section = useAppStore((s) => s.settingsSection);
  const focusSettingId = useAppStore((s) => s.settingsFocusId);
  const openSettings = useAppStore((s) => s.openSettings);
  const closeSettings = useAppStore((s) => s.closeSettings);
  const clearSettingsFocus = useAppStore((s) => s.clearSettingsFocus);
  const settings = useAppStore((s) => s.settings);
  const info = useAppStore((s) => s.info);
  const lowEnd = useAppStore((s) => s.lowEnd);
  const saveSettings = useAppStore((s) => s.saveSettings);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  const soundEnabled = useSyncExternalStore(
    subscribeSoundEnabled,
    getSoundEnabled,
    getSoundEnabled,
  );
  const update = (change: (current: Settings) => Settings) => {
    if (settings) void saveSettings(change(settings));
  };

  const searchItems = useMemo<ShellSearchItem<SettingsSection>[]>(
    () => [
      {
        id: "lang",
        section: "appearance",
        label: t("language.title"),
        keywords: "language yuyan 英文 中文",
      },
      {
        id: "theme",
        section: "appearance",
        label: t("settings.theme"),
        keywords: "theme dark light 深色 浅色",
      },
      {
        id: "accent",
        section: "appearance",
        label: t("accent.label"),
        keywords: "accent color 配色 主题色",
      },
      {
        id: "effects",
        section: "appearance",
        label: t("settings.effects"),
        keywords: "effects lite rich 流畅 精美 动效",
      },
      {
        id: "motion",
        section: "appearance",
        label: t("settings.reduceMotion"),
        keywords: "motion animation 动画",
      },
      {
        id: "sound",
        section: "appearance",
        label: t("settings.soundEffects"),
        keywords: "sound audio 声音 提示音",
      },
      {
        id: "autocover",
        section: "appearance",
        label: t("settings.autoCover"),
        keywords: "cover screenshot 截图 封面",
      },
      {
        id: "gamelang",
        section: "game",
        label: t("settings.defaultGameLanguage"),
        keywords: "language options zh_cn en_us 游戏语言",
      },
      {
        id: "afterlaunch",
        section: "game",
        label: t("settings.afterLaunch"),
        keywords: "minimize release webview 最小化 释放界面 托盘",
      },
      {
        id: "refocus",
        section: "game",
        label: t("settings.refocusOnExit"),
        keywords: "focus window exit 退出 恢复 聚焦",
      },
      {
        id: "memory",
        section: "game",
        label: t("memory.settings.mode"),
        keywords: "memory ram xmx 内存 内存优化",
      },
      {
        id: "gpu",
        section: "game",
        label: t("memory.gpu.label"),
        keywords: "gpu graphics 独显 显卡",
      },
      {
        id: "window",
        section: "game",
        label: t("window.label"),
        keywords: "window resolution maximized fullscreen 分辨率 最大化 全屏",
      },
      {
        id: "linked",
        section: "game",
        label: t("linked.group"),
        keywords: "pcl hmcl minecraft 关联 目录",
      },
      {
        id: "windowTitle",
        section: "game",
        label: t("settings.advanced.windowTitle"),
        keywords: "window title 窗口标题 自定义标题 高级",
      },
      {
        id: "hooks",
        section: "game",
        label: t("settings.advanced.preLaunchHook"),
        keywords: "hook pre post script command 启动前 退出后 钩子 命令 脚本 高级",
      },
      {
        id: "discordRpc",
        section: "game",
        label: t("settings.advanced.discordRpc"),
        keywords: "discord rpc presence 活动状态 游玩状态 高级",
      },
      {
        id: "sync",
        section: "sync",
        label: t("settings.syncPage.enabled"),
        keywords:
          "sync instance options servers resourcepacks shaderpacks command hotbar 实例同步 同步 按键 服务器 通用资源包 光影 命令记录 创造 物品栏",
      },
      {
        id: "java",
        section: "java",
        label: t("java.list.title"),
        keywords: "java jdk jre runtime 运行时",
      },
      {
        id: "jvmargs",
        section: "java",
        label: t("java.args.group"),
        keywords: "jvm gc zgc args 参数 垃圾回收 环境变量",
      },
      {
        id: "gamedir",
        section: "worlds",
        label: t("settings.gameDir"),
        keywords: "directory folder path 游戏目录 路径 文件夹",
      },
      {
        id: "storage",
        section: "worlds",
        label: t("settings.storage.usageGroup"),
        keywords: "disk space cache clean 空间 清理 缓存",
      },
      {
        id: "unused",
        section: "worlds",
        label: t("settings.storage.unusedLabel"),
        keywords: "versions clean 闲置版本 残留",
      },
      {
        id: "backup",
        section: "worlds",
        label: t("settings.autoBackup"),
        keywords: "world save backup dir 世界 存档 自动备份 备份目录",
      },
      {
        id: "profile",
        section: "worlds",
        label: t("settings.storage.profileLabel"),
        keywords: "export import json 导出 导入 迁移",
      },
      {
        id: "platform",
        section: "download",
        label: t("settings.preferredPlatform"),
        keywords: "modrinth curseforge 平台 前置",
      },
      {
        id: "autoDeps",
        section: "download",
        label: t("settings.autoDependencies"),
        keywords: "dependencies mod 前置 依赖 自动安装",
      },
      {
        id: "proxy",
        section: "download",
        label: t("settings.proxy"),
        keywords: "proxy http socks5 tun clash 代理 梯子",
      },
      {
        id: "metered",
        section: "download",
        label: t("settings.meteredPolicy"),
        keywords: "metered hotspot 热点 计费 流量 限速",
      },
      {
        id: "concurrency",
        section: "download",
        label: t("settings.maxConcurrency"),
        keywords: "threads concurrent 并发 线程",
      },
      {
        id: "segments",
        section: "download",
        label: t("settings.segmentMode"),
        keywords: "range segment multi 分段 多连接 多线程 断点续传",
      },
      {
        id: "diskWriters",
        section: "download",
        label: t("settings.diskWriteMode"),
        keywords: "hdd ssd disk write 磁盘 写入 机械盘 固态",
      },
      {
        id: "speedLimit",
        section: "download",
        label: t("settings.speedLimit"),
        keywords: "bandwidth throttle cap 限速 带宽",
      },
      {
        id: "localReuse",
        section: "download",
        label: t("settings.localReuse"),
        keywords: "reuse cache hardlink dedup 本地复用 秒传 硬链接",
      },
      {
        id: "doh",
        section: "download",
        label: t("settings.doh"),
        keywords: "dns doh custom url 域名解析",
      },
      {
        id: "netdiag",
        section: "download",
        label: t("settings.netDiagLabel"),
        keywords: "network diagnostics ping neoforge 连通 延迟 诊断",
      },
      {
        id: "claim",
        section: "download",
        label: t("settings.claimFolder"),
        keywords: "browser claim 补件 下载文件夹",
      },
      {
        id: "developer",
        section: "about",
        label: "Kokoa77",
        keywords: "kokoa kokoa77 author developer github 开发者 作者 主页",
      },
      {
        id: "update",
        section: "about",
        label: t("settings.update.title"),
        keywords: "update auto check 更新 版本 自动检查",
      },
      {
        id: "privacy",
        section: "about",
        label: t("settings.aboutPage.privacyTitle"),
        keywords: "privacy token security 隐私 凭据 安全",
      },
      {
        id: "license",
        section: "about",
        label: t("settings.aboutPage.licenseTitle"),
        keywords: "license gpl open source 许可 开源 致谢",
      },
      {
        id: "onboarding",
        section: "about",
        label: t("settings.restartOnboarding"),
        keywords: "onboarding setup 引导 初始设置",
      },
    ],
    [t],
  );

  const detected = resolveEffects("auto", lowEnd ?? false);
  const pages: Record<SettingsSection, ReactNode> = settings
    ? {
        appearance: (
          <>
            <Group>
              <Row label={t("language.title")} control={<LanguagePicker />} />
              <Row
                label={t("settings.theme")}
                control={
                  <Segmented<ThemeMode>
                    label={t("settings.theme")}
                    value={settings.appearance.theme}
                    options={[
                      { value: "system", label: t("settings.themeSystem") },
                      { value: "light", label: t("settings.themeLight") },
                      { value: "dark", label: t("settings.themeDark") },
                    ]}
                    onChange={(theme) =>
                      update((s) => ({ ...s, appearance: { ...s.appearance, theme } }))
                    }
                  />
                }
              />
            </Group>
            <Group title={t("accent.label")}>
              <div className="px-4 py-3">
                <AccentPicker settings={settings} update={update} />
              </div>
            </Group>
            <Group title={t("settings.motionGroup")}>
              <Row
                label={t("settings.effects")}
                hint={
                  settings.appearance.effects === "auto"
                    ? t("settings.effectsDetected", { level: t(`settings.effects_${detected}`) })
                    : t("settings.effectsHint")
                }
                control={
                  <Segmented<EffectsMode>
                    label={t("settings.effects")}
                    value={settings.appearance.effects}
                    options={[
                      { value: "auto", label: t("settings.effects_auto") },
                      { value: "lite", label: t("settings.effects_lite") },
                      { value: "rich", label: t("settings.effects_rich") },
                    ]}
                    onChange={(effects) =>
                      update((s) => ({ ...s, appearance: { ...s.appearance, effects } }))
                    }
                  />
                }
              />
              <Row
                label={t("settings.reduceMotion")}
                hint={t("settings.reduceMotionHint")}
                control={
                  <Switch
                    label={t("settings.reduceMotion")}
                    checked={settings.appearance.reduceMotion}
                    onChange={(reduceMotion) =>
                      update((s) => ({ ...s, appearance: { ...s.appearance, reduceMotion } }))
                    }
                  />
                }
              />
              <Row
                label={t("settings.soundEffects")}
                hint={t("settings.soundEffectsHint")}
                control={
                  <Switch
                    label={t("settings.soundEffects")}
                    checked={soundEnabled}
                    onChange={setSoundEnabled}
                  />
                }
              />
            </Group>
            <Group title={t("settings.homeGroup")}>
              <Row
                label={t("settings.autoCover")}
                hint={t("settings.autoCoverHint")}
                control={
                  <Switch
                    label={t("settings.autoCover")}
                    checked={settings.appearance.autoCover}
                    onChange={(autoCover) =>
                      update((s) => ({ ...s, appearance: { ...s.appearance, autoCover } }))
                    }
                  />
                }
              />
            </Group>
          </>
        ),
        game: (
          <>
            <p className="text-fg-muted text-sm">{t("settings.gameDefaults")}</p>
            <Group title={t("settings.launchDefaultsGroup")}>
              <Row
                label={t("settings.defaultGameLanguage")}
                hint={t("settings.defaultGameLanguageHint")}
                control={
                  <div className="w-56">
                    <Select
                      label={t("settings.defaultGameLanguage")}
                      value={settings.game.defaultLanguage}
                      onChange={(value) =>
                        update((s) => ({
                          ...s,
                          game: { ...s.game, defaultLanguage: value as GameLanguage },
                        }))
                      }
                      options={GAME_LANGUAGES.map((value) => ({
                        value,
                        label: t(`settings.gameLang.${value}`),
                      }))}
                    />
                  </div>
                }
              />
              <Row
                label={t("settings.afterLaunch")}
                hint={
                  settings.game.afterLaunch === "release"
                    ? t("settings.afterLaunchReleaseHint")
                    : t("settings.afterLaunchHint")
                }
                control={
                  <Segmented<AfterLaunchBehavior>
                    label={t("settings.afterLaunch")}
                    value={settings.game.afterLaunch}
                    options={[
                      { value: "keep", label: t("settings.afterLaunchOption.keep") },
                      { value: "minimize", label: t("settings.afterLaunchOption.minimize") },
                      { value: "release", label: t("settings.afterLaunchOption.release") },
                    ]}
                    onChange={(afterLaunch) =>
                      update((s) => ({
                        ...s,
                        game: {
                          ...s.game,
                          afterLaunch,
                          releaseWebviewWhilePlaying: afterLaunch === "release",
                        },
                      }))
                    }
                  />
                }
              />
              <Row
                label={t("settings.refocusOnExit")}
                hint={t("settings.refocusOnExitHint")}
                control={
                  <Switch
                    label={t("settings.refocusOnExit")}
                    checked={settings.game.refocusOnExit}
                    onChange={(refocusOnExit) =>
                      update((s) => ({ ...s, game: { ...s.game, refocusOnExit } }))
                    }
                  />
                }
              />
            </Group>
            <MemorySettings settings={settings} update={update} />
            <Group title={t("window.group")}>
              <WindowPicker
                value={settings.game.window}
                onChange={(window) => update((s) => ({ ...s, game: { ...s.game, window } }))}
              />
            </Group>
            <LinkedRoots />
            <Group title={t("settings.advanced.group")}>
              <Row
                label={t("settings.advanced.group")}
                hint={`${t("settings.advanced.windowTitle")} · ${t("settings.advanced.preLaunchHook")} · ${t("settings.advanced.discordRpc")}`}
                control={
                  <Button
                    variant="secondary"
                    size="sm"
                    aria-expanded={advancedOpen}
                    onClick={() => setAdvancedOpen((open) => !open)}
                  >
                    {advancedOpen ? (
                      <ChevronUp className="size-3.5" aria-hidden />
                    ) : (
                      <ChevronDown className="size-3.5" aria-hidden />
                    )}
                    {advancedOpen ? t("settings.advanced.collapse") : t("settings.advanced.expand")}
                  </Button>
                }
              />
              {advancedOpen && (
                <>
                  <StackedRow
                    label={t("settings.advanced.windowTitle")}
                    hint={t("settings.advanced.windowTitleHint")}
                  >
                    <ArgsInput
                      value={settings.game.customWindowTitle}
                      placeholder={t("settings.advanced.windowTitlePlaceholder")}
                      label={t("settings.advanced.windowTitle")}
                      onSave={(customWindowTitle) =>
                        update((s) => ({ ...s, game: { ...s.game, customWindowTitle } }))
                      }
                    />
                  </StackedRow>
                  <StackedRow
                    label={t("settings.advanced.preLaunchHook")}
                    hint={t("settings.advanced.preLaunchHookHint")}
                  >
                    <ArgsInput
                      value={settings.game.hooks.preLaunch}
                      placeholder={t("settings.advanced.hookPlaceholder")}
                      label={t("settings.advanced.preLaunchHook")}
                      onSave={(preLaunch) =>
                        update((s) => ({
                          ...s,
                          game: { ...s.game, hooks: { ...s.game.hooks, preLaunch } },
                        }))
                      }
                    />
                  </StackedRow>
                  <StackedRow
                    label={t("settings.advanced.postExitHook")}
                    hint={t("settings.advanced.postExitHookHint")}
                  >
                    <ArgsInput
                      value={settings.game.hooks.postExit}
                      placeholder={t("settings.advanced.hookPlaceholder")}
                      label={t("settings.advanced.postExitHook")}
                      onSave={(postExit) =>
                        update((s) => ({
                          ...s,
                          game: { ...s.game, hooks: { ...s.game.hooks, postExit } },
                        }))
                      }
                    />
                  </StackedRow>
                  <Row
                    label={t("settings.advanced.discordRpc")}
                    hint={t("settings.advanced.discordRpcHint")}
                    control={
                      <Switch
                        label={t("settings.advanced.discordRpc")}
                        checked={settings.game.discordRpc}
                        onChange={(discordRpc) =>
                          update((s) => ({ ...s, game: { ...s.game, discordRpc } }))
                        }
                      />
                    }
                  />
                </>
              )}
            </Group>
          </>
        ),
        sync: <SyncSettings settings={settings} update={update} />,
        java: <JavaSettings settings={settings} update={update} />,
        worlds: <WorldSettings settings={settings} update={update} />,
        download: (
          <DownloadSettings
            settings={settings}
            update={update}
            focusSettingId={focusSettingId}
            onFocusConsumed={clearSettingsFocus}
          />
        ),
        about: info ? <AboutSettings info={info} onCloseSettings={closeSettings} /> : null,
      }
    : {
        appearance: null,
        game: null,
        sync: null,
        java: null,
        worlds: null,
        download: null,
        about: null,
      };

  const current = section ?? "appearance";

  return (
    <SettingsShell
      open={section !== null}
      onClose={closeSettings}
      title={t("settings.title")}
      sections={SECTIONS.map(({ id, icon }) => ({ id, icon, label: t(`settings.${id}`) }))}
      current={current}
      onSection={openSettings}
      onSelectItem={(item) => openSettings(item.section, item.id)}
      searchItems={searchItems}
      footer={t("settings.shortcut")}
    >
      {pages[current]}
    </SettingsShell>
  );
}
