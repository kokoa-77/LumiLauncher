/**
 * 区块标题：14 号中粗，后面跟一个灰色的数字（「单人世界 4」「截图 9」「最近启动 4」）。
 * 面板里的分组小字（设置的「动画」、筛选的「排序」）不用它，那是 12 号灰色。
 */
export function SectionTitle({
  title,
  count,
  className = "",
}: {
  title: string;
  count?: number | null;
  className?: string;
}) {
  return (
    <h2 className={`flex items-baseline gap-2 font-medium text-sm ${className}`}>
      {title}
      {count != null && <span className="font-normal text-fg-muted">{count}</span>}
    </h2>
  );
}
