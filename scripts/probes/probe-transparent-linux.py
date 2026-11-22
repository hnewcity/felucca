#!/usr/bin/env python3
"""Linux REDIRECT smoke test, confined to a fresh network namespace.

Requires root, unshare, iproute2 and iptables. Does not edit host firewall rules.
Build binaries before running; optionally set FELUCCA_PROBE_PROFILE=debug.
"""
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    if sys.platform != "linux" or os.geteuid() != 0:
        raise SystemExit("This smoke test needs root on Linux with unshare, ip and iptables.")
    namespace = str(Path("/proc/self/ns/net").stat().st_ino)
    if len(sys.argv) == 1:
        result = subprocess.run(["unshare", "--net", sys.executable, str(Path(__file__).resolve()), "--inside", namespace])
        raise SystemExit(result.returncode)
    # Check before executing ANY network configuration command. The outer
    # namespace identity travels as an argument, not a permissive env flag.
    assert len(sys.argv) == 3 and sys.argv[1] == "--inside"
    assert namespace != sys.argv[2], "refusing to change rules in the original namespace"
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    profile = os.environ.get("FELUCCA_PROBE_PROFILE", "release")
    binaries = {name: ROOT / "target" / profile / f"felucca-{name}" for name in ["server", "agent"]}
    children = []

    class Echo(socketserver.BaseRequestHandler):
        def handle(self):
            while packet := self.request.recv(16384):
                self.request.sendall(packet)

    class EchoServer(socketserver.ThreadingTCPServer):
        daemon_threads = True

    def free_port():
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            return listener.getsockname()[1]

    with tempfile.TemporaryDirectory(prefix="transparent-probe-", dir=ROOT / "target/tmp") as temporary, EchoServer(("127.0.0.1", 0), Echo) as echo:
        temp = Path(temporary)
        threading.Thread(target=echo.serve_forever, daemon=True).start()
        bridge, public = free_port(), free_port()
        backend = echo.server_address[1]
        key = secrets.token_hex(32)
        state = temp / "traffic.json"
        configs = {
            "server": {"bridge": {"bind": f"127.0.0.1:{bridge}"}, "client": {"probe": {"vkey": key, "allow_config_push": True}},
                       "state": {"path": str(state)}},
            "agent": {"server": f"127.0.0.1:{bridge}", "key_id": "probe", "vkey": key, "transport": "tcp",
                      "reconnect_min": "30ms", "reconnect_max": "100ms",
                      "tunnel": [{"id": 1, "mode": "transparent", "port": public, "bind": "127.0.0.1",
                                  "target_policy": {"allow": ["127.0.0.0/8"], "ports": [backend]}}]},
        }
        try:
            for name, config in configs.items():
                path = temp / f"{name}.json"
                path.write_text(json.dumps(config))
                path.chmod(0o600)
                with (temp / f"{name}.log").open("wb") as log:
                    children.append(subprocess.Popen([str(binaries[name]), "--config", str(path)], stdout=log, stderr=subprocess.STDOUT))
            deadline = time.monotonic() + 10
            while True:
                assert all(child.poll() is None for child in children), "daemon exited during startup"
                try:
                    with socket.create_connection(("127.0.0.1", public), timeout=1):
                        break
                except ConnectionRefusedError:
                    if time.monotonic() >= deadline:
                        raise AssertionError("transparent listener did not start")
                    time.sleep(0.03)
            # Only the throwaway nobody-uid client is intercepted. The root
            # agent's backend connection must not enter its own redirect loop.
            subprocess.run([
                "iptables", "-t", "nat", "-A", "OUTPUT", "-p", "tcp", "-d", "127.0.0.1",
                "--dport", str(backend), "-m", "owner", "--uid-owner", "65534",
                "-j", "REDIRECT", "--to-ports", str(public),
            ], check=True)
            probe = """
import socket,sys
with socket.create_connection(('127.0.0.1',int(sys.argv[1])),timeout=5) as io:
    io.sendall(b'probe')
    io.shutdown(socket.SHUT_WR)
    response=b''
    while True:
        part=io.recv(1024)
        if not part: break
        response+=part
    assert response==b'probe',response
"""
            subprocess.run([sys.executable, "-c", probe, str(backend)], check=True, user=65534)
            for child in reversed(children):
                child.terminate()
                child.wait(timeout=15)
                assert child.returncode == 0, child.returncode
            counters = json.loads(state.read_text())["counters"]
            assert len(counters) == 2, counters
            for _, counts in counters:
                assert counts == {"inlet_bytes": 5, "export_bytes": 5}, counts
            result = {
                "profile": profile, "network_namespace_isolated": True,
                "linux_nat_redirect_original_destination": True, "tcp_half_close": True,
                "counters": counters, "exit_codes": [child.returncode for child in children],
                "binaries": {name: {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                             for name, path in binaries.items()},
            }
            (ROOT / "docs/audit/transparent-linux-runtime.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=3)
            echo.shutdown()


if __name__ == "__main__":
    main()
