import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { SpotArt } from "../art/SpotArt";

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  hint: string;
  action?: ReactNode;
  illustration?: string | ReactNode;
}

export function EmptyState({ icon: Icon, title, hint, action, illustration }: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded-panel border border-line border-dashed px-6 py-14 text-center">
      {typeof illustration === "string" ? (
        <SpotArt src={illustration} />
      ) : (
        (illustration ?? (
          <div className="grid size-12 place-items-center rounded-icon bg-accent-soft text-accent">
            <Icon className="size-6" aria-hidden />
          </div>
        ))
      )}
      <div>
        <p className="font-medium text-fg">{title}</p>
        <p className="mt-1 max-w-sm text-fg-muted text-sm">{hint}</p>
      </div>
      {action}
    </div>
  );
}
