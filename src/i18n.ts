// App internationalisation. Three bundled languages (zh / en / ja); the
// active one is detected from the macOS system locale on first launch and
// can be overridden from Settings — the choice persists in localStorage.
//
// The active language is also mirrored into the backend settings table so the
// Rust side (e.g. the "new articles" notification) can localise its own text.

import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import * as api from "./api";
import zh from "./locales/zh.json";
import en from "./locales/en.json";
import ja from "./locales/ja.json";

export type Language = "zh" | "en" | "ja";

/** Languages offered in the Settings picker, in display order. */
export const LANGUAGES: { code: Language; label: string }[] = [
  { code: "zh", label: "简体中文" },
  { code: "en", label: "English" },
  { code: "ja", label: "日本語" },
];

const STORAGE_KEY = "language";
const SUPPORTED: Language[] = ["zh", "en", "ja"];

/** Resolve the startup language: saved choice → system locale → English. */
export function detectLanguage(): Language {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved && SUPPORTED.includes(saved as Language)) return saved as Language;
  const sys = (navigator.language || "en").toLowerCase();
  if (sys.startsWith("zh")) return "zh";
  if (sys.startsWith("ja")) return "ja";
  return "en";
}

/** Mirror the language into the backend so Rust-side text can be localised,
 *  then rebuild the tray menu in the new language. */
function persistToBackend(lang: Language): void {
  api
    .setSetting("language", lang)
    .then(() => api.refreshTray())
    .catch(() => {});
}

/** Switch the active language and remember it across launches. */
export function setLanguage(lang: Language): void {
  localStorage.setItem(STORAGE_KEY, lang);
  i18n.changeLanguage(lang);
  document.documentElement.lang = lang;
  persistToBackend(lang);
}

i18n.use(initReactI18next).init({
  resources: {
    zh: { translation: zh },
    en: { translation: en },
    ja: { translation: ja },
  },
  lng: detectLanguage(),
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

// Sync the detected language to the backend on startup.
persistToBackend(detectLanguage());

export default i18n;
