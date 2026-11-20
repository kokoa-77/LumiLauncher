import type { AppError } from "./bindings";
import i18n, { getUiLanguage } from "./i18n";

export function getBackendErrorMessage(error: Pick<AppError, "code" | "message">): string {
  if (error.code === "ipc.rejected") return i18n.t("error.ipc");
  if (getUiLanguage() === "zh-CN") return error.message;
  return i18n.getFixedT("en")([`error.backend.${error.code}`, "error.unexpected"]);
}
