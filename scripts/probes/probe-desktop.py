#!/usr/bin/env python3
"""Run the desktop client against a loopback server and check real forwarding.

The client starts with one saved launch URI, exactly as a user would after
pasting the console's command. Success is a byte round trip through a
server-managed tunnel, not a status label in the window.
"""

import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER = ROOT / "target/debug/felucca-server"
DESKTOP = ROOT / "apps/desktop/src-tauri/target/debug/felucca-desktop"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def echo(listener):
    while True:
        try:
            conn, _ = listener.accept()
        except OSError:
            return
        with conn:
            data = conn.recv(1024)
            conn.sendall(b"echo:" + data)


def round_trip(port, payload):
    with socket.create_connection(("127.0.0.1", port), timeout=2) as sock:
        sock.sendall(payload)
        return sock.recv(1024)


def main():
    work = Path(tempfile.mkdtemp(prefix="felucca-desktop-"))
    bridge, tunnel = free_port(), free_port()
    backend = socket.socket()
    backend.bind(("127.0.0.1", 0))
    backend.listen()
    threading.Thread(target=echo, args=(backend,), daemon=True).start()
    backend_port = backend.getsockname()[1]

    cert, key = work / "bridge.pem", work / "bridge.key"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", "/CN=felucca-desktop-probe", "-keyout", key, "-out", cert],
                   check=True, capture_output=True)
    vkey = secrets.token_hex(32)
    server_config = {
        "bridge": {"bind": f"127.0.0.1:{bridge}", "tls": {"cert": str(cert), "key": str(key)}},
        "client": {"desk": {"vkey": vkey}},
        "managed": {"desk": {"tunnel": [{
            "id": 1, "mode": "tcp", "port": tunnel, "bind": "127.0.0.1",
            "backend": [{"addr": f"127.0.0.1:{backend_port}"}],
        }]}},
    }
    server_path = work / "server.json"
    server_path.write_text(json.dumps(server_config), encoding="utf-8")
    fingerprint = subprocess.run([SERVER, "--config", server_path, "--fingerprint"],
                                 check=True, capture_output=True, text=True).stdout.strip()
    query = urllib.parse.urlencode({"key_id": "desk", "vkey": vkey, "transport": "tls",
                                    "fingerprint": fingerprint, "remote": "1"})
    uri = f"felucca://127.0.0.1:{bridge}?{query}"
    state = work / "desktop"
    state.mkdir()
    (state / "connections.json").write_text(json.dumps({"version": 1, "connections": [
        {"id": "probe", "name": "Probe", "uri": uri, "enabled": True}]}), encoding="utf-8")

    processes = []
    result = {"bridge": bridge, "tunnel": tunnel}
    try:
        processes.append(subprocess.Popen([SERVER, "--config", server_path],
                                          stdout=open(work / "server.log", "wb"),
                                          stderr=subprocess.STDOUT))
        env = dict(os.environ, FELUCCA_DESKTOP_DIR=str(state), RUST_LOG="felucca=debug,felucca_desktop=debug")
        processes.append(subprocess.Popen([DESKTOP, "--hidden"], env=env,
                                          stdout=open(work / "desktop.log", "wb"),
                                          stderr=subprocess.STDOUT))
        deadline = time.monotonic() + 30
        payload = secrets.token_hex(8).encode()
        while time.monotonic() < deadline:
            for process in processes:
                if process.poll() is not None:
                    raise SystemExit(f"a process exited early with {process.returncode}; logs in {work}")
            try:
                reply = round_trip(tunnel, payload)
                if reply == b"echo:" + payload:
                    result["forwarded"] = True
                    break
            except OSError:
                pass
            time.sleep(0.5)
        else:
            raise SystemExit(f"no forwarding within 30s; logs in {work}")
        result["desktop_log_has_key"] = vkey in (work / "desktop.log").read_text(errors="replace")
        print(json.dumps(result))
        return 0 if not result["desktop_log_has_key"] else 1
    finally:
        for process in reversed(processes):
            process.terminate()
            try:
                process.wait(5)
            except subprocess.TimeoutExpired:
                process.kill()
        backend.close()


if __name__ == "__main__":
    sys.exit(main())
