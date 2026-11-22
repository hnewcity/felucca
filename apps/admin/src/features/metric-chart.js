import { isMetric } from "./monitor.js";

// ECharts must see a null between discontinuous observations. Keep an explicit
// mapping back to real samples so tooltips never select an invented gap value.
export function chartFrames(samples, interval = 5000) {
  const rows = [];
  const indices = [];
  samples.forEach((sample, index) => {
    const previous = samples[index - 1];
    if (previous && sample.timestamp_ms - previous.timestamp_ms > interval * 2)
      rows.push({ timestamp_ms: previous.timestamp_ms + interval });
    indices.push(rows.length);
    rows.push(sample);
  });
  return { rows, indices };
}

export function visibleRange(range, zoom) {
  if (!zoom) return range;
  const start = Math.max(range[0], zoom[0]);
  const end = Math.min(range[1], zoom[1]);
  return start < end ? [start, end] : range;
}

export function zoomIn(range, zoom, anchor) {
  const current = visibleRange(range, zoom);
  const span = Math.max(10000, (current[1] - current[0]) / 2);
  if (span >= range[1] - range[0]) return null;
  const middle =
    isMetric(anchor) && anchor >= current[0] && anchor <= current[1]
      ? anchor
      : (current[0] + current[1]) / 2;
  const start = Math.max(range[0], Math.min(middle - span / 2, range[1] - span));
  return [start, start + span];
}

export function metricStats(samples, key, range) {
  const values = samples
    .filter((s) => s.timestamp_ms >= range[0] && s.timestamp_ms <= range[1])
    .map((s) => s[key])
    .filter(isMetric);
  if (!values.length) return { min: null, average: null, max: null };
  return {
    min: Math.min(...values),
    average: values.reduce((sum, n) => sum + n, 0) / values.length,
    max: Math.max(...values),
  };
}
