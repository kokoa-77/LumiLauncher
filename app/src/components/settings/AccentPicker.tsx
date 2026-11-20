import { Check } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { Settings } from "@/lib/bindings";
import { ownAccent } from "@/lib/cover";
import { accentStyle, applyAccent } from "@/lib/themeEngine";
import { Switch } from "../ui/Switch";

/** 可选的主色（色相、彩度）。亮度按对比度自动算，深浅色下字都看得清 */
export const ACCENT_SWATCHES = [
  { key: "sky", hue: 245, chroma: 0.15 },
  { key: "lake", hue: 215, chroma: 0.14 },
  { key: "mint", hue: 180, chroma: 0.12 },
  { key: "leaf", hue: 145, chroma: 0.14 },
  { key: "lime", hue: 115, chroma: 0.15 },
  { key: "amber", hue: 70, chroma: 0.15 },
  { key: "coral", hue: 30, chroma: 0.15 },
  { key: "sakura", hue: 355, chroma: 0.13 },
  { key: "violet", hue: 295, chroma: 0.14 },
] as const;

/** 和某个色块是不是同一个颜色（色相差不到 1 度就算） */
function same(a: { hue: number; chroma: number }, b: { hue: number; chroma: number }) {
  const delta = Math.abs(((((a.hue - b.hue) % 360) + 540) % 360) - 180);
  return delta < 1 && Math.abs(a.chroma - b.chroma) < 0.005;
}

/**
 * 主色：一排色块 + 「跟着世界变」。选了马上换（整套界面平滑换色）；
 * 跟着世界变开着时，进到有截图的世界会换成世界的颜色，别的时候用这里选的。
 * 设置弹窗「外观」和首次运行引导共用。
 */
export function AccentPicker({
  settings,
  update,
}: {
  settings: Settings;
  update: (change: (current: Settings) => Settings) => void;
}) {
  const { t } = useTranslation();
  const own = ownAccent(settings);
  const follow = settings.appearance.accentFollowWorld;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2" role="radiogroup" aria-label={t("accent.label")}>
        {ACCENT_SWATCHES.map((swatch) => {
          const selected = same(own, swatch);
          return (
            <button
              key={swatch.key}
              type="button"
              role="radio"
              aria-checked={selected}
              aria-label={t(`accent.swatch.${swatch.key}`)}
              onClick={() => {
                applyAccent(swatch.hue, swatch.chroma);
                update((s) => ({
                  ...s,
                  appearance: {
                    ...s.appearance,
                    accentHue: swatch.hue,
                    accentChroma: swatch.chroma,
                  },
                }));
              }}
              style={accentStyle(swatch.hue, swatch.chroma)}
              className={`accent-scope grid size-8 place-items-center rounded-icon bg-accent text-on-accent ring-offset-2 ring-offset-overlay transition-[scale] duration-(--motion-press) hover:scale-110 ${
                selected ? "ring-2 ring-accent" : ""
              }`}
            >
              {selected && <Check className="size-4" aria-hidden />}
            </button>
          );
        })}
      </div>
      <div className="flex items-center justify-between gap-4">
        <p className="font-medium text-sm">{t("accent.follow")}</p>
        <Switch
          checked={follow}
          label={t("accent.follow")}
          onChange={(accentFollowWorld) =>
            update((s) => ({ ...s, appearance: { ...s.appearance, accentFollowWorld } }))
          }
        />
      </div>
    </div>
  );
}
