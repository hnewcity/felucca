import schemas from "./config-schema.json" with { type: "json" };

export const schema = (descriptor) =>
  typeof descriptor === "string"
    ? schemas[descriptor]
    : descriptor.ref
      ? schemas[descriptor.ref]
      : descriptor;

const seeds = {
  Backend: { addr: "", weight: 1, tls: false, trust: { mode: "webpki" } },
  TunnelConfig: { mode: "tcp", public: true, enabled: true, port: 1, backend: [], hosts: [] },
  PortMapping: { mode: "tcp", server_ports: "", target_ports: "", target_host: "127.0.0.1" },
  FileConfig: { root: "", read_only: true, auto_index: false },
  VisitorConfig: {
    name: "",
    share: "",
    secret: "",
    bind: "127.0.0.1:18080",
    mode: "tcp",
    connection: "relay",
  },
  RemoteFileRoot: { path: "", read_only: true },
  P2pAgentConfig: { enabled: false, max_sessions: 64 },
};

export function initialValue(descriptor) {
  const name = typeof descriptor === "string" ? descriptor : descriptor.ref;
  if (seeds[name]) return structuredClone(seeds[name]);
  const spec = schema(descriptor);
  if (spec.type === "object")
    return Object.fromEntries(
      Object.entries(spec.fields)
        .filter(([, child]) => !child.optional)
        .map(([key, child]) => [key, initialValue(child)]),
    );
  if (spec.type === "array") return [];
  if (spec.type === "map" || spec.type === "json") return {};
  if (spec.type === "trust") return { mode: "webpki" };
  if (spec.type === "portRange") return { from: 1025, to: 65535 };
  if (spec.type === "boolean") return false;
  if (spec.type === "number") return 0;
  if (spec.type === "enum") return spec.options[0];
  return "";
}

export function setField(value, key, next) {
  const entries = Object.entries(value ?? {}).filter(([name]) => name !== key);
  if (next !== undefined) entries.push([key, next]);
  return Object.fromEntries(entries);
}
