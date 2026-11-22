export const encode = encodeURIComponent;

/**
 * A byte count in the caller's locale. The unit stays the binary one the server
 * reports; the digits and decimal separator follow the language.
 */
export function bytes(value = 0, locale) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const n = Math.max(0, Number(value) || 0);
  const index = Math.min(4, n > 0 ? Math.floor(Math.log(n) / Math.log(1024)) : 0);
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(n / 1024 ** index)} ${units[index]}`;
}

export function download(value, name = "felucca-server.json", type = "application/json") {
  const url = URL.createObjectURL(value instanceof Blob ? value : new Blob([value], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export const commaList = (value = "") =>
  value
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
export const omit = (value, keys) =>
  Object.fromEntries(Object.entries(value).filter(([key]) => !keys.includes(key)));

/**
 * Parses pasted JSON. `t` is the caller's translator, so a rejected paste says
 * what is wrong in the language the page is written in.
 */
export function parseObject(text, t) {
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    throw new Error(t("error.invalidJson"));
  }
  if (!value || Array.isArray(value) || typeof value !== "object")
    throw new Error(t("error.notAnObject"));
  return value;
}
