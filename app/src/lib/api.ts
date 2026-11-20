// 命令调用的统一出口：后端返回的 `{ status, data | error }` 在这里拆开，失败时抛出 CommandError。
import { getBackendErrorMessage } from "./backendErrors";
import type { AppError } from "./bindings";
import i18n from "./i18n";

export type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: AppError };

/**
 * 后端框架自己拒绝的调用（命令不存在、参数对不上）给的是一串文字，不是 AppError，
 * 不转一下的话日志里只剩「[undefined]」，提示条也是空的。
 */
function asAppError(error: unknown): AppError {
  if (typeof error === "object" && error !== null && "code" in error && "message" in error) {
    return error as AppError;
  }
  return { code: "ipc.rejected", message: i18n.t("error.ipc"), detail: String(error) };
}

export class CommandError extends Error {
  readonly code: string;
  readonly rawMessage: string;
  readonly detail: string | null;

  constructor(error: AppError) {
    super(getBackendErrorMessage(error));
    this.name = "CommandError";
    this.code = error.code;
    this.rawMessage = error.message;
    this.detail = error.detail ?? null;
  }
}

export function unwrap<T>(result: CommandResult<T>): T {
  if (result.status === "ok") return result.data;
  throw new CommandError(asAppError(result.error));
}
