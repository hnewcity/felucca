import { describe, expect, it } from "vitest";
import schemas from "./config-schema.json" with { type: "json" };
import { catalogs } from "../locales/registry.js";
import {
  fieldActive,
  collectionPreview,
  fieldGroups,
  fieldHelp,
  groupPreview,
  inputRequired,
  optionLabel,
} from "./config-presentation.js";
import { setField } from "./config-model.js";
import { settingInitialValue } from "./settings-model.js";
import { translate } from "../lib/i18n.js";

describe("configuration presentation", () => {
  it("summarizes current draft state without exposing credentials or arbitrary values", () => {
    const t = (key, args) => translate("zh", key, args);
    const value = {
      enabled: false,
      public: false,
      mode: "tcp",
      port: 8080,
      backend: [{ addr: "localhost:3000", weight: 3 }],
      options: { auth: { password: "private-password" } },
      id: 99,
    };
    const preview = collectionPreview("TunnelConfig", value, t);
    expect(preview.status.label).toBe("已停用");
    expect(preview.description).toContain("私有访问");
    expect(preview.description).not.toContain("8080");
    const group = { name: "advanced", fields: ["options", "id"] };
    expect(groupPreview("TunnelConfig", group, value, t)).toContain(t("field.options"));
    expect(groupPreview("TunnelConfig", group, value, t)).not.toContain("private-password");
    expect(
      collectionPreview("TunnelConfig", { ...value, mode: "file" }, t).description,
    ).not.toContain("目标数量");
  });

  it("updates folded access summaries when source and target scope change", () => {
    const t = (key, args) => translate("zh", key, args);
    const group = { name: "access", fields: ["acl_mode", "acl_rules", "target_policy"] };
    const value = {
      acl_mode: "allow",
      acl_rules: "127.0.0.0/8, ::1/128",
      target_policy: { allow: ["ip:127.0.0.0/8", "ip:::1/128"] },
    };
    const preview = groupPreview("TunnelConfig", group, value, t);
    expect(preview).toContain(t("scope.source.deviceLabel"));
    expect(preview).toContain(t("scope.device"));
    expect(
      groupPreview("TunnelConfig", group, { ...value, acl_mode: "off", acl_rules: "" }, t),
    ).toContain(t("scope.source.anyLabel"));
    expect(value.acl_mode).toBe("allow");
  });

  it("does not require dormant protocol settings and preserves their configured values", () => {
    const proxy = { mode: "off", trusted: [] };
    expect(fieldActive("ProxyAccept", "trusted", proxy)).toBe(false);
    expect(fieldActive("ProxyAccept", "trusted", { ...proxy, mode: "required" })).toBe(true);
    const backend = {
      addr: "localhost:3000",
      tls: false,
      trust: { mode: "pinned", fingerprint: "saved" },
    };
    expect(fieldActive("Backend", "trust", backend)).toBe(false);
    expect(fieldActive("Backend", "trust", { ...backend, tls: true })).toBe(true);
    expect(backend.trust.fingerprint).toBe("saved");
    expect(fieldActive("HealthCheck", "path", { type: "tcp" })).toBe(false);
    expect(fieldActive("HealthCheck", "path", { type: "http" })).toBe(true);
    expect(fieldActive("TunnelConfig", "file", { mode: "tcp", file: null })).toBe(false);
    expect(fieldActive("TunnelConfig", "file", { mode: "file" })).toBe(true);
  });
  it("labels every Rust configuration field and enum in every supported language", () => {
    for (const catalog of Object.values(catalogs)) {
      const t = (key) => catalog[key];
      for (const [type, spec] of Object.entries(schemas)) {
        for (const key of Object.keys(spec.fields ?? {})) {
          expect(catalog[`field.${key}`], `${type}.${key}`).toBeTruthy();
          const help = fieldHelp(type, key, spec.fields[key]);
          if (help.hint) expect(catalog[`form.hint.${help.hint}`]).toBeTruthy();
        }
        for (const option of spec.options ?? [])
          expect(optionLabel(type, option, t), `${type}.${option}`).toBeTruthy();
      }
    }
  });

  it("keeps every field exactly once when grouping or excluding fields", () => {
    for (const [type, spec] of Object.entries(schemas)) {
      if (spec.type !== "object") continue;
      for (const exclude of [[], Object.keys(spec.fields).slice(0, 2)]) {
        const fields = fieldGroups(type, spec, exclude).flatMap((group) => group.fields);
        expect(fields.sort()).toEqual(
          Object.keys(spec.fields)
            .filter((key) => !exclude.includes(key))
            .sort(),
        );
        expect(new Set(fields).size).toBe(fields.length);
      }
    }
    const changed = {
      ...schemas.TunnelConfig,
      fields: { ...schemas.TunnelConfig.fields, future_option: { type: "string" } },
    };
    expect(fieldGroups("TunnelConfig", changed).at(-1).fields).toContain("future_option");
  });

  it("starts with service identity and connections, leaving internal identifiers in advanced settings", () => {
    const groups = fieldGroups("TunnelConfig", schemas.TunnelConfig);
    expect(groups[0].fields).toEqual(["remark", "mode", "enabled", "public"]);
    expect(groups[1].fields).toContain("backend");
    expect(groups.find((group) => group.name === "advanced").fields).toContain("id");
  });

  it("uses context for enum values that have different meanings", () => {
    const t = (key) => catalogs.zh[key];
    expect(optionLabel("ConnectMode", "direct", t)).toBe("平台连接节点");
    expect(optionLabel("VisitorConnection", "direct", t)).toBe("仅直连");
  });

  it("preserves unrelated values and raw secrets when restoring one setting", () => {
    const original = {
      public: true,
      id: 17,
      options: { auth: { users: { user: "literal-secret" } } },
      future_option: { flag: true },
    };
    expect(setField(original, "public", undefined)).toEqual({
      id: 17,
      options: original.options,
      future_option: original.future_option,
    });
    expect(original.public).toBe(true);
    for (const [type, key, expected] of [
      ["TunnelConfig", "enabled", true],
      ["TunnelConfig", "public", true],
      ["Backend", "weight", 1],
      ["FileConfig", "read_only", true],
      ["FileConfig", "auto_index", true],
      ["HealthCheck", "interval", "10s"],
    ])
      expect(settingInitialValue(type, key, schemas[type].fields[key])).toEqual(expected);
  });

  it("requires complete numeric drafts without banning intentionally empty header values", () => {
    expect(inputRequired({ type: "number", optional: true }, "port")).toBe(true);
    expect(inputRequired({ type: "string", optional: false }, "addr")).toBe(true);
    expect(inputRequired({ type: "string", optional: false }, "X-Remove")).toBe(false);
  });
});
