import { Check, ChevronDown } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { duration, ease } from "@/lib/motion";
import { VirtualList } from "../VirtualList";

export interface SelectOption {
  value: string;
  label: string;
}

interface SelectProps {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  /** 无障碍名称（读屏用） */
  label: string;
  disabled?: boolean;
  /** 没有选项时按钮上显示的文字 */
  placeholder?: string;
  className?: string;
}

const ROW_HEIGHT = 34;
/** 列表上下的内边距之和（p-1.5） */
const LIST_PADDING = 12;
const MAX_LIST_HEIGHT = 288;
/** 下方剩余空间小于这个值、而上方更宽裕时，才往上展开 */
const MIN_SPACE_BELOW = 160;
const GAP = 4;

interface Placement {
  left: number;
  width: number;
  top?: number;
  bottom?: number;
  maxHeight: number;
  upward: boolean;
}

/**
 * 自己的下拉框（替代系统下拉框）：
 * - 默认往下展开，只有窗口底部真放不下时才往上
 * - 列表挂在页面最外层，不会被弹窗的滚动区域裁掉
 * - 长列表虚拟滚动（NeoForge 一个版本就有两百多个）
 * - 键盘：上下移动、回车选中、Esc 关闭；配色跟主题走
 */
export function Select({
  value,
  options,
  onChange,
  label,
  disabled = false,
  placeholder = "",
  className = "",
}: SelectProps) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [placement, setPlacement] = useState<Placement | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const selectedIndex = options.findIndex((o) => o.value === value);
  const selected = options[selectedIndex];

  const place = () => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (!rect) return;
    const listHeight = Math.min(MAX_LIST_HEIGHT, options.length * ROW_HEIGHT + LIST_PADDING);
    const below = window.innerHeight - rect.bottom - GAP * 2;
    const above = rect.top - GAP * 2;
    const upward = below < Math.min(listHeight, MIN_SPACE_BELOW) && above > below;
    setPlacement({
      left: rect.left,
      width: rect.width,
      ...(upward ? { bottom: window.innerHeight - rect.top + GAP } : { top: rect.bottom + GAP }),
      maxHeight: Math.min(listHeight, upward ? above : below),
      upward,
    });
  };

  // 点外面、窗口大小变化、外面的区域滚动时关闭
  useEffect(() => {
    if (!open) return;
    const outside = (event: Event) => {
      const target = event.target as Node;
      if (!buttonRef.current?.contains(target) && !listRef.current?.contains(target))
        setOpen(false);
    };
    const close = () => setOpen(false);
    window.addEventListener("mousedown", outside);
    window.addEventListener("scroll", outside, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", outside);
      window.removeEventListener("scroll", outside, true);
      window.removeEventListener("resize", close);
    };
  }, [open]);

  const openList = () => {
    if (disabled || options.length === 0) return;
    // 只在打开的那一刻算位置（每次渲染都算会触发无限重画）；窗口大小一变就关掉
    place();
    setActive(Math.max(0, selectedIndex));
    setOpen(true);
  };

  const choose = (index: number) => {
    const option = options[index];
    if (option) onChange(option.value);
    setOpen(false);
    buttonRef.current?.focus();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
        event.preventDefault();
        openList();
      }
      return;
    }
    const last = options.length - 1;
    const moves: Record<string, number> = {
      ArrowDown: Math.min(last, active + 1),
      ArrowUp: Math.max(0, active - 1),
      PageDown: Math.min(last, active + 8),
      PageUp: Math.max(0, active - 8),
      Home: 0,
      End: last,
    };
    if (event.key in moves) {
      event.preventDefault();
      setActive(moves[event.key] ?? active);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      choose(active);
    } else if (event.key === "Escape" || event.key === "Tab") {
      event.stopPropagation();
      setOpen(false);
    }
  };

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => (open ? setOpen(false) : openList())}
        onKeyDown={onKeyDown}
        className={`flex h-9 w-full items-center justify-between gap-2 rounded-control border bg-surface pr-3 pl-4 text-left text-fg text-sm shadow-raised outline-none transition-colors duration-(--motion-press) focus-visible:border-accent disabled:opacity-45 ${
          open ? "border-accent" : "border-line"
        } ${className}`}
      >
        <span className="truncate">{selected?.label ?? placeholder}</span>
        <ChevronDown
          className={`size-4 shrink-0 text-fg-muted transition-transform duration-(--motion-toggle) ${open ? "rotate-180" : ""}`}
          aria-hidden
        />
      </button>
      {typeof document !== "undefined" &&
        createPortal(
          <AnimatePresence>
            {open && placement && (
              <motion.div
                ref={listRef}
                role="listbox"
                aria-label={label}
                className="fixed z-50 overflow-hidden rounded-panel border border-line bg-overlay p-1.5 shadow-overlay"
                style={{
                  left: placement.left,
                  width: placement.width,
                  top: placement.top,
                  bottom: placement.bottom,
                  transformOrigin: placement.upward ? "bottom" : "top",
                }}
                initial={{ opacity: 0, y: placement.upward ? 4 : -4, scaleY: 0.98 }}
                animate={{
                  opacity: 1,
                  y: 0,
                  scaleY: 1,
                  transition: { duration: duration.overlay, ease: ease.out },
                }}
                exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
              >
                <VirtualList
                  items={options}
                  estimate={ROW_HEIGHT}
                  activeIndex={active}
                  style={{
                    maxHeight: placement.maxHeight - LIST_PADDING,
                    height: options.length * ROW_HEIGHT,
                  }}
                  getKey={(option) => option.value}
                  render={(option, index) => {
                    const isSelected = option.value === value;
                    return (
                      <div
                        role="option"
                        aria-selected={isSelected}
                        tabIndex={-1}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => choose(index)}
                        onMouseEnter={() => setActive(index)}
                        onKeyDown={() => undefined}
                        // 选中的是主色浅底 + 主色字（开发合同第 5 节「选中」，D18 验收时发现少了浅底）；
                        // 移到的是灰底，移到选中的那项上还是浅底
                        className={`flex items-center justify-between gap-2 rounded-item px-2.5 text-sm ${
                          isSelected
                            ? "bg-accent-soft text-accent"
                            : index === active
                              ? "bg-surface-hover text-fg"
                              : "text-fg"
                        }`}
                        style={{ height: ROW_HEIGHT }}
                      >
                        <span className="truncate">{option.label}</span>
                        {isSelected && <Check className="size-4 shrink-0" aria-hidden />}
                      </div>
                    );
                  }}
                />
              </motion.div>
            )}
          </AnimatePresence>,
          document.body,
        )}
    </>
  );
}
