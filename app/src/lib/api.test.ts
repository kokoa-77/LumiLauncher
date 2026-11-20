import { beforeEach, describe, expect, it } from "vitest";
import { CommandError, unwrap } from "./api";
import type { AppError } from "./bindings";
import { setLanguagePreference } from "./i18n";

describe("unwrap", () => {
  beforeEach(async () => {
    await setLanguagePreference("zh-CN");
  });

  it("中文界面下把后端给的 AppError 原样带出来", () => {
    const error = { code: "worlds.notFound", message: "找不到这个世界", detail: null };
    expect(() => unwrap({ status: "error", error })).toThrow(
      expect.objectContaining({ code: "worlds.notFound", message: "找不到这个世界" }),
    );
  });

  it("英文界面下按错误码本地化并保留原始 message", async () => {
    await setLanguagePreference("en");
    const error = { code: "worlds.notFound", message: "找不到这个世界", detail: null };
    expect(() => unwrap({ status: "error", error })).toThrow(
      expect.objectContaining({
        code: "worlds.notFound",
        rawMessage: "找不到这个世界",
      }),
    );
  });

  it("框架拒绝时给的一串文字也变成有中文提示、留着原文的错误", () => {
    const raw = "Command worlds_background_pick not found";
    try {
      // 模拟框架给的不是 AppError
      unwrap({ status: "error", error: raw as unknown as AppError });
      expect.unreachable();
    } catch (error) {
      expect(error).toBeInstanceOf(CommandError);
      const e = error as CommandError;
      expect(e.code).toBe("ipc.rejected");
      expect(e.message).not.toBe("");
      expect(e.detail).toBe(raw);
    }
  });
});
