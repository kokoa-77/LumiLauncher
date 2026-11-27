/** 单实例笔记最大字节数（与后端 `instance::MAX_NOTES_BYTES` 一致） */
export const MAX_NOTES_BYTES = 256 * 1024;

/**
 * 在非行内代码片段中执行安全清洗：
 * 1. 转义裸 HTML 标签起始符 `<` → `&lt;`（防止 `<script>`、`<img>`、`<iframe>` 进入 Markdown 渲染管线）；
 * 2. 中和 `javascript:` / `vbscript:` / `data:` 链接目标；
 * 行内代码 `` `...` `` 保持原样交给 marked 自行转义，避免二次转义或误改写代码示例。
 */
function sanitizeOutsideInlineCode(line: string): string {
  let out = "";
  let i = 0;
  let plain = "";
  const flushPlain = () => {
    if (!plain) return;
    out += plain
      .replace(/</g, "&lt;")
      .replace(/\]\(\s*(?:javascript|vbscript|data):[^)]*\)/gi, "](#)");
    plain = "";
  };

  while (i < line.length) {
    if (line[i] === "`") {
      let ticks = 0;
      while (i + ticks < line.length && line[i + ticks] === "`") ticks++;
      const delimiter = "`".repeat(ticks);
      const close = line.indexOf(delimiter, i + ticks);
      if (close !== -1) {
        flushPlain();
        out += line.slice(i, close + ticks);
        i = close + ticks;
        continue;
      }
      plain += delimiter;
      i += ticks;
      continue;
    }
    plain += line[i] ?? "";
    i++;
  }
  flushPlain();
  return out;
}

/**
 * 预处理实例笔记 Markdown（与 `RichText` 的 DOMPurify + CSP 构成多层纵深防 XSS）：
 * 1. 遵循 CommonMark 围栏长度规则（4 个反引号打开的块不会被 3 个反引号提前闭合）；
 * 2. 非代码块中的 GFM 任务清单（`- [ ]` / `- [x]`）转为纯文本复选符号（☐ / ☑），无需放开 `<input>`；
 * 3. 非代码块 / 行内代码中的裸 `<` 与危险链接协议统一安全转义/中和。
 */
export function prepareNotesMarkdown(markdown: string): string {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  let fence: { ch: string; len: number } | null = null;
  const out = lines.map((line) => {
    if (!fence) {
      const openMatch = line.match(/^\s*(`{3,}|~{3,})/);
      if (openMatch?.[1]) {
        const ch = openMatch[1][0] ?? "`";
        const len = openMatch[1].length;
        // CommonMark：反引号围栏的 info string 不能包含反引号
        const rest = line.slice(openMatch[0].length);
        if (ch !== "`" || !rest.includes("`")) {
          fence = { ch, len };
          return line;
        }
      }
    } else {
      const closeMatch = line.match(/^\s*(`{3,}|~{3,})\s*$/);
      if (closeMatch?.[1] && closeMatch[1][0] === fence.ch && closeMatch[1].length >= fence.len) {
        fence = null;
      }
      return line;
    }

    const withTasks = line
      .replace(/^(\s*[-*+]\s+)\[ \]\s+/, "$1☐ ")
      .replace(/^(\s*[-*+]\s+)\[[xX]\]\s+/, "$1☑ ");
    return sanitizeOutsideInlineCode(withTasks);
  });
  return out.join("\n");
}
