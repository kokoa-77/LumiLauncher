// 拖文件进窗口就能装：后端认好拖进来的文件后发 filesDropped，这里决定弹哪个窗口。
// - 只有一个整合包：直接打开导入预览（和「选择文件导入」一样）
// - 只有一个别家的游戏目录或者实例：直接问挂不挂、打开导入窗口（stores/folders.ts）
// - 别的（模组、资源包、光影包、地图、数据包、投影、认不出的）：弹「装到哪」，别家的目录、实例那一行带按钮
// 一包东西（一堆东西塞在一个 zip 里）后端已经拆开，这里拿到的是里面的每一样，`from` 写着来自哪一包。
import { toast } from "sonner";
import { create } from "zustand";
import { type BundleRest, commands, type DropItem, events } from "@/lib/bindings";
import i18n from "@/lib/i18n";
import { logError } from "@/lib/report";
import { useFolderStore } from "./folders";

interface DropStore {
  /** 「装到哪」弹窗里的文件；null 是关着 */
  items: DropItem[] | null;
  /** 要打开导入预览的整合包（一次性编号） */
  pack: string | null;
  /** 一包东西里没拆的别的文件（每包一条），跟着 `items` 一起显示 */
  rests: BundleRest[];
  /** 有文件正被拖到窗口上方 */
  hovering: boolean;
  /** 放下了，后台正在认（一包东西要先拆开） */
  reading: boolean;
  /** 关掉「装到哪」：没装的都告诉后端丢掉 */
  closeItems: (keep?: string[]) => void;
  /** 从「装到哪」里点了某个整合包的「导入」 */
  openPack: (token: string) => void;
  closePack: () => void;
  /** 从「装到哪」里拿走一样（别家的游戏目录挂上了、实例去导入了），后端那边丢掉 */
  takeItem: (token: string) => void;
}

export const useDropStore = create<DropStore>((set, get) => ({
  items: null,
  pack: null,
  rests: [],
  hovering: false,
  reading: false,
  closeItems(keep = []) {
    const left = (get().items ?? []).map((i) => i.token).filter((t) => !keep.includes(t));
    if (left.length > 0) {
      commands.dropsDiscard(left).catch((error: unknown) => logError(error, "丢掉拖进来的文件"));
    }
    set({ items: null, rests: [] });
  },
  openPack(token) {
    set((s) => ({ pack: token, items: s.items?.filter((i) => i.token !== token) ?? null }));
  },
  closePack() {
    set({ pack: null });
  },
  takeItem(token) {
    commands.dropsDiscard([token]).catch((error: unknown) => logError(error, "丢掉拖进来的文件"));
    set((s) => ({ items: s.items?.filter((i) => i.token !== token) ?? null }));
  },
}));

function received(dropped: DropItem[], rests: BundleRest[]) {
  const store = useDropStore;
  store.setState({ reading: false });
  const skins = dropped.filter((i) => i.kind === "skin").map((i) => i.token);
  if (skins.length > 0) {
    commands.dropsDiscard(skins).catch((error: unknown) => logError(error, "丢掉拖进来的皮肤"));
    toast(i18n.t("skin.official"));
  }
  const items = dropped.filter((i) => i.kind !== "skin");
  if (items.length === 0) return;
  const only = items.length === 1 ? items[0] : undefined;
  if (only?.folder) {
    // 只拖进来一个别家的游戏目录或者实例：直接问挂不挂、打开导入窗口，不用先弹「装到哪」
    commands
      .dropsDiscard([only.token])
      .catch((error: unknown) => logError(error, "丢掉拖进来的文件"));
    useFolderStore.getState().show(only.folder);
    return;
  }
  const packs = items.filter((i) => i.kind === "modpack");
  const rest = items.filter((i) => i.kind !== "modpack");
  const first = packs[0];
  if (first && rest.length === 0) {
    // 一次只导入一个整合包；多的丢掉并说一声
    const extra = packs.slice(1).map((p) => p.token);
    if (extra.length > 0) {
      commands.dropsDiscard(extra).catch((error: unknown) => logError(error, "丢掉多的整合包"));
      toast(i18n.t("drop.onePack", { name: first.name }));
    }
    store.setState({ pack: first.token, items: null, rests: [] });
    return;
  }
  store.setState({ items, rests });
}

/** 订阅拖放事件。返回取消订阅的函数。 */
export async function initDropEvents(): Promise<() => void> {
  const unlisten = await Promise.all([
    events.filesDropped.listen(({ payload }) => received(payload.items, payload.rests)),
    events.filesReading.listen(({ payload }) => useDropStore.setState({ reading: payload })),
    events.fileDragHover.listen(({ payload }) => useDropStore.setState({ hovering: payload })),
  ]);
  void commands.dropsShellReady().catch((error: unknown) => logError(error, "读取关联的整合包"));
  return () => {
    for (const stop of unlisten) stop();
  };
}
