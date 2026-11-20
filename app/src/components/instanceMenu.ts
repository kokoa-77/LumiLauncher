import type { TFunction } from "i18next";
import { FolderInput, MonitorUp, Pin, PinOff } from "lucide-react";
import type { MenuItem } from "@/components/ui/Menu";
import type { InstanceSummary } from "@/lib/bindings";
import { useGameStore } from "@/stores/game";

/** 实例卡片和实例页「⋮」里共用的三项（D17k）：置顶、移到分组、桌面快捷方式 */
export function instanceMenuItems(instance: InstanceSummary, t: TFunction): MenuItem[] {
  const store = useGameStore.getState();
  const pinned = instance.pinnedAt !== null;
  return [
    {
      key: "pin",
      label: pinned ? t("instances.unpin") : t("instances.pin"),
      icon: pinned ? PinOff : Pin,
      onSelect: () => void store.setPinned(instance.id, !pinned),
    },
    {
      key: "group",
      label: t("instances.moveGroup"),
      icon: FolderInput,
      onSelect: () => store.askGroup(instance.id),
    },
    {
      key: "shortcut",
      label: t("shortcut.menu"),
      icon: MonitorUp,
      onSelect: () => void store.createShortcut(instance.id),
    },
  ];
}
