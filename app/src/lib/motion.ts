// 动效令牌的 JS 版本，和 styles/tokens.css 保持一致。组件里只许用这里的值。
import type { Transition } from "motion/react";

/** 时长（秒） */
export const duration = {
  press: 0.1,
  toggle: 0.18,
  overlay: 0.22,
  enter: 0.26,
  page: 0.34,
} as const;

/** 缓动曲线 */
export const ease = {
  out: [0.22, 1, 0.36, 1],
  in: [0.4, 0, 1, 1],
  inOut: [0.65, 0, 0.35, 1],
} as const;

/** 两档弹簧 */
export const spring = {
  gentle: { type: "spring", stiffness: 260, damping: 30 },
  snappy: { type: "spring", stiffness: 520, damping: 38 },
} as const satisfies Record<string, Transition>;

/** 路由层级深度：首页 0，平级列表 1，详情页 2。用于判断下钻与返回方向（D25c）。 */
export type RouteDirection = "peer" | "forward" | "back";

export function routeDepth(pathname: string): number {
  return pathname.split("/").filter(Boolean).length;
}

export function routeDirection(from: string, to: string): RouteDirection {
  const fromDepth = routeDepth(from);
  const toDepth = routeDepth(to);
  // 首页与一级页之间保持柔和平级切页；一级页 ↔ 详情页体现下钻与返回的空间方向
  if (fromDepth === 0 || toDepth === 0 || fromDepth === toDepth) return "peer";
  return toDepth > fromDepth ? "forward" : "back";
}

/**
 * 页面切换：旧页面很快淡出（120–180 毫秒），新页面淡入 + 轻微位移；外壳的底板和首页截图同时用 340 毫秒交叉。
 * 平级切页采用 10 像素轻上移；下钻 / 返回采用 14 像素水平方向位移；流畅与减少动效下只保留淡入淡出。
 * 返回给 AnimatePresence 的 variants（配合 `custom={direction}`），确保退场页始终按「当前这一次切换的方向」退场。
 * 使用完整 transform，让 WebView 的合成器驱动位移，主线程可以继续排版和加载数据。
 */
export const pageVariants = (reduceMotion: boolean | null) => ({
  initial: (direction: RouteDirection = "peer") => ({
    opacity: 0,
    transform: reduceMotion
      ? "none"
      : direction === "forward"
        ? "translateX(14px)"
        : direction === "back"
          ? "translateX(-14px)"
          : "translateY(10px)",
  }),
  animate: {
    opacity: 1,
    transform: "none",
    transition: { duration: duration.page, ease: ease.out },
  },
  exit: (direction: RouteDirection = "peer") => ({
    opacity: 0,
    pointerEvents: "none" as const,
    transform: reduceMotion
      ? "none"
      : direction === "forward"
        ? "translateX(-8px)"
        : direction === "back"
          ? "translateX(8px)"
          : "none",
    transition: {
      duration: direction === "peer" ? duration.press : duration.toggle,
      ease: ease.in,
    },
  }),
});

/** 保留旧函数签名，供单测与静态调用便捷使用。 */
export const pageTransition = (
  reduceMotion: boolean | null,
  direction: RouteDirection = "peer",
) => {
  const variants = pageVariants(reduceMotion);
  return {
    initial: variants.initial(direction),
    animate: variants.animate,
    exit: variants.exit(direction),
  };
};
