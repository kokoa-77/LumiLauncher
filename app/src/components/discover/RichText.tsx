// 项目介绍的渲染（只在详情页按需加载，不算进首屏 JS）。
// 介绍是作者写的、从网上来的，不可信：Markdown 先转成 HTML，再用 DOMPurify 清洗，
// 只留排版标签；脚本、内嵌框架、表单、行内样式、事件属性全部去掉，链接和图片地址只认 https。
// 网页本身的安全策略也不许跑内联脚本，这是第二道保险。
import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import { marked } from "marked";
import { type MouseEvent, useMemo } from "react";
import type { BodyFormat } from "@/lib/bindings";
import { reportError } from "@/lib/report";

DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "IMG") {
    node.setAttribute("loading", "lazy");
    node.setAttribute("decoding", "async");
    node.removeAttribute("width");
    node.removeAttribute("height");
  }
  if (node.tagName === "A") node.setAttribute("rel", "noopener noreferrer");
});

const SANITIZE = {
  FORBID_TAGS: [
    "style",
    "iframe",
    "form",
    "input",
    "button",
    "textarea",
    "select",
    "object",
    "embed",
    "video",
    "audio",
  ],
  FORBID_ATTR: ["style", "class", "id"],
  ALLOWED_URI_REGEXP: /^(?:https:|#)/i,
};

export function toSafeHtml(body: string, format: BodyFormat): string {
  const raw =
    format === "markdown" ? (marked.parse(body, { async: false, gfm: true }) as string) : body;
  return DOMPurify.sanitize(raw, SANITIZE);
}

export default function RichText({ body, format }: { body: string; format: BodyFormat }) {
  const html = useMemo(() => toSafeHtml(body, format), [body, format]);
  const onClick = (event: MouseEvent<HTMLDivElement>) => {
    const link = (event.target as HTMLElement).closest("a");
    if (!link) return;
    // 不让启动器窗口跳到别的网页：一律交给系统浏览器
    event.preventDefault();
    const href = link.getAttribute("href") ?? "";
    if (href.startsWith("https://")) {
      openUrl(href).catch((error: unknown) => reportError(error, "打开介绍里的链接"));
    }
  };
  return (
    // 只是拦住里面链接的点击，键盘按回车时链接自己会触发 click
    <div
      className="rich-text"
      onClick={onClick}
      // 上面已经用 DOMPurify 清洗过
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
