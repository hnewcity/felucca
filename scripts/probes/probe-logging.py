#!/usr/bin/env python3
"""Exercise real agent log rotation on loopback using an isolated log directory."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
PROFILE = os.environ.get("FELUCCA_PROBE_PROFILE", "release")
OUTPUT = ROOT / os.environ.get("FELUCCA_PROBE_OUTPUT", "docs/audit")
BINARY = ROOT / "target" / PROFILE / "felucca-agent"


def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="logging-probe-", dir=ROOT / "target/tmp") as directory:
        temp = Path(directory)
        # Reserve the endpoint without listen(); every dial receives refusal and
        # produces a bounded reconnect log, with no external service involved.
        guard = socket.socket()
        guard.bind(("127.0.0.1", 0))
        port = guard.getsockname()[1]
        config = temp / "agent.json"
        config.write_text(json.dumps({"server": f"127.0.0.1:{port}", "vkey": "logging-probe-key", "transport": "tcp", "dial_timeout": "100ms", "reconnect_min": "1ms", "reconnect_max": "2ms"}))
        logfile = temp / "agent.log"
        command = [str(BINARY), "--config", str(config), "--log-file", str(logfile), "--log-json", "--log-max-bytes", "65536", "--log-backups", "2", "--log-rotate-interval", "1s"]
        env = dict(os.environ, RUST_LOG="felucca=trace")
        child = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            end = time.monotonic() + 15
            while not (temp / "agent.log.2").exists():
                if child.poll() is not None:
                    raise AssertionError(f"agent exited early: {child.returncode}")
                if time.monotonic() >= end:
                    raise AssertionError("rotation did not produce two archives: " + (logfile.read_text()[-3000:] if logfile.exists() else "no log file"))
                time.sleep(0.05)
            duplicate = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
            assert duplicate.returncode != 0, "a second rotating writer must be refused"
            child.send_signal(signal.SIGTERM)
            assert child.wait(timeout=10) == 0
            files = [logfile, temp / "agent.log.1", temp / "agent.log.2"]
            events = 0
            for file in files:
                for line in file.read_text().splitlines():
                    assert isinstance(json.loads(line), dict)
                    events += 1
                if os.name != "nt":
                    assert file.stat().st_mode & 0o777 == 0o600
            assert not (temp / "agent.log.3").exists()
            result = {"profile": PROFILE, "build": json.loads(subprocess.check_output([str(BINARY), "--build-info"])), "archives": 2, "json_events": events, "exclusive_writer": True, "sigterm_exit": 0, "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest()}
            (OUTPUT / "logging-runtime.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            guard.close()


if __name__ == "__main__":
    main()
