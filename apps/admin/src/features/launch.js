export function agentTemplate(client, launch, hostname) {
  const port = launch.bridge.bind.split(":").at(-1);
  const host = hostname.includes(":") && !hostname.startsWith("[") ? `[${hostname}]` : hostname;
  return {
    server: `${host}:${port}`,
    key_id: client.id,
    vkey: "",
    transport: "tls",
    trust: { mode: "pinned", fingerprint: launch.fingerprint },
    remote: {
      enabled: true,
      console_scope: true,
      target_policy: { allow: [] },
      file_root: {},
    },
    visitor: [],
  };
}
export function visitorTemplate(share) {
  return {
    name: share.id,
    share: share.id,
    secret: "",
    bind: "127.0.0.1:18080",
    mode: "tcp",
    connection: share.config.direct ? "prefer_direct" : "relay",
  };
}
export function hasMissingCredentials(value) {
  if (Array.isArray(value)) return value.some(hasMissingCredentials);
  if (!value || typeof value !== "object") return false;
  return Object.entries(value).some(
    ([key, entry]) =>
      (["vkey", "secret"].includes(key) && (!entry || entry === "<redacted>")) ||
      hasMissingCredentials(entry),
  );
}
