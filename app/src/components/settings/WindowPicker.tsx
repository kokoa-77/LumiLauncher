import { useTranslation } from "react-i18next";
import type { WindowMode, WindowSettings } from "@/lib/bindings";
import { TextInput } from "../ui/Field";
import { Segmented } from "../ui/Segmented";
import { Row } from "./parts";

const MIN_WIDTH = 320;
const MIN_HEIGHT = 240;
const MAX_SIDE = 16384;

/** 输入框里的数字：不是数字就用原来的，超出范围拉回来 */
function parseSide(text: string, fallback: number, min: number): number {
  const value = Number.parseInt(text.trim(), 10);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(MAX_SIDE, Math.max(min, value));
}

/**
 * 游戏窗口一开始多大（全局设置和实例的「设置」页签共用，D17i）：游戏默认 / 自定义宽高 / 全屏。
 * 放在设置的一组（Group）里面；选「自定义」时多一行宽高。
 */
export function WindowPicker({
  value,
  onChange,
}: {
  value: WindowSettings;
  onChange: (next: WindowSettings) => void;
}) {
  const { t } = useTranslation();
  // 输入框自己是占满一行的，宽度由外面这层定
  const side = (key: "width" | "height", min: number) => (
    <div className="w-24">
      <TextInput
        key={`${key}-${value[key]}`}
        className="px-2 text-center tabular-nums"
        inputMode="numeric"
        defaultValue={value[key]}
        aria-label={t(`window.${key}`)}
        onBlur={(event) => {
          const next = parseSide(event.target.value, value[key], min);
          if (next !== value[key]) onChange({ ...value, [key]: next });
          else event.target.value = String(value[key]);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
        }}
      />
    </div>
  );
  return (
    <>
      <Row
        label={t("window.label")}
        control={
          <Segmented<WindowMode>
            label={t("window.label")}
            value={value.mode}
            options={[
              { value: "default", label: t("window.default") },
              { value: "custom", label: t("window.custom") },
              { value: "maximized", label: t("window.maximized") },
              { value: "fullscreen", label: t("window.fullscreen") },
            ]}
            onChange={(mode) => onChange({ ...value, mode })}
          />
        }
      />
      {value.mode === "custom" && (
        <Row
          label={t("window.size")}
          control={
            <div className="flex items-center gap-2 text-fg-muted text-sm">
              {side("width", MIN_WIDTH)}
              <span aria-hidden>×</span>
              {side("height", MIN_HEIGHT)}
            </div>
          }
        />
      )}
    </>
  );
}
