import { Check, Globe2, Monitor } from "lucide-react";
import { useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import {
  getLanguagePreference,
  type LanguagePreference,
  setLanguagePreference,
  subscribeLanguagePreference,
} from "@/lib/i18n";
import { reportError } from "@/lib/report";
import { Button } from "../ui/Button";
import { StepFrame } from "./parts";

const options = [
  { value: "system", label: "language.system", icon: Monitor },
  { value: "en", label: "language.en", icon: Globe2 },
  { value: "zh-CN", label: "language.zh-CN", icon: Globe2 },
] as const;

export function LanguageStep({ onNext }: { onNext: () => void }) {
  const { t } = useTranslation();
  const preference = useSyncExternalStore(
    subscribeLanguagePreference,
    getLanguagePreference,
    getLanguagePreference,
  );
  const [changing, setChanging] = useState(false);

  const choose = async (value: LanguagePreference) => {
    setChanging(true);
    try {
      await setLanguagePreference(value);
    } catch (error) {
      reportError(error, "Change interface language");
    } finally {
      setChanging(false);
    }
  };

  return (
    <StepFrame
      title={t("onboarding.language.title")}
      hint={t("onboarding.language.hint")}
      footer={
        <Button className="ml-auto" variant="primary" disabled={changing} onClick={onNext}>
          {t("onboarding.next")}
        </Button>
      }
    >
      <fieldset className="flex min-w-0 flex-col gap-2" aria-label={t("language.title")}>
        {options.map(({ value, label, icon: Icon }) => (
          <label
            key={value}
            className="group flex cursor-pointer items-center gap-4 rounded-panel border border-line bg-surface p-4 shadow-raised transition-colors duration-(--motion-press) hover:bg-surface-hover has-checked:border-accent has-checked:bg-accent-soft has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-accent"
          >
            <input
              type="radio"
              className="sr-only"
              name="onboarding-language"
              value={value}
              checked={preference === value}
              disabled={changing}
              onChange={() => void choose(value)}
            />
            <span className="grid size-10 shrink-0 place-items-center rounded-row bg-surface-hover text-fg-muted group-has-checked:bg-accent group-has-checked:text-on-accent">
              <Icon className="size-5" aria-hidden />
            </span>
            <span
              lang={value === "system" ? undefined : value}
              className="min-w-0 flex-1 font-medium text-sm"
            >
              {t(label)}
            </span>
            <Check
              className="invisible size-4 shrink-0 text-accent group-has-checked:visible"
              aria-hidden
            />
          </label>
        ))}
      </fieldset>
    </StepFrame>
  );
}
