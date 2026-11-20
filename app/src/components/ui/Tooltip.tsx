import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useRef, useState } from "react";
import { duration, ease } from "@/lib/motion";

type Side = "right" | "bottom" | "top";

const placement: Record<Side, { className: string; from: { x?: number; y?: number } }> = {
  /** 侧栏：在右边弹出 */
  right: { className: "-translate-y-1/2 top-1/2 left-full ml-2", from: { x: -4 } },
  /** 标题栏：在下面弹出，靠右对齐（贴着窗口右边也不会出界） */
  bottom: { className: "top-full right-0 mt-1.5", from: { y: -4 } },
  /** 右下角的浮动按钮：在上面弹出，靠右对齐 */
  top: { className: "right-0 bottom-full mb-1.5", from: { y: 4 } },
};

/**
 * 鼠标停一会儿（或键盘聚焦）弹出名字。只给只有图标的按钮用（侧栏、标题栏）。
 * 全启动器只用这一种提示，不用系统自带的灰色小框。
 */
export function Tooltip({
  label,
  side = "right",
  children,
}: {
  label: string;
  side?: Side;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const show = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpen(true), 250);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    setOpen(false);
  };
  const { className, from } = placement[side];
  return (
    // 只是转发悬停、聚焦，真正能点的按钮在里面
    <span
      className="relative flex"
      onMouseEnter={show}
      onMouseLeave={hide}
      onMouseDown={hide}
      onFocus={show}
      onBlur={hide}
    >
      {children}
      <AnimatePresence>
        {open && (
          <motion.span
            role="tooltip"
            className={`pointer-events-none absolute z-50 whitespace-nowrap rounded-control border border-line bg-overlay px-2.5 py-1 text-fg text-sm shadow-overlay ${className}`}
            initial={{ opacity: 0, ...from }}
            animate={{
              opacity: 1,
              x: 0,
              y: 0,
              transition: { duration: duration.overlay, ease: ease.out },
            }}
            exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
          >
            {label}
          </motion.span>
        )}
      </AnimatePresence>
    </span>
  );
}
