import { Languages } from "lucide-react";
import { useLocale } from "../app/locale.jsx";
import { Select } from "./Select.jsx";

/**
 * Picks the console language.
 *
 * Each language is named in itself — a reader looking for their own language
 * should not have to read another one to find it. The choice is remembered in
 * this browser and sent to the server with every request, so server messages
 * answer in the same language as the pages.
 *
 * `compact` shows only the icon in the desktop sidebar rail; the shared menu
 * keeps full language names in both layouts.
 */
export function LanguageSwitch({ compact = false }) {
  const { locale, locales, setLocale, t } = useLocale();
  const current = locales.find((option) => option.code === locale);
  return (
    <Select
      className={`locale-switch${compact ? " locale-switch-compact" : ""}`}
      title={`${t("a11y.language")}: ${current.name}`}
      aria-label={t("a11y.language")}
      icon={Languages}
      value={locale}
      onValueChange={setLocale}
      options={locales.map((option) => ({ value: option.code, label: option.name }))}
    />
  );
}
