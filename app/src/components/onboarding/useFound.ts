import { useCallback, useEffect, useState } from "react";
import { unwrap } from "@/lib/api";
import { commands, type FolderFound, type ForeignView, type LinkedRootView } from "@/lib/bindings";
import { logError } from "@/lib/report";

/** 电脑上的别家游戏：已经挂上的、找到还没挂上的 .minecraft（官方、PCL、HMCL），能导入的实例（Prism、Modrinth App……） */
export interface Found {
  loading: boolean;
  /** 已经挂上的（以前挂过、或者在设置里挂的） */
  linked: LinkedRootView[];
  /** 找到、还没挂上的（「添加文件夹」选到的也放这里） */
  roots: LinkedRootView[];
  foreign: ForeignView[];
}

/**
 * 首次进入导入页时查找，离开该步骤后仍保留结果和手动添加项。
 * `addFolder` 把「添加文件夹」选到的东西并进来，返回新加进来的编号（要默认勾上的）
 */
export function useFound(enabled: boolean): {
  found: Found;
  addFolder: (folder: FolderFound) => { roots: string[]; foreign: string[] };
} {
  const [found, setFound] = useState<Found>({
    loading: true,
    linked: [],
    roots: [],
    foreign: [],
  });
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const safe = <T>(work: Promise<T[]>, context: string) =>
      work.catch((error: unknown) => {
        logError(error, context);
        return [] as T[];
      });
    void Promise.all([
      safe(commands.linkedRootsList(), "读取挂上的目录"),
      safe(commands.linkedRootsDiscover().then(unwrap), "找别的启动器的目录"),
      safe(commands.foreignList().then(unwrap), "找别的启动器的实例"),
    ]).then(([linked, roots, foreign]) => {
      if (!cancelled) {
        setFound({ loading: false, linked: linked.filter((r) => !r.missing), roots, foreign });
      }
    });
    return () => {
      cancelled = true;
    };
  }, [enabled]);

  const addFolder = useCallback(
    (folder: FolderFound) => {
      if (folder.kind === "gameRoot") {
        const known =
          found.linked.some((r) => r.path === folder.root.path) ||
          found.roots.some((r) => r.id === folder.root.id);
        if (folder.linked || known) return { roots: [], foreign: [] };
        setFound((s) => ({ ...s, roots: [...s.roots, folder.root] }));
        return { roots: [folder.root.id], foreign: [] };
      }
      const fresh = folder.instances.filter((i) => !found.foreign.some((f) => f.id === i.id));
      setFound((s) => ({ ...s, foreign: [...s.foreign, ...fresh] }));
      return { roots: [], foreign: fresh.filter((i) => i.supported).map((i) => i.id) };
    },
    [found],
  );

  return { found, addFolder };
}
