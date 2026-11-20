import { afterEach, describe, expect, it, vi } from "vitest";
import { commands } from "@/lib/bindings";
import { useAccountStore } from "./accounts";
import { useGameStore } from "./game";

afterEach(() => {
  vi.restoreAllMocks();
  useAccountStore.setState(useAccountStore.getInitialState());
  useGameStore.setState(useGameStore.getInitialState());
});

describe("游玩前的 Microsoft 登录入口", () => {
  it.each(["auth.no_account", "auth.relogin", "auth.xbox_profile", "auth.account_restricted"])(
    "%s 打开登录弹窗并保留受阻原因",
    async (code) => {
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
    },
  );
});
