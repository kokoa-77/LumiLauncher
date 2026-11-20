import { AnimatePresence, motion } from "motion/react";
import { type CSSProperties, type ReactNode, type Ref, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { getUiLanguage } from "@/lib/i18n";
import { duration, ease } from "@/lib/motion";
import { Tag } from "./Tag";

/** 和 Tooltip 一样，鼠标停一会儿才出来，划过去不闪 */
const DELAY = 250;
/** 浮层最宽多少像素 */
const MAX_WIDTH = 320;
/** 下面剩的空间不到这么高就往上放 */
const ROOM = 180;
/** 离窗口边缘至少留这么多 */
const EDGE = 8;

/** 文字真的被截断了（一行放不下，或者超过了限定的行数） */
export const isTruncated = (el: HTMLElement) =>
  el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;

/**
 * 悬停浮层的开关：鼠标停一会儿（或者键盘移到上面）记下位置，移开、按下、滚动、窗口变化时收起。
 * `when` 在打开前再看一眼要不要显示（比如文字没被截断就不用）。
 */
export function useHover<T extends HTMLElement>(when?: (el: T) => boolean) {
  const ref = useRef<T>(null);
  const timer = useRef<number | undefined>(undefined);
  const [anchor, setAnchor] = useState<DOMRect | null>();

  const show = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      const el = ref.current;
      if (el && (!when || when(el))) setAnchor(el.getBoundingClientRect());
    }, DELAY);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    setAnchor((current) => (current === undefined ? undefined : null));
  };

  useEffect(() => () => window.clearTimeout(timer.current), []);
  useEffect(() => {
    if (!anchor) return;
    const close = () => setAnchor(null);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [anchor]);

  return {
    ref,
    anchor,
    handlers: {
      onMouseEnter: show,
      onMouseLeave: hide,
      onMouseDown: hide,
      onFocus: show,
      onBlur: hide,
    },
  };
}

/**
 * 浮出来的那一块：挂在页面最外层（卡片、虚拟列表的 overflow 裁不到它），
 * 贴着触发它的元素放在下面，下面放不下放上面；元素在窗口右半边时靠右对齐，不出界。
 * 不接鼠标，只是看。
 */
function Floating({
  anchor,
  children,
}: {
  anchor: DOMRect | null | undefined;
  children: ReactNode;
}) {
  // 首次触发之前，列表里每段文字都无需挂载浮层和退场动画。
  if (anchor === undefined) return null;
  let style: CSSProperties = {};
  let upward = false;
  if (anchor) {
    upward = window.innerHeight - anchor.bottom < ROOM && anchor.top > ROOM;
    const right = anchor.left + anchor.width / 2 > window.innerWidth / 2;
    style = {
      maxWidth: Math.min(
        MAX_WIDTH,
        (right ? anchor.right : window.innerWidth - anchor.left) - EDGE,
      ),
      ...(right
        ? { right: Math.max(EDGE, window.innerWidth - anchor.right) }
        : { left: Math.max(EDGE, anchor.left) }),
      ...(upward ? { bottom: window.innerHeight - anchor.top + 6 } : { top: anchor.bottom + 6 }),
    };
  }
  return createPortal(
    <AnimatePresence>
      {anchor && (
        <motion.div
          role="tooltip"
          className="pointer-events-none fixed z-50 w-max rounded-panel border border-line bg-overlay p-2 text-fg shadow-overlay"
          style={style}
          initial={{ opacity: 0, y: upward ? 4 : -4 }}
          animate={{ opacity: 1, y: 0, transition: { duration: duration.overlay, ease: ease.out } }}
          exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

/** 给纯图标按钮复用的悬停标签：挂在 document.body，永不被卡片或弹窗 overflow 裁剪 */
export function FloatingTooltip({
  anchor,
  label,
}: {
  anchor: DOMRect | null | undefined;
  label: string;
}) {
  return (
    <Floating anchor={anchor}>
      <span className="block whitespace-nowrap px-0.5 text-sm leading-5">{label}</span>
    </Floating>
  );
}

/**
 * 「+N」：放不下的标签收成一个小胶囊，鼠标停上去把藏起来的那几个都列出来。
 * 读屏软件直接读全部。
 */
export function MoreTag({
  items,
  tone = "neutral",
}: {
  items: string[];
  tone?: "neutral" | "hero";
}) {
  const { ref, anchor, handlers } = useHover<HTMLSpanElement>();
  if (items.length === 0) return null;
  return (
    <>
      <span
        ref={ref}
        className="inline-flex shrink-0"
        aria-label={items.join(getUiLanguage().toLowerCase().startsWith("zh") ? "、" : ", ")}
        {...handlers}
      >
        <Tag tone={tone} className="text-fg-muted">
          +{items.length}
        </Tag>
      </span>
      <Floating anchor={anchor}>
        <div className="flex flex-wrap gap-1.5">
          {items.map((item) => (
            <Tag key={item}>{item}</Tag>
          ))}
        </div>
      </Floating>
    </>
  );
}

type TextTag = "span" | "p" | "h1" | "h3" | "dd";

/**
 * 可能被截断的一段文字（`truncate` 或 `line-clamp-*`）：真被截断了，鼠标停上去显示全文；没截断就什么都不出。
 * 样式照旧写在 className 上。
 */
export function Truncated({
  as = "span",
  text,
  className = "",
  children,
}: {
  as?: TextTag;
  /** 浮出来的全文；没给 `children` 时也是显示的内容 */
  text: string;
  className?: string;
  /** 显示的内容和全文不一样时（比如名字后面跟灰色小字）用它 */
  children?: ReactNode;
}) {
  const { ref, anchor, handlers } = useHover<HTMLElement>(isTruncated);
  const Element = as;
  return (
    <>
      <Element ref={ref as Ref<never>} className={className} {...handlers}>
        {children ?? text}
      </Element>
      <Floating anchor={anchor}>
        <p className="whitespace-pre-line break-words px-1 text-sm leading-5">{text}</p>
      </Floating>
    </>
  );
}

/**
 * 鼠标停上去浮出一段说明（不是因为截断，是本来就藏着的细节，比如两个平台各自的下载量、一串文件名）。
 * 替代系统自带的灰色小框，全启动器一个样子。
 */
export function HoverHint({
  hint,
  className = "",
  children,
}: {
  hint: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const { ref, anchor, handlers } = useHover<HTMLSpanElement>();
  if (hint === null || hint === undefined || hint === "") {
    return <span className={className}>{children}</span>;
  }
  return (
    <>
      <span ref={ref} className={className} {...handlers}>
        {children}
      </span>
      <Floating anchor={anchor}>
        <div className="whitespace-pre-line break-words px-1 text-sm leading-5">{hint}</div>
      </Floating>
    </>
  );
}
