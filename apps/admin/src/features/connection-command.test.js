import { describe, expect, it } from "vitest";
import { connectionCommand } from "./connection-command.js";
import { bridgeAddress } from "./product-flow.js";

describe("one-command connection", () => {
  const config = {
    key_id: "device-one",
    vkey: "sample-key",
    server: "server.test:14028",
    remote: {
      file_root: { shared: { path: "/照片/it's here/$(touch unexpected)", read_only: true } },
    },
  };
  it("carries Unicode paths and shell characters as data, including local grants", () => {
    const command = connectionCommand(config, "https://server.test", "unix", "zh");
    expect(command).toContain("https://server.test/connect.sh");
    expect(command).not.toContain("$(touch");
    expect(command).not.toContain("照片");
    const payload = command.split("'").at(-4);
    expect(
      JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(payload), (c) => c.charCodeAt(0)))),
    ).toEqual(config);
    expect(command).not.toContain("sudo");
  });
  it("uses PowerShell syntax and escapes credentials without executing them", () => {
    const command = connectionCommand(
      { ...config, vkey: "it's $safe" },
      "https://server.test",
      "windows",
    );
    expect(command).toContain("-Token 'it''s $safe'");
    expect(command).toContain("connect.ps1");
    expect(command).not.toContain("ExecutionPolicy");
  });
  it("accepts externally mapped bridge ports without changing ordinary addresses", () => {
    expect(bridgeAddress("server.test:4443", 14028)).toBe("server.test:4443");
    expect(bridgeAddress("[2001:db8::1]:4443", 14028)).toBe("[2001:db8::1]:4443");
    expect(bridgeAddress("2001:db8::1", 14028)).toBe("[2001:db8::1]:14028");
    expect(bridgeAddress("server.test", 14028)).toBe("server.test:14028");
    expect(() => connectionCommand(config, "https://user:password@server.test")).toThrow();
  });
});
