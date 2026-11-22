#!/usr/bin/env python3
"""Combine trusted native release archives into one server-side agents directory.

Reads only the expected agent files, verifies their manifests and SHA-256, and
never executes foreign binaries. The input archives must come from your build
or a trusted publisher; checksums do not authenticate a publisher.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import tarfile
import tempfile
import zipfile

TARGETS = {
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
}
MAX_BINARY = 128 * 1024 * 1024


def read_agents(path):
    is_zip = path.suffix == ".zip"
    with (zipfile.ZipFile(path) if is_zip else tarfile.open(path, "r:gz")) as archive:
        members = archive.infolist() if is_zip else archive.getmembers()
        entries = {}
        for member in members:
            name = member.filename if is_zip else member.name
            if name in entries:
                raise ValueError(f"Duplicate archive member in {path}")
            entries[name] = member

        def read(name, limit):
            member = entries[name]
            size = member.file_size if is_zip else member.size
            regular = (not member.is_dir() and (member.external_attr >> 16) & 0o170000 in (0, 0o100000)) if is_zip else member.isfile()
            if not regular or not 0 < size <= limit:
                raise ValueError(f"Invalid archive member: {name}")
            with (archive.open(member) if is_zip else archive.extractfile(member)) as stream:
                result = stream.read(limit + 1)
            if len(result) != size:
                raise ValueError(f"Invalid archive member size: {name}")
            return result

        found = False
        for name in entries:
            parts = PurePosixPath(name).parts
            if len(parts) != 4 or parts[1] != "agents" or parts[3] != "manifest.json":
                continue
            target = parts[2]
            if target not in TARGETS:
                raise ValueError(f"Unsupported target: {target}")
            manifest = json.loads(read(name, 4096))
            filename = "felucca-agent.exe" if "windows" in target else "felucca-agent"
            if (manifest.get("target") != target or manifest.get("filename") != filename
                    or not re.fullmatch(r"[A-Za-z0-9.+_-]{1,128}", manifest.get("version", ""))
                    or type(manifest.get("protocol")) is not int or manifest["protocol"] < 1):
                raise ValueError(f"Invalid manifest: {name}")
            binary = read(str(PurePosixPath(name).parent / filename), MAX_BINARY)
            if (len(binary) != manifest.get("bytes")
                    or hashlib.sha256(binary).hexdigest() != manifest.get("sha256")):
                raise ValueError(f"Agent checksum mismatch: {target}")
            found = True
            yield target, manifest, binary
        if not found:
            raise ValueError(f"No agents found in {path}")


def bundle(archives, output):
    # Build the complete directory before exposing it. Refuse accidental replacement.
    if output.exists():
        raise ValueError(f"Output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    seen, identity = set(), None
    with tempfile.TemporaryDirectory(prefix=".agents-", dir=output.parent) as temporary:
        staging = Path(temporary) / "agents"
        staging.mkdir()
        for path in archives:
            for target, manifest, binary in read_agents(path):
                version = (manifest["version"], manifest["protocol"])
                if target in seen or (identity is not None and version != identity):
                    raise ValueError("Duplicate target or mixed release versions/protocols")
                identity = version
                seen.add(target)
                folder = staging / target
                folder.mkdir()
                file = folder / manifest["filename"]
                file.write_bytes(binary)
                file.chmod(0o755)
                (folder / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        shutil.move(staging, output)
    return sorted(seen)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archives", nargs="+", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="New agents directory")
    args = parser.parse_args()
    print(json.dumps({"targets": bundle(args.archives, args.output), "directory": str(args.output)}))
