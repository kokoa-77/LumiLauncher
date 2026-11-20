import { getBackendErrorMessage } from "@/lib/backendErrors";
// 实例、游戏会话、后台任务的状态。后端事件在 initGameEvents 里统一订阅，组件只读这里。

import { toast } from "sonner";
import { create } from "zustand";
import { CommandError, unwrap } from "@/lib/api";
import {
  type AppError,
  type BackupAsk,
  commands,
  events,
  type InstanceSummary,
  type JobSnapshot,
  type LogLine,
  type MemoryCheck,
  type PlayRequest,
  type PreflightReport,
  type RecentServer,
  type RecentWorld,
  type SessionSnapshot,
} from "@/lib/bindings";
import i18n from "@/lib/i18n";
import { logError, reportError } from "@/lib/report";
import { playLumiSound } from "@/lib/sound";
import { microsoftLoginReasons, useAccountStore } from "./accounts";
import { initClaimEvents } from "./claims";
import { initDropEvents } from "./drops";

/** 每个实例在前端保留的日志行数（和后端一致） */
const MAX_LOG_LINES = 5000;
let recentWorldsRequest = 0;

/** 即时拒绝和后台联网核验失败共用同一个登录或重试入口。 */
function showLaunchAuthIssue(
  { code, message }: Pick<AppError, "code" | "message">,
  retry: () => void,
): boolean {
  if (microsoftLoginReasons.has(code)) {
    useAccountStore.getState().setAddOpen(true, code);
    return true;
  }

  let explanation = message;
  switch (code) {
    case "auth.expired":
    case "auth.verify_failed":
      explanation = i18n.t("account.microsoft.verifyFailed");
      break;
    case "auth.timeout":
      explanation = i18n.t("account.microsoft.verifyTimeout");
      break;
    case "auth.account_changed":
    case "auth.authorization_changed":
      break;
    default:
      return false;
  }
  toast.error(explanation, {
    action: { label: i18n.t("account.microsoft.retry"), onClick: retry },
  });
  return true;
}

/** 启动前检查发现空闲内存不够，等用户选怎么办（D17h） */
export interface MemoryShort {
  instanceId: string;
  world?: string;
  server?: string;
  check: MemoryCheck;
}

/** 启动前本地预检发现必崩风险，等用户选怎么办（D25d） */
export interface PreflightAsk {
  instanceId: string;
  world?: string;
  server?: string;
  report: PreflightReport;
}

export interface LaunchOptions {
  /** 这次给多少内存（MB），用户在「内存不够」的提示里选了少给一点时传 */
  memoryMb?: number;
  /** 不做启动前检查（用户在提示里已经选了「照样启动」） */
  skipCheck?: boolean;
  /** 不做启动前模组/Java 预检（用户在预检提示里已经选了「仍然启动」，D25d） */
  skipPreflight?: boolean;
}

interface GameStore {
  instances: InstanceSummary[];
  instancesLoaded: boolean;
  sessions: Record<string, SessionSnapshot>;
  /** 正在做启动请求与内存检查的实例。 */
  pendingLaunches: Record<string, boolean>;
  jobs: Record<string, JobSnapshot>;
  logs: Record<string, LogLine[]>;
  /** 日志窗口当前看的实例；null 表示关着 */
  logInstance: string | null;
  /** 每次内容变化（安装、启用）加一，内容页据此刷新 */
  contentVersion: number;
  /** 每个实例的启动记录写完一次就加一，「日志」页签据此刷新列表 */
  recordsVersion: Record<string, number>;
  /** 每个实例最近玩的世界（首页换世界、实例封面用）；还没读完为 null */
  recentWorlds: RecentWorld[] | null;
  /** 各实例的多人服务器（首页直达、换世界/服务器用，D25d）；还没读完为 null */
  recentServers: RecentServer[] | null;
  /** 世界备份变了（备份、恢复、删除，D17j）就加一，备份列表和世界页签据此刷新 */
  backupsVersion: number;
  bumpBackups: () => void;
  /** 大世界第一次自动备份前问用户（D17j）；没有为 null */
  backupAsk: BackupAsk | null;
  dismissBackupAsk: () => void;
  refreshRecentWorlds: () => Promise<void>;

  refreshInstances: () => Promise<void>;
  /**
   * 启动实例；给了 `world` 直接进这个存档，给了 `server` 直接进这个服务器（1.20 起）。
   * 先看一眼空闲内存（几毫秒）：够就直接启动，不够就把 `memoryShort` 填上，弹窗问用户怎么办。
   * 真的开始启动了返回 `true`（内存不够在等用户选、启动出错都是 `false`）
   */
  launch: (
    instanceId: string,
    world?: string,
    server?: string,
    options?: LaunchOptions,
  ) => Promise<boolean>;
  /** 置顶 / 取消置顶（D17k） */
  setPinned: (instanceId: string, pinned: boolean) => Promise<void>;
  /** 挪到分组；`null` 是不分组 */
  setGroup: (instanceId: string, group: string | null) => Promise<void>;
  /** 分组改名；`to` 为 `null` 是解散分组 */
  renameGroup: (from: string, to: string | null) => Promise<void>;
  /** 正在选分组的实例（「移到分组」弹窗）；没有为 null */
  groupAsk: string | null;
  askGroup: (instanceId: string | null) => void;
  /** 在桌面上建快捷方式（给了 `world` 双击直接进这个世界） */
  createShortcut: (instanceId: string, world?: string) => Promise<void>;
  /** 空闲内存不够、等用户选怎么办；没有为 null */
  memoryShort: MemoryShort | null;
  dismissMemoryShort: () => void;
  /** 启动前预检发现问题、等用户选怎么办（D25d）；没有为 null */
  preflightAsk: PreflightAsk | null;
  dismissPreflight: () => void;
  stop: (instanceId: string) => Promise<void>;
  /**
   * 点了「结束」（D17l）：还在准备（下载、补文件）就直接取消；游戏进程已经起来了，
   * 先把 `stopAsk` 填上弹窗问一句（强行结束没保存的进度会丢）
   */
  requestStop: (instanceId: string) => void;
  /** 等用户确认要不要强行结束的实例；没有为 null */
  stopAsk: string | null;
  dismissStopAsk: () => void;
  openLog: (instanceId: string | null) => Promise<void>;
  /** 读这个实例的日志（不打开日志窗口；实例页的「日志」页签用） */
  loadLog: (instanceId: string) => Promise<void>;
  cancelJob: (jobId: string) => Promise<void>;
  bumpContent: () => void;
  /** 任务中心（标题栏右上角）开着吗 */
  tasksOpen: boolean;
  /** 是哪个任务让任务中心自动打开的：它结束几秒后自动收起；自己点开的是 null，不自动收 */
  tasksAutoJob: string | null;
  setTasksOpen: (open: boolean) => void;
  /** 从任务中心收掉一条结束了的任务（只在界面上，后端不受影响） */
  dismissJob: (jobId: string) => void;
  /** 开始一个用户刚点的后台任务（拖进来的文件开始装）：自动打开任务中心看进度 */
  showTasksFor: (jobId: string) => void;
  /** 下载历史弹窗开着吗（D18） */
  historyOpen: boolean;
  setHistoryOpen: (open: boolean) => void;
  /** 重试一个失败或取消的任务（D18）：新任务接替旧的那条；成功返回 true */
  retryJob: (jobId: string) => Promise<boolean>;
}

export const useGameStore = create<GameStore>((set, get) => ({
  instances: [],
  instancesLoaded: false,
  sessions: {},
  pendingLaunches: {},
  jobs: {},
  logs: {},
  logInstance: null,
  contentVersion: 0,
  recordsVersion: {},
  recentWorlds: null,
  recentServers: null,
  backupsVersion: 0,
  backupAsk: null,
  tasksOpen: false,
  tasksAutoJob: null,
  historyOpen: false,
  memoryShort: null,
  preflightAsk: null,
  stopAsk: null,
  groupAsk: null,

  askGroup(instanceId) {
    set({ groupAsk: instanceId });
  },

  async setPinned(instanceId, pinned) {
    try {
      unwrap(await commands.instancesSetPinned(instanceId, pinned));
      await useGameStore.getState().refreshInstances();
    } catch (error) {
      reportError(error, pinned ? "置顶实例" : "取消置顶");
    }
  },

  async setGroup(instanceId, group) {
    try {
      unwrap(await commands.instancesSetGroup(instanceId, group));
      await useGameStore.getState().refreshInstances();
    } catch (error) {
      reportError(error, "移到分组");
    }
  },

  async renameGroup(from, to) {
    try {
      await commands.instancesRenameGroup(from, to);
      await useGameStore.getState().refreshInstances();
    } catch (error) {
      reportError(error, to ? "分组改名" : "解散分组");
    }
  },

  async createShortcut(instanceId, world) {
    try {
      const name = unwrap(await commands.shortcutCreate(instanceId, world ?? null));
      toast.success(i18n.t("shortcut.created", { name }), {
        description: i18n.t(world ? "shortcut.createdWorld" : "shortcut.createdInstance"),
      });
    } catch (error) {
      reportError(error, "创建桌面快捷方式");
    }
  },

  dismissMemoryShort() {
    set({ memoryShort: null });
  },

  dismissPreflight() {
    set({ preflightAsk: null });
  },

  bumpBackups() {
    set((s) => ({ backupsVersion: s.backupsVersion + 1 }));
  },

  dismissBackupAsk() {
    set({ backupAsk: null });
  },

  setTasksOpen(open) {
    set({ tasksOpen: open, tasksAutoJob: null });
  },

  showTasksFor(jobId) {
    set({ tasksOpen: true, tasksAutoJob: jobId });
  },

  setHistoryOpen(open) {
    // 从下载面板点「查看下载历史」：面板收起，弹窗打开
    set(
      open ? { historyOpen: true, tasksOpen: false, tasksAutoJob: null } : { historyOpen: false },
    );
  },

  async retryJob(jobId) {
    try {
      unwrap(await commands.jobsRetry(jobId));
      set((s) => {
        const { [jobId]: _, ...jobs } = s.jobs;
        return { jobs };
      });
      return true;
    } catch (error) {
      reportError(error, "重试任务");
      return false;
    }
  },

  dismissJob(jobId) {
    set((s) => {
      const { [jobId]: _, ...jobs } = s.jobs;
      return { jobs };
    });
  },

  async refreshRecentWorlds() {
    const request = ++recentWorldsRequest;
    const [worldsResult, serversResult] = await Promise.allSettled([
      commands.worldsRecent().then(unwrap),
      commands.worldsRecentServers().then(unwrap),
    ]);
    // 较慢的启动扫描不能覆盖接入目录后已经得到的世界列表。
    if (request !== recentWorldsRequest) return;
    if (worldsResult.status === "fulfilled") {
      set({ recentWorlds: worldsResult.value });
    } else {
      // 读不到存档时首页退回普通卡片，不弹提示
      logError(worldsResult.reason, "读取最近的世界");
      set((s) => ({ recentWorlds: s.recentWorlds ?? [] }));
    }
    if (serversResult.status === "fulfilled") {
      set({ recentServers: serversResult.value });
    } else {
      logError(serversResult.reason, "读取最近的服务器");
      set((s) => ({ recentServers: s.recentServers ?? [] }));
    }
  },

  async refreshInstances() {
    try {
      const instances = await commands.instancesList();
      set({ instances, instancesLoaded: true });
    } catch (error) {
      set({ instancesLoaded: true });
      reportError(error, "读取实例列表");
    }
  },

  async launch(instanceId, world, server, options = {}) {
    if (get().pendingLaunches[instanceId] === true) return false;
    set((s) => ({ pendingLaunches: { ...s.pendingLaunches, [instanceId]: true } }));
    try {
      if (!options.skipPreflight && !options.skipCheck) {
        try {
          const report = unwrap(await commands.launchPreflight(instanceId));
          if (report.issues.length > 0) {
            set({ preflightAsk: { instanceId, world, server, report } });
            return false;
          }
        } catch (error) {
          // 预检出错不拦着：照常启动
          logError(error, "启动前预检");
        }
      }
      if (!options.skipCheck && options.memoryMb === undefined) {
        try {
          const check = unwrap(await commands.memoryCheck(instanceId));
          if (check && !check.enough) {
            set({ memoryShort: { instanceId, world, server, check } });
            return false;
          }
        } catch (error) {
          // 查不出来不拦着：照常启动
          logError(error, "启动前检查内存");
        }
      }
      // 新的一次启动：清空这个实例上次的日志
      set((s) => ({
        logs: { ...s.logs, [instanceId]: [] },
        memoryShort: null,
        preflightAsk: null,
      }));
      try {
        unwrap(
          await commands.launchStart(
            instanceId,
            world ?? null,
            world ? null : (server ?? null),
            options.memoryMb ?? null,
          ),
        );
        return true;
      } catch (error) {
        if (
          error instanceof CommandError &&
          showLaunchAuthIssue(error, () => void get().launch(instanceId, world, server, options))
        ) {
          return false;
        }
        reportError(error, "启动游戏");
        return false;
      }
    } finally {
      set((s) => {
        const { [instanceId]: _, ...remaining } = s.pendingLaunches;
        return { pendingLaunches: remaining };
      });
    }
  },

  async stop(instanceId) {
    try {
      await commands.launchStop(instanceId);
    } catch (error) {
      reportError(error, "结束游戏");
    }
  },

  requestStop(instanceId) {
    const { jobs, stop } = useGameStore.getState();
    const preparing = Object.values(jobs).some(
      (j) => j.key === `launch:${instanceId}` && j.state === "running",
    );
    if (preparing) void stop(instanceId);
    else set({ stopAsk: instanceId });
  },

  dismissStopAsk() {
    set({ stopAsk: null });
  },

  async openLog(instanceId) {
    set({ logInstance: instanceId });
    if (instanceId) await useGameStore.getState().loadLog(instanceId);
  },

  async loadLog(instanceId) {
    try {
      const lines = await commands.launchLog(instanceId);
      set((s) => ({ logs: { ...s.logs, [instanceId]: lines } }));
    } catch (error) {
      reportError(error, "读取游戏日志");
    }
  },

  async cancelJob(jobId) {
    try {
      await commands.jobsCancel(jobId);
    } catch (error) {
      reportError(error, "取消任务");
    }
  },

  bumpContent() {
    set((s) => ({ contentVersion: s.contentVersion + 1 }));
  },
}));

/** 正在运行（还没退出）的会话 */
export function isActive(session: SessionSnapshot | undefined): boolean {
  const kind = session?.state?.kind;
  return kind !== undefined && kind !== "exited" && kind !== "crashed";
}

/** 复制实例的任务 → 复制出来的实例 ID（复制完提示里「去看看」用，D17i） */
const copyTargets = new Map<string, string>();

export function rememberCopy(jobId: string, instanceId: string): void {
  copyTargets.set(jobId, instanceId);
}

/** 导出、复制实例结束（D17i）：导出完能「打开所在文件夹」，复制完刷新实例列表、能「去看看」 */
function onInstanceJob(payload: JobSnapshot, store: typeof useGameStore): void {
  const isExport = payload.key.startsWith("instance-export:");
  const isCopy = payload.key.startsWith("instance-copy:");
  if (!isExport && !isCopy) return;
  if (payload.state === "failed") {
    toast.error(i18n.t(isExport ? "export.failed" : "copy.failed"), {
      description: payload.error ?? undefined,
    });
    return;
  }
  if (payload.state !== "succeeded") return;
  if (isExport) {
    toast.success(i18n.t("export.done"), {
      description: payload.title,
      action: {
        label: i18n.t("export.reveal"),
        onClick: () =>
          void commands
            .instancesRevealExport(payload.id)
            .then(unwrap)
            .catch((error: unknown) => reportError(error, "打开导出的整合包所在文件夹")),
      },
    });
    return;
  }
  const target = copyTargets.get(payload.id);
  copyTargets.delete(payload.id);
  void store.getState().refreshInstances();
  void store.getState().refreshRecentWorlds();
  toast.success(i18n.t("copy.done"), {
    description: payload.title,
    action: target
      ? {
          label: i18n.t("copy.open"),
          // 不从这里引用路由（会循环引用），哈希路由改地址就会跳过去
          onClick: () => {
            window.location.hash = `#/instances/${encodeURIComponent(target)}`;
          },
        }
      : undefined,
  });
}

/**
 * 双击桌面快捷方式（D17k）：直接启动；启动器窗口最小化着，要玩家处理的时候（内存不够、启动出错、
 * 准备失败、游戏崩溃）才把窗口拉出来。
 */
async function playFromShortcut(request: PlayRequest) {
  const store = useGameStore;
  const { instanceId } = request;
  if (!store.getState().instances.some((i) => i.id === instanceId)) {
    void commands.windowShow();
    toast.error(i18n.t("shortcut.missing"), { description: i18n.t("shortcut.missingHint") });
    return;
  }
  if (isActive(store.getState().sessions[instanceId])) {
    // 已经在玩了：把游戏留着，启动器拉出来让玩家看一眼
    void commands.windowShow();
    return;
  }
  const started = await store.getState().launch(instanceId, request.world ?? undefined);
  if (!started) {
    void commands.windowShow();
    return;
  }
  const stop = store.subscribe((state, before) => {
    const failed = Object.values(state.jobs).some(
      (j) =>
        j.key === `launch:${instanceId}` &&
        j.state === "failed" &&
        before.jobs[j.id]?.state === "running",
    );
    const kind = state.sessions[instanceId]?.state.kind;
    const crashed = kind === "crashed" && before.sessions[instanceId]?.state.kind !== "crashed";
    if (failed || crashed || state.memoryShort || state.preflightAsk) {
      void commands.windowShow();
      stop();
    } else if (kind === "inWorld" || kind === "mainMenu" || kind === "exited") {
      stop();
    }
  });
}

/** 任务栏图标上的进度（D17k）：准备时按进度走，游戏窗口出来前一直转，其余不显示 */
function followTaskbarProgress() {
  let last: number | null | undefined;
  return useGameStore.subscribe((state) => {
    const job = Object.values(state.jobs).find(
      (j) => j.key.startsWith("launch:") && j.state === "running",
    );
    let value: number | null = null;
    if (job) {
      const { done, total } = job.progress;
      value = total ? Math.round(((done ?? 0) / total) * 50) / 50 : -1;
    } else if (
      Object.values(state.sessions).some(
        (s) => s.state.kind === "starting" || s.state.kind === "loading",
      )
    ) {
      value = -1;
    }
    if (value === last) return;
    last = value;
    void commands.windowProgress(value);
  });
}

/** 订阅后端事件并读取初始状态。返回取消订阅的函数。 */
export async function initGameEvents(): Promise<() => void> {
  const store = useGameStore;
  // 先读取首页的数据，进度、日志、补件和拖放监听同时建立。
  const instancesReady = store.getState().refreshInstances();
  void store.getState().refreshRecentWorlds();
  const unlisten = await Promise.all([
    events.jobUpdated.listen(({ payload }) => {
      const previous = store.getState().jobs[payload.id];
      store.setState((s) => ({ jobs: { ...s.jobs, [payload.id]: payload } }));
      // 拖进窗口装的文件（D17g）：装完说一声，能直接开始游戏
      if (payload.key.startsWith("drop:") && previous?.state === "running") {
        const instanceId = payload.key.slice("drop:".length, payload.key.lastIndexOf(":"));
        const watching = store.getState().tasksOpen;
        if (payload.state === "succeeded") {
          store.getState().bumpContent();
          void store.getState().refreshRecentWorlds();
          if (!watching)
            toast.success(i18n.t("drop.done"), {
              description: payload.title,
              action: {
                label: i18n.t("drop.play"),
                onClick: () => void store.getState().launch(instanceId),
              },
            });
        } else if (payload.state === "failed" && !watching) {
          toast.error(i18n.t("drop.failed"), { description: payload.error ?? undefined });
        }
      }
      if (previous?.state === "running") onInstanceJob(payload, store);
      const justFinished = previous?.state === "running" && payload.state === "succeeded";
      if (!justFinished) return;
      if (!payload.key.startsWith("launch:")) playLumiSound("complete");
      if (payload.key.startsWith("instance-create:") || payload.key.startsWith("modpack-import:")) {
        void store.getState().refreshInstances();
        void store.getState().refreshRecentWorlds();
      }
      if (payload.key.startsWith("content:")) store.getState().bumpContent();
      // 世界备份、恢复做完（D17j）：备份列表要刷新；恢复后世界的天数、封面也变了
      if (payload.key.startsWith("backup:")) {
        store.getState().bumpBackups();
        void store.getState().refreshRecentWorlds();
      }
    }),
    events.backupAsk.listen(({ payload }) => {
      store.setState({ backupAsk: payload });
    }),
    // 后台重新扫了挂上的别家目录，实例有变化（D19）
    events.instancesChanged.listen(() => {
      void store.getState().refreshInstances();
      void store.getState().refreshRecentWorlds();
    }),
    events.shortcutPlay.listen(({ payload }) => {
      void instancesReady.then(() => playFromShortcut(payload));
    }),
    events.sessionUpdated.listen(({ payload }) => {
      store.setState((s) => ({ sessions: { ...s.sessions, [payload.instanceId]: payload } }));
      // 启动后「上次游玩」变了，首页顺序要更新
      if (payload.state.kind === "starting") void store.getState().refreshInstances();
      // 游戏退出后天数、游玩时长、截图都会变
      if (payload.state.kind === "exited" || payload.state.kind === "crashed") {
        void store.getState().refreshRecentWorlds();
      }
    }),
    events.launchAuthBlocked.listen(({ payload }) => {
      const { instanceId, world, serverAddress, memoryMb } = payload;
      const retry = () =>
        void store.getState().launch(instanceId, world ?? undefined, serverAddress ?? undefined, {
          memoryMb: memoryMb ?? undefined,
        });
      if (!showLaunchAuthIssue(payload, retry)) toast.error(getBackendErrorMessage(payload));
    }),
    events.launchRecordSaved.listen(({ payload }) => {
      store.setState((s) => ({
        recordsVersion: {
          ...s.recordsVersion,
          [payload.instanceId]: (s.recordsVersion[payload.instanceId] ?? 0) + 1,
        },
      }));
    }),
    events.gameLogBatch.listen(({ payload }) => {
      store.setState((s) => {
        const merged = [...(s.logs[payload.instanceId] ?? []), ...payload.lines];
        return { logs: { ...s.logs, [payload.instanceId]: merged.slice(-MAX_LOG_LINES) } };
      });
    }),
  ]);

  const [jobs, sessions] = await Promise.all([commands.jobsList(), commands.launchSessions()]);
  store.setState((current) => ({
    jobs: { ...Object.fromEntries(jobs.map((j) => [j.id, j])), ...current.jobs },
    sessions: {
      ...Object.fromEntries(sessions.map((s) => [s.instanceId, s])),
      ...current.sessions,
    },
  }));
  const stopProgress = followTaskbarProgress();
  // 桌面快捷方式只需要实例和会话，不等待存档、截图或补件列表。
  void commands
    .shortcutTake()
    .then(async (request) => {
      if (!request) return;
      await instancesReady;
      await playFromShortcut(request);
    })
    .catch((error: unknown) => logError(error, "读取快捷方式的启动参数"));

  const [stopClaims, stopDrops] = await Promise.all([initClaimEvents(), initDropEvents()]);

  return () => {
    for (const stop of unlisten) stop();
    stopProgress();
    stopClaims();
    stopDrops();
  };
}
