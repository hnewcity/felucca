import { describe, expect, it } from "vitest";
import { initialValue, schema } from "./config-model.js";
import {
  agentGuidedFields,
  fieldsFor,
  fieldGuidance,
  patchSetting,
  remoteSummary,
  serverSettings,
  setTimeSource,
  settingInitialValue,
} from "./settings-model.js";
import { catalogs } from "../locales/registry.js";

describe("guided settings drafts", () => {
  it("switches time sources without touching credentials, permissions, or other settings", () => {
    const original = {
      vkey: "<redacted>",
      remote: { enabled: true, file_root: {} },
      network: { dns: ["127.0.0.1:53"] },
    };
    const custom = setTimeSource(original, true);
    expect(custom.ntp).toEqual({ server: "" });
    expect(setTimeSource(custom, false)).toEqual(original);
    expect(original).not.toHaveProperty("ntp");
    const configured = {
      ...original,
      ntp: { server: "time.example.com:123", interval: "10m", extra: 7 },
    };
    expect(setTimeSource(configured, true)).toEqual(configured);
    expect(setTimeSource(configured, false)).toEqual(original);
  });

  it("toggles one option without discarding existing limits or opaque fields", () => {
    const original = {
      server: "example.com:14028",
      p2p: { enabled: true, bind_ip: "192.168.1.5", max_sessions: 8, future_option: true },
      vkey: "<redacted>",
    };
    const changed = patchSetting(original, "p2p", { enabled: false }, { max_sessions: 64 });
    expect(changed).toEqual({ ...original, p2p: { ...original.p2p, enabled: false } });
    expect(original.p2p.enabled).toBe(true);
    expect(patchSetting({}, "p2p", { enabled: true }, { max_sessions: 64 })).toEqual({
      p2p: { enabled: true, max_sessions: 64 },
    });
  });

  it("distinguishes unrestricted destinations from no authorization", () => {
    expect(remoteSummary(undefined)).toBe("guide.remote.off");
    expect(remoteSummary({ enabled: false, target_policy: {} })).toBe("guide.remote.off");
    expect(remoteSummary({ enabled: true })).toBe("guide.remote.unrestricted");
    expect(
      remoteSummary({ enabled: true, target_policy: { allow: [], deny: [], ports: [] } }),
    ).toBe("guide.remote.unrestricted");
    const local = { enabled: true, target_policy: { allow: ["ip:127.0.0.1/32"] } };
    expect(remoteSummary(local)).toBe("guide.remote.local");
    expect(
      remoteSummary({ ...local, target_policy: { ...local.target_policy, ports: [8080] } }),
    ).toBe("guide.remote.custom");
    expect(
      remoteSummary({ ...local, file_root: { docs: { path: "/srv/docs", read_only: true } } }),
    ).toBe("guide.remote.files");
  });

  it("starts customization with valid runtime defaults, including read-only file access", () => {
    const field = (type, key) => settingInitialValue(type, key, schema(type).fields[key]);
    expect(field("NtpConfig", "interval")).toBe("5m");
    expect(field("NtpConfig", "timeout")).toBe("5s");
    expect(field("NtpConfig", "max_offset")).toBe("24h");
    expect(field("P2pAgentConfig", "max_sessions")).toBe(64);
    expect(field("RemoteFileRoot", "read_only")).toBe(true);
    expect(initialValue("P2pAgentConfig")).toEqual({ enabled: false, max_sessions: 64 });
    expect(initialValue("RemoteFileRoot").read_only).toBe(true);
  });

  it("keeps guided fields in the schema and gives every server section a localized purpose", () => {
    expect(new Set(agentGuidedFields).size).toBe(agentGuidedFields.length);
    expect(Object.keys(fieldsFor("AgentConfig", agentGuidedFields).fields).sort()).toEqual(
      [...agentGuidedFields].sort(),
    );
    for (const section of serverSettings) {
      expect(schema("ServerConfig").fields).toHaveProperty(section);
      for (const catalog of Object.values(catalogs)) {
        expect(catalog[`guide.server.${section}.title`]).toBeTruthy();
        if (section === "ntp") continue;
        for (const part of ["description", "when", "default"])
          expect(catalog[`guide.server.${section}.${part}`]).toBeTruthy();
      }
    }
    for (const [type, fields] of Object.entries(fieldGuidance)) {
      for (const [field, key] of Object.entries(fields)) {
        expect(schema(type).fields).toHaveProperty(field);
        for (const catalog of Object.values(catalogs)) {
          expect(catalog[`guide.field.${key}.label`]).toBeTruthy();
          expect(catalog[`guide.field.${key}.hint`]).toBeTruthy();
        }
      }
    }
  });
});
