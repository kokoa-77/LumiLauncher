import type { ReactNode } from "react";
import type { Settings } from "@/lib/bindings";
import { TextInput } from "../ui/Field";

/** 设置页改一项：给出「怎么从现在的设置变成新的」 */
export type UpdateSettings = (change: (current: Settings) => Settings) => void;

/** 设置的一组：灰色小标题 + 一块圆角面板，面板里一行一项，行间细线 */
export function Group({
  title,
  badge,
  action,
  children,
}: {
  title?: string;
  /** 紧跟在标题后面的小标签（实例设置里的「跟着全局」） */
  badge?: ReactNode;
  /** 标题那一行右边的操作（比如「添加 Java」） */
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      {(title || action) && (
        <div className="flex min-h-8 items-center justify-between gap-3 px-1">
          {title && (
            <div className="flex min-w-0 items-center gap-2">
              <h3 className="font-medium text-fg-muted text-xs">{title}</h3>
              {badge}
            </div>
          )}
          {action && <div className="flex items-center gap-2">{action}</div>}
        </div>
      )}
      <div className="divide-y divide-line rounded-panel border border-line bg-surface">
        {children}
      </div>
    </section>
  );
}

/** 一行设置：左边名字和说明，右边控件 */
export function Row({
  label,
  hint,
  control,
}: {
  label: string;
  hint?: string;
  control: ReactNode;
}) {
  return (
    <div className="flex min-h-14 items-center justify-between gap-6 px-4 py-3">
      <div className="min-w-0 flex-1">
        <p className="font-medium">{label}</p>
        {hint && <p className="mt-0.5 text-fg-muted text-xs">{hint}</p>}
      </div>
      {/* 控件不许被说明文字挤窄（中文能在任意两个字之间换行，挤窄就变成一字一行） */}
      <div className="shrink-0">{control}</div>
    </div>
  );
}

/** 控件太宽、放不进右边的一行（长输入框）：名字和说明在上，控件占满一整行 */
export function StackedRow({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <div>
        <p className="font-medium">{label}</p>
        {hint && <p className="mt-0.5 text-fg-muted text-xs">{hint}</p>}
      </div>
      {children}
    </div>
  );
}

/** 一行参数输入框：离开输入框时保存；保存后（去掉首尾空白）的值变了就跟着更新（全局设置和实例设置共用） */
export function ArgsInput({
  value,
  placeholder,
  label,
  onSave,
}: {
  value: string;
  placeholder: string;
  label: string;
  onSave: (value: string) => void;
}) {
  return (
    <TextInput
      key={value}
      className="font-mono text-xs"
      defaultValue={value}
      placeholder={placeholder}
      aria-label={label}
      spellCheck={false}
      autoComplete="off"
      onBlur={(event) => {
        const next = event.target.value.trim();
        if (next !== value) onSave(next);
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
    />
  );
}
