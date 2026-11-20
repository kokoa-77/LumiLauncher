import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import {
  getLanguagePreference,
  setLanguagePreference,
  subscribeLanguagePreference,
} from "@/lib/i18n";
import { reportError } from "@/lib/report";
import { Select } from "../ui/Select";

export function LanguagePicker() {
  const { t } = useTranslation();
  const preference = useSyncExternalStore(
    subscribeLanguagePreference,
    getLanguagePreference,
    getLanguagePreference,
  );
  return (
    <div className="w-36 shrink-0">
      <Select
        label={t("language.title")}
        value={preference}
        options={[
          { value: "system", label: t("language.system") },
          { value: "en", label: t("language.en") },
          { value: "zh-CN", label: t("language.zh-CN") },
        ]}
        onChange={(value) => {
          if (value === "system" || value === "en" || value === "zh-CN") {
            void setLanguagePreference(value).catch((error: unknown) =>
              reportError(error, "Change interface language"),
            );
          }
        }}
      />
    </div>
  );
}
