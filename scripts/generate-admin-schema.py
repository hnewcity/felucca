#!/usr/bin/env python3
"""Generate console field metadata from the serialized core configuration model.

This is not a validator: Rust remains authoritative. --check detects model drift.
Defaults are left absent unless the operator sets a field, preserving Rust defaults.
"""
import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "apps/admin/src/features/config-schema.json"


def generate():
    paths = sorted((ROOT / "crates/felucca-core/src/config").glob("*.rs"))
    paths += [ROOT / f"crates/felucca-core/src/{name}.rs"
              for name in ["acl", "access", "traffic", "geodata"]]
    schemas = {}
    for path in paths:
        text = path.read_text()
        for match in re.finditer(r"pub struct (\w+)\s*\{(.*?)\n\}", text, re.S):
            fields = {}
            previous = 0
            for field in re.finditer(r"    pub (\w+): ([^\n]+),", match[2]):
                attrs = match[2][previous:field.start()]
                previous = field.end()
                rename = re.search(r'\brename = "([^"]+)"', attrs)
                key = rename[1] if rename else field[1]
                descriptor = type_of(field[2])
                descriptor["optional"] = "default" in attrs or descriptor.pop("nullable", False)
                fields[key] = descriptor
            if fields:
                schemas[match[1]] = {"type": "object", "fields": fields}
        for match in re.finditer(r"pub enum (\w+)\s*\{(.*?)\n\}", text, re.S):
            body = match[2]
            if "{" in body or re.search(r"^\s+\w+\(", body, re.M):
                continue
            before = text[max(0, match.start()-200):match.start()]
            snake = 'rename_all = "snake_case"' in before
            options, previous = [], 0
            for variant in re.finditer(r"^    (\w+),", body, re.M):
                attrs = body[previous:variant.start()]
                previous = variant.end()
                rename = re.search(r'\brename = "([^"]+)"', attrs)
                name = variant[1]
                options.append(rename[1] if rename else
                               re.sub(r"(?<!^)([A-Z])", r"_\1", name).lower() if snake
                               else name.lower())
            if options:
                schemas[match[1]] = {"type": "enum", "options": options}
    schemas["TrustPolicy"] = {"type": "trust"}
    schemas["PortRange"] = {"type": "portRange"}
    # Only include types reachable from operator-owned configurations.
    used = {}

    def visit(desc):
        if "ref" in desc:
            ref = desc["ref"]
            if ref not in used:
                assert ref in schemas, ref
                used[ref] = schemas[ref]
                visit(schemas[ref])
        for child in desc.get("fields", {}).values():
            visit(child)
        if "item" in desc:
            visit(desc["item"])

    visit({"ref": "ServerConfig"})
    visit({"ref": "AgentConfig"})
    return dict(sorted(used.items()))


def type_of(value):
    if value.startswith("Option<"):
        return {**type_of(value[7:-1]), "nullable": True}
    if value.startswith("Vec<"):
        return {"type": "array", "item": type_of(value[4:-1])}
    if value.startswith(("HashMap<", "BTreeMap<")):
        return {"type": "map", "item": type_of(value.split(", ", 1)[1][:-1])}
    name = value.split("::")[-1]
    if name == "bool":
        return {"type": "boolean"}
    if re.fullmatch(r"[ui](?:8|16|32|64|size)", name):
        # JavaScript cannot represent every u64; the editor refuses unsafe input.
        maximum = min(2 ** int(name[1:]) - 1, 9007199254740991) if name[1:].isdigit() else 9007199254740991
        return {"type": "number", "min": 0 if name.startswith("u") else -9007199254740991, "max": maximum}
    if name in ["String", "PathBuf", "IpAddr", "SocketAddr", "Duration", "Secret", "Value"]:
        return {"type": {"Duration": "duration", "Secret": "secret", "Value": "json"}.get(name, "string")}
    return {"ref": name}


if __name__ == "__main__":
    args = argparse.ArgumentParser()
    args.add_argument("--check", action="store_true")
    options = args.parse_args()
    content = json.dumps(generate(), ensure_ascii=False, indent=2) + "\n"
    if options.check:
        assert OUTPUT.read_text() == content, "Run python3 scripts/generate-admin-schema.py"
    else:
        OUTPUT.write_text(content)
    print("Console configuration schema matches the Rust model.")
