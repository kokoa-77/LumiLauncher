import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { commands, events } from "@/lib/bindings";
import { setLanguagePreference } from "@/lib/i18n";
import { useUpdateStore } from "./updates";

vi.mock("@/lib/bindings", () => ({
  commands: {
    appLog: vi.fn(),
    updatesMode: vi.fn(),
    updatesCheck: vi.fn(),
    updatesInstall: vi.fn(),
  },
  events: { updateProgress: { listen: vi.fn() } },
}));

const offer = { version: "0.7.0", notes: "本次修复了启动问题。" };

beforeEach(async () => {
  await setLanguagePreference("zh-CN");
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.resetAllMocks();
  useUpdateStore.setState(useUpdateStore.getInitialState());
});

describe("启动器更新", () => {
  it("安装版在启动时只获取一次更新信息，等待用户决定是否下载", async () => {
    vi.mocked(commands.updatesMode).mockResolvedValue("installed");
    vi.mocked(commands.updatesCheck).mockResolvedValue({ status: "ok", data: offer });

    await useUpdateStore.getState().check(true);
    await useUpdateStore.getState().check(true);

    expect(useUpdateStore.getState().status).toEqual({ kind: "available", offer });
    expect(commands.updatesCheck).toHaveBeenCalledTimes(1);
    expect(commands.updatesInstall).not.toHaveBeenCalled();
  });

  it("便携版无需联网检查更新", async () => {
    vi.mocked(commands.updatesMode).mockResolvedValue("portable");

    await useUpdateStore.getState().check(true);

    expect(useUpdateStore.getState().mode).toBe("portable");
    expect(commands.updatesCheck).not.toHaveBeenCalled();
  });

  it("用户选择安装后订阅下载进度，完成时交给安装程序", async () => {
    vi.mocked(commands.updatesMode).mockResolvedValue("installed");
    vi.mocked(commands.updatesCheck).mockResolvedValue({ status: "ok", data: offer });
    let publish: () => void = () => {
      throw new Error("尚未订阅更新进度");
    };
    const unlisten = vi.fn();
    vi.mocked(events.updateProgress.listen).mockImplementation(async (callback) => {
      publish = () =>
        callback({
          id: 1,
          event: "update-progress",
          payload: { downloaded: 64, total: 128 },
        });
      return unlisten;
    });
    vi.mocked(commands.updatesInstall).mockImplementation(async () => {
      publish();
      expect(useUpdateStore.getState().status).toEqual({
        kind: "installing",
        offer,
        progress: { downloaded: 64, total: 128 },
      });
      return { status: "ok", data: null };
    });

    await useUpdateStore.getState().check();
    await useUpdateStore.getState().install();

    expect(commands.updatesInstall).toHaveBeenCalledWith("0.7.0");
    expect(useUpdateStore.getState().status).toEqual({ kind: "handoff", offer });
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it.each(["updates.recheck", "updates.signature_invalid"])("%s 要求重新检查更新", async (code) => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.mocked(commands.appLog).mockResolvedValue(undefined);
    vi.mocked(commands.updatesMode).mockResolvedValue("installed");
    vi.mocked(commands.updatesCheck).mockResolvedValue({ status: "ok", data: offer });
    vi.mocked(events.updateProgress.listen).mockResolvedValue(() => undefined);
    vi.mocked(commands.updatesInstall).mockResolvedValue({
      status: "error",
      error: { code, message: "此更新无法安装", detail: null },
    });

    await useUpdateStore.getState().check();
    await useUpdateStore.getState().install();

    expect(useUpdateStore.getState().status).toEqual({
      kind: "installRejected",
      message: "此更新无法安装",
    });
    await useUpdateStore.getState().install();
    expect(commands.updatesInstall).toHaveBeenCalledTimes(1);
  });
});
