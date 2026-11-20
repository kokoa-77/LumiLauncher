import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type ForeignView } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useFolderStore } from "@/stores/folders";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { Truncated } from "./ui/HoverCard";
import { Tag } from "./ui/Tag";

/** `front` 排在前面，`rest` 里重复的不要 */
function merge(front: ForeignView[], rest: ForeignView[]) {
  const known = new Set(front.map((i) => i.id));
  return [...front, ...rest.filter((i) => !known.has(i.id))];
}

/**
 * 从别的启动器导入实例（D19）：列出电脑上 Prism、CurseForge、ATLauncher、Modrinth App 的实例，
 * 点「导入」复制一份进 Lumi（后台任务，进度在下载面板里）；也能自己选文件夹。
 * 选到 PCL、HMCL、官方启动器的游戏目录时不用导入，问一句直接挂上（stores/folders.ts）。
 * `first` 是要排在最前面的（设置里选到的、拖进窗口的实例）。
 */
export function ForeignImportDialog({
  open,
  first,
  onClose,
}: {
  open: boolean;
  first: ForeignView[];
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const openSettings = useAppStore((s) => s.openSettings);
  const [items, setItems] = useState<ForeignView[] | null>(null);
  const [started, setStarted] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setItems(first.length > 0 ? first : null);
    setStarted(new Set());
    commands
      .foreignList()
      .then((result) => setItems(merge(first, unwrap(result))))
      .catch((error: unknown) => {
        reportError(error, "找别的启动器的实例");
        setItems(first);
      });
  }, [open, first]);

  const pickFolder = async () => {
    setBusy(true);
    try {
      const found = unwrap(await commands.foreignPick());
      if (found?.kind === "foreign") {
        setItems((current) => merge(found.instances, current ?? []));
      } else if (found) {
        // PCL、HMCL、官方的游戏目录：问一句直接挂上，挂上了这个窗口也关掉
        useFolderStore.getState().show(found, onClose);
      }
    } catch (error) {
      reportError(error, "选择别的启动器的实例");
    } finally {
      setBusy(false);
    }
  };

  const importOne = async (item: ForeignView) => {
    try {
      unwrap(await commands.foreignImport(item.id));
      setStarted((current) => new Set(current).add(item.id));
      toast.success(t("foreign.started", { name: item.name }));
    } catch (error) {
      reportError(error, "导入别的启动器的实例");
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("foreign.title")}
      footer={
        <Button variant="secondary" disabled={busy} onClick={() => void pickFolder()}>
          <FolderOpen className="size-4" aria-hidden />
          {t("foreign.pick")}
        </Button>
      }
    >
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-3 rounded-panel bg-surface-2 px-3 py-2">
          <p className="min-w-0 text-fg-muted text-xs">{t("foreign.linkHint")}</p>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              onClose();
              openSettings("game");
            }}
          >
            {t("foreign.linkGo")}
          </Button>
        </div>
        {items === null ? (
          <p className="py-6 text-center text-fg-muted text-sm">{t("foreign.loading")}</p>
        ) : items.length === 0 ? (
          <p className="py-6 text-center text-fg-muted text-sm">{t("foreign.empty")}</p>
        ) : (
          <div className="max-h-80 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
            {items.map((item) => (
              <div key={item.id} className="flex min-h-14 items-center gap-3 px-4 py-2.5">
                <div className="min-w-0 flex-1">
                  <p className="flex min-w-0 items-center gap-2">
                    <Truncated text={item.name} className="font-medium" />
                    <Tag>{t(`launchers.${item.launcher}`)}</Tag>
                  </p>
                  <p className="truncate text-fg-muted text-xs">
                    {[item.gameVersion, item.loader].filter(Boolean).join(" · ")}
                  </p>
                </div>
                {!item.supported ? (
                  <Tag>{t("foreign.soon")}</Tag>
                ) : started.has(item.id) ? (
                  <Tag tone="accent">{t("foreign.importing")}</Tag>
                ) : (
                  <Button size="sm" variant="outline" onClick={() => void importOne(item)}>
                    {t("foreign.import")}
                  </Button>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </Dialog>
  );
}
