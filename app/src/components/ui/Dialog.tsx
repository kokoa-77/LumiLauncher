import { X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { duration, ease, spring } from "@/lib/motion";
import { IconButton } from "./IconButton";

interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  /** 宽版（安装确认这类内容多的弹窗） */
  wide?: boolean;
  /** 更宽（下载历史这种一行信息很多的列表） */
  wider?: boolean;
  /** 自己排版（设置弹窗）：不画标题、不加内边距，大小由 className 定 */
  bare?: boolean;
  className?: string;
}

/** 开着的弹窗，后开的在后面（Esc 只关最后一个） */
const openDialogs: number[] = [];
let nextDialogId = 0;

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** 弹窗里能用 Tab 走到的控件（看不见的不算） */
function focusables(panel: HTMLElement): HTMLElement[] {
  return Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.getClientRects().length > 0,
  );
}

/**
 * 打开时焦点放哪：里面已经有自己抢了焦点的输入框就不动；
 * 有 `data-autofocus` 的放它；否则放第一个控件，右上角「×」排最后（免得一回车就关了）
 */
function focusFirst(panel: HTMLElement) {
  if (panel.contains(document.activeElement)) return;
  const items = focusables(panel);
  const target =
    items.find((el) => el.hasAttribute("data-autofocus")) ??
    items.find((el) => !el.hasAttribute("data-dialog-close")) ??
    items[0] ??
    panel;
  target.focus({ preventScroll: true });
}

/**
 * 弹窗：弹簧展开；背后压暗但不模糊（实时模糊在老电脑上最吃力）。Esc、点背后、右上角「×」都能关。
 * 圆角用「弹窗」档，是全界面最圆的面板。自己排版的（`bare`，设置弹窗）自己画「×」，位置和这里一样。
 */
export function Dialog({
  open,
  onClose,
  title,
  children,
  footer,
  wide = false,
  wider = false,
  bare = false,
  className = "",
}: DialogProps) {
  const { t } = useTranslation();
  // 用最新的 onClose，但不因为它变了就重新登记（重新登记会把自己挪到最上面，弹窗叠弹窗时顺序就乱了）
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const panelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const id = ++nextDialogId;
    openDialogs.push(id);
    // 键盘用户：打开时焦点进弹窗，关掉后回到打开它的那个按钮（外部审阅指出）
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (panelRef.current) focusFirst(panelRef.current);
    const onKey = (event: KeyboardEvent) => {
      // 弹窗叠弹窗（设置里点开「参数预设」）：Esc、Tab 只管最上面那层
      if (openDialogs[openDialogs.length - 1] !== id) return;
      if (event.key === "Escape") {
        event.stopPropagation();
        closeRef.current();
        return;
      }
      const panel = panelRef.current;
      if (event.key !== "Tab" || !panel) return;
      // Tab 在弹窗里转圈，不跑到背后被压暗的页面上
      const items = focusables(panel);
      const first = items[0];
      const last = items[items.length - 1];
      if (!first || !last) {
        event.preventDefault();
        panel.focus({ preventScroll: true });
        return;
      }
      const inside = panel.contains(document.activeElement);
      if (event.shiftKey && (!inside || document.activeElement === first)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (!inside || document.activeElement === last)) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      const index = openDialogs.indexOf(id);
      if (index >= 0) openDialogs.splice(index, 1);
      if (opener?.isConnected) opener.focus({ preventScroll: true });
    };
  }, [open]);

  const hasCustomWidth = /\bmax-w-/.test(className);
  const size = bare || hasCustomWidth ? "" : wider ? "max-w-4xl" : wide ? "max-w-xl" : "max-w-md";

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-40 flex items-center justify-center p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1, transition: { duration: duration.overlay, ease: ease.out } }}
          exit={{ opacity: 0, transition: { duration: duration.overlay, ease: ease.in } }}
        >
          <button
            type="button"
            aria-label={title}
            tabIndex={-1}
            className="absolute inset-0 bg-scrim"
            onClick={onClose}
          />
          <motion.div
            ref={panelRef}
            role="dialog"
            aria-modal="true"
            aria-label={title}
            tabIndex={-1}
            className={`relative flex max-h-full w-full flex-col overflow-hidden rounded-dialog border border-line bg-overlay shadow-dialog outline-none ${size} ${className}`}
            initial={{ opacity: 0, scale: 0.96, y: 8 }}
            animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
            exit={{
              opacity: 0,
              scale: 0.98,
              transition: { duration: duration.overlay, ease: ease.in },
            }}
          >
            {bare ? (
              children
            ) : (
              <>
                <IconButton
                  label={t("dialog.close")}
                  onClick={onClose}
                  className="absolute top-3 right-3 z-10"
                  data-dialog-close
                >
                  <X className="size-4" aria-hidden />
                </IconButton>
                <h2 className="shrink-0 truncate pt-5 pr-14 pl-6 font-semibold text-lg leading-snug">
                  {title}
                </h2>
                <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">{children}</div>
                {footer && (
                  <div className="flex shrink-0 justify-end gap-2 px-6 pt-1 pb-5">{footer}</div>
                )}
              </>
            )}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
