import { useSyncExternalStore } from "react";
import { resolveTheme, systemPrefersDark, watchSystemTheme } from "@/lib/theme";
import { useAppStore, useEffectsLevel } from "@/stores/app";

export interface SceneArt {
  night: string;
  day?: string;
}

/** 只请求当前深浅色的插画。流畅模式默认沿用调用方的渐变底色。 */
export function SceneBackground({
  art,
  className = "",
  showInLite = false,
}: {
  art: SceneArt;
  className?: string;
  showInLite?: boolean;
}) {
  const effects = useEffectsLevel();
  const mode = useAppStore((s) => s.settings?.appearance.theme ?? "system");
  const systemDark = useSyncExternalStore(watchSystemTheme, systemPrefersDark, () => false);
  const theme = resolveTheme(mode, systemDark);

  if (effects === "lite" && !showInLite) return null;
  return (
    <img
      alt=""
      aria-hidden
      decoding="async"
      src={theme === "light" ? (art.day ?? art.night) : art.night}
      className={`pointer-events-none absolute inset-0 size-full object-cover ${className}`}
    />
  );
}
