#!/usr/bin/env python3
"""Verify bundle assembly from a real native archive, including rejection paths."""
import argparse
import importlib.util
import json
from pathlib import Path
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("bundle_agents", ROOT / "scripts/bundle-agents.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("archive", type=Path)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
checks = []
with tempfile.TemporaryDirectory(prefix="bundle-probe-", dir=args.output) as temporary:
    scratch = Path(temporary)
    targets = module.bundle([args.archive], scratch / "from-native")
    checks.append("real native archive produces a verified server-side client directory")
    target, manifest, binary = next(module.read_agents(args.archive))
    alternate = scratch / "same-client.zip"
    with zipfile.ZipFile(alternate, "w") as archive:
        prefix = f"fixture/agents/{target}/"
        archive.writestr(prefix + "manifest.json", json.dumps(manifest))
        archive.writestr(prefix + manifest["filename"], binary)
    assert module.bundle([alternate], scratch / "from-zip") == targets
    checks.append("ZIP archive format preserves the same manifest and client bytes")
    for name, inputs in [("duplicate", [args.archive, alternate]), ("existing", [args.archive])]:
        output = scratch / ("from-native" if name == "existing" else name)
        try:
            module.bundle(inputs, output)
        except ValueError:
            checks.append(f"rejects {name} output/target")
        else:
            raise AssertionError(f"accepted {name}")
    with zipfile.ZipFile(alternate, "w") as archive:
        archive.writestr(prefix + "manifest.json", json.dumps(manifest))
        archive.writestr(prefix + manifest["filename"], bytes([binary[0] ^ 1]) + binary[1:])
    try:
        module.bundle([alternate], scratch / "tampered")
    except ValueError:
        assert not (scratch / "tampered").exists()
        checks.append("rejects a changed binary without exposing an incomplete bundle")
    else:
        raise AssertionError("accepted tampered client")
report = {"status": "passed", "targets": targets, "checks": checks,
          "limits": ["Archive assembly only; no foreign OS execution or CI run."]}
(args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
