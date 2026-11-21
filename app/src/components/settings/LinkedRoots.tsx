import { FolderPlus, Link2, Undo2 } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type LinkedRootView } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useFolderStore } from "@/stores/folders";
import { useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Truncated } from "../ui/HoverCard";
import { Tag } from "../ui/Tag";
import { Group } from "./parts";

/** 一个目录：名字 + 哪家的，下面一行几个版本和路径（只显示） */
function RootRow({ root, action }: { root: LinkedRootView; action: ReactNode }) {
  const { t } = useTranslation();
  return (
    <div className="flex min-h-14 items-center gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <p className="flex min-w-0 items-center gap-2">
          <Truncated text={root.name} className="font-medium" />
          <Tag>{t(`launchers.${root.launcher}`)}</Tag>
          {root.missing && <Tag tone="warning">{t("linked.missing")}</Tag>}
        </p>
        <Truncated as="p" text={root.path} className="mt-0.5 select-text text-fg-muted text-xs">
          {t("linked.versions", { count: root.versions })} · {root.path}
        </Truncated>
      </div>
      {action}
    </div>
  );
}

/**
 * 设置 →「游戏」→「别的启动器的目录」：挂上官方、PCL、HMCL 的 .minecraft，里面的版本原地读、直接玩。
 * 挂上的目录一行一个，右边「移除」（只从 Lumi 里移除，那边的文件不动）；下面是在电脑上自动找到的，点「挂上」。
 * 界面只拿目录的编号，路径只用来显示。
 */
export function LinkedRoots() {
  const { t } = useTranslation();
  const refreshInstances = useGameStore((s) => s.refreshInstances);
  const [roots, setRoots] = useState<LinkedRootView[] | null>(null);
  const [found, setFound] = useState<LinkedRootView[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [removing, setRemoving] = useState<LinkedRootView | null>(null);

  const load = useCallback(async () => {
    setRoots(await commands.linkedRootsList());
    try {
      setFound(unwrap(await commands.linkedRootsDiscover()));
    } catch (error) {
      reportError(error, "找别的启动器的目录");
      setFound([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const add = async (suggestionId: string) => {
    const added = unwrap(await commands.linkedRootsAdd(suggestionId));
    toast.success(t("linked.added", { name: added.name, count: added.versions }));
    await Promise.all([load(), refreshInstances()]);
  };

  const addFound = async (suggestionId: string) => {
    setBusy(true);
    try {
      await add(suggestionId);
    } catch (error) {
      reportError(error, "挂上别的启动器的目录");
    } finally {
      setBusy(false);
    }
  };

  // 「添加文件夹」：选到游戏目录直接挂上；选到 Prism 这类的实例，打开导入窗口
  const pickFolder = async () => {
    setBusy(true);
    try {
      const found = unwrap(await commands.linkedRootsPick());
      if (found?.kind === "foreign") {
        useFolderStore.getState().show(found);
      } else if (found?.linked && found.root.hidden === 0) {
        toast(t("linked.already", { name: found.root.name }));
      } else if (found) {
        await add(found.root.id);
      }
    } catch (error) {
      reportError(error, "挂上别的启动器的目录");
    } finally {
      setBusy(false);
    }
  };

  const restore = async (root: LinkedRootView) => {
    setBusy(true);
    try {
      unwrap(await commands.linkedRootsRestore(root.id));
      toast.success(t("linked.restored", { name: root.name, count: root.hidden }));
      await Promise.all([load(), refreshInstances()]);
    } catch (error) {
      reportError(error, "加回移除过的版本");
    } finally {
      setBusy(false);
    }
  };

  const remove = async (root: LinkedRootView) => {
    setBusy(true);
    try {
      const count = unwrap(await commands.linkedRootsRemove(root.id));
      toast.success(t("linked.removed", { name: root.name, count }));
      setRemoving(null);
      await Promise.all([load(), refreshInstances()]);
    } catch (error) {
      reportError(error, "移除挂上的目录");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Group
        title={t("linked.group")}
        action={
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void pickFolder()}>
            <FolderPlus className="size-3.5" aria-hidden />
            {t("linked.add")}
          </Button>
        }
      >
        {roots === null ? (
          <p className="px-4 py-4 text-fg-muted text-sm">{t("linked.loading")}</p>
        ) : roots.length === 0 ? (
          <p className="px-4 py-4 text-fg-muted text-sm">{t("linked.empty")}</p>
        ) : (
          roots.map((root) => (
            <RootRow
              key={root.id}
              root={root}
              action={
                <div className="flex shrink-0 items-center gap-2">
                  {root.hidden > 0 && (
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy}
                      onClick={() => void restore(root)}
                    >
                      <Undo2 className="size-3.5" aria-hidden />
                      {t("linked.restore", { count: root.hidden })}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy}
                    onClick={() => setRemoving(root)}
                  >
                    {t("linked.remove")}
                  </Button>
                </div>
              }
            />
          ))
        )}
      </Group>
      <p className="-mt-3 px-1 text-fg-muted text-xs">{t("linked.note")}</p>

      {found === null ? (
        <p className="px-1 text-fg-muted text-xs">{t("linked.searching")}</p>
      ) : (
        found.length > 0 && (
          <Group title={t("linked.found")}>
            {found.map((root) => (
              <RootRow
                key={root.id}
                root={root}
                action={
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={() => void addFound(root.id)}
                  >
                    <Link2 className="size-3.5" aria-hidden />
                    {t("linked.link")}
                  </Button>
                }
              />
            ))}
          </Group>
        )
      )}

      <Dialog
        open={removing !== null}
        onClose={() => setRemoving(null)}
        title={t("linked.removeTitle", { name: removing?.name ?? "" })}
        footer={
          <>
            <Button variant="ghost" onClick={() => setRemoving(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={busy}
              onClick={() => removing && void remove(removing)}
            >
              {t("linked.remove")}
            </Button>
          </>
        }
      >
        <p className="text-fg-muted text-sm">
          {t("linked.removeBody", {
            launcher: removing ? t(`launchers.${removing.launcher}`) : "",
          })}
        </p>
      </Dialog>
    </>
  );
}
