// The languages the console ships. Codes match the server's catalogs exactly:
// `scripts/check-locales.py` compares the two lists.

import zh from "./zh.json";
import en from "./en.json";
import ja from "./ja.json";
import ko from "./ko.json";
import fr from "./fr.json";
import es from "./es.json";
import ru from "./ru.json";

/** Every catalog, keyed by language code. */
export const catalogs = { zh, en, ja, ko, fr, es, ru };

export const locales = [
  { code: "zh", tag: "zh-CN", name: "中文" },
  { code: "en", tag: "en-US", name: "English" },
  { code: "ja", tag: "ja-JP", name: "日本語" },
  { code: "ko", tag: "ko-KR", name: "한국어" },
  { code: "fr", tag: "fr-FR", name: "Français" },
  { code: "es", tag: "es-ES", name: "Español" },
  { code: "ru", tag: "ru-RU", name: "Русский" },
];

export const localeCodes = locales.map((locale) => locale.code);
export const localeTags = Object.fromEntries(locales.map((l) => [l.code, l.tag]));

/**
 * The language the console was written in, used when the browser asks for
 * nothing we ship.
 */
export const interfaceLanguage = "zh";

/** Where a missing translation falls back to before giving up on the key. */
export const translationFallback = "en";

export function localeTag(code) {
  return localeTags[code] ?? localeTags[interfaceLanguage];
}
