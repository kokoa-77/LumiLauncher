import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { unwrap } from "@/lib/api";
import { commands } from "@/lib/bindings";
import { reportError } from "@/lib/report";

export function RepairDialog({
  instanceId,
  open,
  onClose,
}: {
  instanceId: string;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const close = () => {
    if (!busy) onClose();
  };
  const repair = async () => {
    setBusy(true);
    try {
      unwrap(await commands.instancesRepairFiles(instanceId));
      toast.success(t("diagnostics.repair.queued"));
      onClose();
    } catch (error) {
      reportError(error, t("diagnostics.files.action"));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open={open}
      onClose={close}
      title={t("diagnostics.files.action")}
      footer={
        <>
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => void repair()} disabled={busy}>
            {t("diagnostics.repair.start")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2 text-fg-muted text-sm">
        <p>{t("diagnostics.repair.detail")}</p>
        <p>{t("diagnostics.repair.scope")}</p>
      </div>
    </Dialog>
  );
}
