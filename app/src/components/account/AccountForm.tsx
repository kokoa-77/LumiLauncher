import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, LogIn } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/Button";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { CommandError } from "@/lib/api";
import { logError, reportError, userMessage } from "@/lib/report";
import { microsoftLoginReasons, useAccountStore } from "@/stores/accounts";

const MINECRAFT_STORE = "https://www.minecraft.net/zh-hans/store/minecraft-java-bedrock-edition-pc";

/** 添加账号和首次引导共用。Microsoft 在系统浏览器中处理登录。 */
export function AccountForm({
  reason = null,
  onDone,
}: {
  reason?: string | null;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const loaded = useAccountStore((s) => s.loaded);
  const hasAccount = useAccountStore((s) => s.accounts.length > 0);
  const available = useAccountStore((s) => s.microsoftAvailable);
  const pending = useAccountStore((s) => s.loginPending);
  const cancelling = useAccountStore((s) => s.loginCancelling);
  const login = useAccountStore((s) => s.loginMicrosoft);
  const cancel = useAccountStore((s) => s.cancelMicrosoft);
  const [error, setError] = useState<string | null>(null);
  const ownsAttempt = useRef(false);

  useEffect(
    () => () => {
      if (ownsAttempt.current) {
        ownsAttempt.current = false;
        cancel();
      }
    },
    [cancel],
  );

  const signIn = async () => {
    if (!available || pending) return;
    setError(null);
    ownsAttempt.current = true;
    try {
      if ((await login()) && ownsAttempt.current) onDone();
    } catch (failure) {
      if (failure instanceof CommandError && failure.code === "auth.cancelled") return;
      logError(failure, "Microsoft 登录");
      const key =
        failure instanceof CommandError ? microsoftLoginReasons.get(failure.code) : undefined;
      if (ownsAttempt.current) setError(key ? t(key) : userMessage(failure));
    } finally {
      ownsAttempt.current = false;
    }
  };

  const reasonKey = reason ? microsoftLoginReasons.get(reason) : undefined;
  const capabilityHint = !loaded
    ? t("common.loading")
    : !available || reason === "auth.app_unavailable"
      ? t("account.microsoft.unavailable")
      : null;
  return (
    <div className="flex flex-col items-start gap-3">
      <p className="text-fg-muted text-sm">
        {t(hasAccount ? "account.microsoft.browser" : "account.microsoft.signedOut")}
      </p>
      {reasonKey && reason !== "auth.no_account" && reason !== "auth.app_unavailable" && (
        <p className="text-fg-muted text-sm">{t(reasonKey)}</p>
      )}
      {capabilityHint && (
        <p role="status" className="text-fg-muted text-xs">
          {capabilityHint}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          disabled={pending}
          onClick={() =>
            openUrl(MINECRAFT_STORE).catch((failure: unknown) =>
              reportError(failure, "打开 Minecraft 官方购买页"),
            )
          }
        >
          <ExternalLink className="size-4" aria-hidden />
          {t("account.microsoft.purchase")}
        </Button>
        <Button
          variant="primary"
          disabled={!available || pending}
          data-autofocus
          onClick={() => void signIn()}
        >
          {pending ? <LightSpinner className="size-4" /> : <LogIn className="size-4" aria-hidden />}
          {t("account.microsoft.signIn")}
        </Button>
        {pending && (
          <Button disabled={cancelling} onClick={cancel}>
            {t("common.cancel")}
          </Button>
        )}
      </div>
      {pending && (
        <p role="status" className="text-fg-muted text-xs">
          {t(cancelling ? "account.microsoft.cancelling" : "account.microsoft.waiting")}
        </p>
      )}
      {error && (
        <p role="alert" className="text-danger text-xs">
          {error}
        </p>
      )}
    </div>
  );
}
