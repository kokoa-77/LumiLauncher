import { CircleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/Button";
import { unwrap } from "@/lib/api";
import { type CrashKind, commands } from "@/lib/bindings";
import { logError } from "@/lib/report";

export interface RecoveryActions {
  onJava: () => void;
  onMemory: () => void;
  onMods: () => void;
  onRepair?: () => void;
}

export function CrashAdvice({
  instanceId,
  recordId,
  actions,
}: {
  instanceId: string;
  recordId: string;
  actions: RecoveryActions;
}) {
  const { t } = useTranslation();
  const [kind, setKind] = useState<CrashKind | null>(null);

  useEffect(() => {
    let current = true;
    commands
      .launchRecordDiagnosis(instanceId, recordId)
      .then(unwrap)
      .then((result) => {
        if (current) setKind(result);
      })
      .catch((error: unknown) => {
        logError(error, "分析启动失败原因");
        if (current) setKind("unknown");
      });
    return () => {
      current = false;
    };
  }, [instanceId, recordId]);

  if (!kind) return null;
  const action = {
    java: actions.onJava,
    memory: actions.onMemory,
    dependency: actions.onMods,
    conflict: actions.onMods,
    files: actions.onRepair,
    unknown: undefined,
  }[kind];
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-3 rounded-panel border border-line bg-surface p-3">
      <CircleAlert className="size-4 shrink-0 self-start text-danger" aria-hidden />
      <div role="status" className="min-w-40 flex-1 select-text">
        <p className="font-medium text-sm">{t(`diagnostics.${kind}.title`)}</p>
        <p className="mt-0.5 text-fg-muted text-xs">
          {t(kind === "files" && !action ? "diagnostics.files.linked" : `diagnostics.${kind}.hint`)}
        </p>
      </div>
      {action && (
        <Button size="sm" variant="secondary" onClick={action}>
          {t(`diagnostics.${kind}.action`)}
        </Button>
      )}
    </div>
  );
}
