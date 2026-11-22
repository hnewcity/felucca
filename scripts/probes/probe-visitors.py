#!/usr/bin/env python3
"""Real process checks for private TCP/UDP/SOCKS visitors and reconnect cleanup."""
import hashlib
import http.client
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
PROFILE = os.environ.get("FELUCCA_PROBE_PROFILE", "release")
DIRECT = "--direct" in sys.argv
OUTPUT = ROOT / os.environ.get("FELUCCA_PROBE_OUTPUT", "docs/audit")
BIN = {name: ROOT / "target" / PROFILE / f"felucca-{name}" for name in ["server", "agent"]}


def port():
    with socket.socket() as stream:
        stream.bind(("127.0.0.1", 0))
        return stream.getsockname()[1]


def exact(stream, n):
    output = bytearray()
    while len(output) < n:
        part = stream.recv(n - len(output))
        assert part, "truncated response"
        output.extend(part)
    return bytes(output)


def roundtrip(stream, data):
    errors = []

    def send():
        try:
            stream.sendall(data)
            stream.shutdown(socket.SHUT_WR)
        except Exception as error:
            errors.append(error)

    writer = threading.Thread(target=send, daemon=True)
    writer.start()
    response = bytearray()
    while part := stream.recv(65536):
        response.extend(part)
    writer.join(timeout=5)
    assert not writer.is_alive() and not errors, errors
    assert response == data, (len(response), len(data))


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while data := self.request.recv(16384):
            self.request.sendall(data)


class EchoServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


def command(role, args, data=None):
    return subprocess.run(
        [str(BIN[role]), *args], input=data, capture_output=True, text=True, check=True
    ).stdout.strip()


def main():
    temp = Path(tempfile.mkdtemp(prefix="visitor-probe-", dir=ROOT / "target/tmp"))
    children = []
    success = False
    udp_stop = threading.Event()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(("127.0.0.1", 0))
    udp.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)
    udp.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 262144)
    udp.settimeout(0.1)
    echo = EchoServer(("127.0.0.1", 0), Echo)
    threading.Thread(target=echo.serve_forever, daemon=True).start()

    def udp_echo():
        while not udp_stop.is_set():
            try:
                data, peer = udp.recvfrom(65536)
                udp.sendto(data, peer)
            except socket.timeout:
                pass
    udp_task = threading.Thread(target=udp_echo, daemon=True)
    udp_task.start()

    def config(name, value):
        path = temp / f"{name}.json"
        path.write_text(json.dumps(value))
        path.chmod(0o600)
        return path

    def start(role, path):
        with (temp / f"process-{len(children)}.log").open("wb") as log:
            process = subprocess.Popen([str(BIN[role]), "--config", str(path)], stdout=log, stderr=subprocess.STDOUT)
        children.append(process)
        return process

    def stop(process):
        process.terminate()
        process.wait(timeout=15)
        assert process.returncode == 0, process.returncode

    try:
        bridge, admin, tcp_port, udp_port, socks_port, wrong_port = [port() for _ in range(6)]
        shared = secrets.token_hex(32)
        keys = {name: secrets.token_hex(32) for name in ["provider", "guest", "bad"]}
        token = json.loads(command("server", ["--generate-token"]))
        password_hash = command("server", ["--hash-password"], secrets.token_hex(32) + "\n")
        cert, key = temp / "cert.pem", temp / "key.pem"
        openssl_config = temp / "openssl.cnf"
        openssl_config.write_text("[req]\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\n[ext]\nsubjectAltName=DNS:visitor.test\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
        subprocess.run(
            ["/usr/bin/openssl", "req", "-x509", "-newkey", "rsa:2048",
             "-nodes", "-sha256", "-days", "1", "-config", str(openssl_config), "-subj", "/CN=visitor.test",
             "-keyout", str(key), "-out", str(cert)], check=True, capture_output=True,
        )
        state = temp / "traffic.json"
        server_config = {
            "bridge": {"bind": f"127.0.0.1:{bridge}", "tls": {"cert": str(cert), "key": str(key)}},
            "state": {"path": str(state)},
            "admin": {"bind": f"127.0.0.1:{admin}", "state_path": str(temp / "runtime.json"),
                      "account": {"root": {"role": "admin", "password_hash": password_hash, "token_hash": token["token_hash"]}}},
            "client": {name: {"vkey": value, "allow_config_push": name == "provider"} for name, value in keys.items()},
            "share": {name: {"client": "provider", "tunnel": tunnel, "secret": shared,
                              "allow_clients": ["guest", "bad"], "dynamic": name == "proxy"}
                      for name, tunnel in [("tcp", 1), ("udp", 2), ("proxy", 3)]},
        }
        if DIRECT:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reservation:
                reservation.bind(("127.0.0.1", 0))
                rendezvous = reservation.getsockname()[1]
            server_config["p2p"] = {"bind": f"127.0.0.1:{rendezvous}", "advertise": f"127.0.0.1:{rendezvous}"}
            for share in server_config["share"].values():
                share["direct"] = True
        server_path = config("server", server_config)
        fingerprint = command("server", ["--config", str(server_path), "--fingerprint"])

        def agent(name):
            return {"server": f"127.0.0.1:{bridge}", "key_id": name, "vkey": keys[name], "transport": "tls",
                    "trust": {"mode": "pinned", "fingerprint": fingerprint}, "reconnect_min": "30ms", "reconnect_max": "100ms"}
        provider_config = agent("provider")
        provider_config["tunnel"] = [
            {"id": 1, "mode": "tcp", "public": False, "backend": [{"addr": f"127.0.0.1:{echo.server_address[1]}"}]},
            {"id": 2, "mode": "udp", "public": False, "backend": [{"addr": f"127.0.0.1:{udp.getsockname()[1]}"}]},
            {"id": 3, "mode": "socks5", "public": False, "target_policy": {"allow": ["127.0.0.0/8"]}},
        ]
        guest_config = agent("guest")
        guest_config["visitor"] = [
            {"name": mode, "share": name, "secret": shared, "mode": mode, "bind": f"127.0.0.1:{listen}", "udp_idle": "1s",
             **({"auth": {"users": {"reader": "password"}}} if mode == "socks5" else {})}
            for mode, name, listen in [("tcp", "tcp", tcp_port), ("udp", "udp", udp_port), ("socks5", "proxy", socks_port)]
        ]
        if DIRECT:
            provider_config["p2p"] = {"enabled": True}
            for route in provider_config["tunnel"]:
                route["options"] = {"payload": {"compress": True, "encrypt": True}}
            for local in guest_config["visitor"]:
                local["connection"] = "direct"
        bad_config = agent("bad")
        bad_config["visitor"] = [{"name": "wrong", "share": "tcp", "secret": secrets.token_hex(32), "bind": f"127.0.0.1:{wrong_port}"}]
        server = start("server", server_path)
        provider = start("agent", config("provider", provider_config))
        guest = start("agent", config("guest", guest_config))
        bad = start("agent", config("bad", bad_config))

        def ready():
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                assert all(p.poll() is None for p in [server, provider, guest, bad])
                try:
                    client = http.client.HTTPConnection("127.0.0.1", admin, timeout=1)
                    client.request("GET", "/api/overview", headers={"Authorization": f"Bearer {token['token']}"})
                    response = client.getresponse()
                    view = json.loads(response.read())
                    client.close()
                    if response.status == 200 and all(c["online"] for c in view["clients"]) and len(view["tunnels"]) == 3:
                        assert all(t["spec"]["public"] is False and t["spec"]["port"] == 0 for t in view["tunnels"])
                        time.sleep(0.1)
                        return view["protocol"]
                except (OSError, ValueError):
                    pass
                time.sleep(0.05)
            raise AssertionError("agents did not register")

        protocol = ready()
        data = bytes(range(256)) * 8192
        with socket.create_connection(("127.0.0.1", tcp_port), timeout=10) as stream:
            roundtrip(stream, data)
        sizes = [0, 1, 50000, 13]
        for _ in range(2):
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as local:
                local.settimeout(5)
                local.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)
                local.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 262144)
                for n in sizes:
                    packet = bytes([71]) * n
                    local.sendto(packet, ("127.0.0.1", udp_port))
                    assert local.recvfrom(65536)[0] == packet
        with socket.create_connection(("127.0.0.1", wrong_port), timeout=3) as stream:
            try:
                stream.sendall(b"denied")
                denied = stream.recv(1024)
            except (ConnectionResetError, BrokenPipeError):
                denied = b""
            assert not denied

        def socks():
            stream = socket.create_connection(("127.0.0.1", socks_port), timeout=5)
            stream.sendall(bytes([5, 1, 2]))
            assert exact(stream, 2) == bytes([5, 2])
            stream.sendall(b"\x01\x06reader\x08password")
            assert exact(stream, 2) == bytes([1, 0])
            return stream
        with socks() as stream:
            stream.sendall(bytes([5, 1, 0, 1, 127, 0, 0, 1]) + echo.server_address[1].to_bytes(2, "big"))
            assert exact(stream, 10)[1] == 0
            roundtrip(stream, b"socks")
        with socks() as control:
            control.sendall(bytes([5, 3, 0, 1, 0, 0, 0, 0, 0, 0]))
            answer = exact(control, 10)
            assert answer[:4] == bytes([5, 0, 0, 1])
            relay = (socket.inet_ntoa(answer[4:8]), int.from_bytes(answer[8:], "big"))
            assert relay[0] == "127.0.0.1"
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as local:
                local.settimeout(5)
                local.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)
                local.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 262144)
                header = bytes([0, 0, 0, 1, 127, 0, 0, 1]) + udp.getsockname()[1].to_bytes(2, "big")
                for n in sizes:
                    packet = header + bytes([42]) * n
                    local.sendto(packet, relay)
                    assert local.recvfrom(65536)[0] == packet
        stop(server)
        server = start("server", server_path)
        ready()
        with socket.create_connection(("127.0.0.1", tcp_port), timeout=5) as stream:
            roundtrip(stream, b"restart")
        for child in [bad, guest, provider, server]:
            stop(child)
        counters = json.loads(state.read_text())["counters"]
        expected = len(data) + 3 * sum(sizes) + 5 + 7
        for name in ["provider", "guest"]:
            counts = next(counts for resource, counts in counters if resource == {"kind": "client", "name": name})
            assert counts == {"inlet_bytes": expected, "export_bytes": expected}, (name, counts, expected)
        result = {
            "profile": PROFILE, "protocol": protocol, "direct_only": DIRECT, "private_routes_without_public_ports": 3,
            "tcp_half_close_bytes": len(data), "udp_peer_count": 2, "udp_packet_sizes": sizes,
            "socks_authenticated_connect_and_local_udp": True, "wrong_secret_refused": True,
            "server_restart_preserves_visitor_listener": True,
            "client_payload_bytes_each_direction": expected,
            "sigterm_exit_codes": [child.returncode for child in children],
            "counters": counters,
            "binaries": {name: {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for name, path in BIN.items()},
        }
        OUTPUT.mkdir(parents=True, exist_ok=True)
        (OUTPUT / ("p2p-runtime.json" if DIRECT else "visitor-runtime.json")).write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
        success = True
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=3)
        echo.shutdown()
        echo.server_close()
        udp_stop.set()
        udp_task.join(timeout=1)
        udp.close()
        if success:
            import shutil
            shutil.rmtree(temp)
        else:
            print(f"Diagnostic files retained at {temp}")


if __name__ == "__main__":
    main()
