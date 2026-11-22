import { describe, expect, it } from "vitest";
import { translate } from "./i18n.js";
import {
  catalogs,
  localeCodes,
  localeTag,
  locales,
  translationFallback,
} from "../locales/registry.js";

const placeholders = (text) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("language registry", () => {
  it("ships a catalog for every language it advertises", () => {
    for (const { code } of locales) {
      expect(Object.hasOwn(catalogs, code), code).toBe(true);
    }
    expect(localeCodes).toEqual(Object.keys(catalogs));
    expect(localeTag("ja")).toBe("ja-JP");
  });

  it("falls back to English, so a language can be added without stalling others", () => {
    expect(translationFallback).toBe("en");
    expect(Object.keys(catalogs.en).length).toBeGreaterThan(100);
  });
});

describe("catalogs", () => {
  it("agree on keys, values and placeholders", () => {
    const reference = catalogs.en;
    for (const [code, catalog] of Object.entries(catalogs)) {
      expect(Object.keys(catalog).sort(), `${code} keys`).toEqual(Object.keys(reference).sort());
      for (const [key, value] of Object.entries(catalog)) {
        expect(value.trim(), `${code}:${key} is empty`).not.toBe("");
        expect(placeholders(value), `${code}:${key} placeholders`).toEqual(
          placeholders(reference[key]),
        );
      }
    }
  });
});

describe("translate", () => {
  it("returns the active language and falls back to English", () => {
    expect(translate("zh", "action.save")).toBe(catalogs.zh["action.save"]);
    expect(translate("en", "action.save")).toBe(catalogs.en["action.save"]);
    expect(translate("ja", "action.save")).toBe(catalogs.ja["action.save"]);
  });

  it("substitutes named values and leaves what it cannot fill visible", () => {
    expect(translate("en", "config.lines", { count: 12 })).toBe("12 lines");
    expect(translate("en", "overview.manageNode", { id: "home" })).toBe("Manage home's services");
    expect(translate("en", "overview.manageNode")).toContain("{id}");
  });

  it("picks the plural form the language needs", () => {
    expect(translate("en", "overview.tunnelCount", { count: 1 })).toBe("1");
    expect(translate("zh", "overview.tunnelCount", { count: 2 })).toBe("2 条");
  });

  it("shows the key when no catalog has it, rather than nothing at all", () => {
    expect(translate("en", "no.such.key")).toBe("no.such.key");
  });
});
