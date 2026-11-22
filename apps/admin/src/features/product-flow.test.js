import { describe, expect, it } from "vitest";
import {
  buildService,
  deviceConfig,
  launchCommand,
  launchLink,
  deviceOperations,
  nextServiceId,
  serverAddress,
  serviceDraft,
  serviceDevices,
  suggestPort,
  visitConfig,
} from "./product-flow.js";

const launch = { bridge: { bind: "[::]:14028" }, fingerprint: "a".repeat(64) };
describe("guided product flows", () => {
  it("does not offer a receiving-only device as a service provider", () => {
    const clients = [
      { id: "receiver", managed: false },
      { id: "legacy", managed: false },
      { id: "provider", managed: true },
    ];
    const shares = [{ config: { allow_clients: ["receiver", "provider"] } }];
    expect(serviceDevices(clients, shares).map((row) => row.id)).toEqual(["provider", "legacy"]);
    expect(clients[0].id).toBe("receiver");
  });
  it("creates an identity and centrally managed device in one transaction", () => {
    const operations = deviceOperations("device-one", "key", " My computer ", "team");
    expect(operations.map((item) => item.path)).toEqual([
      "/api/clients/device-one",
      "/api/managed/device-one",
    ]);
    expect(operations[0].value).toMatchObject({
      vkey: "key",
      user: "team",
      remark: "My computer",
      allow_config_push: false,
    });
    expect(operations[1].value).toEqual({ tunnel: [] });
    const scoped = deviceOperations("device-one", "key", "PC", null, { allow: ["ip:10.0.0.0/8"] });
    expect(scoped[1].value).toEqual({ tunnel: [], target_policy: { allow: ["ip:10.0.0.0/8"] } });
  });
  it("pins the server and allows all networks unless an explicit restriction is selected", () => {
    const base = { id: "one", key: "secret", host: "2001:db8::1", launch };
    const local = deviceConfig(base);
    expect(local.server).toBe("[2001:db8::1]:14028");
    expect(local.trust).toEqual({ mode: "pinned", fingerprint: launch.fingerprint });
    expect(local.remote.target_policy.allow).toEqual([]);
    expect(deviceConfig({ ...base, scope: "device" }).remote.target_policy.allow).toEqual([
      "ip:127.0.0.0/8",
      "ip:::1/128",
    ]);
    expect(local.remote.file_root).toEqual({});
    expect(local.remote.console_scope).toBe(true);
    expect(deviceConfig({ ...base, scope: "lan", folder: "/documents" }).remote).toMatchObject({
      file_root: { shared: { path: "/documents", read_only: true } },
      target_policy: { allow: expect.arrayContaining(["ip:192.168.0.0/16"]) },
    });
    expect(deviceConfig({ ...base, scope: "any" }).remote.target_policy.allow).toEqual([]);
    expect(
      deviceConfig({ ...base, scope: "custom", rules: "10.1.0.0/16\nnas.local" }).remote
        .target_policy.allow,
    ).toEqual(["ip:10.1.0.0/16", "nas.local"]);
    expect(() => deviceConfig({ ...base, scope: "custom", rules: " " })).toThrow("scope.empty");
    const command = launchCommand(local, "x86_64-unknown-linux-musl");
    const uri = new URL(command.match(/'(.+)'/)[1]);
    expect(uri.protocol).toBe("felucca:");
    expect(uri.searchParams.get("remote")).toBe("1");
    expect(uri.searchParams.get("vkey")).toBe("secret");
    expect(uri.searchParams.get("fingerprint")).toBe(launch.fingerprint);
    expect(launchCommand(local, "x86_64-pc-windows-msvc")).toMatch(
      /^felucca-agent\.exe --config "/,
    );
    // Folders are local grants that a URI cannot carry; the range comes from the server.
    expect(launchCommand(deviceConfig({ ...base, folder: "/documents" }))).toBeNull();
    expect(launchCommand(deviceConfig({ ...base, scope: "lan" }))).toMatch(/remote=1/);
    // The desktop client opens the same URI the command carries.
    expect(command).toContain(launchLink(local));
    expect(launchLink(deviceConfig({ ...base, folder: "/documents" }))).toBeNull();
  });
  it("does not ask users to choose IDs and avoids known public port collisions", () => {
    const tunnels = [
      { id: 1, port: 18080 },
      { id: 3, port: 18081 },
      { id: 4, port: 18082, public: false },
    ];
    expect(nextServiceId(tunnels)).toBe(2);
    expect(suggestPort(tunnels)).toBe(18082);
  });
  it("turns a web-app task into a port service or a domain route", () => {
    const draft = { ...serviceDraft(), name: "NAS", target: "127.0.0.1:5000" };
    expect(buildService(draft, undefined, 1)).toMatchObject({
      mode: "tcp",
      backend: [{ addr: "127.0.0.1:5000" }],
      hosts: [],
    });
    expect(buildService({ ...draft, domain: "nas.example.com" }, undefined, 2)).toMatchObject({
      mode: "http",
      hosts: ["nas.example.com"],
    });
  });
  it("retains existing advanced settings while editing a name or target", () => {
    const original = {
      id: 4,
      mode: "tcp",
      remark: "SSH",
      public: true,
      enabled: true,
      port: 8022,
      bind: "127.0.0.1",
      backend: [
        {
          addr: "old:22",
          weight: 7,
          tls: true,
          trust: { mode: "pinned", fingerprint: "b".repeat(64) },
        },
        { addr: "backup:22", weight: 2 },
      ],
      balance: "least_conn",
      options: { auth: { users: { alice: "<redacted>" } } },
    };
    const result = buildService(
      { ...serviceDraft(original), name: "Office", target: "new:22" },
      original,
    );
    expect(result).toEqual({
      ...original,
      remark: "Office",
      hosts: [],
      backend: [{ ...original.backend[0], addr: "new:22" }, original.backend[1]],
    });
    expect(original.backend[0].addr).toBe("old:22");
  });
  it("never leaves a public listener on a private service", () => {
    const result = buildService(
      { ...serviceDraft(), name: "SSH", kind: "ssh", target: "127.0.0.1:22", access: "private" },
      undefined,
      9,
    );
    expect(result).toMatchObject({
      id: 9,
      mode: "tcp",
      public: false,
      port: 0,
      bind: null,
      hosts: [],
    });
    expect(
      visitConfig({ id: "share-one", config: { secret: "s", direct: true } }, result, 9000),
    ).toMatchObject({ mode: "tcp", connection: "prefer_direct", bind: "127.0.0.1:9000" });
  });
  it("preserves UDP boundaries and lets the server choose relay when direct is unavailable", () => {
    expect(
      visitConfig({ id: "share", config: { secret: "s", direct: false } }, { mode: "udp" }, 9001),
    ).toMatchObject({ mode: "udp", connection: "prefer_direct" });
  });
  it("rejects incomplete tasks before any server write", () => {
    expect(() => serverAddress("https://server:80/path", 14028)).toThrow();
    expect(() => serverAddress("server:8088", 14028)).toThrow("flow.hostInvalid");
    expect(() =>
      visitConfig({ id: "share", config: { secret: "s" } }, { mode: "tcp" }, 65536),
    ).toThrow("flow.portInvalid");
    expect(() => buildService({ ...serviceDraft(), name: "" }, undefined, 1)).toThrow(
      "flow.nameRequired",
    );
    expect(() => buildService({ ...serviceDraft(), name: "x", port: 65536 }, undefined, 1)).toThrow(
      "flow.portInvalid",
    );
    expect(() =>
      buildService(
        { ...serviceDraft(), name: "x", kind: "socks5", access: "private" },
        undefined,
        1,
      ),
    ).toThrow("flow.privateMode");
  });
});
