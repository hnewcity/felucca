import { schema } from "./config-model.js";

import { commaList, encode, omit, parseObject } from "../lib/format.js";
import { generateName } from "../lib/names.js";

const field = (key, label, value = "", options = {}) => ({ key, label, value, ...options });
const check = (key, label, value = false) => field(key, label, value, { type: "checkbox" });
const secret = (key, label, required) =>
  field(key, label, "", { type: "password", autoComplete: "new-password", required, wide: true });
const nameField = (row, label, t, generation) =>
  field("name", label ?? t("editor.name"), row?.id ?? generateName(generation.prefix, generation), {
    required: true,
    disabled: Boolean(row),
    nameGeneration: row ? undefined : generation,
  });
function name(value, t) {
  const result = value.trim();
  if (!result) throw new Error(t("editor.nameRequired"));
  return result;
}
// Titles and labels arrive already translated by the page that opens the editor.
export function jsonEditor(title, config, save, readOnly = false, t) {
  return {
    title,
    readOnly,
    fields: [
      field("json", t("editor.configuration"), JSON.stringify(config, null, 2), {
        type: "textarea",
      }),
    ],
    save: ({ json }) => save(parseObject(json, t)),
  };
}

export function resourceEditor(
  kind,
  row,
  principal,
  write,
  readOnly = false,
  t,
  rows = [],
  managed,
) {
  const value = structuredClone(row?.config ?? {});
  // Resource names also serve as stable configuration keys, so keep suggestions ASCII.
  const generation = {
    prefix: {
      clients: "device",
      users: "user",
      accounts: "account",
      locals: "local",
      shares: "share",
      platforms: "platform",
      webhooks: "webhook",
    }[kind],
    existingNames: rows.map((entry) => entry.id),
  };
  const label = (key) => t(`nav.${key}`);
  const save = (values, config) =>
    write(`/api/${kind}/${encode(name(values.name, t))}`, "PUT", config);
  const editor = {
    title: row
      ? t("editor.editTitle", { id: row.id })
      : t("editor.newTitle", { kind: label(kind) }),
    fields: [nameField(row, undefined, t, generation)],
    readOnly,
  };
  if (kind === "clients") {
    editor.fields.push(
      field("user", t("editor.user"), value.user ?? principal.user ?? "", {
        disabled: principal.role !== "admin",
      }),
      field("remark", t("flow.deviceName"), value.remark, {
        wide: true,
        nameGeneration: {
          prefix: t("names.device"),
          existingNames: rows.map((entry) => entry.config?.remark || entry.id),
        },
      }),
      secret("vkey", row ? t("editor.vkeyKeep") : t("editor.vkey"), !row),
      check("enabled", t("editor.enableNode"), value.enabled ?? true),
      check("push", t("editor.allowConfigPush"), value.allow_config_push ?? false),
    );
    // Destination scope sent with the managed configuration. The device must
    // run with remote.console_scope; otherwise it refuses the configuration.
    if (row && managed)
      editor.fields.push(
        field("scope", t("editor.consoleScope"), managed.target_policy ?? null, {
          type: "scope",
          hint: t("editor.consoleScopeHint"),
          scopeLabel: t("scope.label.target"),
          initial: { allow: [] },
        }),
      );
    // Identity and authentication are managed by the connection flow.
    editor.fields = editor.fields.map((field) =>
      ["name", "vkey", "push"].includes(field.key) ? { ...field, hidden: true } : field,
    );
    editor.extra = omit(value, ["user", "remark", "vkey", "enabled", "allow_config_push"]);
    editor.save = (v, extra) => {
      const client = {
        ...extra,
        user: v.user.trim() || null,
        remark: v.remark,
        vkey: v.vkey || value.vkey,
        enabled: v.enabled,
        allow_config_push: v.push,
      };
      if (!(row && managed)) return save(v, client);
      const id = encode(name(v.name, t));
      const next = { ...managed };
      if (v.scope) next.target_policy = v.scope;
      else delete next.target_policy;
      return write("/api/batch", "POST", {
        operations: [
          { method: "PUT", path: `/api/clients/${id}`, value: client },
          { method: "PUT", path: `/api/managed/${id}`, value: next },
        ],
      });
    };
  } else if (kind === "users") {
    editor.fields.push(
      field("config", t("editor.configuration"), value, {
        type: "schema",
        descriptor: "UserEntry",
        wide: true,
      }),
    );
    editor.save = (v) => save(v, v.config);
  } else if (kind === "accounts") {
    editor.fields.push(
      field("role", t("editor.role"), value.role ?? "viewer", {
        options: [
          ["admin", t("role.admin")],
          ["operator", t("role.operator")],
          ["viewer", t("role.viewer")],
        ],
      }),
      field("user", t("editor.userAdminBlank"), value.user),
      secret("password", row ? t("editor.passwordKeep") : t("editor.passwordNew"), !row),
      check("disabled", t("editor.disableAccount"), value.disabled ?? false),
    );
    editor.extra = omit(value, ["role", "user", "password_hash", "disabled"]);
    editor.save = (v, extra = editor.extra) =>
      save(v, {
        ...value,
        ...extra,
        role: v.role,
        user: v.user.trim() || null,
        disabled: v.disabled,
        ...(v.password ? { password: v.password } : {}),
      });
  } else if (kind === "locals") {
    editor.fields.push(
      field("config", t("editor.serviceConfig"), row ? value : { tunnel: [] }, {
        type: "schema",
        descriptor: "LocalConfig",
        wide: true,
      }),
    );
    editor.save = (v) => save(v, v.config);
  } else if (kind === "shares") {
    editor.fields = [
      nameField(row, t("editor.shareName"), t, generation),
      field("client", t("editor.shareClient"), value.client, { required: true }),
      field("tunnel", t("editor.tunnelId"), value.tunnel, {
        type: "number",
        min: 1,
        required: true,
      }),
      secret("secret", row ? t("editor.shareSecretKeep") : t("editor.shareSecret"), !row),
      field("clients", t("editor.allowClients"), (value.allow_clients ?? []).join(", "), {
        wide: true,
      }),
      check("dynamic", t("editor.allowDynamic"), value.dynamic ?? false),
      check("direct", t("editor.allowDirect"), value.direct ?? false),
    ];
    editor.extra = omit(value, [
      "client",
      "tunnel",
      "secret",
      "allow_clients",
      "dynamic",
      "direct",
    ]);
    editor.save = (v, extra) =>
      save(v, {
        ...extra,
        client: v.client.trim(),
        tunnel: Number(v.tunnel),
        secret: v.secret || value.secret,
        allow_clients: commaList(v.clients),
        dynamic: v.dynamic,
        direct: v.direct,
      });
  } else if (kind === "platforms") {
    editor.fields.push(
      field("user", t("editor.userFullBlank"), value.user ?? principal.user ?? ""),
      field("scope", t("editor.scope"), value.scope ?? "account", {
        options: [
          ["account", t("common.specifiedUser")],
          ["full", t("editor.scope.full")],
        ],
      }),
      field("mode", t("editor.platformMode"), value.mode ?? "direct", {
        options: [
          ["direct", t("editor.mode.direct")],
          ["reverse", t("editor.mode.reverse")],
          ["dual", t("editor.mode.dual")],
        ],
      }),
      secret("token", row ? t("editor.tokenKeep") : t("editor.token"), !row),
      field("reverse", t("editor.reverseUrl"), value.reverse_url, { wide: true }),
      check("enabled", t("action.enable"), value.enabled ?? true),
    );
    editor.extra = omit(value, ["user", "scope", "mode", "token", "reverse_url", "enabled"]);
    editor.save = (v, extra) =>
      save(v, {
        ...extra,
        scope: v.scope,
        user: v.scope === "full" ? null : v.user.trim(),
        mode: v.mode,
        token: v.token || value.token,
        reverse_url: v.reverse.trim() || null,
        enabled: v.enabled,
      });
  } else if (kind === "webhooks") {
    const delivery = value.delivery ?? {};
    editor.fields.push(
      field("user", t("editor.userFullBlank"), value.user ?? principal.user ?? "", {
        disabled: principal.role !== "admin",
      }),
      field("url", t("editor.url"), delivery.url, { required: true, wide: true }),
      secret("secret", row ? t("editor.signingSecretKeep") : t("editor.signingSecret"), !row),
      check("enabled", t("action.enable"), delivery.enabled ?? true),
    );
    editor.extra = omit(delivery, ["url", "signing_key", "enabled"]);
    editor.save = (v, extra) =>
      save(v, {
        ...value,
        user: v.user.trim() || null,
        delivery: {
          ...extra,
          url: v.url.trim(),
          signing_key: v.secret || delivery.signing_key,
          enabled: v.enabled,
        },
      });
  }
  if (editor.extra !== undefined) {
    const type = {
      clients: "ClientEntry",
      accounts: "AdminAccount",
      shares: "ShareConfig",
      platforms: "PlatformConfig",
      webhooks: "CallbackConfig",
    }[kind];
    const controlled = {
      clients: ["user", "remark", "vkey", "enabled", "allow_config_push"],
      accounts: ["role", "user", "password_hash", "disabled"],
      shares: ["client", "tunnel", "secret", "allow_clients", "dynamic", "direct"],
      platforms: ["user", "scope", "mode", "token", "reverse_url", "enabled"],
      webhooks: ["url", "signing_key", "enabled"],
    }[kind];
    editor.extraDescriptor = { type: "object", fields: omit(schema(type).fields, controlled) };
  }
  return editor;
}

export function tunnelEditor(tunnel, path, write, t, tunnels = []) {
  const value = structuredClone(
    tunnel ?? { mode: "tcp", enabled: true, bind: "0.0.0.0", backend: [], hosts: [], options: {} },
  );
  const generation = {
    prefix: t("names.service"),
    existingNames: tunnels.map((entry) => entry.remark),
  };
  return {
    title: tunnel ? t("editor.editTunnelTitle", { id: tunnel.id }) : t("tunnels.newTunnel"),
    fields: [
      field("id", t("table.id"), value.id, {
        type: "number",
        min: 1,
        required: true,
        disabled: Boolean(tunnel),
      }),
      field("mode", t("table.type"), value.mode, {
        options: [
          ["tcp", t("editor.mode.tcp")],
          ["udp", t("editor.mode.udp")],
          ["transparent", t("editor.mode.transparent")],
          ["http", t("editor.mode.http")],
          ["socks5", t("editor.mode.socks5")],
          ["httpproxy", t("editor.mode.httpproxy")],
          ["mixedproxy", t("editor.mode.mixedproxy")],
          ["file", t("editor.mode.file")],
        ],
      }),
      check("public", t("editor.publishPort"), value.public ?? true),
      check("enabled", t("action.enable"), value.enabled ?? true),
      field("port", t("editor.publicPort"), value.port, {
        type: "number",
        min: 1,
        max: 65535,
        disabled: (v) => !v.public,
        required: (v) => v.public,
      }),
      field("bind", t("editor.bindAddress"), value.bind ?? "0.0.0.0", {
        disabled: (v) => !v.public,
      }),
      field("backend", t("field.backend"), value.backend ?? [], {
        type: "schema",
        descriptor: { type: "array", item: { ref: "Backend" } },
        wide: true,
      }),
      field("hosts", t("editor.hosts"), (value.hosts ?? []).join(", "), { wide: true }),
      field(
        "remark",
        t("flow.serviceName"),
        tunnel ? value.remark : generateName(generation.prefix, generation),
        { wide: true, nameGeneration: generation },
      ),
    ],
    extraDescriptor: {
      type: "object",
      fields: omit(schema("TunnelConfig").fields, [
        "id",
        "mode",
        "public",
        "port",
        "bind",
        "backend",
        "hosts",
        "remark",
        "enabled",
      ]),
    },
    extra: omit(value, [
      "id",
      "mode",
      "public",
      "port",
      "bind",
      "backend",
      "hosts",
      "remark",
      "enabled",
    ]),
    save: (v, extra) => {
      const config = {
        ...extra,
        id: Number(v.id),
        mode: v.mode,
        public: v.public,
        port: v.public ? Number(v.port) : 0,
        bind: v.public ? v.bind || null : null,
        remark: v.remark,
        enabled: v.enabled,
        hosts: commaList(v.hosts),
        backend: structuredClone(extra.backend ?? v.backend),
      };
      if (config.mode === "file" && !config.file) throw new Error(t("editor.fileRootRequired"));
      return write(`${path}/tunnels/${config.id}`, "PUT", config);
    },
  };
}

export function structuredEditor(title, value, descriptor, save, t, readOnly = false) {
  return {
    title,
    readOnly,
    fields: [
      field("config", t("editor.configuration"), structuredClone(value), {
        type: "schema",
        descriptor,
        wide: true,
      }),
    ],
    save: (v) => save(v.config),
  };
}
