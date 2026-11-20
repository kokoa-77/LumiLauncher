import { Plus, Trash2 } from "lucide-react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { EnvVar } from "@/lib/bindings";
import { Button } from "../ui/Button";
import { TextInput } from "../ui/Field";
import { IconButton } from "../ui/IconButton";

/** 名字能不能用：不能空、不能有「=」（系统不认） */
function nameProblem(name: string): boolean {
  return name.includes("=");
}

/**
 * 环境变量（全局设置和实例设置共用，D17i 验收时加）：一行一个「名字 = 值」，右边删除，下面「添加」。
 * 输入框离开时保存；名字空着的那行不保存（还在填）。放在设置的一组（Group）里面。
 * 外面切换「单独设置」时换一个 key，让这里重新读一遍。
 */
export function EnvVarsEditor({
  value,
  onChange,
}: {
  value: EnvVar[];
  onChange: (next: EnvVar[]) => void;
}) {
  const { t } = useTranslation();
  // 本地一份：新加的空行在名字填好之前不交给后端（后端会把空名字去掉）。
  // 另外存一份在 ref 里：离开输入框时读最新的（打完字马上切走时，这一轮渲染可能还没跑完）
  const [rows, setRowsState] = useState<EnvVar[]>(() => value);
  const latest = useRef(rows);
  const setRows = (next: EnvVar[]) => {
    latest.current = next;
    setRowsState(next);
  };

  const commit = (next: EnvVar[]) => {
    const valid = next.filter((v) => v.name.trim() !== "" && !nameProblem(v.name));
    const same =
      valid.length === value.length &&
      valid.every((v, i) => v.name.trim() === value[i]?.name && v.value === value[i]?.value);
    if (!same) onChange(valid.map((v) => ({ name: v.name.trim(), value: v.value })));
  };

  const edit = (index: number, patch: Partial<EnvVar>) =>
    setRows(latest.current.map((row, i) => (i === index ? { ...row, ...patch } : row)));

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      {rows.length === 0 && <p className="text-fg-muted text-sm">{t("env.empty")}</p>}
      {rows.map((row, index) => (
        // 行没有自己的编号，用位置当 key：删除时整列重排也没关系（输入框离开就已经保存了）
        <div key={index} className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <div className="w-56 shrink-0">
              <TextInput
                className="font-mono text-xs"
                value={row.name}
                placeholder={t("env.name")}
                aria-label={t("env.name")}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => edit(index, { name: e.target.value })}
                onBlur={() => commit(latest.current)}
              />
            </div>
            <span className="text-fg-muted" aria-hidden>
              =
            </span>
            <div className="min-w-0 flex-1">
              <TextInput
                className="font-mono text-xs"
                value={row.value}
                placeholder={t("env.value")}
                aria-label={t("env.value")}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => edit(index, { value: e.target.value })}
                onBlur={() => commit(latest.current)}
              />
            </div>
            <IconButton
              label={t("env.remove")}
              size="sm"
              danger
              onClick={() => {
                const next = latest.current.filter((_, i) => i !== index);
                setRows(next);
                commit(next);
              }}
            >
              <Trash2 className="size-4" aria-hidden />
            </IconButton>
          </div>
          {nameProblem(row.name) && <p className="px-1 text-warning text-xs">{t("env.badName")}</p>}
        </div>
      ))}
      <div>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => setRows([...latest.current, { name: "", value: "" }])}
        >
          <Plus className="size-3.5" aria-hidden />
          {t("env.add")}
        </Button>
      </div>
    </div>
  );
}
