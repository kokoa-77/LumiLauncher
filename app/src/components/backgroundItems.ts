import { Expand, ImagePlus, Images, RotateCcw, Shrink } from "lucide-react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import { commands, type WorldView } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import type { MenuItem } from "./ui/Menu";

/**
 * 「⋯」菜单里换背景的几项（首页、世界页签共用）：
 * 换背景图…（后端弹出选图片的窗口，界面拿不到路径）；精选自动封面…（从最近 5 张自动封面里挑选并锁定）；
 * 选过图以后多两项：铺满 / 完整显示、恢复默认背景。
 * 换完重读最近的世界：首页、实例卡片、实例页横幅、世界卡片一起换图，主色跟着新图变。
 */
export function useBackgroundItems(
  instanceId: string,
  world: Pick<WorldView, "folder" | "imageKind" | "imageFit"> | null,
  onAutoCovers?: () => void,
): MenuItem[] {
  const { t } = useTranslation();
  const refresh = useGameStore((s) => s.refreshRecentWorlds);
  if (!world) return [];
  const custom = world.imageKind === "custom";
  const contain = world.imageFit === "contain";

  const pick = () => {
    commands
      .worldsBackgroundPick(instanceId, world.folder)
      .then(unwrap)
      .then((picked) => {
        if (picked) void refresh();
      })
      .catch((error: unknown) => reportError(error, "换背景图"));
  };
  const fit = () => {
    commands
      .worldsBackgroundFit(instanceId, world.folder, contain ? "cover" : "contain")
      .then(unwrap)
      .then(() => refresh())
      .catch((error: unknown) => reportError(error, "改背景图的显示方式"));
  };
  const reset = () => {
    commands
      .worldsBackgroundClear(instanceId, world.folder)
      .then(unwrap)
      .then(() => refresh())
      .catch((error: unknown) => reportError(error, "恢复默认背景"));
  };

  return [
    {
      key: "background",
      label: t(custom ? "background.change" : "background.pick"),
      icon: ImagePlus,
      onSelect: pick,
    },
    ...(onAutoCovers
      ? [
          {
            key: "background-auto",
            label: t("background.autoMenu"),
            icon: Images,
            onSelect: onAutoCovers,
          },
        ]
      : []),
    ...(custom
      ? [
          {
            key: "background-fit",
            label: t(contain ? "background.cover" : "background.contain"),
            icon: contain ? Expand : Shrink,
            onSelect: fit,
          },
          {
            key: "background-reset",
            label: t("background.reset"),
            icon: RotateCcw,
            onSelect: reset,
          },
        ]
      : []),
  ];
}
