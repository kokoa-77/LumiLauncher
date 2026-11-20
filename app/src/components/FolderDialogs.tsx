import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type LinkedRootView } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useFolderStore } from "@/stores/folders";
import { useGameStore } from "@/stores/game";
import { ForeignImportDialog } from "./ForeignImportDialog";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";

/** 「添加 .minecraft 文件夹」：别家的游戏目录（导入窗口里选到的、拖进来的），点一下就挂上 */
function AddGameRootDialog() {
  const { t } = useTranslation();
  const root = useFolderStore((s) => s.gameRoot);
  const close = useFolderStore((s) => s.closeGameRoot);
  const refreshInstances = useGameStore((s) => s.refreshInstances);
  const [busy, setBusy] = useState(false);
  // 关上的动画里内容还要在
  const [shown, setShown] = useState<LinkedRootView | null>(root);
  useEffect(() => {
    if (root) setShown(root);
  }, [root]);

  const add = async () => {
    if (!root) return;
    setBusy(true);
    try {
      const added = unwrap(await commands.linkedRootsAdd(root.id));
      toast.success(t("linked.added", { name: added.name, count: added.versions }));
      const after = useFolderStore.getState().afterAdd;
      close();
      after?.();
      await refreshInstances();
    } catch (error) {
      reportError(error, "挂上别的启动器的目录");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={root !== null}
      onClose={close}
      title={t("linked.confirm.title")}
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void add()}>
            {t("linked.confirm.ok")}
          </Button>
        </>
      }
    >
      <p className="text-fg-muted text-sm">{t("linked.confirm.body")}</p>
      {shown && (
        <p className="mt-3 select-text break-all text-fg-muted text-xs">
          {t(`launchers.${shown.launcher}`)} · {t("linked.versions", { count: shown.versions })} ·{" "}
          {shown.path}
        </p>
      )}
    </Dialog>
  );
}

/**
 * 别家启动器的文件夹认出来以后弹的窗口（D19），哪里都能叫出来（stores/folders.ts）：
 * 「添加 .minecraft 文件夹」和「从别的启动器导入」。
 */
export function FolderDialogs() {
  const foreign = useFolderStore((s) => s.foreign);
  const closeForeign = useFolderStore((s) => s.closeForeign);
  return (
    <>
      <ForeignImportDialog open={foreign !== null} first={foreign ?? []} onClose={closeForeign} />
      <AddGameRootDialog />
    </>
  );
}
