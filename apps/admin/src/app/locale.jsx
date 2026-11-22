import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { translate } from "../lib/i18n.js";
import { interfaceLanguage, localeCodes, localeTag, locales } from "../locales/registry.js";

const STORAGE_KEY = "felucca.locale";

function readStored() {
  try {
    const value = window.localStorage.getItem(STORAGE_KEY);
    return localeCodes.includes(value) ? value : null;
  } catch {
    // A browser with storage disabled still gets a language, just not a memory.
    return null;
  }
}

function initial() {
  const stored = readStored();
  if (stored) return stored;
  const accepted = window.navigator.languages ?? [window.navigator.language];
  for (const tag of accepted) {
    const code = tag.toLowerCase().split("-")[0];
    if (localeCodes.includes(code)) return code;
  }
  return interfaceLanguage;
}

const LocaleContext = createContext(null);

export function LocaleProvider({ children }) {
  const [locale, setLocale] = useState(initial);
  const choose = useCallback((next) => {
    setLocale(next);
    try {
      window.localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Choosing a language must not fail because storage is unavailable.
    }
  }, []);
  useEffect(() => {
    document.documentElement.lang = localeTag(locale);
  }, [locale]);
  const value = useMemo(() => {
    const t = (key, args) => translate(locale, key, args);
    return {
      locale,
      tag: localeTag(locale),
      locales,
      setLocale: choose,
      t,
      // Numbers, dates and lists follow the same locale as the text.
      number: (value, options) => new Intl.NumberFormat(localeTag(locale), options).format(value),
      dateTime: (value, options) =>
        new Intl.DateTimeFormat(localeTag(locale), options).format(value),
      time: (value) =>
        new Intl.DateTimeFormat(localeTag(locale), {
          hour: "2-digit",
          minute: "2-digit",
          second: "2-digit",
          hourCycle: "h23",
        }).format(value),
      list: (items) =>
        new Intl.ListFormat(localeTag(locale), { style: "long", type: "conjunction" }).format(
          items,
        ),
    };
  }, [choose, locale]);
  return <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>;
}

export function useLocale() {
  const value = useContext(LocaleContext);
  if (!value) throw new Error("locale provider is missing");
  return value;
}
