import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { unwrap } from "@/lib/api";
import { commands } from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";

/**
 * 大世界（超过 1 GB）第一次要自动备份时问一句：备不备都记住，这个世界以后不再问。
 * 后端在游戏退出后发 `backupAsk` 事件。
 */
export function BackupAskDialog() {
  const { t } = useTranslation();
  const ask = useGameStore((s) => s.backupAsk);
  const dismiss = useGameStore((s) => s.dismissBackupAsk);
  const keep = useAppStore((s) => s.settings?.worlds.keepBackups ?? 3);
  const [busy, setBusy] = useState(false);

  const decide = async (auto: boolean) => {
    if (!ask) return;
    setBusy(true);
    try {
      const job = unwrap(await commands.backupsDecide(ask.instanceId, ask.folder, auto));
      if (job) toast.success(t("backup.started", { name: ask.worldName }));
      dismiss();
    } catch (error) {
      reportError(error, "记下大世界要不要自动备份");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={ask !== null}
      onClose={dismiss}
      title={t("backup.askTitle", { name: ask?.worldName ?? "" })}
      footer={
        <>
          <Button variant="ghost" disabled={busy} onClick={() => void decide(false)}>
            {t("backup.askNo")}
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void decide(true)}>
            {t("backup.askYes")}
          </Button>
        </>
      }
    >
      <p className="text-fg-muted text-sm">
        {t("backup.askBody", { n: keep, size: formatSize(ask?.bytes ?? 0) })}
      </p>
    </Dialog>
  );
}
