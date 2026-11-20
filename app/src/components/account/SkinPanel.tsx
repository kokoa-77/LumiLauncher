import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";
import { lazy, Suspense } from "react";
import { useTranslation } from "react-i18next";
import type { AccountView } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useSkin } from "@/stores/accounts";
import { useAppStore, useEffectsLevel } from "@/stores/app";
import { PlayerHead } from "../PlayerHead";
import { Button } from "../ui/Button";
import { SkinFigure } from "./SkinFigure";

// 3D 预览（three.js 很大）只在这里用到，第一次打开时才加载
const SkinViewer3D = lazy(() => import("./SkinViewer3D"));

/**
 * 皮肤预览：精美模式是能转动的 3D 人物（拖动转身）；流畅模式是 2D 正面 + 背面。
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

/** 在 Minecraft 官网管理正式皮肤。 */
export function SkinControls() {
  const { t } = useTranslation();
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
