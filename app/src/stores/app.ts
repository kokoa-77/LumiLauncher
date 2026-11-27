import { create } from "zustand";
import { unwrap } from "@/lib/api";
import {
  type AppInfo,
  commands,
  type GameDirInfo,
  type ImageFit,
  type ImageKind,
  type Settings,
} from "@/lib/bindings";
import { cachedLowEnd, type EffectsLevel, resolveEffects } from "@/lib/effects";
import { reportError } from "@/lib/report";

/** 首页铺满整窗的封面（首页设置，外壳画在最底下） */
export interface Backdrop {
  url: string;
  /** 截图、自动封面、自己选的图、还是只有世界自带的 64×64 小图标（小图标平滑放大当一片柔和的颜色） */
  kind: ImageKind | null;
  /** 自己选的图可能是「完整显示」（立绘靠右） */
  fit: ImageFit;
}

/** 设置弹窗的分类 */
export type SettingsSection =
  | "appearance"
  | "game"
  | "sync"
  | "java"
  | "worlds"
  | "download"
  | "about";

interface AppStore {
  info: AppInfo | null;
  settings: Settings | null;
  gameDir: GameDirInfo | null;
  /** 这台电脑是不是低配（「画面效果：自动」时用）；还没检测过为 null */
  lowEnd: boolean | null;
  /** 设置弹窗：开着时是哪一类，关着为 null */
  settingsSection: SettingsSection | null;
  /** 设置搜索或诊断跳转时自动展开/滚动的高亮目标 ID */
  settingsFocusId: string | null;
  /** 命令面板（Ctrl + K）开着吗 */
  paletteOpen: boolean;
  /** 当前页面自己报的名字（项目详情页读到名字后填，标题栏「现在在哪」的最后一级用） */
  pageTitle: string | null;
  setPageTitle: (title: string | null) => void;
  /** 首页的世界封面；不在首页或者这个世界没有图时为 null */
  backdrop: Backdrop | null;
  setBackdrop: (backdrop: Backdrop | null) => void;
  /** 弹出系统窗口换游戏目录；换成功返回 true */
  chooseGameDir: () => Promise<boolean>;
  /** 显示窗口前读取界面设置。 */
  boot: () => Promise<void>;
  /** 首帧后读取设置页和硬件判断所需的信息。 */
  loadAppDetails: () => Promise<void>;
  /** 乐观更新：界面立刻变，保存失败再退回原值 */
  saveSettings: (next: Settings) => Promise<void>;
  setLowEnd: (lowEnd: boolean) => void;
  openSettings: (section?: SettingsSection, focusId?: string | null) => void;
  clearSettingsFocus: () => void;
  closeSettings: () => void;
  setPaletteOpen: (open: boolean) => void;
}

export const useAppStore = create<AppStore>((set, get) => ({
  info: null,
  settings: null,
  gameDir: null,
  lowEnd: cachedLowEnd(),
  settingsSection: null,
  settingsFocusId: null,
  paletteOpen: false,
  pageTitle: null,
  backdrop: null,

  setBackdrop(backdrop) {
    const current = get().backdrop;
    if (
      current?.url === backdrop?.url &&
      current?.kind === backdrop?.kind &&
      current?.fit === backdrop?.fit
    ) {
      return;
    }
    set({ backdrop });
  },

  async boot() {
    set({ settings: await commands.settingsGet() });
  },

  async loadAppDetails() {
    const [info, gameDir] = await Promise.all([commands.appInfo(), commands.gameDirInfo()]);
    set((current) => ({ info, gameDir: current.gameDir ?? gameDir }));
  },

  async chooseGameDir() {
    try {
      const chosen = unwrap(await commands.gameDirChoose());
      if (!chosen) return false;
      set({ gameDir: chosen, settings: await commands.settingsGet() });
      return true;
    } catch (error) {
      reportError(error, "更换游戏目录");
      return false;
    }
  },

  async saveSettings(next) {
    const previous = get().settings;
    set({ settings: next });
    try {
      set({ settings: unwrap(await commands.settingsSet(next)) });
    } catch (error) {
      set({ settings: previous });
      reportError(error, "保存设置");
    }
  },

  setLowEnd(lowEnd) {
    set({ lowEnd });
  },

  openSettings(section = "appearance", focusId = null) {
    set({ settingsSection: section, settingsFocusId: focusId });
  },

  clearSettingsFocus() {
    set({ settingsFocusId: null });
  },

  closeSettings() {
    set({ settingsSection: null, settingsFocusId: null });
  },

  setPageTitle(pageTitle) {
    set({ pageTitle });
  },

  setPaletteOpen(paletteOpen) {
    set({ paletteOpen });
  },
}));

/** 现在实际用的画面效果（设置里选的，或者自动判断的） */
export function useEffectsLevel(): EffectsLevel {
  const mode = useAppStore((s) => s.settings?.appearance.effects ?? "auto");
  const lowEnd = useAppStore((s) => s.lowEnd);
  return resolveEffects(mode, lowEnd ?? false);
}
