import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type InstanceSummary } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { rememberCopy, useGameStore } from "@/stores/game";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field, TextInput } from "../ui/Field";
import { Switch } from "../ui/Switch";

/**
 * 复制实例（D17i）：起个新名字，默认带上世界。复制在后台进行，任务中心有进度，
 * 复制完右下角提示，点「去看看」打开新实例。
 */
export function CopyDialog({
  instance,
  open,
  onClose,
}: {
  instance: InstanceSummary;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [worlds, setWorlds] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) {
      setName(t("copy.nameDefault", { name: instance.name }));
      setWorlds(true);
    }
  }, [open, instance.name, t]);

  const submit = async () => {
    setBusy(true);
    try {
      const started = unwrap(await commands.instancesCopy(instance.id, name.trim(), worlds));
      rememberCopy(started.jobId, started.instanceId);
      useGameStore.getState().showTasksFor(started.jobId);
      onClose();
    } catch (error) {
      reportError(error, "复制实例");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("copy.title")}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={busy || name.trim() === ""}
            onClick={() => void submit()}
          >
            {t("copy.submit")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("copy.name")}>
          <TextInput
            value={name}
            maxLength={64}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && name.trim() !== "" && !busy) void submit();
            }}
          />
        </Field>
        <div className="flex items-center justify-between gap-6">
          <div className="min-w-0">
            <p className="font-medium text-sm">{t("copy.worlds")}</p>
            <p className="mt-0.5 text-fg-muted text-xs">{t("copy.worldsHint")}</p>
          </div>
          <Switch label={t("copy.worlds")} checked={worlds} onChange={setWorlds} />
        </div>
      </div>
    </Dialog>
  );
}

/**
 * 删除实例（D17i）：整个文件夹移到系统回收站，删完回到实例库。
 * 挂上的别家版本（D19）只从 Lumi 里移除，别家的文件不动。
 */
export function DeleteDialog({
  instance,
  open,
  onClose,
  onDeleted,
}: {
  instance: InstanceSummary;
  open: boolean;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const linked = instance.linked;

  const submit = async () => {
    setBusy(true);
    try {
      unwrap(await commands.instancesDelete(instance.id));
      toast.success(t(linked ? "delete.unlinked" : "delete.done", { name: instance.name }));
      onClose();
      onDeleted();
      const store = useGameStore.getState();
      await Promise.all([store.refreshInstances(), store.refreshRecentWorlds()]);
    } catch (error) {
      reportError(error, "删除实例");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t(linked ? "delete.unlinkTitle" : "delete.title", { name: instance.name })}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant={linked ? "primary" : "danger"}
            disabled={busy}
            onClick={() => void submit()}
          >
            {t(linked ? "delete.unlinkConfirm" : "delete.confirm")}
          </Button>
        </>
      }
    >
      <p className="text-fg-muted text-sm">
        {linked ? t("delete.unlinkBody", { launcher: t(`launchers.${linked}`) }) : t("delete.body")}
      </p>
    </Dialog>
  );
}
