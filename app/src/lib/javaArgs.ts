// Java 参数的小工具（参数预设弹窗用，D17i 验收时加）。
// 只按空白拆：预设里的参数都不带空格；用户自己写的带引号的参数原样保留，不会被拆坏。

/** 常用参数预设：一键加上 / 去掉 */
export interface ArgPreset {
  id: string;
  /** 加的参数（不带空格的一个个） */
  args: string[];
}

export const ARG_PRESETS: ArgPreset[] = [
  { id: "disableExplicitGc", args: ["-XX:+DisableExplicitGC"] },
  { id: "stringDedup", args: ["-XX:+UseStringDeduplication"] },
  { id: "preferIpv4", args: ["-Djava.net.preferIPv4Stack=true"] },
  { id: "utf8", args: ["-Dfile.encoding=UTF-8"] },
];

function tokens(text: string): string[] {
  return text.split(/\s+/).filter(Boolean);
}

/** 这几个参数是不是都已经写上了 */
export function hasArgs(text: string, args: string[]): boolean {
  const have = new Set(tokens(text));
  return args.every((a) => have.has(a));
}

/** 加上（已经有的不重复加），加在最后 */
export function addArgs(text: string, args: string[]): string {
  const have = new Set(tokens(text));
  const missing = args.filter((a) => !have.has(a));
  return [text.trim(), ...missing].filter(Boolean).join(" ");
}

/** 去掉这几个参数，别的原样留着 */
export function removeArgs(text: string, args: string[]): string {
  const drop = new Set(args);
  return text
    .split(/(\s+)/)
    .filter((part) => !drop.has(part))
    .join("")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/**
 * 自己写了选垃圾回收器的参数（`-XX:+UseG1GC`、`-XX:+UseZGC`……）：这时「内存回收方式」的预设不生效
 * （和后端 `launch/jvm.rs` 的规则一样：两个一起写 Java 会直接起不来，后端只用你写的）
 */
export function picksCollector(text: string): boolean {
  return tokens(text).some((t) => {
    const rest = t.startsWith("-XX:+Use") ? t.slice("-XX:+Use".length) : null;
    return rest !== null && rest.endsWith("GC") && rest.length > 2;
  });
}
