import { describe, expect, it } from "vitest";
import { catalogs, replyText, translate } from "./i18n.js";

const placeholders = (text) =>
  [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("desktop catalogs", () => {
  it("cover the same keys and placeholders in every language", () => {
    const reference = catalogs.en;
    for (const [code, catalog] of Object.entries(catalogs)) {
      expect(Object.keys(catalog).sort(), code).toEqual(
        Object.keys(reference).sort(),
      );
      for (const [key, text] of Object.entries(catalog)) {
        expect(text.trim(), `${code} ${key}`).not.toBe("");
        expect(placeholders(text), `${code} ${key}`).toEqual(
          placeholders(reference[key]),
        );
      }
    }
  });
  it("cover every key the Rust side can reply with", async () => {
    const { readFileSync } = await import("node:fs");
    const rust = ["connections.rs", "main.rs"]
      .map((file) =>
        readFileSync(
          new URL(`../src-tauri/src/${file}`, import.meta.url),
          "utf8",
        ),
      )
      .join("\n");
    const keys = [...rust.matchAll(/Reply::Key\("([^"]+)"\)/g)].map(
      (m) => m[1],
    );
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) expect(catalogs.en, key).toHaveProperty(key);
  });
  it("renders keyed and pre-rendered replies", () => {
    expect(translate("zh", "action.removeConfirm", { name: "A" })).toContain(
      "A",
    );
    expect(replyText("en", { key: "error.duplicate" })).toBe(
      catalogs.en["error.duplicate"],
    );
    expect(replyText("en", { text: "server said no" })).toBe("server said no");
  });
});
