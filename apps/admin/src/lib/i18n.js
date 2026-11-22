// Locale-aware text: lookup with fallback, named interpolation and plurals.

import { catalogs, translationFallback } from "../locales/registry.js";

const pluralRules = new Map();

function category(locale, count) {
  if (!pluralRules.has(locale)) {
    pluralRules.set(locale, new Intl.PluralRules(locale));
  }
  return pluralRules.get(locale).select(count);
}

function template(locale, key) {
  return catalogs[locale]?.[key] ?? catalogs[translationFallback]?.[key];
}

function fill(text, args) {
  if (!args) return text;
  return text.replace(/\{(\w+)\}/g, (match, name) =>
    Object.hasOwn(args, name) ? String(args[name]) : match,
  );
}

/**
 * One message in one language.
 *
 * `{name}` placeholders are replaced from `args`, and a `count` argument picks
 * the plural form (`key.one`, `key.other`, …) the language needs. A key that is
 * missing from the catalog returns the key itself, which is unmistakable in the
 * console and caught by the catalog test.
 */
export function translate(locale, key, args) {
  const chosen = args && Object.hasOwn(args, "count") ? category(locale, args.count) : null;
  if (chosen) {
    const plural = template(locale, `${key}.${chosen}`);
    if (plural) return fill(plural, args);
  }
  const text = template(locale, key);
  return text === undefined ? key : fill(text, args);
}
