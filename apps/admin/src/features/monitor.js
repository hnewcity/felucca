import { bytes } from "../lib/format.js";

export const isMetric = (value) => typeof value === "number" && Number.isFinite(value);
export const percentage = (value) => (isMetric(value) ? `${value.toFixed(1)}%` : "—");
export const capacity = (value, tag) => (isMetric(value) ? bytes(value, tag) : "—");
export const rate = (value, tag) => (isMetric(value) ? `${bytes(value, tag)}/s` : "—");
export const timeLabel = (value, tag) => new Date(value).toLocaleTimeString(tag, { hour12: false });
export function preciseTime(value, tag) {
  const date = new Date(value);
  const offset = -date.getTimezoneOffset();
  const zone = `UTC${offset >= 0 ? "+" : "−"}${String(Math.floor(Math.abs(offset) / 60)).padStart(2, "0")}:${String(Math.abs(offset) % 60).padStart(2, "0")}`;
  return `${date.getFullYear()}/${String(date.getMonth() + 1).padStart(2, "0")}/${String(date.getDate()).padStart(2, "0")} ${timeLabel(value, tag)}.${String(date.getMilliseconds()).padStart(3, "0")} ${zone}`;
}
/**
 * An elapsed time as a reader's language writes it: the parts that are zero are
 * dropped, and every language keeps its own unit words.
 */
export function uptime(seconds, t) {
  if (!isMetric(seconds)) return "—";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const parts = [];
  if (days) parts.push(t("units.days", { count: days }));
  if (hours) parts.push(t("units.hours", { count: hours }));
  parts.push(t("units.minutes", { count: minutes }));
  return t("units.joined", { parts: parts.join(t("units.separator")) });
}

// Choose a real sample, never an interpolated value between observations.
export function nearestIndex(samples, timestamp) {
  if (!samples.length) return -1;
  let low = 0;
  let high = samples.length - 1;
  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (samples[mid].timestamp_ms < timestamp) low = mid + 1;
    else high = mid;
  }
  return low > 0 &&
    timestamp - samples[low - 1].timestamp_ms <= samples[low].timestamp_ms - timestamp
    ? low - 1
    : low;
}

export function domain(samples, end, windowSeconds) {
  const start = Math.max(end - windowSeconds * 1000, samples[0]?.timestamp_ms ?? end);
  return [Math.min(start, end - 30000), end];
}
export function rateCeiling(samples, keys) {
  const max = Math.max(1024, ...samples.flatMap((s) => keys.map((key) => s[key] || 0)));
  const step = 10 ** Math.floor(Math.log10(max));
  return Math.ceil(max / step) * step;
}
