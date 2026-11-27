import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle,
  Check,
  Copy,
  ExternalLink,
  LogIn,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/Button";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { CommandError } from "@/lib/api";
import { type AuthStage, commands } from "@/lib/bindings";
import { logError, reportError, userMessage } from "@/lib/report";
import { microsoftLoginReasons, useAccountStore } from "@/stores/accounts";

function minecraftStoreUrl(language: string): string {
  const segment = language.toLowerCase().startsWith("zh") ? "zh-hans" : "en-us";
  return `https://www.minecraft.net/${segment}/store/minecraft-java-bedrock-edition-pc`;
}

const RECOVERY_URLS: Record<string, { labelKey: string; url: string } | undefined> = {
  "auth.xbox_profile": { labelKey: "account.recovery.openXbox", url: "https://www.xbox.com/" },
  "auth.xbox_child": {
    labelKey: "account.recovery.openFamily",
    url: "https://account.microsoft.com/family/",
  },
  "auth.account_restricted": {
    labelKey: "account.recovery.openFamily",
    url: "https://account.microsoft.com/family/",
  },
  "auth.xbox_banned": {
    labelKey: "account.recovery.openXboxEnforcement",
    url: "https://support.xbox.com/help/family-online-safety/enforcement/enforcement-history",
  },
  "auth.xbox_region": {
    labelKey: "account.recovery.openXboxRegions",
    url: "https://www.xbox.com/regions",
  },
  "auth.no_profile": {
    labelKey: "account.recovery.openMinecraftProfile",
    url: "https://www.minecraft.net/msaprofile/mygames/editprofile",
  },
};

const STAGE_ORDER: AuthStage[] = [
  "waitingBrowser",
  "exchangingToken",
  "authorizingXsts",
  "verifyingMinecraft",
];

function stageStepIndex(stage: AuthStage | null): number {
  switch (stage) {
    case "waitingBrowser":
      return 0;
    case "exchangingToken":
    case "authenticatingXbox":
      return 1;
    case "authorizingXsts":
      return 2;
    case "verifyingMinecraft":
    case "syncingProfile":
      return 3;
    default:
      return 0;
  }
}

function formatCountdown(remainingMs: number): string {
  const totalSec = Math.max(0, Math.ceil(remainingMs / 1000));
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${String(min).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
}

export function AuthProgressPanel({
  stage,
  cancelling,
  remainingMs,
  copiedUrl,
  onReopen,
  onCopyUrl,
}: {
  stage: AuthStage | null;
  cancelling: boolean;
  remainingMs: number;
  copiedUrl: boolean;
  onReopen: () => void;
  onCopyUrl: () => void;
}) {
  const { t } = useTranslation();
  const activeIdx = stageStepIndex(stage);
  return (
    <div className="w-full rounded-panel border border-line bg-surface-2 p-3">
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="font-medium">
          {t(
            cancelling
              ? "account.microsoft.cancelling"
              : `account.stage.${stage ?? "waitingBrowser"}`,
          )}
        </span>
        {stage === "waitingBrowser" && remainingMs > 0 && (
          <span className="font-mono text-fg-muted tabular-nums">
            {formatCountdown(remainingMs)}
          </span>
        )}
      </div>
      <ol className="mt-2.5 grid grid-cols-4 gap-1.5" aria-label={t("account.stageLabel")}>
        {STAGE_ORDER.map((step, idx) => {
          const done = idx < activeIdx;
          const current = idx === activeIdx;
          return (
            <li key={step} className="flex flex-col gap-1">
              <span
                className={`h-1.5 rounded-full transition-colors ${
                  done ? "bg-emerald-500" : current ? "bg-accent" : "bg-line"
                }`}
              />
              <span className={`truncate text-2xs ${current ? "text-fg" : "text-fg-muted"}`}>
                {t(`account.stageShort.${step}`)}
              </span>
            </li>
          );
        })}
      </ol>
      {stage === "waitingBrowser" && !cancelling && (
        <div className="mt-2.5 flex flex-wrap gap-2">
          <Button size="sm" variant="outline" onClick={onReopen}>
            <RefreshCw className="size-3.5" aria-hidden />
            {t("account.reopenBrowser")}
          </Button>
          <Button size="sm" variant="ghost" onClick={onCopyUrl}>
            {copiedUrl ? (
              <Check className="size-3.5 text-emerald-500" aria-hidden />
            ) : (
              <Copy className="size-3.5" aria-hidden />
            )}
            {t(copiedUrl ? "account.copiedUrl" : "account.copyUrl")}
          </Button>
        </div>
      )}
    </div>
  );
}

/** 添加账号和首次引导共用：系统浏览器 OAuth 2.1 PKCE + 四阶段进度 + 智能恢复卡片。 */
export function AccountForm({
  reason = null,
  onDone,
}: {
  reason?: string | null;
  onDone: () => void;
}) {
  const { t, i18n } = useTranslation();
  const loaded = useAccountStore((s) => s.loaded);
  const hasAccount = useAccountStore((s) => s.accounts.length > 0);
  const available = useAccountStore((s) => s.microsoftAvailable);
  const pending = useAccountStore((s) => s.loginPending);
  const cancelling = useAccountStore((s) => s.loginCancelling);
  const authStage = useAccountStore((s) => s.authStage);
  const authExpiresAt = useAccountStore((s) => s.authExpiresAt);
  const login = useAccountStore((s) => s.loginMicrosoft);
  const cancel = useAccountStore((s) => s.cancelMicrosoft);

  const [error, setError] = useState<{ code: string | null; text: string } | null>(null);
  const [copiedUrl, setCopiedUrl] = useState(false);
  const [remainingMs, setRemainingMs] = useState(0);
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

  useEffect(() => {
    if (!pending || !authExpiresAt || authStage !== "waitingBrowser") {
      setRemainingMs(0);
      return;
    }
    const tick = () => setRemainingMs(Math.max(0, authExpiresAt - Date.now()));
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [pending, authExpiresAt, authStage]);

  const signIn = async () => {
    if (!available || pending) return;
    setError(null);
    setCopiedUrl(false);
    ownsAttempt.current = true;
    try {
      if ((await login()) && ownsAttempt.current) onDone();
    } catch (failure) {
      if (failure instanceof CommandError && failure.code === "auth.cancelled") return;
      logError(failure, "Microsoft 登录");
      const code = failure instanceof CommandError ? failure.code : null;
      const key = code ? microsoftLoginReasons.get(code) : undefined;
      if (ownsAttempt.current) {
        setError({ code, text: key ? t(key) : userMessage(failure) });
      }
    } finally {
      ownsAttempt.current = false;
    }
  };

  const reopenBrowser = () => {
    void commands.accountsMicrosoftReopenBrowser().catch((err: unknown) => {
      reportError(err, "重新打开 Microsoft 登录页面");
    });
  };

  const copyAuthUrl = () => {
    void commands
      .accountsMicrosoftCopyUrl()
      .then((res) => {
        if (res.status === "ok") {
          setCopiedUrl(true);
          window.setTimeout(() => setCopiedUrl(false), 2500);
        }
      })
      .catch((err: unknown) => reportError(err, "复制登录链接"));
  };

  const storeUrl = minecraftStoreUrl(i18n.language);
  const reasonKey = reason ? microsoftLoginReasons.get(reason) : undefined;
  const activeCode = error?.code ?? (reason && reason !== "auth.no_account" ? reason : null);
  const recovery =
    activeCode === "auth.no_entitlement"
      ? { labelKey: "account.microsoft.purchase", url: storeUrl }
      : activeCode
        ? RECOVERY_URLS[activeCode]
        : undefined;
  const capabilityHint = !loaded
    ? t("common.loading")
    : !available || reason === "auth.app_unavailable"
      ? t("account.microsoft.unavailable")
      : null;

  return (
    <div className="flex w-full flex-col items-start gap-3">
      <p className="text-fg-muted text-sm">
        {t(hasAccount ? "account.microsoft.browser" : "account.microsoft.signedOut")}
      </p>
      {reasonKey && reason !== "auth.no_account" && reason !== "auth.app_unavailable" && !error && (
        <div className="flex w-full flex-col gap-2 rounded-panel border border-amber-500/35 bg-amber-500/10 p-3 text-xs">
          <p className="text-fg leading-relaxed">{t(reasonKey)}</p>
          {recovery && (
            <div>
              <Button
                size="sm"
                variant="outline"
                onClick={() =>
                  openUrl(recovery.url).catch((failure: unknown) =>
                    reportError(failure, "打开官方帮助页面"),
                  )
                }
              >
                <ExternalLink className="size-3.5" aria-hidden />
                {t(recovery.labelKey)}
              </Button>
            </div>
          )}
        </div>
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
            openUrl(storeUrl).catch((failure: unknown) =>
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
        <AuthProgressPanel
          stage={authStage}
          cancelling={cancelling}
          remainingMs={remainingMs}
          copiedUrl={copiedUrl}
          onReopen={reopenBrowser}
          onCopyUrl={copyAuthUrl}
        />
      )}

      {error && (
        <div
          role="alert"
          className="flex w-full flex-col gap-2 rounded-panel border border-danger/40 bg-danger/10 p-3 text-xs"
        >
          <div className="flex items-start gap-2 text-danger">
            <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
            <p className="min-w-0 flex-1 leading-relaxed">{error.text}</p>
          </div>
          {recovery && (
            <div>
              <Button
                size="sm"
                variant="outline"
                onClick={() =>
                  openUrl(recovery.url).catch((failure: unknown) =>
                    reportError(failure, "打开官方帮助页面"),
                  )
                }
              >
                <ExternalLink className="size-3.5" aria-hidden />
                {t(recovery.labelKey)}
              </Button>
            </div>
          )}
        </div>
      )}

      <div className="flex w-full items-center gap-2 pt-1 text-2xs text-fg-muted">
        <ShieldCheck className="size-3.5 shrink-0 text-accent" aria-hidden />
        <span>{t("account.trustStrip")}</span>
      </div>
    </div>
  );
}
