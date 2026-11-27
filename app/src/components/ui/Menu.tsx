import { MoreVertical, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ComponentType, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { duration, ease, spring } from "@/lib/motion";
import { IconButton } from "./IconButton";

export interface MenuItem {
  key: string;
  label: string;
  icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  onSelect: () => void;
  /** 危险操作（删除）：红字，和上面的项之间隔一条线 */
  danger?: boolean;
  /** 与上一项之间加一条分割线（例如「从 Lumi 移除」这类非破坏性但需视觉区分的底部操作） */
  separated?: boolean;
}

/** 下面剩的空间不到这么高就往上弹 */
const ROOM = 260;
/** 按钮左边剩的空间不到这么宽（菜单最宽大概这么宽）就往右展开，不然会跑出窗口 */
const ROOM_X = 248;
/** 菜单项一个接一个出来，每个晚这么多秒（四五项加起来不到 400 毫秒） */
const STAGGER = 0.03;

/**
 * 「⋮」更多菜单：点开弹出（默认按剩余空间决定向下还是向上，`up` 时总是向上），点外面、按 Esc、滚动时收起。
 * 弹层挂在页面最外层、按按钮的位置定位：放在卡片、虚拟列表里也不会被别的行盖住或者被裁掉。
 * 打开时按钮上的「⋯」转成「×」（点它就收起），菜单从按钮那个角弹出来，菜单项一个接一个滑进来；
 * 流畅模式和「减少动效」下 MotionConfig 会关掉转动和位移，只剩淡入淡出。
 */
export function Menu({
  label,
  items,
  up = false,
  trigger = "ghost",
  size = "md",
  icon: TriggerIcon = MoreVertical,
}: {
  label: string;
  items: MenuItem[];
  up?: boolean;
  trigger?: "ghost" | "hero";
  /** `cta` 和中号主按钮一样高（48 像素，实例页横幅里放在「开始游戏」旁边） */
  size?: "md" | "cta" | "lg";
  icon?: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
}) {
  const [anchor, setAnchor] = useState<DOMRect | null>();
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const open = anchor != null;

  useEffect(() => {
    if (!open) return;
    const close = () => setAnchor(null);
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) close();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    const onScroll = (event: Event) => {
      if (!menuRef.current?.contains(event.target as Node)) close();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open]);

  const upward =
    anchor != null && (up || (window.innerHeight - anchor.bottom < ROOM && anchor.top > ROOM));
  // 平时和按钮右边对齐、往左展开；按钮靠窗口左边（分组标题旁边）时和按钮左边对齐、往右展开
  const rightward = anchor != null && anchor.right < ROOM_X;
  const iconSize = size === "md" ? "size-4" : size === "cta" ? "size-[18px]" : "size-5";
  // 菜单项从按钮那边滑进来：往上弹的从下面来，往下弹的从上面来
  const from = upward ? 6 : -6;

  return (
    <div ref={rootRef} className="relative shrink-0">
      <IconButton
        label={label}
        variant={trigger}
        size={size}
        aria-haspopup="menu"
        aria-expanded={open}
        pressed={open}
        onClick={(event) => {
          event.stopPropagation();
          setAnchor(open ? null : event.currentTarget.getBoundingClientRect());
        }}
      >
        {/* 两个图标叠在一起：打开时「⋯」转着淡出、「×」转着进来 */}
        {anchor === undefined ? (
          <TriggerIcon className={iconSize} aria-hidden />
        ) : (
          <span className="grid *:col-start-1 *:row-start-1" aria-hidden>
            <motion.span
              className="grid place-items-center"
              initial={{ opacity: 1, rotate: 0, scale: 1 }}
              animate={
                open ? { opacity: 0, rotate: 90, scale: 0.6 } : { opacity: 1, rotate: 0, scale: 1 }
              }
              transition={spring.snappy}
            >
              <TriggerIcon className={iconSize} aria-hidden />
            </motion.span>
            <motion.span
              className="grid place-items-center"
              initial={{ opacity: 0, rotate: -90, scale: 0.6 }}
              animate={
                open ? { opacity: 1, rotate: 0, scale: 1 } : { opacity: 0, rotate: -90, scale: 0.6 }
              }
              transition={spring.snappy}
            >
              <X className={iconSize} aria-hidden />
            </motion.span>
          </span>
        )}
      </IconButton>
      {anchor !== undefined &&
        createPortal(
          <AnimatePresence>
            {anchor && (
              <motion.div
                ref={menuRef}
                role="menu"
                className={`fixed z-50 min-w-48 overflow-hidden rounded-panel border border-line bg-overlay p-1.5 text-fg shadow-overlay ${
                  upward
                    ? rightward
                      ? "origin-bottom-left"
                      : "origin-bottom-right"
                    : rightward
                      ? "origin-top-left"
                      : "origin-top-right"
                }`}
                style={{
                  ...(rightward
                    ? { left: anchor.left }
                    : { right: window.innerWidth - anchor.right }),
                  ...(upward
                    ? { bottom: window.innerHeight - anchor.top + 8 }
                    : { top: anchor.bottom + 4 }),
                }}
                initial={{ opacity: 0, scale: 0.96, y: upward ? 4 : -4 }}
                animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
                exit={{
                  opacity: 0,
                  scale: 0.98,
                  transition: { duration: duration.press, ease: ease.in },
                }}
                onClick={(event) => event.stopPropagation()}
              >
                {items.map(
                  ({ key, label: itemLabel, icon: Icon, onSelect, danger, separated }, index) => (
                    <motion.button
                      key={key}
                      type="button"
                      role="menuitem"
                      // 往上弹时离按钮最近的是最后一项，从它开始
                      initial={{ opacity: 0, y: from }}
                      animate={{
                        opacity: 1,
                        y: 0,
                        transition: {
                          duration: duration.toggle,
                          ease: ease.out,
                          delay: STAGGER * (upward ? items.length - 1 - index : index),
                        },
                      }}
                      onClick={() => {
                        setAnchor(null);
                        onSelect();
                      }}
                      className={`flex w-full items-center gap-2.5 whitespace-nowrap rounded-item px-2.5 py-2 text-left text-sm transition-colors duration-(--motion-press) hover:bg-surface-hover ${
                        (separated || (danger && !items[index - 1]?.danger)) && index > 0
                          ? "relative mt-2 before:absolute before:inset-x-1 before:-top-1 before:h-px before:bg-line"
                          : ""
                      } ${danger ? "text-danger" : ""}`}
                    >
                      <Icon
                        className={`size-4 shrink-0 ${danger ? "text-danger" : "text-fg-muted"}`}
                        aria-hidden
                      />
                      {itemLabel}
                    </motion.button>
                  ),
                )}
              </motion.div>
            )}
          </AnimatePresence>,
          document.body,
        )}
    </div>
  );
}
