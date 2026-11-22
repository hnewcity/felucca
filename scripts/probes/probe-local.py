#!/usr/bin/env python3
"""Release CLI: agent-free local aliases, runtime config precedence and shutdown."""
import hashlib
import json
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while data := self.request.recv(16384):
            self.request.sendall(data)


class EchoServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


def stop(process):
    process.terminate()
    try:
        process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)
        raise AssertionError("daemon did not drain on SIGTERM")
    assert process.returncode == 0, process.returncode


def connect(port, process):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        assert process.poll() is None, process.returncode
        try:
            return socket.create_connection(("127.0.0.1", port), timeout=10)
        except ConnectionRefusedError:
            time.sleep(0.02)
    raise AssertionError("local alias never became ready")


def main():
    binary = ROOT / "target" / os.environ.get("FELUCCA_PROBE_PROFILE", "release") / "felucca-server"
    with tempfile.TemporaryDirectory(prefix="local-probe-", dir=ROOT / "target/tmp") as directory:
        directory = Path(directory)
        runtime = directory / "runtime.json"
        traffic = directory / "traffic.json"
        bootstrap = directory / "bootstrap.toml"
        direct, alias = free_port(), free_port()
        with EchoServer(("127.0.0.1", 0), Echo) as backend:
            threading.Thread(target=backend.serve_forever, daemon=True).start()
            config = f"""
[bridge]
bind="127.0.0.1:0"
[state]
path="{traffic}"
save_interval="1h"
[[local.office.tunnel]]
id=1
mode="tcp"
port={direct}
bind="127.0.0.1"
backend=[{{addr="127.0.0.1:{backend.server_address[1]}"}}]
[[local.office.tunnel]]
id=2
mode="tcp"
port={alias}
bind="127.0.0.1"
backend=[{{addr="tunnel://local.office/1"}}]
[local.office.tunnel.options.payload]
compress=true
encrypt=true
"""
            bootstrap.write_text(config)
            children = []
            try:
                for generation in range(2):
                    # The second bootstrap is valid but points to an unavailable
                    # backend. Saved runtime state must retain the original route.
                    if generation:
                        bootstrap.write_text(config.replace(
                            f"127.0.0.1:{backend.server_address[1]}", "127.0.0.1:1"))
                    with (directory / f"server-{generation}.log").open("wb") as out:
                        process = subprocess.Popen(
                            [str(binary), "--config", str(bootstrap),
                             "--runtime-config", str(runtime)], cwd=ROOT,
                            stdout=out, stderr=subprocess.STDOUT)
                    children.append(process)
                    with connect(alias, process) as stream:
                        payload = b"local-configuration"
                        stream.sendall(payload)
                        stream.shutdown(socket.SHUT_WR)
                        received = bytearray()
                        while chunk := stream.recv(65536):
                            received.extend(chunk)
                        assert received == payload
                    stop(process)
                    snapshot = json.loads(runtime.read_text())
                    assert snapshot["version"] == 1
                    assert snapshot["revision"] == 0
                    assert snapshot["config"]["local"]["office"]["tunnel"][0]["backend"][0]["addr"].endswith(
                        f":{backend.server_address[1]}")
                    counters = json.loads(traffic.read_text())["counters"]
                    for resource, counts in counters:
                        expected = len(payload) * (generation + 1)
                        if resource["kind"] == "client":
                            expected *= 2  # Both local hops are charged.
                        assert counts == {"inlet_bytes": expected, "export_bytes": expected}, (resource, counts)
                subprocess.run([str(binary), "--config", str(bootstrap),
                                "--runtime-config", str(runtime), "--check"],
                               cwd=ROOT, check=True, capture_output=True)
                runtime.write_text("{corrupt")
                failed = subprocess.run([str(binary), "--config", str(bootstrap),
                                         "--runtime-config", str(runtime), "--check"],
                                        cwd=ROOT, capture_output=True)
                assert failed.returncode != 0
                assert runtime.read_text() == "{corrupt"
                result = {
                    "agent_processes": 0,
                    "local_alias_hops": 2,
                    "runtime_config_overrides_bootstrap": True,
                    "corrupt_snapshot_refused_without_overwrite": True,
                    "restarts": 1,
                    "sigterm_exit_codes": [child.returncode for child in children],
                    "traffic": counters,
                    "binary": {"bytes": binary.stat().st_size,
                               "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()},
                }
                (ROOT / "docs/audit/local-runtime.json").write_text(json.dumps(result, indent=2) + "\n")
                print(json.dumps(result, indent=2))
            finally:
                for child in children:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=3)
                backend.shutdown()


if __name__ == "__main__":
    main()
