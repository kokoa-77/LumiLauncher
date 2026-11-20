import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { VirtualList } from "./VirtualList";

/**
 * 网格的虚拟滚动（截图、世界可能有几百个）：按容器宽度算一行放几个，每一行当成虚拟列表的一项。
 * `minWidth` 是每格最窄多宽，`gap` 是格子之间的空隙（像素，横竖一样）。
 */
export function VirtualGrid<T>({
  items,
  minWidth,
  gap = 12,
  estimate,
  getKey,
  render,
  className = "",
}: {
  items: T[];
  minWidth: number;
  gap?: number;
  /** 一格多宽时一行大概多高（不含空隙） */
  estimate: (cellWidth: number) => number;
  getKey: (item: T) => string;
  render: (item: T, index: number) => ReactNode;
  className?: string;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setWidth(Math.floor(entry.contentRect.width));
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  const columns = Math.max(1, Math.floor((width + gap) / (minWidth + gap)));
  const rows = useMemo(() => {
    const out: { start: number; items: T[] }[] = [];
    for (let i = 0; i < items.length; i += columns) {
      out.push({ start: i, items: items.slice(i, i + columns) });
    }
    return out;
  }, [items, columns]);
  const cell = width > 0 ? (width - gap * (columns - 1)) / columns : minWidth;

  return (
    <div ref={boxRef} className={`min-h-0 ${className}`}>
      {width > 0 && (
        <VirtualList
          className="h-full"
          items={rows}
          estimate={estimate(cell) + gap}
          getKey={(row) => `${columns}:${row.items[0] ? getKey(row.items[0]) : row.start}`}
          render={(row) => (
            <div
              className="grid"
              style={{
                gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
                columnGap: gap,
                paddingBottom: gap,
              }}
            >
              {row.items.map((item, i) => (
                <div key={getKey(item)} className="min-w-0">
                  {render(item, row.start + i)}
                </div>
              ))}
            </div>
          )}
        />
      )}
    </div>
  );
}
