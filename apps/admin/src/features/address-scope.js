// Address lists edited as presets or one rule per line. Presets only fill in
// the rule list; the server still validates every rule.

const LOCAL = ["127.0.0.0/8", "::1/128"];
const PRIVATE = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7"];

export const scopePresets = {
  device: LOCAL,
  lan: [...LOCAL, ...PRIVATE],
  private: PRIVATE,
};

// Rules typed by the user. Commas, semicolons and line breaks all separate rules.
export function parseRules(text) {
  return [
    ...new Set(
      String(text ?? "")
        .split(/[,;\n\r]+/)
        .map((rule) => rule.trim())
        .filter(Boolean),
    ),
  ];
}

const bare = (rule) => rule.replace(/^ip:/, "");
const same = (rules, preset) => {
  const set = new Set(rules.map(bare));
  return set.size === preset.length && preset.every((rule) => set.has(rule));
};

// The preset a rule list matches, or "custom". The single-address local rule
// written by earlier connection packages still reads as "this device".
export function presetOf(rules, presets) {
  const list = (rules ?? []).map(bare);
  if (!list.length) return presets.includes("any") ? "any" : "custom";
  for (const name of presets) {
    if (name === "any" || name === "custom") continue;
    if (same(list, scopePresets[name])) return name;
    if (name === "device" && same(list, ["127.0.0.1/32"])) return name;
    if (name === "lan" && same(list, ["127.0.0.1/32", ...PRIVATE])) return name;
  }
  return "custom";
}

// Destination rules keep the `ip:` spelling the configuration files use.
export const targetRule = (rule) =>
  /^(ip|domain|suffix|regex|geoip|geosite):/.test(rule) ||
  rule === "*" ||
  rule.startsWith("*.") ||
  !/^[0-9a-f:.]+(\/\d+)?$/i.test(rule)
    ? rule
    : `ip:${rule}`;

export const showRules = (rules) => (rules ?? []).map(bare).join("\n");

// Destination scope -> TargetPolicy. Deny rules and ports are preserved.
// An empty custom list would mean "any", so it stays an explicit error.
export function targetPolicy(choice, text, previous = {}) {
  if (choice === "custom" && !parseRules(text).length) throw new Error("scope.empty");
  const allow =
    choice === "any"
      ? []
      : (choice === "custom" ? parseRules(text) : scopePresets[choice]).map(targetRule);
  return { ...previous, allow };
}

// Source scope -> acl_mode/acl_rules. "any" turns filtering off.
export function sourceAcl(choice, text) {
  if (choice === "any") return { acl_mode: "off", acl_rules: "" };
  const rules = choice === "custom" ? parseRules(text) : scopePresets[choice];
  return { acl_mode: "allow", acl_rules: rules.join(", ") };
}

export function sourcePreset(mode, rules) {
  if (!mode || mode === "off") return parseRules(rules).length ? "custom" : "any";
  if (mode !== "allow") return "custom";
  return presetOf(parseRules(rules), ["device", "lan", "private"]);
}
