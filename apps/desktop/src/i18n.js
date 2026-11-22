// The window follows the language the Rust side chose, so the tray menu and
// the window never disagree.

import zh from "./locales/zh.json";
import en from "./locales/en.json";
import ja from "./locales/ja.json";
import ko from "./locales/ko.json";
import fr from "./locales/fr.json";
import es from "./locales/es.json";
import ru from "./locales/ru.json";

export const catalogs = { zh, en, ja, ko, fr, es, ru };

export function translate(locale, key, args) {
  const text = catalogs[locale]?.[key] ?? catalogs.en[key];
  if (text === undefined) return key;
  if (!args) return text;
  return text.replace(/\{(\w+)\}/g, (match, name) =>
    Object.hasOwn(args, name) ? String(args[name]) : match,
  );
}

/** A Rust reply: `{ key }` to translate here, `{ text }` already in this language. */
export function replyText(locale, reply) {
  if (reply?.key) return translate(locale, reply.key);
  if (reply?.text) return reply.text;
  return String(reply ?? "");
}
