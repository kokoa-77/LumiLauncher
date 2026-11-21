import { Check, Wand2 } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import type { GcPreset } from "@/lib/bindings";
import { ARG_PRESETS, addArgs, hasArgs, picksCollector, removeArgs } from "@/lib/javaArgs";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { HoverHint } from "../ui/HoverCard";
import { ArgsInput, StackedRow } from "./parts";

export interface JavaArgsValue {
  gcPreset: GcPreset;
  jvmArgs: string;
  gameArgs: string;
}

const GC_OPTIONS: GcPreset[] = ["auto", "zgc", "off"];

/**
 * 「Java 参数」「游戏参数」两行（全局设置和实例设置共用）：
 * Java 参数旁边是「参数预设」，内存回收方式和常用参数都在预设里点选，不用自己记参数。
 * 放在设置的一组（Group）里面。
 */
export function JavaArgsRows({
  value,
  onChange,
}: {
  value: JavaArgsValue;
  onChange: (next: JavaArgsValue) => void;
}) {
  const { t } = useTranslation();
  const [presets, setPresets] = useState(false);
  const userGc = picksCollector(value.jvmArgs);
  return (
    <>
      <StackedRow label={t("java.args.jvm")}>
        <div className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <ArgsInput
              value={value.jvmArgs}
              placeholder={t("java.args.jvmPlaceholder")}
              label={t("java.args.jvm")}
              onSave={(jvmArgs) => onChange({ ...value, jvmArgs })}
            />
          </div>
          <Button variant="secondary" onClick={() => setPresets(true)}>
            <Wand2 className="size-4" aria-hidden />
            {t("java.presets.open")}
          </Button>
        </div>
        {/* 平时是一句实用建议；换了回收方式写换成了什么；自己写了回收器时提醒预设不生效 */}
        <p className={`px-1 text-xs ${userGc ? "text-warning" : "text-fg-muted"}`}>
          {userGc
            ? t("java.args.gcCustom")
            : value.gcPreset === "auto"
              ? t("java.args.gcTip")
              : t("java.args.gcNow", { gc: t(`java.args.gc_${value.gcPreset}`) })}
        </p>
      </StackedRow>
      <StackedRow label={t("java.args.game")}>
        <ArgsInput
          value={value.gameArgs}
          placeholder={t("java.args.gamePlaceholder")}
          label={t("java.args.game")}
          onSave={(gameArgs) => onChange({ ...value, gameArgs })}
        />
      </StackedRow>
      <PresetsDialog
        open={presets}
        onClose={() => setPresets(false)}
        value={value}
        onChange={onChange}
      />
    </>
  );
}

/** 「做完了」的样子：主色浅底 + 主色字 + 勾（docs/DESIGN.md 第 5 节） */
const DONE =
  "inline-flex h-8 shrink-0 items-center gap-1.5 rounded-control bg-accent-soft px-3 font-medium text-accent text-xs";

/** 预设里的一行：名字 + 一句说明（+ 参数原文），右边一个按钮 */
function PresetRow({
  title,
  description,
  args,
  action,
}: {
  title: string;
  description: string;
  args?: string;
  action: ReactNode;
}) {
  return (
    <div className="flex items-center gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <p className="font-medium text-sm">{title}</p>
        <p className="mt-0.5 text-fg-muted text-xs">{description}</p>
        {args && <p className="mt-1 truncate font-mono text-2xs text-fg-muted">{args}</p>}
      </div>
      {action}
    </div>
  );
}

/** 参数预设：内存回收方式（三选一）、常用参数（一键加上 / 去掉，改的是「Java 参数」那一行） */
function PresetsDialog({
  open,
  onClose,
  value,
  onChange,
}: {
  open: boolean;
  onClose: () => void;
  value: JavaArgsValue;
  onChange: (next: JavaArgsValue) => void;
}) {
  const { t } = useTranslation();
  const userGc = picksCollector(value.jvmArgs);
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t("java.presets.title")}
      wide
      footer={
        <Button variant="primary" onClick={onClose}>
          {t("java.presets.done")}
        </Button>
      }
    >
      <div className="flex flex-col gap-5">
        <section className="flex flex-col gap-2">
          <h3 className="px-1 font-medium text-fg-muted text-xs">{t("java.presets.gcGroup")}</h3>
          {userGc && <p className="px-1 text-warning text-xs">{t("java.presets.gcUserWrote")}</p>}
          <div className="divide-y divide-line rounded-panel border border-line bg-surface">
            {GC_OPTIONS.map((gc) => (
              <PresetRow
                key={gc}
                title={t(`java.args.gc_${gc}`)}
                description={t(`java.presets.gc_${gc}`)}
                action={
                  gc === value.gcPreset ? (
                    <span className={DONE}>
                      <Check className="size-3.5" aria-hidden />
                      {t("java.presets.inUse")}
                    </span>
                  ) : (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => onChange({ ...value, gcPreset: gc })}
                    >
                      {t("java.presets.use")}
                    </Button>
                  )
                }
              />
            ))}
          </div>
        </section>
        <section className="flex flex-col gap-2">
          <h3 className="px-1 font-medium text-fg-muted text-xs">{t("java.presets.argsGroup")}</h3>
          <div className="divide-y divide-line rounded-panel border border-line bg-surface">
            {ARG_PRESETS.map((preset) => {
              const added = hasArgs(value.jvmArgs, preset.args);
              return (
                <PresetRow
                  key={preset.id}
                  title={t(`java.presets.${preset.id}.title`)}
                  description={t(`java.presets.${preset.id}.hint`)}
                  args={preset.args.join(" ")}
                  action={
                    added ? (
                      <HoverHint hint={t("java.presets.removeHint")}>
                        <button
                          type="button"
                          aria-pressed
                          aria-label={`${t("java.presets.added")}，${t("java.presets.removeHint")}`}
                          className={`${DONE} transition-opacity duration-(--motion-press) hover:opacity-80`}
                          onClick={() =>
                            onChange({ ...value, jvmArgs: removeArgs(value.jvmArgs, preset.args) })
                          }
                        >
                          <Check className="size-3.5" aria-hidden />
                          {t("java.presets.added")}
                        </button>
                      </HoverHint>
                    ) : (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          onChange({ ...value, jvmArgs: addArgs(value.jvmArgs, preset.args) })
                        }
                      >
                        {t("java.presets.add")}
                      </Button>
                    )
                  }
                />
              );
            })}
          </div>
        </section>
      </div>
    </Dialog>
  );
}
