#!/usr/bin/env python3
"""Exercise the compiled asset build script in an isolated source-tree copy."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / os.environ.get("FELUCCA_PROBE_OUTPUT", "target/tmp/react-admin-build")
OUTPUT.mkdir(parents=True, exist_ok=True)
scripts = list((ROOT / "target/debug/build").glob("felucca-server-*/build-script-build"))
if not scripts:
    raise SystemExit("Build felucca-cli debug binaries first.")
script = max(scripts, key=lambda path: path.stat().st_mtime)

with tempfile.TemporaryDirectory(prefix="admin-build-", dir=ROOT / "target/tmp") as temp:
    checkout = Path(temp)
    app = checkout / "apps/admin"
    shutil.copytree(ROOT / "apps/admin", app, ignore=shutil.ignore_patterns("node_modules"))
    manifest = checkout / "crates/felucca-server"
    manifest.mkdir(parents=True)
    out = checkout / "output"
    out.mkdir()
    env = {**os.environ, "CARGO_MANIFEST_DIR": str(manifest), "OUT_DIR": str(out)}

    def execute():
        return subprocess.run([str(script)], env=env, capture_output=True, text=True)

    fresh = execute()
    assert fresh.returncode == 0, fresh.stderr
    embedded = (out / "admin_assets.rs").read_text()
    assert '"font/woff2"' in embedded and '"/favicon.svg"' in embedded
    with (app / "src/main.jsx").open("a") as source:
        source.write("\n// source changed after bundling\n")
    stale = execute()
    assert stale.returncode != 0 and "Management UI is stale" in stale.stderr
    (app / "dist/.source-hash").unlink()
    missing = execute()
    assert missing.returncode != 0 and "Build the management UI first" in missing.stderr

result = {
    "fresh_bundle_accepted": True,
    "changed_source_rejected": True,
    "missing_bundle_rejected": True,
    "embedded_font_and_icon": True,
    "isolated_copy": True,
}
(OUTPUT / "build-guard.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
