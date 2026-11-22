#!/usr/bin/env python3
"""Check that every Felucca language stays complete and consistent.

The Rust crates, the console and the manual each keep their own copy of the
language list. This script is the one place that compares them, so a language
added on one surface and forgotten on another fails loudly instead of quietly
falling back to English.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST_CATALOGS = ROOT / "crates/felucca-i18n/catalogs"
CONSOLE_LOCALES = ROOT / "apps/admin/src/locales"
MANUAL = ROOT / "docs/manual"
CONSOLE_SOURCE = ROOT / "apps/admin/src"

PLACEHOLDER = re.compile(r"\{(\w+)\}")


def placeholders(text):
    return sorted(PLACEHOLDER.findall(text))


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def catalogs(directory):
    return {path.stem: load(path) for path in sorted(directory.glob("*.json"))}


def console_codes():
    registry = (CONSOLE_LOCALES / "registry.js").read_text(encoding="utf-8")
    codes = re.findall(r'code:\s*"([a-z]{2})"', registry)
    return codes


def check_language_lists(rust, console, problems):
    if set(rust) != set(console):
        problems.append(
            "language lists differ: rust-only "
            f"{sorted(set(rust) - set(console))}, console-only "
            f"{sorted(set(console) - set(rust))}"
        )
    if "en" not in console:
        problems.append("the console has no English catalog to fall back to")
    for directory, name in ((MANUAL, "manual"),):
        found = sorted(p.name for p in directory.iterdir() if p.is_dir())
        if found != sorted(console):
            problems.append(f"{name} languages {found} differ from the console {sorted(console)}")


def check_catalog(name, entries, reference, problems):
    missing = sorted(set(reference) - set(entries))
    extra = sorted(set(entries) - set(reference))
    if missing:
        problems.append(f"{name} is missing {len(missing)} keys, e.g. {missing[:3]}")
    if extra:
        problems.append(f"{name} has {len(extra)} keys no other language uses, e.g. {extra[:3]}")
    for key, value in entries.items():
        if not isinstance(value, str) or not value.strip():
            problems.append(f"{name} has an empty value for {key!r}")
        elif key in reference and placeholders(value) != placeholders(reference[key]):
            problems.append(
                f"{name} changes the placeholders of {key!r}: "
                f"{placeholders(value)} instead of {placeholders(reference[key])}"
            )


def check_manual(problems):
    """Every language carries the same twenty pages, so a link never dead-ends."""
    reference = sorted(path.name for path in (MANUAL / "en").iterdir())
    for language in sorted(path.name for path in MANUAL.iterdir() if path.is_dir()):
        found = sorted(path.name for path in (MANUAL / language).iterdir())
        if found != reference:
            missing = sorted(set(reference) - set(found))
            extra = sorted(set(found) - set(reference))
            problems.append(
                f"manual/{language} does not match manual/en: missing {missing}, extra {extra}"
            )
            continue
        if language == "zh":
            # Chinese is the sibling reference, not a translation of English: its
            # examples and ports are chosen independently.
            continue
        for page in reference:
            if page.endswith(".md"):
                compare_page(language, page, problems)


FENCE = re.compile(r"^```.*$", re.MULTILINE)
HEADING = re.compile(r"^(#{1,6})\s", re.MULTILINE)
CODE_SPAN = re.compile(r"`[^`\n]+`")
LINK = re.compile(r"\]\(([^)\s]+)")
ROW = re.compile(r"^\s*\|.*\|\s*$", re.MULTILINE)


def strip_switcher(text):
    """Drops the language bar, whose links are the one thing each page differs in."""
    return "\n".join(
        line
        for line in text.splitlines()
        if not (line.count("](") >= 4 and "README.md)" in line)
    )


def shape(text):
    """The parts of a page a translation must not change."""
    text = strip_switcher(text)
    blocks = FENCE.findall(text)
    fences = FENCE.split(text)
    code = [fences[index] for index in range(1, len(fences), 2)]
    return {
        "headings": HEADING.findall(text),
        "code": code,
        "fences": blocks,
        "spans": sorted(CODE_SPAN.findall(text)),
        "links": sorted(LINK.findall(text)),
        "rows": [row.count("|") for row in ROW.findall(text)],
    }


def compare_page(language, page, problems):
    reference = shape((MANUAL / "en" / page).read_text(encoding="utf-8"))
    found = shape((MANUAL / language / page).read_text(encoding="utf-8"))
    for key, expected in reference.items():
        if found[key] != expected:
            if key == "code":
                index = next(
                    (
                        position
                        for position, (left, right) in enumerate(zip(found[key], expected))
                        if left != right
                    ),
                    min(len(found[key]), len(expected)),
                )
                problems.append(f"manual/{language}/{page}: code block {index + 1} differs")
            else:
                problems.append(
                    f"manual/{language}/{page}: {key} differ "
                    f"({len(found[key])} instead of {len(expected)})"
                )


def check_console_sources(problems):
    """No page may keep its own Chinese: every string belongs in a catalog."""
    offenders = []
    for path in sorted(CONSOLE_SOURCE.rglob("*.js*")):
        if CONSOLE_LOCALES in path.parents or "locales" in path.parts:
            continue
        if path.name.endswith(".test.js"):
            continue
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if re.search(r"[一-龥]", line):
                offenders.append(f"{path.relative_to(ROOT)}:{number}")
    if offenders:
        problems.append(f"hardcoded text left in the console: {offenders[:8]}")


def main():
    problems = []
    rust = catalogs(RUST_CATALOGS)
    console = catalogs(CONSOLE_LOCALES)
    codes = console_codes()
    check_language_lists(rust, codes, problems)
    for name, entries in sorted(console.items()):
        check_catalog(f"console/{name}", entries, console["en"], problems)
    for name, entries in sorted(rust.items()):
        check_catalog(f"rust/{name}", entries, rust["en"], problems)
    check_manual(problems)
    check_console_sources(problems)
    if problems:
        print("i18n check failed:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print(
        f"i18n check passed: {len(rust)} server languages x {len(rust['en'])} messages, "
        f"{len(console)} console languages x {len(console['en'])} strings, "
        f"{len(sorted(p.name for p in MANUAL.iterdir() if p.is_dir()))} manual languages"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
