// 账户列表、Microsoft 登录、会话预热健康度与官方皮肤/披风预览（按哈希缓存）。
// 界面只拿账户编号、健康状态和材质图片，令牌、路径、OAuth state 都在后端。

import { useEffect } from "react";
import { create } from "zustand";
import { unwrap } from "@/lib/api";
import {
  type AccountHealthView,
  type AccountsState,
  type AccountView,
  type AuthSecurityInfo,
  type AuthStage,
  commands,
  events,
  type SkinModel,
} from "@/lib/bindings";
import { logError, reportError } from "@/lib/report";

/** 标题栏的首次运行引导步骤。 */
export const ONBOARDING_STEPS = ["language", "account", "find", "look", "ready"] as const;
/** 最后是整窗的「开始使用」欢迎页。 */
export const ONBOARDING_FLOW = [...ONBOARDING_STEPS, "welcome"] as const;
export type OnboardingStep = (typeof ONBOARDING_FLOW)[number];

/** 启动遇到这些错误时打开登录弹窗，并用同一套文案说明原因。 */
export const microsoftLoginReasons = new Map([
  ["auth.no_account", "account.microsoft.required"],
  ["auth.relogin", "account.microsoft.expired"],
  ["auth.app_unavailable", "account.microsoft.unavailable"],
  ["auth.app_denied", "account.microsoft.appDenied"],
  ["auth.no_entitlement", "account.microsoft.noEntitlement"],
  ["auth.no_profile", "account.microsoft.noProfile"],
  ["auth.xbox_profile", "account.microsoft.xboxProfile"],
  ["auth.xbox_banned", "account.microsoft.xboxBanned"],
  ["auth.xbox_region", "account.microsoft.xboxRegion"],
  ["auth.xbox_child", "account.microsoft.xboxChild"],
  ["auth.account_restricted", "account.microsoft.accountRestricted"],
]);

interface AccountsStore {
  loaded: boolean;
  selected: string | null;
  accounts: AccountView[];
  microsoftAvailable: boolean;
  /** 要不要走首次运行引导 */
  onboarding: boolean;
  security: AuthSecurityInfo | null;
  health: Record<string, AccountHealthView>;
  /** 皮肤图片：皮肤哈希 → `data:` 地址 */
  skins: Record<string, string>;
  /** 披风贴图：披风哈希 → `data:` 地址 */
  capes: Record<string, string>;
  /** 账号弹层开着没有 */
  menuOpen: boolean;
  /** Microsoft 登录弹窗及本次打开的原因 */
  addOpen: boolean;
  loginReason: string | null;
  loginPending: boolean;
  loginCancelling: boolean;
  authStage: AuthStage | null;
  authExpiresAt: number | null;
  /** 因缺账号被拦下的启动操作，登录成功后自动续跑 */
  pendingLaunchRetry: (() => void) | null;
  /** 账号安全透明中心弹窗 */
  securityOpen: boolean;
  /** 皮肤预览弹窗开着时是哪个账户 */
  skinFor: string | null;
  /** 皮肤/披风变更操作进行中 */
  skinBusy: boolean;
  /** 首次运行引导最后点了「新建实例」：布局里常驻的新建实例弹窗打开 */
  startNew: boolean;
  /** 首次运行引导走到哪一步（标题栏中间的步骤胶囊也看它） */
  onboardingStep: OnboardingStep;
  /** 走到过的最远一步（这之前的胶囊都能点回去） */
  onboardingReached: number;
  /** 找游戏正在处理，或者需要玩家确认跳过失败项 */
  onboardingFindBlocked: boolean;

  refresh(): Promise<void>;
  prewarm(accountId?: string): Promise<AccountHealthView | null>;
  loginMicrosoft(): Promise<boolean>;
  cancelMicrosoft(): void;
  loadSkin(account: AccountView): void;
  loadCape(hash: string): void;
  refreshProfile(accountId: string): Promise<void>;
  uploadSkin(accountId: string, model: SkinModel | null): Promise<boolean>;
  resetSkin(accountId: string): Promise<void>;
  setCape(accountId: string, capeId: string | null): Promise<void>;
  select(id: string): Promise<void>;
  remove(id: string): Promise<void>;
  setMenuOpen(open: boolean): void;
  setAddOpen(open: boolean, reason?: string, retry?: (() => void) | null): void;
  setSecurityOpen(open: boolean): void;
  openSkin(id: string | null): void;
  setStartNew(open: boolean): void;
  setOnboardingStep(step: OnboardingStep): void;
  finishOnboarding(): Promise<void>;
  /** 设置「关于」里的「重新进行初始设置」：从第一步再走一遍（账号、实例、存档都不动） */
  restartOnboarding(): Promise<void>;
}

function fromState(state: AccountsState) {
  const health: Record<string, AccountHealthView> = {};
  for (const [key, value] of Object.entries(state.health ?? {})) {
    if (value) health[key] = value;
  }
  return {
    loaded: true,
    selected: state.selected,
    accounts: state.accounts,
    microsoftAvailable: state.microsoftAvailable,
    onboarding: state.onboarding,
    security: state.security ?? null,
    health,
  };
}

// 取消命令没有单独的登录 ID。等取消请求结束后才能发起下一次登录。
let cancelInFlight: Promise<void> | null = null;

export const useAccountStore = create<AccountsStore>((set, get) => ({
  loaded: false,
  selected: null,
  accounts: [],
  microsoftAvailable: false,
  onboarding: false,
  security: null,
  health: {},
  skins: {},
  capes: {},
  menuOpen: false,
  addOpen: false,
  loginReason: null,
  loginPending: false,
  loginCancelling: false,
  authStage: null,
  authExpiresAt: null,
  pendingLaunchRetry: null,
  securityOpen: false,
  skinFor: null,
  skinBusy: false,
  startNew: false,
  onboardingStep: ONBOARDING_STEPS[0],
  onboardingReached: 0,
  onboardingFindBlocked: false,

  async refresh() {
    try {
      set(fromState(await commands.accountsList()));
    } catch (error) {
      set({ loaded: true });
      reportError(error, "读取账户");
    }
  },

  async prewarm(accountId) {
    if (!get().microsoftAvailable) return null;
    try {
      const view = unwrap(await commands.accountsPrewarm(accountId ?? null));
      const targetId = accountId ?? get().selected;
      if (targetId) {
        set((s) => ({ health: { ...s.health, [targetId]: view } }));
      }
      return view;
    } catch {
      return null;
    }
  },

  async loginMicrosoft() {
    if (!get().microsoftAvailable || get().loginPending) return false;
    set({
      loginPending: true,
      loginCancelling: false,
      loginReason: null,
      authStage: "waitingBrowser",
      authExpiresAt: null,
    });
    try {
      const next = unwrap(await commands.accountsMicrosoftLogin());
      set(fromState(next));
      const ok = !get().loginCancelling;
      if (ok) {
        const retry = get().pendingLaunchRetry;
        if (retry) {
          set({ pendingLaunchRetry: null });
          queueMicrotask(retry);
        }
      }
      return ok;
    } finally {
      await cancelInFlight;
      cancelInFlight = null;
      set({
        loginPending: false,
        loginCancelling: false,
        authStage: null,
        authExpiresAt: null,
      });
    }
  },

  cancelMicrosoft() {
    if (!get().loginPending || get().loginCancelling) return;
    set({ loginCancelling: true });
    cancelInFlight = commands.accountsMicrosoftCancel().then(
      () => undefined,
      (error: unknown) => {
        logError(error, "取消 Microsoft 登录");
        set({ loginCancelling: false });
      },
    );
  },

  loadSkin(account) {
    const hash = account.skinHash;
    if (!hash || hash in get().skins) return;
    set((s) => ({ skins: { ...s.skins, [hash]: "" } }));
    commands
      .accountsSkin(account.id)
      .then((url) => {
        if (url) set((s) => ({ skins: { ...s.skins, [hash]: url } }));
      })
      .catch((error: unknown) => logError(error, "读取皮肤图片"));
  },

  loadCape(hash) {
    if (!hash || hash in get().capes) return;
    set((s) => ({ capes: { ...s.capes, [hash]: "" } }));
    commands
      .accountsCapeTexture(hash)
      .then((url) => {
        if (url) set((s) => ({ capes: { ...s.capes, [hash]: url } }));
      })
      .catch((error: unknown) => logError(error, "读取披风图片"));
  },

  async refreshProfile(accountId) {
    if (get().skinBusy) return;
    set({ skinBusy: true });
    try {
      set(fromState(unwrap(await commands.accountsSkinRefresh(accountId))));
    } catch (error) {
      reportError(error, "刷新官方皮肤档案");
    } finally {
      set({ skinBusy: false });
    }
  },

  async uploadSkin(accountId, model) {
    if (get().skinBusy) return false;
    set({ skinBusy: true });
    try {
      const updated = unwrap(await commands.accountsSkinUpload(accountId, model));
      if (!updated) return false;
      set(fromState(updated));
      return true;
    } catch (error) {
      reportError(error, "上传皮肤");
      return false;
    } finally {
      set({ skinBusy: false });
    }
  },

  async resetSkin(accountId) {
    if (get().skinBusy) return;
    set({ skinBusy: true });
    try {
      set(fromState(unwrap(await commands.accountsSkinReset(accountId))));
    } catch (error) {
      reportError(error, "重置默认皮肤");
    } finally {
      set({ skinBusy: false });
    }
  },

  async setCape(accountId, capeId) {
    if (get().skinBusy) return;
    set({ skinBusy: true });
    try {
      set(fromState(unwrap(await commands.accountsCapeSet(accountId, capeId))));
    } catch (error) {
      reportError(error, "切换官方披风");
    } finally {
      set({ skinBusy: false });
    }
  },

  async select(id) {
    try {
      unwrap(await commands.accountsSelect(id));
      set({ selected: id });
    } catch (error) {
      reportError(error, "切换账户");
    }
  },

  async remove(id) {
    try {
      set(fromState(unwrap(await commands.accountsRemove(id))));
    } catch (error) {
      reportError(error, "移除账户");
    }
  },

  setMenuOpen(open) {
    set({ menuOpen: open });
  },
  setAddOpen(open, reason, retry) {
    set((s) => ({
      addOpen: open,
      loginReason: open ? (reason ?? null) : null,
      pendingLaunchRetry: open ? (retry !== undefined ? retry : s.pendingLaunchRetry) : null,
      menuOpen: open ? false : s.menuOpen,
    }));
  },
  setSecurityOpen(open) {
    set((s) => ({ securityOpen: open, menuOpen: open ? false : s.menuOpen }));
  },
  openSkin(id) {
    set({ skinFor: id, menuOpen: id ? false : get().menuOpen });
  },
  setStartNew(open) {
    set({ startNew: open });
  },
  setOnboardingStep(step) {
    const { onboardingStep, onboardingFindBlocked } = get();
    if (onboardingStep === "find" && onboardingFindBlocked) return;
    set((s) => ({
      onboardingStep: step,
      onboardingReached: Math.max(s.onboardingReached, ONBOARDING_FLOW.indexOf(step)),
    }));
  },

  async finishOnboarding() {
    try {
      unwrap(await commands.accountsFinishOnboarding());
    } catch (error) {
      // 记不下来也让玩家先用，下次打开再走一遍引导
      logError(error, "记下首次运行引导走完了");
    }
    set({ onboarding: false });
  },

  async restartOnboarding() {
    try {
      unwrap(await commands.accountsRestartOnboarding());
      set({
        onboarding: true,
        onboardingStep: ONBOARDING_STEPS[0],
        onboardingReached: 0,
        onboardingFindBlocked: false,
        menuOpen: false,
      });
    } catch (error) {
      reportError(error, "重新进行初始设置");
    }
  },
}));

/** 订阅后端推送的登录四阶段进度与会话健康状态，并在空闲时温和预热一次选中账户。 */
export async function initAccountEvents(): Promise<() => void> {
  const unlisten = await Promise.all([
    events.authProgressUpdated.listen(({ payload }) => {
      useAccountStore.setState({
        authStage: payload.stage,
        authExpiresAt:
          payload.expiresInSecs !== null ? Date.now() + payload.expiresInSecs * 1000 : null,
      });
    }),
    events.accountHealthUpdated.listen(({ payload }) => {
      useAccountStore.setState((s) => ({
        health: {
          ...s.health,
          [payload.accountId]: {
            status: payload.status,
            verifiedAtMs: payload.verifiedAtMs,
            errorCode: payload.errorCode,
          },
        },
      }));
    }),
  ]);
  const timer = window.setTimeout(() => {
    const state = useAccountStore.getState();
    if (state.microsoftAvailable && state.selected && !state.onboarding) {
      void state.prewarm(state.selected);
    }
  }, 1500);
  return () => {
    window.clearTimeout(timer);
    for (const stop of unlisten) stop();
  };
}

/** 现在用的账户 */
export function useCurrentAccount(): AccountView | null {
  return useAccountStore(
    (s) => s.accounts.find((a) => a.id === s.selected) ?? s.accounts[0] ?? null,
  );
}

/** 账户的皮肤图片（没有皮肤、还没读到时是 `null`）；第一次用到时去后端取 */
export function useSkin(account: AccountView | null | undefined): string | null {
  const url = useAccountStore((s) => (account?.skinHash ? s.skins[account.skinHash] : undefined));
  const loadSkin = useAccountStore((s) => s.loadSkin);
  useEffect(() => {
    if (account?.skinHash) loadSkin(account);
  }, [account, loadSkin]);
  return url || null;
}

/** 按披风哈希读取官方披风贴图；首次用到时向后端拉取。 */
export function useCape(hash: string | null | undefined): string | null {
  const url = useAccountStore((s) => (hash ? s.capes[hash] : undefined));
  const loadCape = useAccountStore((s) => s.loadCape);
  useEffect(() => {
    if (hash) loadCape(hash);
  }, [hash, loadCape]);
  return url || null;
}
