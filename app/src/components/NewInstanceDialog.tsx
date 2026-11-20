import { ArrowRightLeft, Compass, FileDown, type LucideIcon, Sparkles } from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { useFolderStore } from "@/stores/folders";
import type { PackSourceRef } from "./ImportPackDialog";
import { Dialog } from "./ui/Dialog";

const CreateInstanceDialog = lazy(async () => ({
  default: (await import("./CreateInstanceDialog")).CreateInstanceDialog,
}));
const ImportPackDialog = lazy(async () => ({
  default: (await import("./ImportPackDialog")).ImportPackDialog,
}));

function Choice({
  icon: Icon,
  title,
  hint,
  onClick,
}: {
  icon: LucideIcon;
  title: string;
  hint: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex items-start gap-3 rounded-panel border border-line bg-surface p-4 text-left transition-[background-color,border-color] duration-(--motion-press) hover:border-accent hover:bg-surface-hover focus-visible:border-accent"
    >
      <span className="grid size-10 shrink-0 place-items-center rounded-row bg-accent-soft text-accent">
        <Icon className="size-5" aria-hidden />
      </span>
      <span className="min-w-0">
        <span className="block font-medium">{title}</span>
        <span className="block text-fg-muted text-sm">{hint}</span>
      </span>
    </button>
  );
}

/**
 * 新建实例的统一入口：先选怎么建——全新创建、导入整合包文件、在线找整合包、从别的启动器导入（D19）。
 */
export function NewInstanceDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // 第一次选了才加载后续弹窗，打开过就挂着，保留关闭动画。
  const [creating, setCreating] = useState<boolean | null>(null);
  const [importSource, setImportSource] = useState<PackSourceRef | null | undefined>();
  const openForeign = useFolderStore((s) => s.openForeign);

  return (
    <>
      <Dialog open={open} onClose={onClose} title={t("newInstance.title")}>
        <div className="flex flex-col gap-2">
          <Choice
            icon={Sparkles}
            title={t("newInstance.fresh")}
            hint={t("newInstance.freshHint")}
            onClick={() => {
              onClose();
              setCreating(true);
            }}
          />
          <Choice
            icon={FileDown}
            title={t("newInstance.file")}
            hint={t("newInstance.fileHint")}
            onClick={() => {
              onClose();
              setImportSource({ kind: "file" });
            }}
          />
          <Choice
            icon={Compass}
            title={t("newInstance.online")}
            hint={t("newInstance.onlineHint")}
            onClick={() => {
              onClose();
              navigate("/discover?kind=modpack");
            }}
          />
          <Choice
            icon={ArrowRightLeft}
            title={t("newInstance.foreign")}
            hint={t("newInstance.foreignHint")}
            onClick={() => {
              onClose();
              openForeign();
            }}
          />
        </div>
      </Dialog>
      <Suspense fallback={null}>
        {creating !== null && (
          <CreateInstanceDialog open={creating} onClose={() => setCreating(false)} />
        )}
        {importSource !== undefined && (
          <ImportPackDialog source={importSource} onClose={() => setImportSource(null)} />
        )}
      </Suspense>
    </>
  );
}
