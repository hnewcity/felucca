import { describe, expect, it } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, existsSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectionPackage, preferredTarget } from "./connection-package.js";

describe("runnable connection packages", () => {
  it("does not preselect an incompatible operating system when the current device has no package", () => {
    const packages = [{ target: "aarch64-unknown-linux-gnu" }];
    expect(preferredTarget(packages, "MacIntel")).toBe("");
    expect(preferredTarget(packages, "Linux aarch64")).toBe(packages[0].target);
    expect(preferredTarget([], "Win32")).toBe("");
  });
  it("extracts with a standard ZIP reader, preserves credentials and Unix modes, and safely starts from a path with spaces", async () => {
    const root = mkdtempSync(join(tmpdir(), "felucca package "));
    try {
      const binary = new TextEncoder().encode("#!/bin/sh\nprintf '%s\\n' \"$@\" > arguments.txt\n");
      const config = { key_id: "example", vkey: "keep-this-exactly", server: "localhost:14028" };
      const name = "device'; touch INJECTED; echo '$(touch ALSO_INJECTED)";
      const blob = connectionPackage({
        config,
        binary,
        id: "felucca-test",
        target: "aarch64-apple-darwin",
        name,
        locale: "zh",
        t: (_key, args) => args?.name ?? "Package instructions",
      });
      const archive = join(root, "connection.zip");
      writeFileSync(archive, new Uint8Array(await blob.arrayBuffer()));
      const listing = JSON.parse(
        execFileSync(
          process.platform === "win32" ? "python" : "python3",
          [
            "-c",
            `
import json, sys, zipfile
from pathlib import Path
with zipfile.ZipFile(sys.argv[1]) as z:
    assert z.testzip() is None
    z.extractall(sys.argv[2])
    print(json.dumps({i.filename: (i.external_attr >> 16) & 0o777 for i in z.infolist()}))
`,
            archive,
            root,
          ],
          { encoding: "utf8" },
        ),
      );
      expect(listing["felucca-test/Connect.command"]).toBe(0o700);
      expect(listing["felucca-test/connection.json"]).toBe(0o600);
      const folder = join(root, "felucca-test");
      expect(JSON.parse(readFileSync(join(folder, "connection.json"), "utf8"))).toEqual(config);
      expect(readFileSync(join(folder, "felucca-agent"))).toEqual(Buffer.from(binary));
      if (process.platform !== "win32") {
        execFileSync("sh", ["-n", join(folder, "Install.command")]);
        execFileSync("sh", [join(folder, "Connect.command")], { cwd: root });
        expect(readFileSync(join(folder, "arguments.txt"), "utf8").split("\n")).toEqual([
          "--config",
          "./connection.json",
          "--lang",
          "zh",
          "",
        ]);
        expect(existsSync(join(folder, "INJECTED"))).toBe(false);
        expect(existsSync(join(folder, "ALSO_INJECTED"))).toBe(false);
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
