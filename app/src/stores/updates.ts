import { create } from "zustand";
import { CommandError, unwrap } from "@/lib/api";
import {
  commands,
  events,
  type UpdateMode,
  type UpdateOffer,
  type UpdateProgress,
} from "@/lib/bindings";
import { logError, userMessage } from "@/lib/report";

type UpdateStatus =
  | { kind: "idle" | "checking" | "current" }
  | { kind: "checkError" | "installRejected"; message: string }
  | { kind: "available" | "handoff"; offer: UpdateOffer }
  | { kind: "installing"; offer: UpdateOffer; progress: UpdateProgress }
  | { kind: "installError"; offer: UpdateOffer; message: string };

interface UpdateStore {
  mode: UpdateMode | null;
  status: UpdateStatus;
  startupChecked: boolean;
  loadMode(): Promise<UpdateMode | null>;
  check(startup?: boolean): Promise<UpdateOffer | null>;
  install(): Promise<void>;
}

let modeRequest: Promise<UpdateMode | null> | null = null;

export const useUpdateStore = create<UpdateStore>((set, get) => ({
  mode: null,
  status: { kind: "idle" },
  startupChecked: false,

  loadMode() {
    const mode = get().mode;
    if (mode) return Promise.resolve(mode);
    modeRequest ??= commands
      .updatesMode()
      .then((next) => {
        set({ mode: next });
        return next;
      })
      .catch((error: unknown) => {
        logError(error, "读取启动器更新模式");
        set({ status: { kind: "checkError", message: userMessage(error) } });
        return null;
      })
      .finally(() => {
        modeRequest = null;
      });
    return modeRequest;
  },

  async check(startup = false) {
    const { startupChecked, status } = get();
    if (startup) {
      if (startupChecked) return null;
      set({ startupChecked: true });
      if (status.kind !== "idle") return null;
    }
    if (status.kind === "checking" || status.kind === "installing" || status.kind === "handoff") {
      return null;
    }

    set({ status: { kind: "checking" } });
    const mode = await get().loadMode();
    if (mode !== "installed") {
      if (mode !== null) set({ status: { kind: "idle" } });
      return null;
    }
    try {
      const offer = unwrap(await commands.updatesCheck());
      set({ status: offer ? { kind: "available", offer } : { kind: "current" } });
      return offer;
    } catch (error) {
      logError(error, "检查启动器更新");
      set({ status: { kind: "checkError", message: userMessage(error) } });
      return null;
    }
  },

  async install() {
    const { mode, status } = get();
    if (mode !== "installed" || (status.kind !== "available" && status.kind !== "installError")) {
      return;
    }
    const { offer } = status;
    set({ status: { kind: "installing", offer, progress: { downloaded: 0, total: null } } });
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await events.updateProgress.listen(({ payload }) => {
        set((state) =>
          state.status.kind === "installing"
            ? { status: { ...state.status, progress: payload } }
            : state,
        );
      });
      unwrap(await commands.updatesInstall(offer.version));
      set({ status: { kind: "handoff", offer } });
    } catch (error) {
      logError(error, "安装启动器更新");
      const message = userMessage(error);
      const mustRecheck =
        error instanceof CommandError &&
        (error.code === "updates.recheck" || error.code === "updates.signature_invalid");
      set({
        status: mustRecheck
          ? { kind: "installRejected", message }
          : { kind: "installError", offer, message },
      });
    } finally {
      unlisten?.();
    }
  },
}));
