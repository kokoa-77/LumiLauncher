import { useVirtualizer } from "@tanstack/react-virtual";
import { type CSSProperties, type ReactNode, useEffect, useMemo, useRef } from "react";

/** 滚动时虚拟列表会重新定位行，仅为变动的项目重新创建内容。 */
function RowContent<T>({
  item,
  index,
  render,
}: {
  item: T;
  index: number;
  render: (item: T, index: number) => ReactNode;
}) {
  return useMemo(() => render(item, index), [item, index, render]);
}

interface VirtualListProps<T> {
  items: T[];
  /** 每项的预估高度（像素），实际高度会自动测量 */
  estimate: number;
  /** 额外渲染的屏外行数，含重卡片的页面可以缩小 */
  overscan?: number;
  getKey: (item: T, index: number) => string;
  render: (item: T, index: number) => ReactNode;
  className?: string;
  /** 新内容到来时，如果原本在底部就跟着滚到底（日志窗口用） */
  followTail?: boolean;
  /** 让这一项保持在可见范围内（下拉框：打开时定位到选中项，键盘移动时跟着滚） */
  activeIndex?: number;
  style?: CSSProperties;
  /** 快滚到底时调用（无限滚动：再加载一页）。最后几项进入可见范围就触发 */
  onEndReached?: () => void;
  /** 一打开就滚到这个位置（从详情页回来时停在原处） */
  initialOffset?: number;
  /** 滚动时报告位置 */
  onScrollOffset?: (offset: number) => void;
  /** 列表下方的内容（加载中、到底了……） */
  footer?: ReactNode;
  /** 每一项外面的类名（比如左右留白） */
  itemClassName?: string;
  /** 拿到滚动的那个容器（「回到顶部」用） */
  scrollRef?: (el: HTMLDivElement | null) => void;
}

/** 长列表虚拟滚动：只渲染看得见的部分（docs/DESIGN.md 第 7 节：超过 30 项的列表必须用）。 */
export function VirtualList<T>({
  items,
  estimate,
  overscan = 12,
  getKey,
  render,
  className = "",
  followTail = false,
  activeIndex,
  style,
  onEndReached,
  initialOffset,
  onScrollOffset,
  footer,
  itemClassName = "",
  scrollRef,
}: VirtualListProps<T>) {
  const parentRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => estimate,
    getItemKey: (index) => getKey(items[index] as T, index),
    overscan,
    initialOffset,
  });
  const virtualItems = virtualizer.getVirtualItems();
  const lastIndex = virtualItems[virtualItems.length - 1]?.index ?? -1;
  const endRef = useRef(onEndReached);
  endRef.current = onEndReached;

  useEffect(() => {
    if (lastIndex >= 0 && lastIndex >= items.length - 3) endRef.current?.();
  }, [lastIndex, items.length]);

  useEffect(() => {
    if (followTail && atBottom.current && items.length > 0) {
      virtualizer.scrollToIndex(items.length - 1, { align: "end" });
    }
  }, [followTail, items.length, virtualizer]);

  useEffect(() => {
    if (activeIndex !== undefined && activeIndex >= 0) {
      virtualizer.scrollToIndex(activeIndex, { align: "auto" });
    }
  }, [activeIndex, virtualizer]);

  return (
    <div
      ref={(el) => {
        parentRef.current = el;
        scrollRef?.(el);
      }}
      style={style}
      className={`overflow-y-auto ${className}`}
      onScroll={(event) => {
        const el = event.currentTarget;
        if (followTail) atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        onScrollOffset?.(el.scrollTop);
      }}
    >
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualItems.map((row) => (
          <div
            key={row.key}
            data-index={row.index}
            ref={virtualizer.measureElement}
            className={`absolute top-0 left-0 w-full ${itemClassName}`}
            style={{ transform: `translateY(${row.start}px)` }}
          >
            <RowContent item={items[row.index] as T} index={row.index} render={render} />
          </div>
        ))}
      </div>
      {footer}
    </div>
  );
}
