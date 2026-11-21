import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { isActive, useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";

/**
 * 强行结束游戏前问一句（外部审阅指出）：游戏进程已经起来了，强行结束没保存的进度会丢。
 * 还在准备（下载、补文件）时点「结束」不问，直接取消（`requestStop` 里分开）。
 */
export function StopGameDialog() {
  const { t } = useTranslation();
  const instanceId = useGameStore((s) => s.stopAsk);
  const session = useGameStore((s) => (instanceId ? s.sessions[instanceId] : undefined));
  const instance = useGameStore((s) => s.instances.find((i) => i.id === instanceId));
  const dismiss = useGameStore((s) => s.dismissStopAsk);
  const stop = useGameStore((s) => s.stop);
  const running = isActive(session);

  // 问的时候游戏自己退了：不用再问
  useEffect(() => {
    if (instanceId && !running) dismiss();
  }, [instanceId, running, dismiss]);

  const confirm = () => {
    if (instanceId) void stop(instanceId);
    dismiss();
  };

  return (
    <Dialog
      open={!!instanceId && running}
      onClose={dismiss}
      title={t("stopGame.title", { name: instance?.name ?? instanceId ?? "" })}
      footer={
        <>
          <Button variant="ghost" onClick={dismiss}>
            {t("common.cancel")}
          </Button>
          <Button variant="danger" onClick={confirm}>
            {t("stopGame.confirm")}
          </Button>
        </>
      }
    >
      <p className="text-fg-muted text-sm">{t("stopGame.body")}</p>
    </Dialog>
  );
}
