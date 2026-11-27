import { marked } from "marked";
import { describe, expect, it } from "vitest";
import { prepareNotesMarkdown } from "./notesMarkdown";

describe("实例级笔记 Markdown 预处理与安全转义", () => {
  it("将正文里的任务清单转为安全符号，并严格按围栏长度保留代码块原样", () => {
    const md = [
      "- [ ] 准备下界传送门",
      "- [x] 调好 6G 内存",
      "````markdown",
      "```",
      "- [ ] 嵌套 4 反引号代码块内不要转换",
      "```",
      "````",
      "- [ ] 围栏结束后恢复转换",
    ].join("\n");
    const prepared = prepareNotesMarkdown(md);
    expect(prepared).toContain("- ☐ 准备下界传送门");
    expect(prepared).toContain("- ☑ 调好 6G 内存");
    expect(prepared).toContain("- [ ] 嵌套 4 反引号代码块内不要转换");
    expect(prepared).toContain("- ☐ 围栏结束后恢复转换");
  });

  it("安全转义 script、img onerror、iframe 与危险链接，同时保留行内代码示例", () => {
    const input = [
      "# 正常标题",
      "> 引用块正常保留",
      "<script>window.__xss = 1</script>",
      '<img src="x" onerror="alert(1)" />',
      '<iframe src="https://evil.example"></iframe>',
      "[恶意链接](javascript:alert(1))",
      "[安全链接](https://modrinth.com)",
      "行内代码 `<script>` 与 `[示例](javascript:void(0))` 保持代码原样",
    ].join("\n\n");

    const html = marked.parse(prepareNotesMarkdown(input), { async: false, gfm: true }) as string;
    expect(html).toContain("<h1>正常标题</h1>");
    expect(html).toContain("<blockquote>");
    expect(html).toContain('href="https://modrinth.com"');
    expect(html).toContain("&lt;script&gt;window.__xss = 1&lt;/script&gt;");
    expect(html).toContain("<code>&lt;script&gt;</code>");
    expect(html).toContain("<code>[示例](javascript:void(0))</code>");
    expect(html.toLowerCase()).not.toContain("<script");
    expect(html.toLowerCase()).not.toContain("<img");
    expect(html.toLowerCase()).not.toContain("<iframe");
    expect(html.toLowerCase()).not.toContain('href="javascript:');
  });
});
