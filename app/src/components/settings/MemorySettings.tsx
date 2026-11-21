import { Sparkles } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { Settings } from "@/lib/bindings";
import { gb, SLIDER_MIN_MB, SLIDER_STEP_MB, sliderMax, snapToSlider } from "@/lib/memory";
import { useMemoryPolling, useMemoryStore } from "@/stores/memory";
import { MemoryBar } from "../MemoryBar";
import { Button } from "../ui/Button";
import { Segmented } from "../ui/Segmented";
import { Slider } from "../ui/Slider";
import { Switch } from "../ui/Switch";
import { Group, Row, type UpdateSettings } from "./parts";

/** 和后端一样：给系统和别的程序留 1 GB */
const HEADROOM_MB = 1024;

type Mode = "auto" | "manual";

export interface MemoryValue {
  auto: boolean;
  maxMemoryMb: number;
}

/**
 * 内存的「自动 / 手动」+ 内存条 + 手动时的滑块（全局设置和实例的「设置」页签共用）。
 * 放在设置的一组（Group）里面，占两行。
 */
export function MemoryPicker({
  value,
  onChange,
  autoMb,
}: {
  value: MemoryValue;
  onChange: (next: MemoryValue) => void;
  /** 自动时内存条上「给游戏」那一段；不传用这台电脑自动最多给的 */
  autoMb?: number;
}) {
  const { t } = useTranslation();
  useMemoryPolling();
  const status = useMemoryStore((s) => s.status);
  // 拖动滑块时先只改这里，松手才保存
  const [draft, setDraft] = useState<number | null>(null);

  const auto = value.auto;
  const total = status?.totalMb ?? 0;
  const manual = draft ?? (status ? snapToSlider(value.maxMemoryMb, total) : value.maxMemoryMb);
  const gameMb = auto ? (autoMb ?? status?.autoMaxMb ?? 0) : manual;

  const setMode = (mode: Mode) =>
    onChange({
      auto: mode === "auto",
      maxMemoryMb:
        mode === "manual" && status ? snapToSlider(value.maxMemoryMb, total) : value.maxMemoryMb,
    });

  const commit = (mb: number) => {
    setDraft(null);
    if (mb !== value.maxMemoryMb) onChange({ auto: false, maxMemoryMb: mb });
  };

  let warning: string | null = null;
  if (!auto && status) {
    if (manual > status.availableMb - HEADROOM_MB) {
      warning = t("memory.settings.overFree", {
        gb: gb(Math.max(0, status.availableMb - HEADROOM_MB)),
      });
    } else if (manual > total / 2) {
      warning = t("memory.settings.overHalf");
    }
  }

  return (
    <>
      <Row
        label={t("memory.settings.mode")}
        hint={auto ? t("memory.settings.autoHint") : t("memory.settings.manualHint")}
        control={
          <Segmented<Mode>
            label={t("memory.settings.mode")}
            value={auto ? "auto" : "manual"}
            options={[
              { value: "auto", label: t("memory.settings.auto") },
              { value: "manual", label: t("memory.settings.manual") },
            ]}
            onChange={setMode}
          />
        }
      />
      <div className="flex flex-col gap-4 px-4 py-4">
        {status ? (
          <MemoryBar
            totalMb={status.totalMb}
            availableMb={status.availableMb}
            gameMb={gameMb}
            gameIsMax={auto && autoMb === undefined}
          />
        ) : (
          <p className="text-fg-muted text-xs">{t("memory.settings.unknown")}</p>
        )}
        {!auto && status && (
          <div className="flex items-center gap-4">
            <Slider
              value={manual}
              min={SLIDER_MIN_MB}
              max={sliderMax(total)}
              step={SLIDER_STEP_MB}
              onChange={setDraft}
              onCommit={commit}
              label={t("memory.settings.slider")}
              valueText={t("memory.gb", { gb: gb(manual) })}
            />
            <span className="w-16 shrink-0 text-right font-medium tabular-nums">
              {t("memory.gb", { gb: gb(manual) })}
            </span>
          </div>
        )}
        {warning && <p className="text-warning text-xs">{warning}</p>}
      </div>
    </>
  );
}

/**
 * 设置 →「游戏」里的内存和显卡：
 * 内存条（每两秒刷新，数字和任务管理器对得上）、自动 / 手动、手动时的滑块、整理内存；高性能显卡开关。
 */
export function MemorySettings({
  settings,
  update,
}: {
  settings: Settings;
  update: UpdateSettings;
}) {
  const { t } = useTranslation();
  const cleaning = useMemoryStore((s) => s.cleaning);
  const clean = useMemoryStore((s) => s.clean);

  return (
    <>
      <Group title={t("memory.settings.group")}>
        <MemoryPicker
          value={{ auto: settings.game.memoryAuto, maxMemoryMb: settings.game.maxMemoryMb }}
          onChange={({ auto, maxMemoryMb }) =>
            update((s) => ({ ...s, game: { ...s.game, memoryAuto: auto, maxMemoryMb } }))
          }
        />
        <Row
          label={t("memory.clean.label")}
          hint={t("memory.clean.hint")}
          control={
            <Button variant="secondary" disabled={cleaning} onClick={() => void clean()}>
              <Sparkles className="size-4" aria-hidden />
              {cleaning ? t("memory.clean.busy") : t("memory.clean.button")}
            </Button>
          }
        />
      </Group>
      <Group title={t("memory.gpu.group")}>
        <Row
          label={t("memory.gpu.label")}
          hint={t("memory.gpu.hint")}
          control={
            <Switch
              label={t("memory.gpu.label")}
              checked={settings.game.highPerformanceGpu}
              onChange={(highPerformanceGpu) =>
                update((s) => ({ ...s, game: { ...s.game, highPerformanceGpu } }))
              }
            />
          }
        />
      </Group>
    </>
  );
}
