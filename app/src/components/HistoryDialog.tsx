import { History } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type HistoryView } from "@/lib/bindings";
import { formatDateTime } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { EmptyState } from "./ui/EmptyState";
import { Tag } from "./ui/Tag";

export function formatTime(ms: number): string {
  return formatDateTime(ms);
}

/** 改动历史：安装、更新、删除都记在这里，每条都能撤销（文件在内容仓库里）。 */
export function HistoryDialog({
  instanceId,
  open,
  onClose,
}: {
  instanceId: string;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const bumpContent = useGameStore((s) => s.bumpContent);
  const [entries, setEntries] = useState<HistoryView[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = async () => {
    try {
      setEntries(unwrap(await commands.contentHistory(instanceId)));
    } catch (error) {
      reportError(error, "读取改动历史");
      setEntries([]);
    }
  };

  useEffect(() => {
    if (open) void load();
  }, [open, instanceId]);

  const rollback = async (entry: HistoryView) => {
    setBusy(entry.id);
    try {
      unwrap(await commands.contentRollback(instanceId, entry.id));
      toast.success(t("history.rolledBack", { summary: entry.summary }));
      bumpContent();
      await load();
    } catch (error) {
      reportError(error, "撤销改动");
    } finally {
      setBusy(null);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} wide title={t("history.title")}>
      {entries?.length === 0 ? (
        <EmptyState icon={History} title={t("history.empty")} hint={t("history.emptyHint")} />
      ) : (
        <ul className="flex flex-col divide-y divide-line">
          {entries?.map((entry) => (
            <li key={entry.id} className="flex items-center gap-3 py-2.5">
              <Tag>{t(`history.action.${entry.action}`)}</Tag>
              <div className="min-w-0 flex-1">
                <p
                  className={`truncate text-sm ${entry.rolledBack ? "text-fg-muted line-through" : ""}`}
                >
                  {entry.summary}
                </p>
                <p className="text-fg-muted text-xs">{formatTime(entry.at ?? 0)}</p>
              </div>
              {entry.rolledBack ? (
                <span className="shrink-0 text-fg-muted text-xs">{t("history.done")}</span>
              ) : (
                <Button
                  variant="secondary"
                  disabled={busy !== null}
                  onClick={() => void rollback(entry)}
                >
                  {t("history.undo")}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}
