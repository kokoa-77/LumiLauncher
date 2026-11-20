import type { TFunction } from "i18next";
import { AlertTriangle, Coffee, Package, Play } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import type { PreflightIssue } from "@/lib/bindings";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { Tag } from "./ui/Tag";

function issueSummary(issue: PreflightIssue, t: TFunction) {
  switch (issue.kind) {
    case "javaTooOld":
      return t("preflight.issue.javaTooOld", { required: issue.target, actual: issue.detail });
    case "wrongLoader":
      return t("preflight.issue.wrongLoader", {
        mod: issue.modName ?? issue.detail,
        declared: issue.target,
      });
    case "duplicateModId":
      return t("preflight.issue.duplicateModId", { id: issue.target });
    case "missingDependency":
      return t("preflight.issue.missingDependency", {
        mod: issue.modName ?? issue.detail,
        dep: issue.target,
      });
  }
}

/**
 * 启动前极速预检提示（D25d）：仅在本地扫描命中高确定性必崩风险时弹出，
 * 给出清晰中文原因和直接入口（「去管理模组 / 去调整 Java / 仍然启动」）。
 */
export function PreflightDialog() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const openSettings = useAppStore((s) => s.openSettings);
  const ask = useGameStore((s) => s.preflightAsk);
  const dismiss = useGameStore((s) => s.dismissPreflight);
  const launch = useGameStore((s) => s.launch);
  const instances = useGameStore((s) => s.instances);

  const instance = ask ? instances.find((i) => i.id === ask.instanceId) : undefined;
  const issues = ask?.report.issues ?? [];
  const hasJavaIssue = issues.some((i) => i.kind === "javaTooOld");
  const hasModIssue = issues.some((i) => i.kind !== "javaTooOld");

  return (
    <Dialog
      open={ask !== null}
      onClose={dismiss}
      wide
      title={t("preflight.title", { name: instance?.name ?? ask?.instanceId ?? "" })}
      footer={
        <>
          <Button variant="ghost" onClick={dismiss}>
            {t("common.cancel")}
          </Button>
          {hasJavaIssue && (
            <Button
              variant="secondary"
              onClick={() => {
                dismiss();
                openSettings("java");
              }}
            >
              <Coffee className="size-4" aria-hidden />
              {t("preflight.openJava")}
            </Button>
          )}
          {hasModIssue && ask && (
            <Button
              variant="secondary"
              onClick={() => {
                const targetId = ask.instanceId;
                dismiss();
                navigate(`/instances/${encodeURIComponent(targetId)}?tab=content`);
              }}
            >
              <Package className="size-4" aria-hidden />
              {t("preflight.openMods")}
            </Button>
          )}
          {ask && (
            <Button
              variant="primary"
              onClick={() => {
                const { instanceId, world, server } = ask;
                dismiss();
                void launch(instanceId, world, server, { skipPreflight: true });
              }}
            >
              <Play className="size-4" aria-hidden />
              {t("preflight.launchAnyway")}
            </Button>
          )}
        </>
      }
    >
      {ask && (
        <div className="flex flex-col gap-3">
          <div className="flex items-start gap-2.5 rounded-panel bg-warning-soft px-3.5 py-2.5 text-sm">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
            <p className="min-w-0 flex-1">
              {t("preflight.intro", {
                count: issues.length,
                mods: ask.report.checkedMods,
                ms: ask.report.elapsedMs,
              })}
            </p>
          </div>
          <ul className="flex max-h-72 flex-col divide-y divide-line overflow-y-auto rounded-panel border border-line">
            {issues.map((issue, index) => (
              <li
                key={`${issue.kind}-${issue.target}-${issue.modName ?? index}`}
                className="flex items-start gap-3 px-3.5 py-2.5"
              >
                <Tag className="mt-0.5 shrink-0">{t(`preflight.kind.${issue.kind}`)}</Tag>
                <div className="min-w-0 flex-1">
                  <p className="text-sm">{issueSummary(issue, t)}</p>
                  {issue.detail && (
                    <p className="mt-0.5 truncate text-fg-muted text-xs">{issue.detail}</p>
                  )}
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Dialog>
  );
}
