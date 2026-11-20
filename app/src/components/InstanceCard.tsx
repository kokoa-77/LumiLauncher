import { Package } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import type { InstanceSummary } from "@/lib/bindings";
import { PlayButton } from "./PlayButton";
import { Truncated } from "./ui/HoverCard";

/** 实例的字母头像（封面图和「随实例变色」在第 16 天做） */
export function InstanceAvatar({ name, large = false }: { name: string; large?: boolean }) {
  const letter = Array.from(name.trim())[0]?.toUpperCase() ?? "?";
  return (
    <div
      className={`grid shrink-0 place-items-center bg-accent-soft font-semibold text-accent ${
        large ? "size-16 rounded-panel text-2xl" : "size-11 rounded-row text-lg"
      }`}
      aria-hidden
    >
      {letter}
    </div>
  );
}

export function instanceSubtitle(instance: InstanceSummary): string {
  return [instance.gameVersion, instance.loader].filter(Boolean).join(" · ");
}

export function InstanceCard({ instance }: { instance: InstanceSummary }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const open = () => navigate(`/instances/${encodeURIComponent(instance.id)}`);
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(event) => {
        if (event.key === "Enter") open();
      }}
      className="group flex items-center gap-4 rounded-panel border border-line bg-surface p-4 transition-[translate,background-color] duration-(--motion-press) ease-out hover:-translate-y-0.5 hover:bg-surface-hover"
    >
      <InstanceAvatar name={instance.name} />
      <div className="min-w-0 flex-1">
        <Truncated as="p" className="truncate font-medium" text={instance.name} />
        <p className="truncate text-fg-muted text-xs">{instanceSubtitle(instance)}</p>
        {instance.pack && (
          <p className="mt-1 flex items-center gap-1 text-fg-muted text-xs">
            <Package className="size-3" aria-hidden />
            {t("instances.fromPack", { pack: instance.pack })}
          </p>
        )}
      </div>
      <PlayButton instanceId={instance.id} />
    </div>
  );
}
