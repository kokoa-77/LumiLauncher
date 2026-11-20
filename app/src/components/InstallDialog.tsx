import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { PlanItem } from "@/lib/bindings";
import { Button } from "./ui/Button";
import { Checkbox } from "./ui/Checkbox";
import { Dialog } from "./ui/Dialog";
import { Tag } from "./ui/Tag";

function formatSize(bytes: number): string {
  return bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

interface InstallDialogProps {
  plan: PlanItem[] | null;
  selectDependencies: boolean;
  instanceName: string;
  busy: boolean;
  onConfirm: (items: PlanItem[]) => void;
  onClose: () => void;
}

/** 安装确认：列出要装的全部内容（含自动带上的前置和光影加载器）。 */
export function InstallDialog({
  plan,
  selectDependencies,
  instanceName,
  busy,
  onConfirm,
  onClose,
}: InstallDialogProps) {
  const { t } = useTranslation();
  const [selection, setSelection] = useState<{
    plan: PlanItem[] | null;
    choices: Map<PlanItem, boolean>;
  }>();
  const choices = selection?.plan === plan ? selection?.choices : undefined;
  const included = (item: PlanItem) =>
    item.reason !== "dependency" || (choices?.get(item) ?? selectDependencies);
  const items = (plan ?? []).filter(included);
  const skipped = (plan ?? []).filter((item) => !included(item));
  const total = items.reduce((sum, item) => sum + (item.size ?? 0), 0);
  const extra = items.filter((item) => item.reason !== "requested").length;
  const manual = items.filter((item) => item.delivery === "manual").length;
  const close = () => {
    if (!busy) onClose();
  };
  return (
    <Dialog
      open={plan !== null}
      onClose={close}
      wide
      title={t("install.title", { name: instanceName })}
      footer={
        <>
          <Button variant="ghost" disabled={busy} onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={busy || items.length === 0}
            onClick={() => onConfirm(items)}
          >
            {t("install.confirm", { n: items.length, size: formatSize(total) })}
          </Button>
        </>
      }
    >
      {extra > 0 && (
        <p className="mb-3 text-fg-muted text-sm">{t("install.extraHint", { n: extra })}</p>
      )}
      {manual > 0 && <p className="mb-3 text-sm">{t("install.manualHint", { n: manual })}</p>}
      {skipped.length > 0 && (
        <p className="mb-3 text-warning text-sm">
          {t("install.skippedDependencies", {
            names: skipped.map((item) => item.title).join("、"),
          })}
        </p>
      )}
      <ul className="flex flex-col divide-y divide-line">
        {plan?.map((item) => (
          <li key={`${item.source}:${item.versionId}`} className="flex items-center gap-3 py-2.5">
            {item.reason === "dependency" && (
              <Checkbox
                label={t("install.selectDependency", { title: item.title })}
                checked={included(item)}
                disabled={busy}
                onChange={(checked) =>
                  setSelection((previous) => {
                    const next = new Map(previous?.plan === plan ? previous?.choices : undefined);
                    next.set(item, checked);
                    return { plan, choices: next };
                  })
                }
              />
            )}
            {item.iconUrl ? (
              <img src={item.iconUrl} alt="" className="size-9 shrink-0 rounded-row" />
            ) : (
              <div className="size-9 shrink-0 rounded-row bg-surface-hover" />
            )}
            <div className="min-w-0 flex-1">
              <p className="truncate font-medium text-sm">{item.title}</p>
              <p className="truncate text-fg-muted text-xs">
                {t(`discover.sourceName.${item.source}`)} · {item.versionNumber} ·{" "}
                {formatSize(item.size ?? 0)}
              </p>
              {item.delivery !== "direct" && (
                <p
                  className={`truncate text-xs ${
                    item.delivery === "manual" ? "text-warning" : "text-fg-muted"
                  }`}
                >
                  {t(`install.delivery.${item.delivery}`)}
                </p>
              )}
            </div>
            {item.reason !== "requested" && (
              <Tag tone="accent">{t(`install.reason.${item.reason}`)}</Tag>
            )}
          </li>
        ))}
      </ul>
    </Dialog>
  );
}
