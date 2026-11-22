#!/usr/bin/env python3
"""Stage a locally built agent for authenticated connection-package downloads."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import sys


def notarized(binary, target):
    """Only label a Mac binary after checking its Developer ID and online ticket."""
    if "apple-darwin" not in target or sys.platform != "darwin":
        return False
    details = subprocess.run(["codesign", "--display", "--verbose=2", str(binary)],
                             capture_output=True, text=True, timeout=30)
    if details.returncode or "Authority=Developer ID Application:" not in details.stderr:
        return False
    return subprocess.run(
        ["codesign", "--verify", "--strict", "--check-notarization", str(binary)],
        capture_output=True, timeout=120,
    ).returncode == 0


def stage(binary, output):
    build = json.loads(subprocess.check_output([str(binary.resolve()), "--build-info"], timeout=15))
    if build["program"] != "felucca-agent" or not re.fullmatch(r"[a-zA-Z0-9_-]+", build["target"]):
        raise ValueError("invalid agent build identity")
    folder = output / build["target"]
    folder.mkdir(parents=True, exist_ok=True)
    name = "felucca-agent.exe" if "windows" in build["target"] else "felucca-agent"
    with tempfile.TemporaryDirectory(prefix=".agent-", dir=output) as temporary:
        staged = Path(temporary) / name
        shutil.copyfile(binary, staged)
        staged.chmod(0o755)
        digest = hashlib.sha256()
        with staged.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        manifest = {key: build[key] for key in ("version", "target", "protocol")}
        manifest.update(filename=name, bytes=staged.stat().st_size, sha256=digest.hexdigest(),
                        notarized=notarized(staged, build["target"]))
        os.replace(staged, folder / name)
        staged_manifest = Path(temporary) / "manifest.json"
        staged_manifest.write_text(json.dumps(manifest, indent=2) + "\n")
        staged_manifest.chmod(0o644)
        os.replace(staged_manifest, folder / "manifest.json")
    print(json.dumps(manifest))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    stage(args.binary, args.output)
