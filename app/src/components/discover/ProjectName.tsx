import { useTranslation } from "react-i18next";
import { Truncated } from "@/components/ui/HoverCard";
import { displayName } from "@/lib/discover";

/**
 * 项目名：有中文名时「中文名」后面跟灰色小字的英文名，没有时只有英文名。
 * 放不下被截断时，鼠标停上去看「中文名（英文名）」全文。
 */
export function ProjectName({
  title,
  titleZh,
  as = "h3",
  className = "",
}: {
  title: string;
  titleZh?: string | null;
  as?: "span" | "p" | "h3";
  className?: string;
}) {
  const { i18n } = useTranslation();
  if (!titleZh || i18n.language !== "zh-CN")
    return <Truncated as={as} className={className} text={title} />;
  return (
    <Truncated as={as} className={className} text={displayName({ title, titleZh })}>
      {titleZh}
      <span className="ml-1.5 font-normal text-fg-muted text-xs">{title}</span>
    </Truncated>
  );
}
