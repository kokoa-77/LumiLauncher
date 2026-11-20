import { create } from "zustand";
import { unwrap } from "@/lib/api";
import { commands, type Favorite, type FavoriteKind, type ProjectHit } from "@/lib/bindings";
import { reportError } from "@/lib/report";

interface FavoritesStore {
  /** 还没读过是 null */
  items: Favorite[] | null;
  load: () => Promise<void>;
  /** 收藏 / 取消收藏：界面先变，保存失败再变回去并提示 */
  toggle: (kind: FavoriteKind, hit: ProjectHit) => Promise<void>;
}

const same = (a: ProjectHit, b: ProjectHit) => a.source === b.source && a.projectId === b.projectId;

/** 收藏夹（存在后端的 favorites.json，打开收藏夹不用联网）。 */
export const useFavorites = create<FavoritesStore>((set, get) => ({
  items: null,
  async load() {
    try {
      set({ items: await commands.favoritesList() });
    } catch (error) {
      reportError(error, "读取收藏夹");
      set({ items: [] });
    }
  },
  async toggle(kind, hit) {
    const before = get().items ?? [];
    const exists = before.some((f) => same(f.hit, hit));
    set({
      items: exists
        ? before.filter((f) => !same(f.hit, hit))
        : [{ kind, hit, addedAt: Date.now() }, ...before],
    });
    try {
      unwrap(await commands.favoritesToggle(kind, hit));
    } catch (error) {
      set({ items: before });
      reportError(error, "保存收藏夹");
    }
  },
}));

/** 这个项目在收藏夹里吗（两个平台合成一条时，任意一边收藏了都算） */
export function useIsFavorite(hit: ProjectHit, otherIds: Partial<Record<string, string>> = {}) {
  return useFavorites(
    (s) =>
      s.items?.some(
        (f) =>
          (f.hit.source === hit.source && f.hit.projectId === hit.projectId) ||
          otherIds[f.hit.source] === f.hit.projectId,
      ) ?? false,
  );
}
