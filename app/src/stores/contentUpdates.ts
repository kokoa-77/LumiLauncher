import { create } from "zustand";
import { unwrap } from "@/lib/api";
import {
  type ContentKind,
  commands,
  type UpdateCheckResult,
  type UpdateInfo,
} from "@/lib/bindings";
import { useGameStore } from "@/stores/game";

/** 会话级更新检查结果有效期（10 分钟）；期间实例内容有变化会通过 `contentVersion` 自动失效。 */
const CACHE_TTL_MS = 10 * 60 * 1000;

export interface UpdateCacheEntry {
  contentVersion: number;
  checkedAt: number;
  result: UpdateCheckResult;
}

interface ContentUpdatesState {
  entries: Record<string, UpdateCacheEntry>;
  checking: Record<string, boolean>;
  check: (
    instanceId: string,
    kind: ContentKind,
    options?: { force?: boolean },
  ) => Promise<UpdateCheckResult | null>;
  invalidate: (instanceId?: string) => void;
  clear: () => void;
}

export function updateCacheKey(instanceId: string, kind: ContentKind): string {
  return `${instanceId}:${kind}`;
}

export function updatesMapOf(entry: UpdateCacheEntry | undefined): Record<string, UpdateInfo> {
  if (!entry) return {};
  return Object.fromEntries(entry.result.updates.map((u) => [u.fileName, u]));
}

/** 正在进行的请求按 key 复用，避免首页与实例页并发重复打接口。 */
const inflight = new Map<string, Promise<UpdateCheckResult | null>>();

function isFresh(
  entry: UpdateCacheEntry | undefined,
  contentVersion: number,
): entry is UpdateCacheEntry {
  if (!entry) return false;
  if (entry.contentVersion !== contentVersion) return false;
  if (entry.result.lookupFailed) return false;
  return Date.now() - entry.checkedAt < CACHE_TTL_MS;
}

/**
 * 首页与实例内容页共享的会话级更新检查缓存：
 * - 首次进入实例自动静默检查，首页已查过则 0ms 秒显；
 * - 内容变更（安装、更新、删除、回滚、锁定版本）时通过 `contentVersion` 或 `invalidate` 立即失效；
 * - 手动点「刷新」传 `force: true` 绕过缓存强制重查。
 */
export const useContentUpdatesStore = create<ContentUpdatesState>((set, get) => ({
  entries: {},
  checking: {},

  async check(instanceId, kind, options) {
    const force = options?.force ?? false;
    const key = updateCacheKey(instanceId, kind);
    const contentVersion = useGameStore.getState().contentVersion;
    const cached = get().entries[key];
    if (!force && isFresh(cached, contentVersion)) {
      return cached.result;
    }
    if (!force) {
      const pending = inflight.get(key);
      if (pending) return pending;
    }

    const request = (async () => {
      set((state) => ({ checking: { ...state.checking, [key]: true } }));
      try {
        const result = unwrap(await commands.contentCheckUpdates(instanceId, kind));
        // 若检查期间内容版本已经变化，不把过期结果写回缓存
        const latestVersion = useGameStore.getState().contentVersion;
        if (latestVersion === contentVersion) {
          set((state) => ({
            entries: {
              ...state.entries,
              [key]: { contentVersion, checkedAt: Date.now(), result },
            },
          }));
        }
        return result;
      } finally {
        inflight.delete(key);
        set((state) => {
          if (!state.checking[key]) return state;
          const next = { ...state.checking };
          delete next[key];
          return { checking: next };
        });
      }
    })();

    inflight.set(key, request);
    return request;
  },

  invalidate(instanceId) {
    if (!instanceId) {
      inflight.clear();
      set({ entries: {} });
      return;
    }
    const prefix = `${instanceId}:`;
    for (const key of inflight.keys()) {
      if (key.startsWith(prefix)) inflight.delete(key);
    }
    set((state) => {
      const next: Record<string, UpdateCacheEntry> = {};
      for (const [k, v] of Object.entries(state.entries)) {
        if (!k.startsWith(prefix)) next[k] = v;
      }
      return { entries: next };
    });
  },

  clear() {
    inflight.clear();
    set({ entries: {}, checking: {} });
  },
}));
