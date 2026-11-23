#!/usr/bin/env python3
"""Package native binaries only after querying their actual build identities.

No upload or publishing. Digests refer to binaries inside the archive, suitable
for the explicit updater after extracting the binary and trusting the manifest.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def package():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=["release", "debug"], default="release")
    parser.add_argument("--binary-dir", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    binaries = args.binary_dir or ROOT / "target" / args.profile
    suffix = ".exe" if os.name == "nt" else ""
    records = []
    for role in ["felucca-server", "felucca-agent"]:
        path = (binaries / (role + suffix)).resolve(strict=True)
        build = json.loads(subprocess.check_output([str(path), "--build-info"], timeout=15))
        if build.get("program") != role:
            raise ValueError("binary role mismatch")
        for key in ["version", "target"]:
            if not isinstance(build.get(key), str) or not re.fullmatch(r"[A-Za-z0-9.+_-]{1,128}", build[key]):
                raise ValueError("invalid binary build identity")
        if type(build.get("protocol")) is not int or build["protocol"] < 1:
            raise ValueError("invalid binary protocol")
        records.append((path, build))
    first = records[0][1]
    if any(any(info[k] != first[k] for k in ["version", "target", "protocol"]) for _, info in records):
        raise ValueError("server and agent must have identical version, target and protocol")
    args.output.mkdir(parents=True, exist_ok=True)
    stem = f"felucca-{first['version']}-{first['target']}-{args.profile}"
    manifest = {"version": first["version"], "target": first["target"], "protocol": first["protocol"], "profile": args.profile, "files": {}}
    with tempfile.TemporaryDirectory(prefix=".felucca-package-", dir=args.output) as scratch:
        staging = Path(scratch) / stem
        staging.mkdir()
        for src, _ in records:
            dst = staging / src.name
            shutil.copy2(src, dst)
            manifest["files"][src.name] = {"sha256": digest(dst), "bytes": dst.stat().st_size}
        subprocess.run([sys.executable, str(ROOT / "scripts/stage-agent.py"),
                        str(records[1][0]), "--output", str(staging / "agents")], check=True)
        shutil.copy2(ROOT / "README.md", staging / "README.md")
        shutil.copytree(ROOT / "examples", staging / "examples", ignore=shutil.ignore_patterns("*.local.toml", "*.key", "*.pem"))
        (staging / "docs").mkdir()
        for doc in (ROOT / "docs").glob("*.md"):
            shutil.copy2(doc, staging / "docs" / doc.name)
        (staging / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
        archive = args.output / (stem + (".zip" if os.name == "nt" else ".tar.gz"))
        temp_archive = Path(scratch) / archive.name
        if os.name == "nt":
            with zipfile.ZipFile(temp_archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
                for file in sorted(staging.rglob("*")):
                    if file.is_file():
                        output.write(file, file.relative_to(staging.parent))
        else:
            with tarfile.open(temp_archive, "w:gz") as output:
                output.add(staging, arcname=stem)
        os.replace(temp_archive, archive)
        (args.output / (archive.name + ".sha256")).write_text(f"{digest(archive)}  {archive.name}\n", encoding="ascii", newline="\n")
        (args.output / (stem + ".manifest.json")).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
        print(json.dumps({"archive": str(archive), **manifest}, indent=2))


if __name__ == "__main__":
    package()
