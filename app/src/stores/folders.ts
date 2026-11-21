// 别家启动器的文件夹：导入窗口里选到的、设置里选到的、拖进窗口的，认出来以后都走这里。
// - 别家的游戏目录（PCL、HMCL、官方的 .minecraft）：弹「添加 .minecraft 文件夹」，点一下就挂上
// - 别家启动器的实例（Prism、CurseForge……）：打开「从别的启动器导入」，选到的排在最前面
// 弹窗本身在 components/FolderDialogs.tsx，挂在 AppShell 里，哪里都能叫出来。
import { toast } from "sonner";
import { create } from "zustand";
import type { FolderFound, ForeignView, LinkedRootView } from "@/lib/bindings";
import i18n from "@/lib/i18n";

interface FolderStore {
  /** 「添加 .minecraft 文件夹」弹窗里的目录；null 是关着 */
  gameRoot: LinkedRootView | null;
  /** 挂上以后要做的（比如顺手关掉导入窗口） */
  afterAdd: (() => void) | null;
  /** 「从别的启动器导入」窗口；null 是关着，数组里是要排在最前面的实例（选到的、拖进来的） */
  foreign: ForeignView[] | null;
  openForeign: (first?: ForeignView[]) => void;
  closeForeign: () => void;
  closeGameRoot: () => void;
  /** 认出来的文件夹：游戏目录问一句挂不挂（已经挂上的说一声），实例打开导入窗口 */
  show: (found: FolderFound, afterAdd?: () => void) => void;
}

export const useFolderStore = create<FolderStore>((set) => ({
  gameRoot: null,
  afterAdd: null,
  foreign: null,
  openForeign(first = []) {
    set({ foreign: first });
  },
  closeForeign() {
    set({ foreign: null });
  },
  closeGameRoot() {
    set({ gameRoot: null, afterAdd: null });
  },
  show(found, afterAdd) {
    if (found.kind === "foreign") {
      set({ foreign: found.instances });
      return;
    }
    if (found.linked && found.root.hidden === 0) {
      toast(i18n.t("linked.already", { name: found.root.name }));
      afterAdd?.();
      return;
    }
    set({ gameRoot: found.root, afterAdd: afterAdd ?? null });
  },
}));
