import { describe, expect, it } from "vitest";
import {
  parseRules,
  presetOf,
  showRules,
  sourceAcl,
  sourcePreset,
  targetPolicy,
  targetRule,
} from "./address-scope.js";

describe("address scopes", () => {
  it("splits rules on commas, semicolons and lines and drops duplicates", () => {
    expect(parseRules(" 10.0.0.0/8,\n192.168.1.0/24; 10.0.0.0/8\n\n")).toEqual([
      "10.0.0.0/8",
      "192.168.1.0/24",
    ]);
  });
  it("adds the ip: prefix only to addresses and networks", () => {
    expect(targetRule("10.0.0.0/8")).toBe("ip:10.0.0.0/8");
    expect(targetRule("::1")).toBe("ip:::1");
    expect(targetRule("ip:10.0.0.1")).toBe("ip:10.0.0.1");
    expect(targetRule("nas.local")).toBe("nas.local");
    expect(targetRule("*.corp.example")).toBe("*.corp.example");
    expect(targetRule("geoip:cn")).toBe("geoip:cn");
  });
  it("recognizes presets, including the older single-address local rule", () => {
    const presets = ["device", "lan", "any", "custom"];
    expect(presetOf(undefined, presets)).toBe("any");
    expect(presetOf([], presets)).toBe("any");
    expect(presetOf(["ip:127.0.0.1/32"], presets)).toBe("device");
    expect(presetOf(["ip:::1/128", "ip:127.0.0.0/8"], presets)).toBe("device");
    expect(presetOf(targetPolicy("lan").allow, presets)).toBe("lan");
    expect(presetOf(["ip:10.0.0.0/8"], presets)).toBe("custom");
    expect(presetOf([], ["device", "custom"])).toBe("custom");
  });
  it("keeps deny rules and ports when the allowed scope changes", () => {
    const previous = { allow: ["ip:10.0.0.0/8"], deny: ["ip:10.0.0.1"], ports: [80] };
    expect(targetPolicy("device", "", previous)).toEqual({
      allow: ["ip:127.0.0.0/8", "ip:::1/128"],
      deny: ["ip:10.0.0.1"],
      ports: [80],
    });
    expect(showRules(previous.allow)).toBe("10.0.0.0/8");
  });
  it("maps source presets to acl modes and back", () => {
    expect(sourceAcl("any")).toEqual({ acl_mode: "off", acl_rules: "" });
    const lan = sourceAcl("lan");
    expect(lan.acl_mode).toBe("allow");
    expect(sourcePreset(lan.acl_mode, lan.acl_rules)).toBe("lan");
    expect(sourcePreset(undefined, "")).toBe("any");
    expect(sourcePreset("deny", "203.0.113.0/24")).toBe("custom");
    expect(sourcePreset("off", "203.0.113.0/24")).toBe("custom");
  });
});
