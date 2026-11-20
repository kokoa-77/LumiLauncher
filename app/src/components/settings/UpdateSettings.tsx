import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { formatSize } from "@/lib/format";
import { duration, ease } from "@/lib/motion";
import { reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useUpdateStore } from "@/stores/updates";
import { Button } from "../ui/Button";
import { LightSpinner } from "../ui/LightSpinner";
import { ProgressBar } from "../ui/ProgressBar";
import { Switch } from "../ui/Switch";
import { Group, Row } from "./parts";

const RELEASES_URL = "https://github.com/kokoa-77/LumiLauncher/releases";

export function UpdateSettings() {
  const { t } = useTranslation();
  const mode = useUpdateStore((state) => state.mode);
  const status = useUpdateStore((state) => state.status);
  const loadMode = useUpdateStore((state) => state.loadMode);
  const check = useUpdateStore((state) => state.check);
  const install = useUpdateStore((state) => state.install);
  const settings = useAppStore((state) => state.settings);
  const saveSettings = useAppStore((state) => state.saveSettings);
  const label = t("settings.update.title");

  useEffect(() => {
    void loadMode();
  }, [loadMode]);

  const autoCheckRow = settings ? (
    <Row
      label={t("settings.update.autoCheck")}
      hint={t("settings.update.autoCheckHint")}
      control={
        <Switch
          label={t("settings.update.autoCheck")}
          checked={settings.autoCheckUpdates}
          onChange={(autoCheckUpdates) => void saveSettings({ ...settings, autoCheckUpdates })}
        />
      }
    />
  ) : null;

  if (mode === "portable") {
    return (
      <Group>
        <Row
          label={label}
          hint={t("settings.update.portable")}
          control={
            <Button
              variant="outline"
              onClick={() =>
                openUrl(RELEASES_URL).catch((error: unknown) => reportError(error, "打开发行页"))
              }
            >
              <ExternalLink className="size-4" aria-hidden />
              {t("settings.update.releases")}
            </Button>
          }
        />
        {autoCheckRow}
      </Group>
    );
  }

  if (mode === "disabled") {
    return (
      <Group>
        <Row label={label} hint={t("settings.update.disabled")} control={null} />
        {autoCheckRow}
      </Group>
    );
  }

  let hint: string | undefined;
  let control: ReactNode;
  const progress = status.kind === "installing" ? status.progress : null;
  const downloaded = progress?.downloaded ?? 0;
  const total = progress?.total ?? 0;
  const transferring = progress !== null || status.kind === "handoff";
  const completedDownload = status.kind === "handoff" || (total > 0 && downloaded >= total);
  switch (status.kind) {
    case "checking":
      hint = t("settings.update.checking");
      control = <LightSpinner className="size-4" />;
      break;
    case "installing":
      hint = completedDownload
        ? t("settings.update.verifying")
        : t("settings.update.downloading", { version: status.offer.version });
      control = <LightSpinner className="size-4 text-accent" />;
      break;
    case "handoff":
      hint = t("settings.update.handoff");
      control = <LightSpinner className="size-4 text-accent" />;
      break;
    case "available":
    case "installError":
      hint = t("settings.update.available", { version: status.offer.version });
      control = (
        <Button variant="primary" onClick={() => void install()}>
          {t(
            status.kind === "installError"
              ? "settings.update.retryInstall"
              : "settings.update.install",
          )}
        </Button>
      );
      break;
    case "current":
    case "checkError":
    case "installRejected":
    case "idle":
      if (mode === null && status.kind === "idle") {
        hint = t("common.loading");
        control = <LightSpinner className="size-4" />;
      } else {
        if (status.kind === "current") hint = t("settings.update.current");
        control = (
          <Button variant="outline" onClick={() => void check()}>
            {t("settings.update.check")}
          </Button>
        );
      }
      break;
  }

  const notes = "offer" in status ? status.offer.notes : null;
  return (
    <Group>
      <Row label={label} hint={hint} control={control} />
      {autoCheckRow}
      <AnimatePresence initial={false}>
        {transferring && (
          <motion.div
            key="update-progress"
            className="flex flex-col gap-2 px-4 py-3"
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={{ duration: duration.overlay, ease: ease.out }}
          >
            <ProgressBar
              value={completedDownload ? 1 : total > 0 ? downloaded / total : null}
              label={hint ?? label}
            />
            <p className="text-fg-muted text-xs tabular-nums">
              {status.kind === "handoff"
                ? t("settings.update.downloaded")
                : total > 0
                  ? t("settings.update.progressTotal", {
                      downloaded: formatSize(downloaded),
                      total: formatSize(total),
                    })
                  : t("settings.update.progress", { downloaded: formatSize(downloaded) })}
            </p>
          </motion.div>
        )}
      </AnimatePresence>
      {"message" in status && (
        <p role="alert" className="px-4 py-3 text-danger text-xs">
          {t(
            status.kind === "checkError"
              ? "settings.update.checkError"
              : "settings.update.installError",
            {
              message: status.message,
            },
          )}
        </p>
      )}
      {notes?.trim() && (
        <div className="px-4 py-3">
          <h3 className="font-medium text-sm">{t("settings.update.notes")}</h3>
          <p className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap break-words text-fg-muted text-xs">
            {notes}
          </p>
        </div>
      )}
    </Group>
  );
}
