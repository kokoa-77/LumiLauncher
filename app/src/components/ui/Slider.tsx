import type { CSSProperties } from "react";

interface SliderProps {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
  /** 松手（或键盘改完）时调用：保存设置放在这里，拖动过程中不反复保存 */
  onCommit?: (value: number) => void;
  label: string;
  /** 读屏软件念的值，例如「6 GB」 */
  valueText?: string;
  disabled?: boolean;
}

/**
 * 滑块：系统自带的 range 输入框（键盘、读屏都能用），样子在 global.css 的 `.ui-slider`：
 * 胶囊轨道，左边已选的部分是主色，圆形把手。只有拖到的值在变，不做动画。
 */
export function Slider({
  value,
  min,
  max,
  step,
  onChange,
  onCommit,
  label,
  valueText,
  disabled = false,
}: SliderProps) {
  const fill = max > min ? ((value - min) / (max - min)) * 100 : 0;
  return (
    <input
      type="range"
      className="ui-slider w-full"
      style={{ "--fill": `${Math.min(100, Math.max(0, fill))}%` } as CSSProperties}
      value={value}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      aria-label={label}
      aria-valuetext={valueText}
      onChange={(event) => onChange(Number(event.target.value))}
      onPointerUp={(event) => onCommit?.(Number(event.currentTarget.value))}
      onKeyUp={(event) => onCommit?.(Number(event.currentTarget.value))}
    />
  );
}
