// 账户列表、Microsoft 登录和只读皮肤预览（按哈希缓存）。
// 界面只拿账户编号和皮肤图片，令牌、路径都在后端。

import { useEffect } from "react";
import { create } from "zustand";
import { unwrap } from "@/lib/api";
import { type AccountsState, type AccountView, commands } from "@/lib/bindings";
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
  ["auth.no_entitlement", "account.microsoft.noEntitlement"],
  ["auth.no_profile", "account.microsoft.noProfile"],
  ["auth.xbox_profile", "account.microsoft.xboxProfile"],
  ["auth.account_restricted", "account.microsoft.accountRestricted"],
]);

interface AccountsStore {
  loaded: boolean;
  selected: string | null;
  accounts: AccountView[];
  microsoftAvailable: boolean;
  /** 要不要走首次运行引导 */
  onboarding: boolean;
  /** 皮肤图片：皮肤哈希 → `data:` 地址 */
  skins: Record<string, string>;
  /** 账号弹层开着没有 */
  menuOpen: boolean;
  /** Microsoft 登录弹窗及本次打开的原因 */
  addOpen: boolean;
  loginReason: string | null;
  loginPending: boolean;
  loginCancelling: boolean;
  /** 皮肤预览弹窗开着时是哪个账户 */
  skinFor: string | null;
  /** 首次运行引导最后点了「新建实例」：布局里常驻的新建实例弹窗打开 */
  startNew: boolean;
  /** 首次运行引导走到哪一步（标题栏中间的步骤胶囊也看它） */
  onboardingStep: OnboardingStep;
  /** 走到过的最远一步（这之前的胶囊都能点回去） */
  onboardingReached: number;
  /** 找游戏正在处理，或者需要玩家确认跳过失败项 */
  onboardingFindBlocked: boolean;

  refresh(): Promise<void>;
  loginMicrosoft(): Promise<boolean>;
  cancelMicrosoft(): void;
  loadSkin(account: AccountView): void;
  select(id: string): Promise<void>;
  remove(id: string): Promise<void>;
  setMenuOpen(open: boolean): void;
  setAddOpen(open: boolean, reason?: string): void;
  openSkin(id: string | null): void;
  setStartNew(open: boolean): void;
  setOnboardingStep(step: OnboardingStep): void;
  finishOnboarding(): Promise<void>;
  /** 设置「关于」里的「重新进行初始设置」：从第一步再走一遍（账号、实例、存档都不动） */
  restartOnboarding(): Promise<void>;
}

function fromState(state: AccountsState) {
  return {
    loaded: true,
    selected: state.selected,
    accounts: state.accounts,
    microsoftAvailable: state.microsoftAvailable,
    onboarding: state.onboarding,
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
  skins: {},
  menuOpen: false,
  addOpen: false,
  loginReason: null,
  loginPending: false,
  loginCancelling: false,
  skinFor: null,
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

  async loginMicrosoft() {
    if (!get().microsoftAvailable || get().loginPending) return false;
    set({ loginPending: true, loginCancelling: false, loginReason: null });
    try {
      set(fromState(unwrap(await commands.accountsMicrosoftLogin())));
      return !get().loginCancelling;
    } finally {
      await cancelInFlight;
      cancelInFlight = null;
      set({ loginPending: false, loginCancelling: false });
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
    if (!hash || get().skins[hash]) return;
    // 先占个位，同一张不重复取
    set((s) => ({ skins: { ...s.skins, [hash]: "" } }));
    commands
      .accountsSkin(account.id)
      .then((url) => {
        if (url) set((s) => ({ skins: { ...s.skins, [hash]: url } }));
      })
      .catch((error: unknown) => logError(error, "读取皮肤图片"));
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
      unwrap(await commands.accountsRemove(id));
      await get().refresh();
    } catch (error) {
      reportError(error, "移除账户");
    }
  },

  setMenuOpen(open) {
    set({ menuOpen: open });
  },
  setAddOpen(open, reason) {
    set({ addOpen: open, loginReason: reason ?? null, menuOpen: open ? false : get().menuOpen });
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
