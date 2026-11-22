import { initialValue, schema, setField } from "./config-model.js";
import { presetOf } from "./address-scope.js";

export const agentConnectionFields = ["server", "key_id", "vkey", "transport", "trust"];
export const agentGuidedFields = [...agentConnectionFields, "remote", "visitor", "p2p", "ntp"];
export const serverSettings = [
  "bridge",
  "http",
  "allowed_ports",
  "acl_mode",
  "geo",
  "ntp",
  "state",
  "log",
  "admin",
  "node",
];
export const settingsGroups = [
  { id: "access", pages: ["users", "accounts", "locals"] },
  { id: "maintenance", pages: ["diagnostics", "config", "agent"] },
  { id: "integrations", pages: ["node", "platforms", "webhooks"] },
];

export function fieldsFor(type, keys) {
  return {
    type: "object",
    fields: Object.fromEntries(
      Object.entries(schema(type).fields).filter(([key]) => keys.includes(key)),
    ),
  };
}

export function patchSetting(config, key, patch, defaults = {}) {
  return setField(config, key, { ...defaults, ...config?.[key], ...patch });
}

export function setTimeSource(config, custom) {
  return setField(config, "ntp", custom ? (config.ntp ?? { server: "" }) : undefined);
}

// Use the actual serde defaults when a user starts customizing these fields.
export function settingInitialValue(type, key, descriptor) {
  const defaults = {
    TunnelConfig: { public: true, enabled: true, balance: "round_robin" },
    Backend: { weight: 1, tls: false },
    FileConfig: { read_only: true, auto_index: true, prefix: "/" },
    HealthCheck: {
      interval: "10s",
      timeout: "5s",
      failures_to_evict: 3,
      successes_to_restore: 2,
      path: "/",
    },
    NtpConfig: { interval: "5m", timeout: "5s", max_offset: "24h" },
    P2pAgentConfig: { max_sessions: 64 },
    RemoteFileRoot: { read_only: true },
    VisitorConfig: { max_connections: 64, udp_idle: "60s" },
    AgentConfig: {
      transport: "tls",
      ws_path: "/ws",
      reconnect_min: "1s",
      reconnect_max: "60s",
      dial_timeout: "10s",
    },
  };
  return defaults[type]?.[key] ?? initialValue(descriptor);
}

export function remoteSummary(remote) {
  if (!remote?.enabled) return "guide.remote.off";
  const policy = remote.target_policy;
  if (Object.keys(remote.file_root ?? {}).length) return "guide.remote.files";
  if (
    presetOf(policy?.allow, ["device"]) === "device" &&
    !policy.deny?.length &&
    !policy.ports?.length
  )
    return "guide.remote.local";
  if (presetOf(policy?.allow, ["lan"]) === "lan" && !policy.deny?.length && !policy.ports?.length)
    return "guide.remote.lan";
  if (!policy?.allow?.length && !policy?.deny?.length && !policy?.ports?.length)
    return "guide.remote.unrestricted";
  return "guide.remote.custom";
}

// Context matters: "server" is a time source in NtpConfig, not the Felucca server.
export const fieldGuidance = {
  AgentConfig: {
    server: "server",
    key_id: "deviceIdentity",
    vkey: "deviceSecret",
    transport: "transport",
    trust: "trust",
  },
  RemotePolicy: {
    enabled: "remoteEnabled",
    console_scope: "consoleScope",
    target_policy: "targets",
    file_root: "folders",
  },
  TargetPolicy: { allow: "allow", deny: "deny", ports: "targetPorts" },
  RemoteFileRoot: { path: "folderPath", read_only: "readOnly" },
  VisitorConfig: {
    name: "entryName",
    share: "share",
    secret: "shareSecret",
    bind: "visitorBind",
    connection: "connectionMode",
    target: "visitorTarget",
    mode: "visitorMode",
  },
  P2pAgentConfig: { enabled: "directEnabled", bind_ip: "directIp", max_sessions: "directLimit" },
  NtpConfig: {
    server: "timeServer",
    interval: "timeInterval",
    timeout: "timeTimeout",
    max_offset: "timeOffset",
  },
  NetworkConfig: {
    local_ip: "sourceIp",
    dns: "dns",
    proxy: "proxy",
    forward_local_ip: "forwardIp",
  },
  BridgeConfig: { bind: "bridgeBind", strict_clock: "strictClock", tls: "bridgeTls" },
};
