// 错误兜底：任何没被处理的错误都弹提示 + 写进启动器日志，不白屏、不静默吞掉。
import { toast } from "sonner";
import { CommandError } from "./api";
import { commands } from "./bindings";
import i18n from "./i18n";

function describe(error: unknown): string {
  if (error instanceof CommandError) {
    return `[${error.code}] ${error.rawMessage}${error.detail ? `（${error.detail}）` : ""}`;
  }
  if (error instanceof Error) return `${error.name}: ${error.message}\n${error.stack ?? ""}`;
  return String(error);
}

/** 只写日志、不弹提示（界面自己把错误显示在原地时用）。 */
export function logError(error: unknown, context: string): void {
  console.error(context, error);
  commands.appLog("error", `${context}：${describe(error)}`).catch((logFailure: unknown) => {
    console.error("写入启动器日志失败", logFailure);
  });
}

/** 给用户看的错误文字：后端给的错误本身就是中文人话，直接用；其他意外错误给统一提示。 */
export function userMessage(error: unknown): string {
  return error instanceof CommandError ? error.message : i18n.t("error.unexpected");
}

/** 上报一个错误：写日志并弹提示。`context` 说明是在做什么时出错的，只写进日志。 */
export function reportError(error: unknown, context: string): void {
  logError(error, context);
  toast.error(userMessage(error));
}

export function installGlobalErrorHandlers(): void {
  window.addEventListener("error", (event) => {
    reportError(event.error ?? event.message, "页面脚本错误");
  });
  window.addEventListener("unhandledrejection", (event) => {
    reportError(event.reason, "未处理的异步错误");
  });
}
