import { describe, expect, it } from "vitest";
import { domain, nearestIndex, percentage, rate, rateCeiling, preciseTime } from "./monitor.js";

describe("real metric samples", () => {
  const samples = [
    { timestamp_ms: 10000, cpu: 0 },
    { timestamp_ms: 15000, cpu: 20 },
    { timestamp_ms: 20000, cpu: null },
    { timestamp_ms: 25000, cpu: 50 },
    { timestamp_ms: 45000, cpu: 80 },
  ];
  it("selects the nearest actual point, including range boundaries and empty history", () => {
    expect(nearestIndex([], 5)).toBe(-1);
    expect(nearestIndex(samples, 0)).toBe(0);
    expect(nearestIndex(samples, 16000)).toBe(1);
    expect(nearestIndex(samples, 99999)).toBe(4);
  });
  it("does not format unavailable observations as zero", () => {
    expect(percentage(0)).toBe("0.0%");
    expect(percentage(null)).toBe("—");
    expect(rate(undefined)).toBe("—");
  });
  it("labels real times with milliseconds and timezone and bounds the plot", () => {
    expect(preciseTime(12345)).toContain(".345 UTC");
    expect(domain(samples, 50000, 300)).toEqual([10000, 50000]);
    expect(domain([], 50000, 300)).toEqual([20000, 50000]);
    expect(rateCeiling([{ rx: null, tx: 1987 }], ["rx", "tx"])).toBe(2000);
  });
});
