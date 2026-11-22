#!/usr/bin/env python3
"""Real release processes: impaired KCP/FEC, encrypted TCP/UDP, final snapshots."""
import hashlib
import json
from pathlib import Path
import secrets
import socket
import socketserver
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]


def free_port(kind=socket.SOCK_STREAM):
    with socket.socket(socket.AF_INET, kind) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while data := self.request.recv(16384):
            self.request.sendall(data)


class EchoServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


class UdpWorker:
    def __init__(self, server=None):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 256 * 1024)
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 256 * 1024)
        self.socket.bind(("127.0.0.1", 0))
        self.socket.settimeout(0.02)
        self.address = self.socket.getsockname()
        self.server = server
        self.stop = threading.Event()
        self.counts = dict(dropped=0, duplicated=0, reordered=0, parity_packets=0)
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        client = None
        held = None
        sequence = 0
        while not self.stop.is_set():
            try:
                packet, source = self.socket.recvfrom(65536)
            except socket.timeout:
                if held:
                    self.socket.sendto(*held)
                    held = None
                continue
            if self.server is None:
                self.socket.sendto(packet, source)
                continue
            if source == self.server:
                assert client is not None
                target = client
            else:
                client, target = source, self.server
            sequence += 1
            if len(packet) > 18 and packet[:4] == b"FKP5" and packet[14] >= packet[12]:
                self.counts["parity_packets"] += 1
            if sequence % 17 == 0:
                self.counts["dropped"] += 1
                continue
            if sequence % 19 == 0 and held is None:
                held = (packet, target)
                continue
            self.socket.sendto(packet, target)
            if sequence % 29 == 0:
                self.counts["duplicated"] += 1
                self.socket.sendto(packet, target)
            if held:
                self.counts["reordered"] += 1
                self.socket.sendto(*held)
                held = None

    def close(self):
        self.stop.set()
        self.thread.join(timeout=2)
        assert not self.thread.is_alive()
        self.socket.close()


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
            raise AssertionError("SIGTERM did not stop the daemon")
    assert process.returncode == 0, process.returncode


def ready(port, processes):
    end = time.monotonic() + 20
    while time.monotonic() < end:
        assert all(p.poll() is None for p in processes)
        try:
            return socket.create_connection(("127.0.0.1", port), timeout=30)
        except ConnectionRefusedError:
            time.sleep(0.02)
    raise AssertionError("listener did not become ready")


def main():
    binaries = {role: ROOT / "target/release" / f"felucca-{role}" for role in ("server", "agent")}
    bridge, public_tcp = free_port(socket.SOCK_DGRAM), free_port()
    public_udp = free_port(socket.SOCK_DGRAM)
    relay = UdpWorker(("127.0.0.1", bridge))
    udp = UdpWorker()
    try:
        with tempfile.TemporaryDirectory(prefix="kcp-probe-", dir=ROOT / "target/tmp") as directory:
            directory = Path(directory)
            key = secrets.token_hex(32)
            state = directory / "state.json"
            server_config = directory / "server.toml"
            server_config.write_text(f"""
[state]
path="{state}"
save_interval="1h"
[bridge]
bind="127.0.0.1:0"
kcp_bind="127.0.0.1:{bridge}"
[client.probe]
vkey="{key}"
allow_config_push=true
""")
            server_config.chmod(0o600)
            with EchoServer(("127.0.0.1", 0), Echo) as echo:
                threading.Thread(target=echo.serve_forever, daemon=True).start()
                agent_config = directory / "agent.toml"
                agent_config.write_text(f"""
server="127.0.0.1:{relay.address[1]}"
key_id="probe"
vkey="{key}"
transport="kcp"
reconnect_min="30ms"
reconnect_max="100ms"
[network]
local_ip="127.0.0.1"
forward_local_ip=true
[[tunnel]]
id=1
mode="tcp"
port={public_tcp}
bind="127.0.0.1"
backend=[{{addr="127.0.0.1:{echo.server_address[1]}"}}]
[tunnel.options.payload]
compress=true
encrypt=true
[[tunnel]]
id=2
mode="udp"
port={public_udp}
bind="127.0.0.1"
backend=[{{addr="127.0.0.1:{udp.address[1]}"}}]
[tunnel.options.payload]
compress=true
encrypt=true
""")
                agent_config.chmod(0o600)
                processes = []
                logs = []
                try:
                    for role, config in (("server", server_config), ("agent", agent_config)):
                        log = open(directory / f"{role}.log", "wb")
                        logs.append(log)
                        processes.append(subprocess.Popen(
                            [str(binaries[role]), "--config", str(config)], cwd=ROOT,
                            stdout=log, stderr=subprocess.STDOUT,
                        ))
                    payload = b"Z" * (1024 * 1024) + secrets.token_bytes(1024 * 1024)
                    with ready(public_tcp, processes) as stream:
                        received = bytearray()
                        errors = []

                        def read():
                            try:
                                while chunk := stream.recv(65536):
                                    received.extend(chunk)
                            except Exception as error:
                                errors.append(repr(error))

                        reader = threading.Thread(target=read, daemon=True)
                        reader.start()
                        stream.sendall(payload)
                        stream.shutdown(socket.SHUT_WR)
                        reader.join(timeout=30)
                        assert not reader.is_alive() and not errors, errors
                        assert received == payload
                    udp_bytes = 0
                    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                        sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 256 * 1024)
                        sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 256 * 1024)
                        sock.settimeout(15)
                        sock.connect(("127.0.0.1", public_udp))
                        for size in (0, 1, 50000, 13):
                            data = secrets.token_bytes(size)
                            sock.send(data)
                            assert sock.recv(65536) == data
                            udp_bytes += size
                    stop(processes[1])
                    stop(processes[0])
                    snapshot = json.loads(state.read_text())
                    totals = {
                        json.dumps(resource, sort_keys=True): traffic
                        for resource, traffic in snapshot["counters"]
                    }
                    expected = {
                        "client": len(payload) + udp_bytes,
                        "tunnel:1": len(payload),
                        "tunnel:2": udp_bytes,
                    }
                    for resource, traffic in snapshot["counters"]:
                        kind = resource["kind"]
                        label = f"tunnel:{resource['id']}" if kind == "tunnel" else kind
                        assert traffic == {
                            "inlet_bytes": expected[label],
                            "export_bytes": expected[label],
                        }, (resource, traffic)
                    assert len(totals) == 3
                    assert all(relay.counts.values()), relay.counts
                    for log in logs:
                        log.flush()
                    assert all(key not in (directory / f"{role}.log").read_text() for role in binaries)
                    result = {
                        "transport": "kcp",
                        "fec": {"data_shards": 10, "parity_shards": 3},
                        "payload": {"compress": True, "encrypt": True},
                        "tcp_roundtrip_bytes": len(payload),
                        "udp_packet_sizes": [0, 1, 50000, 13],
                        "impairment": relay.counts,
                        "sigterm_exit_codes": [p.returncode for p in processes],
                        "counters": snapshot["counters"],
                        "binaries": {
                            role: {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                            for role, path in binaries.items()
                        },
                    }
                    (ROOT / "docs/audit/kcp-runtime.json").write_text(json.dumps(result, indent=2) + "\n")
                    print(json.dumps(result, indent=2))
                finally:
                    for process in processes:
                        if process.poll() is None:
                            process.kill()
                            process.wait(timeout=3)
                    for log in logs:
                        log.close()
                    echo.shutdown()
    finally:
        relay.close()
        udp.close()


if __name__ == "__main__":
    main()
