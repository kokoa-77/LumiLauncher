import i18n from "i18next";
import { initReactI18next } from "react-i18next";

export type UiLanguage = "en" | "zh-CN";
export type LanguagePreference = "system" | UiLanguage;

const storageKey = "mcl.language";
const listeners = new Set<() => void>();
const loadCatalog = {
  en: () => import("../locales/en.json"),
  "zh-CN": () => import("../locales/zh-CN.json"),
};

function loadLanguagePreference(): LanguagePreference {
  try {
    const saved = globalThis.localStorage?.getItem(storageKey);
    if (saved === "en" || saved === "zh-CN") return saved;
  } catch (error) {
    console.warn("Unable to read the interface language preference.", error);
  }
  return "system";
}

function systemLanguage(): UiLanguage {
  const browser = globalThis.navigator;
  const locales = browser?.languages?.length ? browser.languages : [browser?.language];
  for (const locale of locales) {
    if (!locale) continue;
    const tag = locale.toLowerCase().replaceAll("_", "-");
    if (tag === "en" || tag.startsWith("en-")) return "en";
    if (tag === "zh" || /^zh-hans(?:-|$)/.test(tag) || /^zh-(?:cn|sg)$/.test(tag)) {
      return "zh-CN";
    }
  }
  return "en";
}

let preference = loadLanguagePreference();
let requested = preference;
let request = 0;

export function getLanguagePreference(): LanguagePreference {
  return preference;
}

export function subscribeLanguagePreference(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export async function setLanguagePreference(next: LanguagePreference): Promise<void> {
  requested = next;
  const pending = ++request;
  const language = next === "system" ? systemLanguage() : next;
  if (!i18n.hasResourceBundle(language, "translation")) {
    const catalog = (await loadCatalog[language]()).default;
    if (pending !== request) return;
    i18n.addResourceBundle(language, "translation", catalog);
  }
  if (pending !== request) return;
  preference = next;
  try {
    globalThis.localStorage?.setItem(storageKey, next);
  } catch (error) {
    console.warn("Unable to save the interface language preference.", error);
  }
  await i18n.changeLanguage(language);
  for (const listener of listeners) listener();
}

export function getUiLanguage(): UiLanguage {
  return i18n.language === "zh-CN" ? "zh-CN" : "en";
}

const initialLanguage = preference === "system" ? systemLanguage() : preference;
const initialCatalog = (await loadCatalog[initialLanguage]()).default;
await i18n.use(initReactI18next).init({
  lng: initialLanguage,
  fallbackLng: false,
  supportedLngs: ["en", "zh-CN"],
  resources: { [initialLanguage]: { translation: initialCatalog } },
  interpolation: { escapeValue: false },
  initAsync: false,
});

function updateDocumentLanguage() {
  if (typeof document !== "undefined") document.documentElement.lang = getUiLanguage();
}

updateDocumentLanguage();
i18n.on("languageChanged", updateDocumentLanguage);
globalThis.window?.addEventListener("languagechange", () => {
  if (requested === "system") {
    void setLanguagePreference("system").catch((error: unknown) => {
      console.warn("Unable to update the interface language.", error);
    });
  }
});

export default i18n;
