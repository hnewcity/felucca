import { targetPolicy } from "./address-scope.js";
// User tasks are mapped to the existing protocol/configuration at this boundary.
// Forms never reconstruct the fields they do not own.
export const deviceName = (row) => row?.config?.remark || row?.remark || row?.id || "";
export const serviceName = (tunnel) =>
  tunnel.remark || `${tunnel.mode.toUpperCase()} ${tunnel.port || tunnel.id}`;
export function serviceDevices(clients, shares = []) {
  const receiving = new Set(shares.flatMap((share) => share.config.allow_clients));
  return clients
    .filter((client) => client.managed || !receiving.has(client.id))
    .sort((a, b) => Number(Boolean(b.managed)) - Number(Boolean(a.managed)));
}
export function randomKey() {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}
export const newIdentity = (prefix) => `${prefix}-${randomKey().slice(0, 16)}`;
export function nextServiceId(tunnels) {
  const used = new Set(tunnels.map((tunnel) => tunnel.id));
  let id = 1;
  while (used.has(id)) id++;
  return id;
}
export function suggestPort(tunnels, start = 18080) {
  const used = new Set(
    tunnels.filter((tunnel) => tunnel.public !== false).map((tunnel) => tunnel.port),
  );
  for (let port = start; port <= 65535; port++) if (!used.has(port)) return port;
  return "";
}
export function serverAddress(host, port) {
  const clean = host.trim().replace(/^\[|\]$/g, "");
  if (!clean || /[\s/@?#\\]/.test(clean) || clean.includes("://"))
    throw new Error("flow.hostInvalid");
  try {
    new URL(`http://${clean.includes(":") ? `[${clean}]` : clean}`);
  } catch {
    throw new Error("flow.hostInvalid");
  }
  if (!Number.isInteger(Number(port)) || Number(port) < 1 || Number(port) > 65535)
    throw new Error("flow.portInvalid");
  return `${clean.includes(":") ? `[${clean}]` : clean}:${port}`;
}
export function bridgeAddress(host, port) {
  const clean = host.trim();
  if (/^\[[^\]]+\]:\d+$/.test(clean) || /^[^:\s]+:\d+$/.test(clean)) {
    const split = clean.lastIndexOf(":");
    return serverAddress(clean.slice(0, split), clean.slice(split + 1));
  }
  return serverAddress(host, port);
}
export function deviceConfig({
  id,
  key,
  host,
  launch,
  scope = "any",
  rules = "",
  folder = "",
  visitor,
}) {
  const port = launch.bridge.bind.split(":").at(-1);
  return {
    server: bridgeAddress(host, port),
    key_id: id,
    vkey: key,
    transport: "tls",
    trust: { mode: "pinned", fingerprint: launch.fingerprint },
    p2p: { enabled: true },
    remote: {
      enabled: true,
      // The console may later widen or narrow this from the device editor.
      console_scope: true,
      target_policy: targetPolicy(scope, rules),
      file_root: folder.trim()
        ? {
            shared: { path: folder.trim(), read_only: true },
          }
        : {},
    },
    visitor: visitor ? [visitor] : [],
  };
}
// The chosen range also goes to the server, so later commands and reconnects
// keep explicit restrictions instead of reverting to unrestricted defaults.
export function deviceOperations(id, key, name, user = null, targetPolicy) {
  return [
    {
      method: "PUT",
      path: `/api/clients/${encodeURIComponent(id)}`,
      value: {
        user,
        remark: name.trim(),
        vkey: key,
        enabled: true,
        allow_config_push: false,
      },
    },
    {
      method: "PUT",
      path: `/api/managed/${encodeURIComponent(id)}`,
      value: targetPolicy ? { tunnel: [], target_policy: targetPolicy } : { tunnel: [] },
    },
  ];
}
export function serviceDraft(tunnel, tunnels = []) {
  return {
    name: tunnel?.remark ?? "",
    kind: tunnel?.mode ?? "website",
    target: tunnel?.backend?.[0]?.addr ?? "127.0.0.1:3000",
    domain: tunnel?.hosts?.[0] ?? "",
    access: tunnel?.public === false ? "private" : "public",
    port: tunnel?.port || suggestPort(tunnels),
    bind: tunnel?.bind ?? "0.0.0.0",
    folder: tunnel?.file?.root ?? "shared",
    enabled: tunnel?.enabled ?? true,
  };
}
export function buildService(draft, original, id) {
  const next = structuredClone(original ?? {});
  const mode =
    {
      website: draft.domain.trim() && draft.access === "public" ? "http" : "tcp",
      ssh: "tcp",
      rdp: "tcp",
    }[draft.kind] ?? draft.kind;
  const publicAccess = draft.access === "public";
  if (
    publicAccess &&
    (!Number.isInteger(Number(draft.port)) || Number(draft.port) < 1 || Number(draft.port) > 65535)
  )
    throw new Error("flow.portInvalid");
  if (!draft.name.trim()) throw new Error("flow.nameRequired");
  const hasTarget = ["tcp", "udp", "http"].includes(mode);
  if (hasTarget && (!draft.target.trim() || !/:\d+$/.test(draft.target.trim())))
    throw new Error("flow.targetInvalid");
  if (mode === "http" && (!draft.domain.trim() || !publicAccess))
    throw new Error("flow.domainRequired");
  if (!publicAccess && !["tcp", "udp", "file"].includes(mode)) throw new Error("flow.privateMode");
  // A mode change must not leave stale, incompatible protocol options behind.
  if (original && original.mode !== mode) throw new Error("flow.modeFixed");
  Object.assign(next, {
    id: original?.id ?? id,
    mode,
    remark: draft.name.trim(),
    enabled: draft.enabled,
    public: publicAccess,
    port: publicAccess ? Number(draft.port) : 0,
    bind: publicAccess ? draft.bind : null,
  });
  next.hosts =
    mode === "http"
      ? [draft.domain.trim(), ...(original?.hosts?.slice(1) ?? [])]
      : (original?.hosts ?? []);
  if (!publicAccess) next.hosts = [];
  next.backend = hasTarget
    ? [
        { ...(original?.backend?.[0] ?? {}), addr: draft.target.trim() },
        ...(original?.backend?.slice(1) ?? []),
      ]
    : [];
  if (mode === "file")
    next.file = {
      ...(original?.file ?? {}),
      root: draft.folder.trim(),
      read_only: original?.file?.read_only ?? true,
      auto_index: original?.file?.auto_index ?? true,
    };
  return next;
}
export function visitConfig(share, service, port, lan = false) {
  serverAddress("127.0.0.1", port);
  return {
    name: share.id,
    share: share.id,
    secret: share.config.secret,
    bind: `${lan ? "0.0.0.0" : "127.0.0.1"}:${Number(port)}`,
    mode: service.mode === "udp" ? "udp" : "tcp",
    // The server's current permission decides whether direct is offered.
    // A later permission change must not require editing the downloaded file.
    connection: "prefer_direct",
  };
}
export function serviceEndpoint(service, host, launch = {}) {
  if (service.public === false) return null;
  const endpoint = serverAddress(host, service.port);
  if (service.mode === "http")
    return `${Number(launch.https_bind?.split(":").at(-1)) === service.port ? "https" : "http"}://${service.hosts[0]}:${service.port}`;
  if (service.mode === "file") return `http://${endpoint}${service.file?.prefix || "/"}`;
  return endpoint;
}

// One command for a device whose services are all managed by the server.
// Shared folders and visitor entries are local grants, so they need a file.
// The access range comes from the server's managed configuration.
export function launchLink(config) {
  const remote = config.remote ?? {};
  if (Object.keys(remote.file_root ?? {}).length || config.visitor?.length) return null;
  const query = new URLSearchParams({
    key_id: config.key_id,
    vkey: config.vkey,
    transport: config.transport,
    fingerprint: config.trust.fingerprint,
    remote: "1",
  });
  return `felucca://${config.server}?${query}`;
}
export function launchCommand(config, target = "") {
  const uri = launchLink(config);
  if (!uri) return null;
  const binary = target.includes("windows") ? "felucca-agent.exe" : "./felucca-agent";
  return target.includes("windows") ? `${binary} --config "${uri}"` : `${binary} --config '${uri}'`;
}
