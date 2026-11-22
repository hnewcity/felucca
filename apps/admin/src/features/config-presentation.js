import { schema } from "./config-model.js";
import { presetOf, sourcePreset } from "./address-scope.js";

export const fieldLabels = {
  TunnelConfig: { remark: "flow.serviceName", mode: "flow.serviceKind" },
  PlatformConfig: { mode: "editor.platformMode" },
  ProxyAccept: { mode: "form.proxyAcceptance" },
  PortMapping: { mode: "form.transportProtocol" },
  HealthCheck: { type: "form.healthMethod" },
};

// Presentation only: grouping must never remove fields or rewrite stored values.
const groups = {
  TunnelConfig: {
    basics: ["remark", "mode", "enabled", "public"],
    connection: ["port", "bind", "hosts", "backend", "file"],
    access: ["acl_mode", "acl_rules", "target_policy"],
    advanced: ["balance", "health", "options", "id"],
  },
  PortMapping: {
    basics: ["remark", "mode", "server_ports", "target_host", "target_ports", "bind"],
    access: ["acl_mode", "acl_rules", "target_policy"],
    advanced: ["tls", "trust", "health", "options"],
  },
  LocalConfig: {
    basics: ["tunnel", "mapping"],
    access: ["target_policy", "acl_mode", "acl_rules"],
  },
  UserEntry: {
    basics: ["remark", "disabled"],
    access: ["allowed_ports", "acl_mode", "acl_rules", "target_policy"],
    advanced: ["limits", "quota"],
  },
  RouteOptions: {
    access: ["auth", "proxy_accept", "limits"],
    routing: ["http"],
    advanced: ["proxy_protocol", "payload", "transparent_tproxy"],
  },
  HttpProxyConfig: {
    connection: ["bind", "tls_bind", "http3_bind"],
    security: ["default_tls", "certificates", "acme", "force_https"],
    advanced: ["proxy_accept", "host_limit"],
  },
  AdminConfig: {
    connection: ["bind", "tls"],
    access: ["account", "session_ttl"],
    advanced: ["state_path", "profiling"],
  },
  BridgeConfig: {
    connection: ["bind", "tls"],
    advanced: ["quic_bind", "kcp_bind", "kcp", "mux", "strict_clock", "accept_backlog"],
  },
};

export function fieldGroups(type, spec, exclude = []) {
  const remaining = new Set(Object.keys(spec.fields).filter((key) => !exclude.includes(key)));
  if (!groups[type]) return [{ name: null, fields: [...remaining] }];
  const result = [];
  for (const [name, keys] of Object.entries(groups[type])) {
    const fields = keys.filter((key) => remaining.delete(key));
    if (fields.length) result.push({ name, fields });
  }
  if (remaining.size) {
    const advanced = result.find((group) => group.name === "advanced");
    if (advanced) advanced.fields.push(...remaining);
    else result.push({ name: "advanced", fields: [...remaining] });
  }
  return result;
}

export function fieldActive(type, key, value) {
  if (type === "ProxyAccept" && key === "trusted") return !!value?.mode && value.mode !== "off";
  if (type === "Backend" && key === "trust") return Boolean(value?.tls);
  if (type === "HealthCheck") {
    if (key !== "type" && (!value?.type || value.type === "none")) return false;
    if (["path", "expect_status"].includes(key) && value?.type !== "http") return false;
  }
  if (type === "TunnelConfig") {
    if (key === "file") return value?.mode === "file" || value?.file != null;
    if (key === "hosts")
      return ["http", "file"].includes(value?.mode) || Boolean(value?.hosts?.length);
    if (key === "backend")
      return ["tcp", "udp", "http"].includes(value?.mode) || Boolean(value?.backend?.length);
    if (["port", "bind"].includes(key)) return value?.public !== false;
  }
  return true;
}

const help = {
  ManagedConfig: { tunnel: "services", mapping: "mappings", target_policy: "deviceTargets" },
  LocalConfig: { tunnel: "services", mapping: "mappings" },
  TunnelConfig: {
    remark: "serviceName",
    mode: "serviceMode",
    public: "public",
    enabled: "enabled",
    id: "serviceId",
    port: "publicPort",
    bind: "publicBind",
    hosts: "hosts",
    backend: "backends",
    file: "file",
    balance: "balance",
    health: "health",
    target_policy: "serviceTargets",
    options: "routeOptions",
  },
  Backend: { addr: "backendAddress", weight: "weight", tls: "backendTls", trust: "trust" },
  PortMapping: {
    mode: "mappingMode",
    server_ports: "serverPorts",
    target_ports: "targetPorts",
    target_host: "targetHost",
    bind: "publicBind",
  },
  FileConfig: { root: "folder", prefix: "prefix", read_only: "readOnly", auto_index: "autoIndex" },
  HealthCheck: { type: "healthType", path: "healthPath", expect_status: "statusCodes" },
  TlsConfig: { cert: "cert", key: "key" },
};

const examples = {
  addr: "127.0.0.1:3000",
  bind: "127.0.0.1:8080",
  server: "server.example.com:14028",
  url: "https://example.com/callback",
  reverse_url: "wss://example.com/connect",
  path: "/srv/felucca",
  local_ip: "192.168.1.10",
  bind_ip: "192.168.1.10",
  target_host: "192.168.1.10",
  server_ports: "8000-8002",
  target_ports: "3000-3002",
  email: "admin@example.com",
  cert: "/etc/felucca/cert.pem",
  key: "/etc/felucca/key.pem",
};

export function fieldHelp(type, key, descriptor) {
  const spec = schema(descriptor);
  return {
    hint: help[type]?.[key],
    example:
      type === "NtpConfig" && key === "server"
        ? "time.example.com:123"
        : type === "TunnelConfig" && key === "bind"
          ? "0.0.0.0"
          : spec.type === "duration"
            ? "10s, 5m, 1h"
            : examples[key],
    multiline: ["body_template", "content", "header_templates"].includes(key),
  };
}

// Identical serialized values can mean different things in different selectors.
export function optionLabel(type, option, t) {
  if (type === "ConnectMode") return t(`editor.mode.${option}`);
  if (type === "AdminRole") return t(`role.${option}`);
  if (type === "TunnelMode") return t(`editor.mode.${option}`);
  if (type === "PlatformScope")
    return t(option === "full" ? "editor.scope.full" : "form.accountScope");
  return t(`option.${option}`);
}

export function collectionLabel(descriptor, caption, t) {
  const name = typeof descriptor === "string" ? descriptor : descriptor.ref;
  const label = name && t(`form.item.${name}`);
  return label && label !== `form.item.${name}` ? label : caption || t("form.entry");
}

export function groupPreview(type, group, value, t) {
  if (type === "TunnelConfig" && group.name === "access")
    return t("form.accessPreview", {
      source: t(`scope.source.${sourcePreset(value?.acl_mode, value?.acl_rules)}Label`),
      target: t(
        `scope.${presetOf(value?.target_policy?.allow, ["device", "lan", "any", "custom"])}`,
      ),
    });
  const configured = group.fields.filter((key) => {
    const item = value?.[key];
    return item != null && (typeof item !== "object" || Object.keys(item).length > 0);
  });
  // Only labels, never arbitrary field values: groups may contain credentials.
  return configured.length
    ? t("form.configuredPreview", {
        fields: configured.map((key) => t(fieldLabels[type]?.[key] ?? `field.${key}`)).join(" · "),
      })
    : t(`form.group.${group.name}Hint`);
}

export function collectionPreview(descriptor, value, t) {
  const type = typeof descriptor === "string" ? descriptor : descriptor.ref;
  if (type === "TunnelConfig")
    return {
      description: [
        value?.mode && optionLabel("TunnelMode", value.mode, t),
        value?.public === false
          ? t("form.privateEntry")
          : value?.port
            ? t("form.portPreview", { port: value.port })
            : null,
        ["tcp", "udp", "http"].includes(value?.mode)
          ? t("form.targetsPreview", { count: value?.backend?.length ?? 0 })
          : null,
      ]
        .filter(Boolean)
        .join(" · "),
      status: {
        label: t(value?.enabled === false ? "status.disabled" : "status.enabled"),
        tone: value?.enabled === false ? "" : "on",
      },
    };
  if (type === "Backend")
    return {
      description: [
        t("form.weightPreview", { weight: value?.weight ?? 1 }),
        value?.tls ? t("form.tlsOn") : null,
      ]
        .filter(Boolean)
        .join(" · "),
    };
  return {};
}

export function revealInvalidField(event) {
  for (let element = event.target.parentElement; element; element = element.parentElement)
    if (element.tagName === "DETAILS") element.open = true;
  const input = event.target;
  const first = input.form?.querySelector("input:invalid, textarea:invalid, select:invalid");
  if (first && first !== input) return;
  // Wait until opened sections have laid out; keep the first error above sticky actions.
  requestAnimationFrame(() => {
    if (input.isConnected && !input.validity?.valid) input.scrollIntoView({ block: "center" });
  });
}

export function inputError(input, t) {
  if (!input.validity) return "";
  if (input.validity.valid) return "";
  if (input.validity.valueMissing) return t("form.required");
  if (input.validity.rangeUnderflow || input.validity.rangeOverflow)
    return t("form.numberRange", { min: input.min || "0", max: input.max });
  if (input.validity.stepMismatch || input.validity.badInput) return t("form.integer");
  return t("form.invalid");
}

export function inputRequired(spec, key) {
  if (["number", "duration", "enum"].includes(spec.type)) return true;
  // Serde requires a field to exist, which does not always mean it must be nonempty.
  // Empty header values, for example, have a valid meaning and must remain editable.
  return (
    !spec.optional &&
    [
      "addr",
      "server",
      "root",
      "path",
      "share",
      "name",
      "url",
      "secret",
      "vkey",
      "cert",
      "key",
      "server_ports",
      "target_ports",
    ].includes(key)
  );
}
