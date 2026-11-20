import { Coffee, FileArchive, Gamepad2, ListTodo, type LucideIcon, Package } from "lucide-react";
import type { JobSnapshot, JobSource } from "@/lib/bindings";
import { instanceOf } from "@/lib/jobs";
import { useGameStore } from "@/stores/game";
import { InstanceThumb } from "../discover/TargetPicker";

const SOURCE_ICONS: Record<JobSource, LucideIcon> = {
  game: Gamepad2,
  java: Coffee,
  modrinth: Package,
  curseForge: Package,
  local: FileArchive,
  other: ListTodo,
};

/**
 * 任务的小图标：装模组的用模组图标，属于某个实例的用实例的小封面，别的按来源画一个图标。
 * 图标小方块不是控件，用「列表行」档圆角。
 */
export function JobIcon({ job, className = "size-10" }: { job: JobSnapshot; className?: string }) {
  const instanceId = instanceOf(job);
  const instance = useGameStore((s) => s.instances.find((i) => i.id === instanceId));
  const world = useGameStore(
    (s) => s.recentWorlds?.find((r) => r.instanceId === instanceId)?.world,
  );
  if (job.iconUrl) {
    return (
      <img
        src={job.iconUrl}
        alt=""
        loading="lazy"
        className={`shrink-0 rounded-row bg-surface-hover object-cover ${className}`}
      />
    );
  }
  if (instance) {
    return (
      <InstanceThumb instance={instance} world={world} className={`rounded-row ${className}`} />
    );
  }
  const Icon = SOURCE_ICONS[job.source];
  return (
    <span
      className={`grid shrink-0 place-items-center rounded-row bg-surface-hover text-fg-muted ${className}`}
    >
      <Icon className="size-4" aria-hidden />
    </span>
  );
}
