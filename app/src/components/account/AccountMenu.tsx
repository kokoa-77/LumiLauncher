import { Check, LogOut, Plus, ShieldCheck, Shirt } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AccountHealthStatus, AccountView } from "@/lib/bindings";
import { duration, ease, spring } from "@/lib/motion";
import { useAccountStore, useCurrentAccount, useSkin } from "@/stores/accounts";
import { PlayerHead } from "../PlayerHead";
import { Button } from "../ui/Button";
import { IconButton } from "../ui/IconButton";
import { Tag } from "../ui/Tag";
import { SkinFigure } from "./SkinFigure";

function healthDotClass(status: AccountHealthStatus | undefined): string {
  switch (status) {
    case "ready":
      return "bg-emerald-500";
    case "needsRelogin":
    case "networkError":
      return "bg-amber-500";
    case "verifying":
      return "bg-accent animate-pulse";
    default:
      return "bg-fg-muted/45";
  }
}

/**
 * 右上角的账号胶囊（像素头像 + 名字 + 账号类型）和点开的账号弹层：
 * 上面是 2D 人物、名字、会话健康、皮肤与安全中心入口；下面是切换账号、二次确认退出和添加账号。
 */
export function AccountMenu() {
  const { t } = useTranslation();
  const current = useCurrentAccount();
  const skin = useSkin(current);
  const open = useAccountStore((s) => s.menuOpen);
  const setOpen = useAccountStore((s) => s.setMenuOpen);
  const setAddOpen = useAccountStore((s) => s.setAddOpen);
  const healthStatus = useAccountStore((s) => (current ? s.health[current.id]?.status : undefined));
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, setOpen]);

  const name = current?.name ?? t("account.none");
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => (current ? setOpen(!open) : setAddOpen(true))}
        className={`flex h-8 items-center gap-2 rounded-icon border border-line pr-2 pl-1 text-fg text-sm shadow-raised transition-colors duration-(--motion-press) hover:bg-surface-hover ${
          open ? "bg-surface-hover" : "bg-surface"
        }`}
      >
        {current ? (
          <PlayerHead name={current.name} skin={skin} className="size-6" />
        ) : (
          <span className="grid size-6 place-items-center rounded-[3px] bg-surface-2">
            <Plus className="size-3.5 text-fg-muted" aria-hidden />
          </span>
        )}
        <span className="max-w-32 truncate">{name}</span>
        {current && (
          <Tag className="flex items-center gap-1 px-1.5! py-0! text-2xs">
            <span className={`size-1.5 rounded-full ${healthDotClass(healthStatus)}`} aria-hidden />
            {t("account.microsoft.label")}
          </Tag>
        )}
      </button>
      <AnimatePresence>
        {open && current && (
          <motion.div
            role="dialog"
            aria-label={t("account.menu")}
            className="absolute top-full right-0 z-30 mt-1.5 w-80 origin-top-right overflow-hidden rounded-panel border border-line bg-overlay text-fg shadow-overlay"
            initial={{ opacity: 0, scale: 0.97, y: -6 }}
            animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
            exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
          >
            <CurrentCard key={current.id} account={current} skin={skin} />
            <AccountList currentId={current.id} />
            <div className="border-line border-t p-2">
              <Button
                variant="ghost"
                className="w-full justify-start"
                onClick={() => setAddOpen(true)}
              >
                <Plus className="size-4" aria-hidden />
                {t("account.add")}
              </Button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** 弹层上半：2D 人物、名字、皮肤工坊与安全中心入口。 */
function CurrentCard({ account, skin }: { account: AccountView; skin: string | null }) {
  const { t } = useTranslation();
  const openSkin = useAccountStore((s) => s.openSkin);
  const setSecurityOpen = useAccountStore((s) => s.setSecurityOpen);
  const healthStatus = useAccountStore((s) => s.health[account.id]?.status);
  return (
    <div className="flex items-center gap-4 p-4">
      <div className="grid h-36 w-20 shrink-0 place-items-center rounded-panel bg-surface-2">
        {skin ? (
          <SkinFigure skin={skin} model={account.skinModel} className="h-32" />
        ) : (
          <PlayerHead name={account.name} className="size-12" />
        )}
      </div>
      <div className="flex min-w-0 flex-1 flex-col items-start gap-2">
        <p className="w-full truncate font-semibold text-base">{account.name}</p>
        <div className="flex flex-wrap items-center gap-1.5">
          <Tag className="flex items-center gap-1">
            <span className={`size-1.5 rounded-full ${healthDotClass(healthStatus)}`} aria-hidden />
            {t("account.microsoft.label")}
          </Tag>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <Button size="sm" variant="outline" onClick={() => openSkin(account.id)}>
            <Shirt className="size-3.5" aria-hidden />
            {t("skin.preview")}
          </Button>
          <IconButton label={t("security.open")} size="sm" onClick={() => setSecurityOpen(true)}>
            <ShieldCheck className="size-3.5 text-accent" aria-hidden />
          </IconButton>
        </div>
      </div>
    </div>
  );
}

/** 切换 Microsoft 账号（退出采用轻量两次点击确认）。 */
function AccountList({ currentId }: { currentId: string }) {
  const { t } = useTranslation();
  const accounts = useAccountStore((s) => s.accounts);
  const select = useAccountStore((s) => s.select);
  const remove = useAccountStore((s) => s.remove);
  const [confirmRemoveId, setConfirmRemoveId] = useState<string | null>(null);

  useEffect(() => {
    if (!confirmRemoveId) return;
    const timer = window.setTimeout(() => setConfirmRemoveId(null), 4000);
    return () => window.clearTimeout(timer);
  }, [confirmRemoveId]);

  return (
    <div className="border-line border-t p-2">
      <p className="px-2 pt-1 pb-1.5 text-fg-muted text-xs">{t("account.list")}</p>
      <ul className="flex max-h-60 flex-col gap-0.5 overflow-y-auto">
        {accounts.map((account) => (
          <AccountRow
            key={account.id}
            account={account}
            selected={account.id === currentId}
            confirming={confirmRemoveId === account.id}
            onSelect={() => {
              setConfirmRemoveId(null);
              void select(account.id);
            }}
            onRequestRemove={() => setConfirmRemoveId(account.id)}
            onConfirmRemove={() => {
              setConfirmRemoveId(null);
              void remove(account.id);
            }}
          />
        ))}
      </ul>
    </div>
  );
}

function AccountRow({
  account,
  selected,
  confirming,
  onSelect,
  onRequestRemove,
  onConfirmRemove,
}: {
  account: AccountView;
  selected: boolean;
  confirming: boolean;
  onSelect: () => void;
  onRequestRemove: () => void;
  onConfirmRemove: () => void;
}) {
  const { t } = useTranslation();
  const skin = useSkin(account);
  const status = useAccountStore((s) => s.health[account.id]?.status);
  return (
    <li
      className={`group flex items-center gap-1 rounded-row pr-1 transition-colors duration-(--motion-press) ${
        selected ? "bg-accent-soft" : "hover:bg-surface-hover"
      }`}
    >
      <button
        type="button"
        aria-pressed={selected}
        onClick={onSelect}
        className="flex min-w-0 flex-1 items-center gap-2.5 px-2 py-1.5 text-left"
      >
        <PlayerHead name={account.name} skin={skin} className="size-7" />
        <span
          className={`min-w-0 flex-1 truncate text-sm ${selected ? "font-medium text-accent" : ""}`}
        >
          {account.name}
        </span>
        <span className={`size-1.5 shrink-0 rounded-full ${healthDotClass(status)}`} aria-hidden />
      </button>
      {confirming ? (
        <button
          type="button"
          onClick={onConfirmRemove}
          className="inline-flex h-6 shrink-0 items-center gap-1 rounded-chip bg-danger px-2 font-medium text-2xs text-on-accent"
        >
          <Check className="size-3" aria-hidden />
          {t("account.confirmSignOut")}
        </button>
      ) : (
        <IconButton
          label={t("account.signOut")}
          size="sm"
          danger
          className="opacity-70 transition-opacity hover:opacity-100 focus-visible:opacity-100"
          onClick={onRequestRemove}
        >
          <LogOut className="size-3.5" aria-hidden />
        </IconButton>
      )}
    </li>
  );
}
