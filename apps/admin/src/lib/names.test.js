import { afterEach, describe, expect, it, vi } from "vitest";
import { generateName } from "./names.js";

afterEach(() => vi.restoreAllMocks());

describe("name suggestions", () => {
  it("avoids existing names and the current value even when randomness repeats", () => {
    vi.spyOn(crypto, "getRandomValues").mockImplementation((bytes) => bytes.fill(0));
    const existingNames = [" Device-AAAAA ", "device-aaaaa-2"];
    const result = generateName("device", { existingNames, current: "device-aaaaa-3" });
    expect(result).toBe("device-aaaaa-4");
    expect(generateName("device", { existingNames, current: result })).not.toBe(result);
    expect(existingNames).toEqual([" Device-AAAAA ", "device-aaaaa-2"]);
  });

  it("keeps resource keys portable and supports localized display names", () => {
    expect(generateName("account")).toMatch(/^account-[a-z2-9]{5}$/);
    expect(generateName("设备")).toMatch(/^设备-[a-z2-9]{5}$/);
    expect(generateName("Service", { existingNames: [undefined, null, ""] })).toMatch(/^Service-/);
  });
});
