import type { InputHTMLAttributes, ReactNode, TextareaHTMLAttributes } from "react";

const control =
  "h-9 w-full rounded-control border border-line bg-surface px-4 text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus:border-accent disabled:opacity-45";

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="font-medium text-sm">{label}</span>
      {children}
      {hint && <span className="text-fg-muted text-xs">{hint}</span>}
    </label>
  );
}

export function TextInput({ className = "", ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={`${control} ${className}`} {...props} />;
}

/** 多行输入框（世界笔记的随手记）：放得下好几行，按容器用圆角矩形，胶囊会切掉四角的字 */
export function TextArea({
  className = "",
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={`w-full resize-none rounded-panel border border-line bg-surface px-4 py-2.5 text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus:border-accent disabled:opacity-45 ${className}`}
      {...props}
    />
  );
}
