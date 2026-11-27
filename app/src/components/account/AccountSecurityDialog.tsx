import type { TFunction } from "i18next";
import { AlertTriangle, CheckCircle2, LockKeyhole, RefreshCw, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useAccountStore, useCurrentAccount } from "@/stores/accounts";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { LightSpinner } from "../ui/LightSpinner";
import { Tag } from "../ui/Tag";

function formatVerifiedAgo(ms: number | null, t: TFunction): string {
  if (!ms) return t("security.healthUnknown");
  const diffMin = Math.max(0, Math.floor((Date.now() - ms) / 60_000));
  if (diffMin < 1) return t("home.world.justNow");
  if (diffMin < 60) return t("home.world.minutesAgo", { n: diffMin });
  return t("home.world.hoursAgo", { n: Math.floor(diffMin / 60) });
}

/** 账户安全透明中心：展示真实的本机凭据库、协议防线与账号健康度。 */
export function AccountSecurityDialog() {
  const { t } = useTranslation();
  const open = useAccountStore((s) => s.securityOpen);
  const setOpen = useAccountStore((s) => s.setSecurityOpen);
  const security = useAccountStore((s) => s.security);
  const current = useCurrentAccount();
  const health = useAccountStore((s) => (current ? s.health[current.id] : undefined));
  const prewarm = useAccountStore((s) => s.prewarm);
  const [checking, setChecking] = useState(false);

  const checkNow = async () => {
    if (!current || checking) return;
    setChecking(true);
    try {
      await prewarm(current.id);
    } finally {
      setChecking(false);
    }
  };

  const persistentVault = security?.vaultKind === "windowsCredentialManager";

  return (
    <Dialog open={open} onClose={() => setOpen(false)} title={t("security.title")} wide>
      <div className="flex flex-col gap-4">
        <div
          className={`flex items-start gap-3 rounded-panel border p-3.5 ${
            persistentVault
              ? "border-emerald-500/30 bg-emerald-500/8"
              : "border-amber-500/40 bg-amber-500/10"
          }`}
        >
          {persistentVault ? (
            <ShieldCheck className="mt-0.5 size-5 shrink-0 text-emerald-500" aria-hidden />
          ) : (
            <AlertTriangle className="mt-0.5 size-5 shrink-0 text-amber-500" aria-hidden />
          )}
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <p className="font-medium text-sm">
                {persistentVault ? t("security.vaultDpapiTitle") : t("security.vaultMemoryTitle")}
              </p>
              <Tag>
                {persistentVault ? t("security.vaultPersistentTag") : t("security.vaultMemoryTag")}
              </Tag>
            </div>
            <p className="mt-1 text-fg-muted text-xs leading-relaxed">
              {persistentVault ? t("security.vaultDpapiDesc") : t("security.vaultMemoryDesc")}
            </p>
          </div>
        </div>

        <div className="grid gap-2.5 sm:grid-cols-2">
          <PostureCard
            title={t("security.pkceTitle")}
            value={`${security?.pkceMethod ?? "S256"} + ${security?.responseMode ?? "form_post"}`}
            desc={t("security.pkceDesc")}
          />
          <PostureCard
            title={t("security.pipeTitle")}
            value={
              security?.sessionPipe !== false ? t("security.pipeActive") : t("security.pipeOff")
            }
            desc={t("security.pipeDesc")}
          />
          <PostureCard
            title={t("security.loopbackTitle")}
            value={security?.callbackMode ?? "dualstack-localhost"}
            desc={t("security.loopbackDesc")}
          />
          <PostureCard
            title={t("security.zeroServerTitle")}
            value={t("security.zeroServerValue")}
            desc={t("security.zeroServerDesc")}
          />
        </div>

        {current && (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-panel border border-line bg-surface p-3.5">
            <div className="min-w-0">
              <p className="font-medium text-sm">
                {t("security.healthRowTitle", { name: current.name })}
              </p>
              <p className="mt-0.5 flex items-center gap-1.5 text-fg-muted text-xs">
                <CheckCircle2 className="size-3.5 text-emerald-500" aria-hidden />
                <span>
                  {t(`security.healthStatus.${health?.status ?? "unknown"}`)} ·{" "}
                  {formatVerifiedAgo(health?.verifiedAtMs ?? null, t)}
                </span>
              </p>
            </div>
            <Button size="sm" variant="outline" disabled={checking} onClick={() => void checkNow()}>
              {checking ? (
                <LightSpinner className="size-3.5" />
              ) : (
                <RefreshCw className="size-3.5" aria-hidden />
              )}
              {t("security.checkNow")}
            </Button>
          </div>
        )}

        <div className="flex items-start gap-2.5 rounded-panel border border-line bg-surface-2 px-3.5 py-2.5 text-fg-muted text-xs">
          <LockKeyhole className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
          <p>{t("security.antiPhishingTip")}</p>
        </div>
      </div>
    </Dialog>
  );
}

function PostureCard({ title, value, desc }: { title: string; value: string; desc: string }) {
  return (
    <div className="rounded-panel border border-line bg-surface p-3">
      <p className="text-fg-muted text-2xs">{title}</p>
      <p className="mt-0.5 font-medium text-xs">{value}</p>
      <p className="mt-1 text-fg-muted text-2xs leading-relaxed">{desc}</p>
    </div>
  );
}
