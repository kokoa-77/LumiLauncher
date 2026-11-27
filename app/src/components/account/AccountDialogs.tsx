import { useTranslation } from "react-i18next";
import { useAccountStore } from "@/stores/accounts";
import { Dialog } from "../ui/Dialog";
import { AccountForm } from "./AccountForm";
import { AccountSecurityDialog } from "./AccountSecurityDialog";
import { SkinControls, SkinPreview } from "./SkinPanel";

/** Microsoft 登录、账户安全透明中心与官方皮肤/披风工坊弹窗（首次打开按需加载）。 */
export function AccountDialogs() {
  return (
    <>
      <AddAccountDialog />
      <SkinDialog />
      <AccountSecurityDialog />
    </>
  );
}

function AddAccountDialog() {
  const { t } = useTranslation();
  const open = useAccountStore((s) => s.addOpen);
  const setOpen = useAccountStore((s) => s.setAddOpen);
  const reason = useAccountStore((s) => s.loginReason);
  const cancel = useAccountStore((s) => s.cancelMicrosoft);
  const close = () => {
    cancel();
    setOpen(false);
  };

  return (
    <Dialog open={open} onClose={close} title={t("account.addTitle")} wide>
      {open && <AccountForm reason={reason} onDone={() => setOpen(false)} />}
    </Dialog>
  );
}

function SkinDialog() {
  const { t } = useTranslation();
  const accountId = useAccountStore((s) => s.skinFor);
  const account = useAccountStore((s) => s.accounts.find((a) => a.id === s.skinFor) ?? null);
  const openSkin = useAccountStore((s) => s.openSkin);
  return (
    <Dialog
      open={accountId !== null && account !== null}
      onClose={() => openSkin(null)}
      title={t("skin.title", { name: account?.name ?? "" })}
      wide
    >
      {account && (
        <div className="flex flex-col gap-5 sm:flex-row">
          <SkinPreview account={account} />
          <div className="min-w-0 flex-1">
            <SkinControls account={account} />
          </div>
        </div>
      )}
    </Dialog>
  );
}
