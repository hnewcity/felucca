import { describe, expect, it } from "vitest";
import { agentTemplate, visitorTemplate, hasMissingCredentials } from "./launch.js";
import { initialValue, schema, setField } from "./config-model.js";

describe("deployment configuration", () => {
  it("pins the active bridge identity and requires explicitly supplied credentials", () => {
    const value = agentTemplate(
      { id: "one" },
      {
        bridge: { bind: "0.0.0.0:8024" },
        fingerprint: "abc",
      },
      "2001:db8::1",
    );
    expect(value.server).toBe("[2001:db8::1]:8024");
    expect(value.trust).toEqual({ mode: "pinned", fingerprint: "abc" });
    expect(value.remote.target_policy.allow).toEqual([]);
    expect(hasMissingCredentials(value)).toBe(true);
    value.vkey = "supplied-node-key";
    expect(hasMissingCredentials(value)).toBe(false);
    value.visitor.push(visitorTemplate({ id: "files", config: { direct: true } }));
    expect(value.visitor[0].connection).toBe("prefer_direct");
    value.visitor[0].secret = "<redacted>";
    expect(hasMissingCredentials(value)).toBe(true);
    value.visitor[0].secret = "supplied-share-key";
    expect(hasMissingCredentials(value)).toBe(false);
  });
  it("keeps defaults absent and resource names as plain object keys", () => {
    expect(initialValue("FileConfig")).toMatchObject({ read_only: true });
    const value = setField({ existing: 1 }, "__proto__", { valid: true });
    expect(Object.hasOwn(value, "__proto__")).toBe(true);
    expect(Object.getPrototypeOf(value)).toBe(Object.prototype);
    expect(setField(value, "existing", undefined)).not.toHaveProperty("existing");
    expect(schema("AgentConfig").fields).toHaveProperty("remote");
    expect(schema("TunnelConfig").fields).toHaveProperty("health");
  });
});
