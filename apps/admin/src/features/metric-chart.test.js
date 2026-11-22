import { describe, expect, it } from "vitest";
import { chartFrames, metricStats, visibleRange, zoomIn } from "./metric-chart.js";

describe("ECharts time series adaptation", () => {
  const samples = [
    { timestamp_ms: 10000, cpu: 0 },
    { timestamp_ms: 15000, cpu: null },
    { timestamp_ms: 20000, cpu: 30 },
    { timestamp_ms: 50000, cpu: 60 },
  ];
  it("breaks missing intervals without changing the original samples or tooltip indices", () => {
    const frames = chartFrames(samples);
    expect(frames.rows).toHaveLength(5);
    expect(frames.rows[3]).toEqual({ timestamp_ms: 25000 });
    expect(frames.indices.map((index) => frames.rows[index])).toEqual(samples);
    expect(frames.rows[0].cpu).toBe(0);
    expect(frames.rows[1].cpu).toBeNull();
  });
  it("computes visible-window statistics from finite observations only", () => {
    expect(metricStats(samples, "cpu", [10000, 20000])).toEqual({ min: 0, average: 15, max: 30 });
    expect(metricStats(samples, "cpu", [30000, 40000])).toEqual({
      min: null,
      average: null,
      max: null,
    });
  });
  it("preserves absolute zoom bounds during live updates and clamps expired history", () => {
    expect(visibleRange([10000, 60000], [20000, 40000])).toEqual([20000, 40000]);
    expect(visibleRange([30000, 60000], [20000, 40000])).toEqual([30000, 40000]);
    expect(visibleRange([50000, 60000], [20000, 40000])).toEqual([50000, 60000]);
    expect(zoomIn([10000, 50000], null, 10000)).toEqual([10000, 30000]);
    expect(zoomIn([10000, 50000], [20000, 40000])).toEqual([25000, 35000]);
    expect(zoomIn([10000, 15000], null)).toBeNull();
  });
});
