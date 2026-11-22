#!/usr/bin/env python3
"""Sign and notarize copied release binaries using an existing Keychain profile.

This explicitly submits only the two compiled programs to Apple's notary service.
It never uploads deployment files or connection credentials, changes Gatekeeper,
or installs a service. Run only as part of an authorized release.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile


def run(arguments, **kwargs):
    result = subprocess.run(arguments, capture_output=True, text=True, **kwargs)
    if result.returncode:
        # Commands use a Keychain profile, never a plaintext credential argument.
        raise RuntimeError(f"{arguments[0]} failed: {result.stderr.strip()}")
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--identity", required=True, help="Developer ID Application identity")
    parser.add_argument("--keychain-profile", required=True)
    parser.add_argument("--keychain", type=Path)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS with Xcode command-line tools is required")
    if not args.identity.startswith("Developer ID Application:"):
        parser.error("a Developer ID Application certificate is required")
    if args.output.exists():
        parser.error("output must be a new directory; existing releases are preserved")
    source = args.binary_dir.resolve(strict=True)
    identities = [
        json.loads(run([str(source / name), "--build-info"], timeout=30))
        for name in ("felucca-server", "felucca-agent")
    ]
    if any(info["program"] != name or "apple-darwin" not in info["target"]
           for info, name in zip(identities, ("felucca-server", "felucca-agent"))):
        parser.error("expected native macOS server and agent builds")
    if any(identities[0][key] != identities[1][key]
           for key in ("target", "version", "protocol")):
        parser.error("server and agent identities must match")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".felucca-notary-", dir=args.output.parent) as folder:
        staging = Path(folder)
        signed = staging / "signed"
        signed.mkdir()
        binaries = []
        for info in identities:
            name = info["program"]
            binary = signed / name
            shutil.copyfile(source / name, binary)
            binary.chmod(0o755)
            sign = ["codesign", "--force", "--options", "runtime", "--timestamp",
                    "--identifier", f"org.felucca.{name}", "--sign", args.identity]
            if args.keychain:
                sign.extend(["--keychain", str(args.keychain)])
            run([*sign, str(binary)], timeout=120)
            run(["codesign", "--verify", "--strict", str(binary)], timeout=30)
            details = subprocess.run(["codesign", "--display", "--verbose=2", str(binary)],
                                     capture_output=True, text=True, check=True, timeout=30)
            if "Authority=Developer ID Application:" not in details.stderr:
                raise RuntimeError("Developer ID signature was not produced")
            binaries.append(binary)
        archive = staging / "felucca-programs.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for binary in binaries:
                output.write(binary, binary.name)
        auth = ["--keychain-profile", args.keychain_profile]
        if args.keychain:
            auth.extend(["--keychain", str(args.keychain)])
        submission = subprocess.run(
            ["xcrun", "notarytool", "submit", str(archive), *auth,
             "--wait", "--timeout", "20m", "--output-format", "json"],
            capture_output=True, text=True, timeout=1320,
        )
        try:
            response = json.loads(submission.stdout)
        except ValueError:
            raise RuntimeError("notarytool did not return a submission result") from None
        if not response.get("id"):
            raise RuntimeError("notarytool did not return a submission ID")
        log_path = signed / "notarization-log.json"
        run(["xcrun", "notarytool", "log", response["id"], *auth, str(log_path)], timeout=120)
        log = json.loads(log_path.read_text())
        if response.get("status") != "Accepted" or log.get("status") != "Accepted":
            # Preserve diagnostic evidence even when no distributable output is produced.
            rejected = args.output.with_name(args.output.name + "-notarization-rejected.json")
            rejected.write_text(json.dumps(log, indent=2) + "\n")
            raise RuntimeError(f"notarization was not accepted; see {rejected}")
        for binary in binaries:
            run(["codesign", "--verify", "--strict", "--check-notarization", str(binary)],
                timeout=120)
        # Standalone Mach-O binaries cannot carry stapled tickets. Their signatures
        # are kept byte-for-byte; Gatekeeper retrieves their tickets online.
        report = {"status": "Accepted", "submission": response["id"],
                  "build": identities[0], "files": {}, "issues": log.get("issues") or []}
        for binary in binaries:
            report["files"][binary.name] = hashlib.sha256(binary.read_bytes()).hexdigest()
        (signed / "notarization.json").write_text(json.dumps(report, indent=2) + "\n")
        os.rename(signed, args.output)
        print(json.dumps({"status": "Accepted", "output": str(args.output)}))


if __name__ == "__main__":
    main()
