import { describe, expect, it, vi } from "vitest";
import { translate } from "../lib/i18n.js";
import { resourceEditor, tunnelEditor } from "./resource-editors.js";

const t = (key, args) => translate("en", key, args);
const values = (editor) =>
  Object.fromEntries(editor.fields.map((field) => [field.key, field.value]));
const principal = { role: "admin", account: "root" };
describe("configuration round trips", () => {
  it.each(["users", "accounts", "locals", "shares", "platforms", "webhooks"])(
    "suggests a new %s name without changing existing resource identities",
    (kind) => {
      const rows = [{ id: "keep-this-name", config: {} }];
      const created = resourceEditor(kind, null, principal, vi.fn(), false, t, rows);
      const suggested = created.fields.find((field) => field.key === "name");
      expect(suggested.value).toMatch(/^[a-z]+-[a-z2-9]{5}$/);
      expect(suggested.nameGeneration.existingNames).toEqual(["keep-this-name"]);
      const edited = resourceEditor(kind, rows[0], principal, vi.fn(), false, t, rows);
      const existing = edited.fields.find((field) => field.key === "name");
      expect(existing.value).toBe("keep-this-name");
      expect(existing.disabled).toBe(true);
      expect(existing.nameGeneration).toBeUndefined();
    },
  );
  it("preserves redacted credentials and advanced client configuration", async () => {
    const write = vi.fn();
    const config = {
      user: "team",
      remark: "",
      vkey: "<redacted>",
      enabled: true,
      allow_config_push: false,
      limits: { max_connections: 20 },
    };
    const editor = resourceEditor(
      "clients",
      { id: "node/one", config },
      principal,
      write,
      false,
      t,
    );
    await editor.save({ ...values(editor), remark: "new remark" }, editor.extra);
    expect(write).toHaveBeenCalledWith("/api/clients/node%2Fone", "PUT", {
      ...config,
      remark: "new remark",
    });
  });
  it("preserves existing destination restrictions when editing a device name", async () => {
    const write = vi.fn();
    const managed = {
      tunnel: [],
      target_policy: {
        allow: ["domain:nas.local"],
        deny: ["ip:10.0.0.0/8"],
        ports: [443],
      },
    };
    const editor = resourceEditor(
      "clients",
      {
        id: "restricted",
        config: {
          user: "team",
          remark: "NAS",
          vkey: "<redacted>",
          enabled: true,
        },
      },
      principal,
      write,
      false,
      t,
      [],
      managed,
    );
    await editor.save({ ...values(editor), remark: "Renamed" }, editor.extra);
    expect(write.mock.calls[0][2].operations[1]).toEqual({
      method: "PUT",
      path: "/api/managed/restricted",
      value: managed,
    });
  });
  it("keeps backend weights and options when toggling a tunnel private and public", async () => {
    const write = vi.fn();
    const config = {
      id: 1,
      mode: "tcp",
      public: true,
      port: 8022,
      bind: "127.0.0.1",
      enabled: true,
      remark: "",
      backend: [{ addr: "127.0.0.1:22", weight: 3 }],
      hosts: [],
      options: { proxy_protocol: "v2" },
    };
    const editor = tunnelEditor(config, "/api/managed/one", write, t);
    await editor.save({ ...values(editor), public: false }, editor.extra);
    expect(write.mock.calls[0][2]).toEqual({ ...config, public: false, port: 0, bind: null });
    await editor.save(values(editor), editor.extra);
    expect(write.mock.calls[1][2]).toEqual(config);
  });
  it("preserves an account's password hash when the password field is blank", async () => {
    const write = vi.fn();
    const config = {
      role: "viewer",
      user: "team",
      disabled: false,
      password_hash: "<redacted>",
      token_hash: "<redacted>",
    };
    const editor = resourceEditor("accounts", { id: "one", config }, principal, write, false, t);
    await editor.save(values(editor));
    expect(write.mock.calls[0][2]).toEqual(config);
    expect(write.mock.calls[0][2]).not.toHaveProperty("password");
  });
  it("preserves sharing secrets while normalizing the visitor list", async () => {
    const write = vi.fn();
    const config = {
      client: "provider",
      tunnel: 7,
      secret: "<redacted>",
      allow_clients: ["one"],
      direct: true,
      dynamic: false,
    };
    const editor = resourceEditor("shares", { id: "office", config }, principal, write, false, t);
    await editor.save({ ...values(editor), clients: " one, two, " }, editor.extra);
    expect(write.mock.calls[0][2]).toEqual({ ...config, allow_clients: ["one", "two"] });
  });
  it("edits backend addresses without stripping TLS, weight or pinned trust", async () => {
    const write = vi.fn();
    const backend = [
      {
        addr: "old:443",
        weight: 9,
        tls: true,
        trust: { mode: "pinned", fingerprint: "aa".repeat(32) },
      },
    ];
    const editor = tunnelEditor(
      {
        id: 1,
        mode: "tcp",
        public: true,
        port: 8080,
        enabled: true,
        backend,
        options: { proxy_protocol: "v2" },
      },
      "/api/managed/one",
      write,
      t,
    );
    await editor.save(
      { ...values(editor), backend: [{ ...backend[0], addr: "new:443" }] },
      editor.extra,
    );
    expect(write.mock.calls[0][2].backend).toEqual([{ ...backend[0], addr: "new:443" }]);
    expect(backend[0].addr).toBe("old:443");
    const advanced = [{ ...backend[0], weight: 4 }];
    await editor.save(values(editor), { ...editor.extra, backend: advanced });
    expect(write.mock.calls[1][2].backend).toEqual(advanced);
    expect(write.mock.calls[1][2].options).toEqual({ proxy_protocol: "v2" });
  });
  it("revokes a token while preserving the account password", async () => {
    const write = vi.fn();
    const editor = resourceEditor(
      "accounts",
      {
        id: "root",
        config: {
          role: "admin",
          user: "",
          password_hash: "<redacted>",
          token_hash: "<redacted>",
        },
      },
      principal,
      write,
      false,
      t,
    );
    await editor.save(values(editor), { token_hash: null });
    expect(write.mock.calls[0][2]).toMatchObject({ password_hash: "<redacted>", token_hash: null });
    expect(write.mock.calls[0][2]).not.toHaveProperty("password");
  });
});
