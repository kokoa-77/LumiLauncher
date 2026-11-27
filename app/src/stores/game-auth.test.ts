import { afterEach, describe, expect, it, vi } from "vitest";
import { type AccountsState, type AccountView, commands } from "@/lib/bindings";
import { setLanguagePreference } from "@/lib/i18n";
import { useAccountStore } from "./accounts";
import { useGameStore } from "./game";

const baseSecurity = {
  vaultKind: "windowsCredentialManager" as const,
  vaultPersistent: true,
  pkceMethod: "S256",
  responseMode: "form_post",
  sessionPipe: true,
  callbackMode: "dualstack-localhost",
};

const sampleAccount: AccountView = {
  id: "acc-1",
  name: "Alex",
  uuid: "54f2906a-88dd-444e-bcb3-c68132a30a57",
  hasSkin: true,
  skinModel: "slim",
  skinHash: "abc123",
  capes: [],
};

afterEach(async () => {
  vi.restoreAllMocks();
  await setLanguagePreference("zh-CN");
  useAccountStore.setState(useAccountStore.getInitialState());
  useGameStore.setState(useGameStore.getInitialState());
});

describe("游玩前的 Microsoft 登录入口与状态保护", () => {
  it.each([
    "auth.no_account",
    "auth.relogin",
    "auth.xbox_profile",
    "auth.xbox_child",
    "auth.account_restricted",
  ])("%s 打开登录弹窗并保留受阻原因", async (code) => {
    vi.spyOn(commands, "launchStart").mockResolvedValue({
      status: "error",
      error: { code, message: "需要通过 Microsoft 登录", detail: null },
    });
    vi.spyOn(commands, "appLog").mockResolvedValue(undefined);
    vi.spyOn(console, "error").mockImplementation(() => undefined);

    const started = await useGameStore.getState().launch("test", "世界", undefined, {
      skipCheck: true,
    });

    expect(started).toBe(false);
    expect(useAccountStore.getState()).toMatchObject({ addOpen: true, loginReason: code });
    expect(useAccountStore.getState().pendingLaunchRetry).toBeTypeOf("function");
  });

  it("loadSkin 并发去重即使占位为空字符串也不会重复请求", () => {
    const spy = vi.spyOn(commands, "accountsSkin").mockResolvedValue(null);
    const store = useAccountStore.getState();
    store.loadSkin(sampleAccount);
    store.loadSkin(sampleAccount);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("登录完成后自动续跑此前被拦截的启动请求", async () => {
    const retry = vi.fn();
    const nextState: AccountsState = {
      selected: sampleAccount.id,
      accounts: [sampleAccount],
      onboarding: false,
      microsoftAvailable: true,
      security: baseSecurity,
      health: {},
    };
    vi.spyOn(commands, "accountsMicrosoftLogin").mockResolvedValue({
      status: "ok",
      data: nextState,
    });
    useAccountStore.setState({ microsoftAvailable: true });
    useAccountStore.getState().setAddOpen(true, "auth.no_account", retry);

    const ok = await useAccountStore.getState().loginMicrosoft();
    await Promise.resolve();

    expect(ok).toBe(true);
    expect(retry).toHaveBeenCalledTimes(1);
    expect(useAccountStore.getState().pendingLaunchRetry).toBeNull();
  });
});
