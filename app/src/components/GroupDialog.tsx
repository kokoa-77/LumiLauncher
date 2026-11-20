import { Check, Folder, FolderMinus } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { TextInput } from "@/components/ui/Field";
import { useGameStore } from "@/stores/game";

/** 全部分组名（按名字排） */
export function groupNames(instances: { group: string | null }[]): string[] {
  const names = new Set<string>();
  for (const instance of instances) if (instance.group) names.add(instance.group);
  return [...names].sort((a, b) => a.localeCompare(b, "zh-CN"));
}

/**
 * 「移到分组」（D17k）：点一个已有的分组就挪过去；下面输入新名字新建一个。
 * 实例卡片和实例页的「⋮」都打开它（`useGameStore.groupAsk`）。
 */
export function GroupDialog() {
  const { t } = useTranslation();
  const instanceId = useGameStore((s) => s.groupAsk);
  const askGroup = useGameStore((s) => s.askGroup);
  const setGroup = useGameStore((s) => s.setGroup);
  const instances = useGameStore((s) => s.instances);
  const instance = instances.find((i) => i.id === instanceId);
  const groups = useMemo(() => groupNames(instances), [instances]);
  const [name, setName] = useState("");

  useEffect(() => {
    if (instanceId) setName("");
  }, [instanceId]);

  const close = () => askGroup(null);
  const move = (group: string | null) => {
    if (!instance) return;
    close();
    if (group !== instance.group) void setGroup(instance.id, group);
  };
  const typed = name.trim();

  const row = (key: string, label: string, group: string | null, Icon: typeof Folder) => {
    const current = (instance?.group ?? null) === group;
    return (
      <button
        key={key}
        type="button"
        onClick={() => move(group)}
        className={`flex w-full items-center gap-3 rounded-row px-3 py-2 text-left text-sm transition-colors duration-(--motion-press) ${
          current ? "bg-accent-soft text-accent" : "hover:bg-surface-hover"
        }`}
      >
        <Icon className="size-4 shrink-0" aria-hidden />
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {current && <Check className="size-4 shrink-0" aria-hidden />}
      </button>
    );
  };

  return (
    <Dialog
      open={instance !== undefined}
      onClose={close}
      title={t("instances.group.title", { name: instance?.name ?? "" })}
    >
      <div className="flex flex-col gap-4">
        <div className="flex max-h-72 flex-col gap-0.5 overflow-y-auto">
          {groups.map((group) => row(group, group, group, Folder))}
          {row("none", t("instances.group.none"), null, FolderMinus)}
        </div>
        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (typed) move(typed);
          }}
        >
          <TextInput
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t("instances.group.newPlaceholder")}
            aria-label={t("instances.group.newLabel")}
            maxLength={24}
            className="flex-1"
          />
          <Button type="submit" variant="primary" disabled={!typed}>
            {t("instances.group.create")}
          </Button>
        </form>
      </div>
    </Dialog>
  );
}
