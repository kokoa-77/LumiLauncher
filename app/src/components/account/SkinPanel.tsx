import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, ExternalLink, RefreshCw, RotateCcw, Upload } from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AccountView, SkinModel } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useAccountStore, useCape, useSkin } from "@/stores/accounts";
import { useAppStore, useEffectsLevel } from "@/stores/app";
import { PlayerHead } from "../PlayerHead";
import { Button } from "../ui/Button";
import { Tag } from "../ui/Tag";
import { SkinFigure } from "./SkinFigure";

// 3D 预览（three.js 很大）只在这里用到，第一次打开时才加载
const SkinViewer3D = lazy(() => import("./SkinViewer3D"));

type UploadModelChoice = "auto" | SkinModel;

/**
 * 皮肤与披风预览：精美模式是能转动的 3D 人物（拖动转身，含官方披风）；流畅模式是 2D 正面 + 背面。
 * 还没有皮肤时是一个大头像 + 一句话。
 */
export function SkinPreview({
  account,
  width = 200,
  height = 300,
  className = "bg-surface-2",
}: {
  account: AccountView;
  width?: number;
  height?: number;
  /** 底色：弹窗里是浅一层的面板，引导页里是卡片 */
  className?: string;
}) {
  const { t } = useTranslation();
  const skin = useSkin(account);
  const activeCape = account.capes.find((c) => c.active);
  const cape = useCape(activeCape?.hash);
  const effects = useEffectsLevel();
  const reduceMotion = useAppStore((s) => s.settings?.appearance.reduceMotion ?? false);
  const box = `grid shrink-0 place-items-center rounded-panel ${className}`;
  if (!account.hasSkin || !skin) {
    return (
      <div className={`${box} gap-3 p-4 text-center`} style={{ width, height }}>
        <PlayerHead name={account.name} className="size-16" />
        <p className="text-fg-muted text-xs">{t("skin.none")}</p>
      </div>
    );
  }
  const flat = (
    <div className="flex items-end justify-center gap-4">
      <SkinFigure skin={skin} model={account.skinModel} className="h-40" />
      <SkinFigure skin={skin} model={account.skinModel} side="back" className="h-40" />
    </div>
  );
  return (
    <div className={box} style={{ width, height }}>
      {effects === "lite" ? (
        flat
      ) : (
        <Suspense fallback={<SkinFigure skin={skin} model={account.skinModel} className="h-48" />}>
          <SkinViewer3D
            skin={skin}
            cape={cape}
            model={account.skinModel}
            width={width}
            height={height}
            still={reduceMotion}
          />
        </Suspense>
      )}
    </div>
  );
}

/** 官方皮肤上传、模型切换、档案刷新与官方披风切换工坊。 */
export function SkinControls({ account }: { account?: AccountView | null }) {
  const { t } = useTranslation();
  const skinBusy = useAccountStore((s) => s.skinBusy);
  const refreshProfile = useAccountStore((s) => s.refreshProfile);
  const uploadSkin = useAccountStore((s) => s.uploadSkin);
  const resetSkin = useAccountStore((s) => s.resetSkin);
  const setCape = useAccountStore((s) => s.setCape);
  const [uploadModel, setUploadModel] = useState<UploadModelChoice>("auto");

  if (!account) {
    return (
      <div className="flex flex-col items-start gap-3">
        <p className="text-fg-muted text-sm">{t("skin.official")}</p>
        <Button
          variant="outline"
          onClick={() =>
            openUrl("https://www.minecraft.net/msaprofile/mygames/editprofile").catch(
              (error: unknown) => reportError(error, "打开 Minecraft 皮肤管理"),
            )
          }
        >
          <ExternalLink className="size-4" aria-hidden />
          {t("skin.openOfficial")}
        </Button>
      </div>
    );
  }

  const activeCape = account.capes.find((c) => c.active);

  return (
    <div className="flex flex-col items-start gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-fg-muted text-xs">{t("skin.currentModel")}</span>
        <Tag>{t(`skin.model.${account.skinModel}`)}</Tag>
      </div>

      <div className="flex w-full flex-col gap-2">
        <p className="font-medium text-xs">{t("skin.uploadModelLabel")}</p>
        <div className="flex flex-wrap gap-1.5">
          {(["auto", "classic", "slim"] as const).map((choice) => {
            const active = uploadModel === choice;
            return (
              <button
                key={choice}
                type="button"
                aria-pressed={active}
                disabled={skinBusy}
                onClick={() => setUploadModel(choice)}
                className={`rounded-chip border px-2.5 py-1 text-xs transition-colors ${
                  active
                    ? "border-accent bg-accent-soft font-medium text-accent"
                    : "border-line bg-surface hover:bg-surface-hover"
                }`}
              >
                {t(`skin.modelChoice.${choice}`)}
              </button>
            );
          })}
        </div>
      </div>

      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          size="sm"
          disabled={skinBusy}
          onClick={() => void uploadSkin(account.id, uploadModel === "auto" ? null : uploadModel)}
        >
          <Upload className="size-3.5" aria-hidden />
          {t("skin.upload")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={skinBusy}
          onClick={() => void refreshProfile(account.id)}
        >
          <RefreshCw className="size-3.5" aria-hidden />
          {t("skin.refresh")}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={skinBusy || !account.hasSkin}
          onClick={() => void resetSkin(account.id)}
        >
          <RotateCcw className="size-3.5" aria-hidden />
          {t("skin.reset")}
        </Button>
      </div>

      <div className="w-full border-line border-t pt-3">
        <p className="font-medium text-xs">{t("skin.capesTitle")}</p>
        {account.capes.length === 0 ? (
          <p className="mt-1 text-fg-muted text-xs">{t("skin.noCapes")}</p>
        ) : (
          <div className="mt-2 flex flex-wrap gap-1.5">
            <button
              type="button"
              disabled={skinBusy}
              aria-pressed={!activeCape}
              onClick={() => void setCape(account.id, null)}
              className={`inline-flex items-center gap-1 rounded-chip border px-2.5 py-1 text-xs transition-colors ${
                !activeCape
                  ? "border-accent bg-accent-soft font-medium text-accent"
                  : "border-line bg-surface hover:bg-surface-hover"
              }`}
            >
              {!activeCape && <Check className="size-3" aria-hidden />}
              {t("skin.capeNone")}
            </button>
            {account.capes.map((cape) => (
              <button
                key={cape.id}
                type="button"
                disabled={skinBusy}
                aria-pressed={cape.active}
                onClick={() => void setCape(account.id, cape.id)}
                className={`inline-flex items-center gap-1 rounded-chip border px-2.5 py-1 text-xs transition-colors ${
                  cape.active
                    ? "border-accent bg-accent-soft font-medium text-accent"
                    : "border-line bg-surface hover:bg-surface-hover"
                }`}
              >
                {cape.active && <Check className="size-3" aria-hidden />}
                {cape.alias}
              </button>
            ))}
          </div>
        )}
      </div>

      <Button
        variant="ghost"
        size="sm"
        className="px-0! text-fg-muted"
        onClick={() =>
          openUrl("https://www.minecraft.net/msaprofile/mygames/editprofile").catch(
            (error: unknown) => reportError(error, "打开 Minecraft 皮肤管理"),
          )
        }
      >
        <ExternalLink className="size-3.5" aria-hidden />
        {t("skin.openOfficial")}
      </Button>
    </div>
  );
}
