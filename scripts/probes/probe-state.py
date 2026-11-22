#!/usr/bin/env python3
"""Exercise CLI SIGTERM persistence and quota restoration using real binaries."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import socketserver
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
PROFILE = os.environ.get("FELUCCA_PROBE_PROFILE", "release")
OUTPUT = ROOT / os.environ.get("FELUCCA_PROBE_OUTPUT", "docs/audit")


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while data := self.request.recv(16384):
            self.request.sendall(data)


class EchoServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
            raise AssertionError("daemon did not stop after SIGTERM")
    assert process.returncode == 0, f"daemon exited with {process.returncode}"


def connect_when_ready(port, processes):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        assert all(p.poll() is None for p in processes), "daemon exited during startup"
        try:
            return socket.create_connection(("127.0.0.1", port), timeout=2)
        except ConnectionRefusedError:
            time.sleep(0.03)
    raise AssertionError("public listener was not ready")


def main():
    binaries = {role: ROOT / "target" / PROFILE / f"felucca-{role}" for role in ["server", "agent"]}
    checked = []
    for example in sorted((ROOT / "examples").glob("*.toml")):
        role = "server" if "server" in example.name else "agent"
        subprocess.run([str(binaries[role]), "--config", str(example), "--check"],
                       cwd=ROOT, check=True, capture_output=True)
        checked.append(example.name)
    with tempfile.TemporaryDirectory(prefix="state-probe-", dir=ROOT / "target/tmp") as tmp:
        tmp = Path(tmp)
        certificate, key = tmp / "cert.pem", tmp / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt",
                        "ec_paramgen_curve:prime256v1", "-nodes", "-days", "1",
                        "-subj", "/CN=felucca-probe", "-keyout", str(key),
                        "-out", str(certificate)], check=True, capture_output=True)
        der = subprocess.run(["openssl", "x509", "-in", str(certificate),
                              "-outform", "DER"], check=True, capture_output=True).stdout
        fingerprint = hashlib.sha256(der).hexdigest()
        bridge, public = free_port(), free_port()
        shared = secrets.token_hex(32)
        state = tmp / "state.json"
        server = tmp / "server.toml"
        server.write_text(f"""
[state]
path = "{state}"
save_interval = "1h"
[bridge]
bind = "127.0.0.1:{bridge}"
[bridge.tls]
cert = "{certificate}"
key = "{key}"
[user.team.limits]
quota = {{ flow_limit = 10 }}
[client.probe]
vkey = "{shared}"
user = "team"
allow_config_push = true
""")
        server.chmod(0o600)
        printed = subprocess.run([str(binaries["server"]), "--config", str(server),
                                  "--fingerprint"], check=True, capture_output=True, text=True).stdout.strip()
        assert printed == fingerprint, "configured certificate fingerprint differs from OpenSSL"
        with EchoServer(("127.0.0.1", 0), Echo) as echo:
            threading.Thread(target=echo.serve_forever, daemon=True).start()
            agent = tmp / "agent.toml"
            agent.write_text(f"""
server = "127.0.0.1:{bridge}"
key_id = "probe"
vkey = "{shared}"
transport = "tls"
reconnect_min = "30ms"
reconnect_max = "100ms"
[trust]
mode = "pinned"
fingerprint = "{fingerprint}"
[[tunnel]]
id = 77
mode = "tcp"
port = {public}
bind = "127.0.0.1"
backend = [{{addr = "127.0.0.1:{echo.server_address[1]}"}}]
""")
            agent.chmod(0o600)
            children = []
            try:
                for generation in range(2):
                    with (tmp / f"server-{generation}.log").open("wb") as out:
                        server_process = subprocess.Popen([str(binaries["server"]), "--config", str(server)],
                                                          stdout=out, stderr=subprocess.STDOUT)
                    children.append(server_process)
                    with (tmp / f"agent-{generation}.log").open("wb") as out:
                        agent_process = subprocess.Popen([str(binaries["agent"]), "--config", str(agent)],
                                                         stdout=out, stderr=subprocess.STDOUT)
                    children.append(agent_process)
                    with connect_when_ready(public, [server_process, agent_process]) as connection:
                        if generation == 0:
                            connection.sendall(b"probe")
                            connection.shutdown(socket.SHUT_WR)
                            data = bytearray()
                            while chunk := connection.recv(1024):
                                data.extend(chunk)
                            assert data == b"probe", "forwarded echo differs"
                        else:
                            try:
                                connection.sendall(b"must be refused")
                                data = connection.recv(1024)
                            except (ConnectionResetError, BrokenPipeError):
                                data = b""
                            assert data == b"", "exhausted user regained traffic after restart"
                    stop(agent_process)
                    stop(server_process)
                    snapshot = json.loads(state.read_bytes())
                    assert snapshot["version"] == 1
                    assert len(snapshot["counters"]) == 3
                    for resource, counts in snapshot["counters"]:
                        assert counts == {"inlet_bytes": 5, "export_bytes": 5}, (resource, counts)
            finally:
                for child in children:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=3)
                echo.shutdown()
        result = {
            "profile": PROFILE,
            "examples_checked": checked,
            "certificate_matches_openssl": True,
            "sigterm_exit_status": 0,
            "restarts": 1,
            "user_client_tunnel_bytes_each": 10,
            "exhausted_user_refused_after_restart": True,
            "binaries": {
                role: {
                    "bytes": binary.stat().st_size,
                    "sha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
                    "build": json.loads(subprocess.check_output([str(binary), "--build-info"], timeout=15)),
                }
                for role, binary in binaries.items()
            },
        }
        OUTPUT.mkdir(parents=True, exist_ok=True)
        (OUTPUT / "state-runtime.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
