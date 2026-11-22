#!/usr/bin/env python3
"""Protocol and feature matrix beyond the transports: WebSocket on h2/h3, TLS backends, PROXY protocol, upstream chains, DNS, NTP, IPv6, load balancing, health checks, half-close and quotas.

Each scenario drives a feature with a real client against real felucca
server/agent processes bound to 127.0.0.1 only:

  * WebSocket over HTTP/2 extended CONNECT, and over HTTP/3
  * HTTP/2 all the way to an HTTP/2 backend (backend_http2)
  * TLS backends (pinned, and the webpki refusal that proves checking is real)
  * a real TLS handshake inside an HTTP CONNECT tunnel
  * SOCKS5 / HTTP-proxy authentication with several accounts
  * PROXY protocol v1/v2 egress to TCP and UDP backends, and ingress through a
    relay that puts a header in front of the bridge
  * an upstream SOCKS5 and HTTP proxy chain (with auth and remote_dns),
    custom DNS, local_ip/forward_local_ip, NTP clock calibration
  * IPv6 end to end
  * every load-balancing mode plus active health checks (evict and restore)
  * TCP half-close propagation
  * quotas, rate limits and connection caps

Artifacts land in target/tmp/protocol-features-audit
unless FELUCCA_PROBE_OUTPUT says otherwise.
"""

import base64
import errno
import json
import os
import re
import secrets
import shutil
import signal
import socket
import socketserver
import ssl
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER = ROOT / "target" / "debug" / "felucca-server"
AGENT = ROOT / "target" / "debug" / "felucca-agent"
HERE = ROOT / (
    os.environ.get("FELUCCA_PROBE_OUTPUT")
    or "target/tmp/protocol-features-audit")

def venv_python():
    """Interpreter carrying the optional probe dependencies (aioquic, h2)."""
    candidates = [os.environ.get("FELUCCA_PROBE_PYTHON"),
                  ROOT / "target" / "probe-venv" / "bin" / "python",
                  ROOT / "target" / "tmp" / "h3-venv" / "bin" / "python"]
    for candidate in candidates:
        if not candidate:
            continue
        try:
            probe = subprocess.run([str(candidate), "-c", "import aioquic, h2"],
                                   capture_output=True, timeout=60)
        except OSError:
            continue
        if probe.returncode == 0:
            return Path(candidate)
    return Path(sys.executable)


VENV = venv_python()
RUN = secrets.token_hex(6)


results = []
processes = []
used_ports = set()
notes = []

CLEAN_ENV = {k: v for k, v in os.environ.items()
             if not k.startswith("FELUCCA_") and k.upper() not in
             ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")}
CLEAN_ENV["RUST_LOG"] = "info"
CLEAN_ENV["NO_COLOR"] = "1"
if os.environ.get("MATRIX2_DEBUG"):
    CLEAN_ENV["RUST_LOG"] = os.environ["MATRIX2_DEBUG"]

ANSI = re.compile(r"\x1b\[[0-9;]*m")


def say(message):
    print(message, flush=True)


def check(name, ok, detail=""):
    results.append({"name": name, "ok": bool(ok), "detail": str(detail)[:400]})
    say(("  PASS " if ok else "  FAIL ") + name + (f"  — {detail}" if detail else ""))
    return ok


def note(text):
    notes.append(text)
    say(f"  note  {text}")


def free_port():
    while True:
        # A free TCP port can already belong to an unrelated UDP listener.
        # These matrices allocate both kinds, so check both before returning.
        with socket.socket() as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            tcp.bind(("127.0.0.1", 0))
            port = tcp.getsockname()[1]
            if port in used_ports:
                continue
            try:
                udp.bind(("127.0.0.1", port))
            except OSError as error:
                if error.errno != errno.EADDRINUSE:
                    raise
                continue
            used_ports.add(port)
            return port


def spawn(command, log, env=None):
    handle = log.open("wb")
    process = subprocess.Popen(
        [str(part) for part in command], cwd=str(ROOT), env=env or CLEAN_ENV,
        stdin=subprocess.DEVNULL, stdout=handle, stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    handle.close()
    processes.append(process)
    return process


def py(script, *arguments, timeout=90):
    try:
        return subprocess.run([str(VENV), str(script)] + [str(item) for item in arguments],
                              cwd=str(ROOT), env=CLEAN_ENV, capture_output=True,
                              timeout=timeout)
    except subprocess.TimeoutExpired as error:
        return subprocess.CompletedProcess([], 143,
                                           error.stdout or b"TIMEOUT", b"timed out")


def node(script, *arguments, timeout=60):
    return subprocess.run(["node", str(script)] + [str(item) for item in arguments],
                          cwd=str(ROOT), env=CLEAN_ENV, capture_output=True, timeout=timeout)


def wait_for_log(path, needle, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and needle in ANSI.sub("", path.read_text(errors="replace")):
            return True
        time.sleep(0.1)
    return False


def wait_port(port, family=socket.AF_INET, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.socket(family, socket.SOCK_STREAM) as sock:
                sock.settimeout(1)
                sock.connect(("::1" if family == socket.AF_INET6 else "127.0.0.1", port))
            return True
        except OSError:
            time.sleep(0.1)
    return False


def curl(arguments, timeout=30):
    command = ["curl", "-sS", "--noproxy", "*", "--max-time", str(timeout)] + [
        str(item) for item in arguments]
    done = subprocess.run(command, cwd=str(ROOT), env=CLEAN_ENV,
                          capture_output=True, timeout=timeout + 10)
    return done.returncode, done.stdout.decode(errors="replace"), done.stderr.decode(errors="replace")


def http_get(url, headers=None, timeout=20):
    request = urllib.request.Request(url, headers=headers or {})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=timeout) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()
    except Exception as error:  # noqa: BLE001 - surfaced in the report
        return -1, {}, repr(error).encode()


def recv_exact(sock, count):
    buffer = b""
    while len(buffer) < count:
        chunk = sock.recv(count - len(buffer))
        if not chunk:
            raise AssertionError(f"short read: {buffer!r}")
        buffer += chunk
    return buffer


def finger(cert):
    """SHA-256 of the DER end-entity certificate, as felucca pins it."""
    done = subprocess.run(["openssl", "x509", "-in", str(cert), "-outform", "DER"],
                          check=True, capture_output=True, env=CLEAN_ENV)
    digest = subprocess.run(["openssl", "dgst", "-sha256", "-hex"],
                            input=done.stdout, check=True, capture_output=True,
                            env=CLEAN_ENV).stdout.decode()
    return digest.split("=")[-1].strip().lower()


def cert_pair(name, subject):
    """A self-signed leaf; SAN always matches the subject host."""
    host = subject.split("=")[-1]
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                    "-days", "30", "-subj", subject,
                    "-addext", f"subjectAltName=DNS:{host},IP:127.0.0.1",
                    "-keyout", str(HERE / f"{name}.key"),
                    "-out", str(HERE / f"{name}.pem")], check=True, capture_output=True,
                   env=CLEAN_ENV)
    return HERE / f"{name}.pem", HERE / f"{name}.key"


# --------------------------------------------------------------------------- #
# Backends
# --------------------------------------------------------------------------- #
class Labelled(BaseHTTPRequestHandler):
    """HTTP backend that answers with its own identity and what it received."""
    protocol_version = "HTTP/1.1"
    label = "?"

    def reply(self, payload, content_type="application/json", status=200):
        raw = payload if isinstance(payload, bytes) else payload.encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        state = getattr(self.server, "health_ok", True)
        if self.path.startswith("/healthz"):
            self.reply("ok" if state else "unwell", "text/plain",
                       200 if state else 503)
            return
        if self.path.startswith("/big"):
            self.reply(bytes(range(256)) * 4096, "application/octet-stream")
            return
        self.reply(json.dumps({
            "backend": self.server.label, "path": self.path,
            "proto": self.request_version,
            "xff": self.headers.get("X-Forwarded-For", ""),
            "host": self.headers.get("Host", ""),
            "alpn": self.headers.get("X-Backend-Alpn", ""),
        }))

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.reply(json.dumps({"backend": self.server.label, "posted": True}))

    def do_PUT(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.reply("mutated", "text/plain", status=405)

    def log_message(self, *args):
        pass


class Stop(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def stop(self):
        threading.Thread(target=self.shutdown, daemon=True).start()
        threading.Thread(target=self.server_close, daemon=True).start()


def labelled_backend(label):
    cls = type(f"Labelled{label}", (Labelled,), {"label": label})
    server = Stop(("127.0.0.1", 0), cls)
    server.label = label
    server.health_ok = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class HalfClose(BaseHTTPRequestHandler):
    """Reads to EOF (proving FIN crosses the tunnel) before answering."""
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        seen = 0
        while True:
            chunk = self.rfile.read(4096)
            if not chunk:
                break
            seen += len(chunk)
        self.reply(f"saw-eof-after-{seen}\n")

    def reply(self, text):
        raw = text.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def log_message(self, *args):
        pass


def half_close_backend():
    server = Stop(("127.0.0.1", 0), HalfClose)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def tls_backend(label):
    """A TLS HTTP backend, recording the handshake the agent negotiated."""
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(str(HERE / f"{label}.pem"), str(HERE / f"{label}.key"))
    state = {"version": "", "cipher": "", "alpn": ""}

    class TlsStop(Stop):
        def get_request(self):
            connection, address = self.socket.accept()
            try:
                wrapped = context.wrap_socket(connection, server_side=True)
            except ssl.SSLError as error:
                state["version"] = f"handshake-failed: {error}"
                connection.close()
                raise
            state["version"] = wrapped.version() or ""
            state["cipher"] = wrapped.cipher()[0] if wrapped.cipher() else ""
            state["alpn"] = wrapped.selected_alpn_protocol() or ""
            return wrapped, address

    cls = type(f"Labelled{label}", (Labelled,), {"label": label})
    server = TlsStop(("127.0.0.1", 0), cls)
    server.label = label
    server.state = state
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class ProxyHeader(socketserver.StreamRequestHandler):
    """Consumes a PROXY protocol header, then reports what it parsed."""
    def readline(self):
        line = b""
        while not line.endswith(b"\r\n") and len(line) < 108:
            chunk = self.rfile.read(1)
            if not chunk:
                break
            line += chunk
        return line

    def handle(self):
        first = self.rfile.read(1)
        if not first:
            return
        if first == b"P":
            line = first + self.readline()
            fields = line.split()
            if len(fields) >= 6:
                info = {"version": "v1", "family": fields[1].decode(),
                        "source": fields[2].decode(), "destination": fields[3].decode(),
                        "sport": fields[4].decode(), "dport": fields[5].decode()}
            else:
                info = {"version": "v1", "raw": line.decode(errors="replace")}
        elif first == b"\r":
            signature = first + self.rfile.read(11)
            header = signature + self.rfile.read(4)
            length = int.from_bytes(header[14:16], "big")
            body = self.rfile.read(length) if length else b""
            family = header[13]
            if header[12] != 0x21 or (family & 0x0F) == 0:
                info = {"version": "v2", "command": header[12] != 0x21 and "LOCAL"
                        or "UNSPEC"}
            elif family & 0x0F == 0x1:
                info = {"version": "v2", "family": "TCP" if family & 0x0F == 0x1
                        else "DATAGRAM",
                        "source": socket.inet_ntoa(body[0:4]),
                        "destination": socket.inet_ntoa(body[4:8]),
                        "sport": str(int.from_bytes(body[8:10], "big")),
                        "dport": str(int.from_bytes(body[10:12], "big"))}
            else:
                info = {"version": "v2", "family": "v6",
                        "source": socket.inet_ntop(socket.AF_INET6, body[0:16]),
                        "destination": socket.inet_ntop(socket.AF_INET6, body[16:32]),
                        "sport": str(int.from_bytes(body[32:34], "big")),
                        "dport": str(int.from_bytes(body[34:36], "big"))}
        else:
            info = {"version": "none", "first": (first + self.rfile.read(16)).decode(
                errors="replace")}
        payload = json.dumps(info).encode()
        self.wfile.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                         b"Content-Length: " + str(len(payload)).encode() +
                         b"\r\nConnection: close\r\n\r\n" + payload)

    def handle_error(self, request, client_address):
        pass


def proxy_header_backend():
    server = Stop(("127.0.0.1", 0), ProxyHeader)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class UdpProxyHeader(socketserver.BaseRequestHandler):
    seen = []

    def handle(self):
        data, sock = self.request
        UdpProxyHeader.seen.append(len(data))
        info = {"version": "none"}
        payload = data
        if data.startswith(b"\r\n\r\n\x00\r\nQUIT\n"):
            length = int.from_bytes(data[14:16], "big")
            family = data[13]
            payload = data[16 + length:]
            # The high nibble is the address family (0x10 = IPv4, 0x20 = IPv6);
            # the low nibble is the transport (1 = stream, 2 = datagram).
            if (family & 0xF0) == 0x10:
                info = {"version": "v2", "family": "AF_INET",
                        "source": socket.inet_ntoa(data[16:20]),
                        "destination": socket.inet_ntoa(data[20:24]),
                        "sport": str(int.from_bytes(data[24:26], "big")),
                        "dport": str(int.from_bytes(data[26:28], "big"))}
            elif (family & 0xF0) == 0x20:
                info = {"version": "v2", "family": "AF_INET6",
                        "source": socket.inet_ntop(socket.AF_INET6, data[16:32]),
                        "destination": socket.inet_ntop(socket.AF_INET6, data[32:48]),
                        "sport": str(int.from_bytes(data[48:50], "big")),
                        "dport": str(int.from_bytes(data[50:52], "big"))}
            else:
                info = {"version": "v2", "family": f"unspec-{family:#x}",
                        "dgram": family & 0x0F}
        elif data.startswith(b"PROXY "):
            line = data.split(b"\r\n")[0].decode(errors="replace")
            payload = data.split(b"\r\n", 1)[1] if b"\r\n" in data else b""
            fields = line.split()
            info = {"version": "v1", "source": fields[2],
                    "destination": fields[3], "sport": fields[4], "dport": fields[5]}
        else:
            info = {"version": "none", "first": payload[:32].decode(errors="replace")}
        try:
            sock.sendto(json.dumps(info).encode(), self.client_address)
        except OSError:
            pass


class StopUdp(socketserver.ThreadingUDPServer):
    daemon_threads = True
    allow_reuse_address = True


def udp_proxy_header_backend():
    server = StopUdp(("127.0.0.1", 0), UdpProxyHeader)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


HOSTS = {"bridge.test": "127.0.0.1", "backend.test": "127.0.0.1",
         "felucca.test": "127.0.0.1"}


class Upstream(socketserver.StreamRequestHandler):
    """A SOCKS5 or HTTP CONNECT proxy used as the agent's upstream."""
    log = []

    def handle(self):
        try:
            if self.server.kind == "socks5":
                self.socks()
            else:
                self.http()
        except (OSError, IndexError):
            pass

    def socks(self):
        self.rfile.read(1)                      # version
        methods = self.rfile.read(self.rfile.read(1)[0])
        if self.server.auth:
            if 2 not in methods:
                self.wfile.write(b"\x05\xff")
                Upstream.log.append({"accepted": False, "why": "no-auth-method"})
                return
            self.wfile.write(b"\x05\x02")
            self.rfile.read(1)                 # RFC 1929 version byte
            user = self.rfile.read(self.rfile.read(1)[0])
            password = self.rfile.read(self.rfile.read(1)[0])
            if (user.decode(), password.decode()) != self.server.auth:
                self.wfile.write(b"\x01\x01")
                Upstream.log.append({"accepted": False, "why": "bad-credentials"})
                return
            self.wfile.write(b"\x01\x00")
        else:
            if 0 not in methods:
                self.wfile.write(b"\x05\xff")
                Upstream.log.append({"accepted": False, "why": "no-auth-method"})
                return
            self.wfile.write(b"\x05\x00")
        request = self.rfile.read(4)
        atyp = request[3]
        if atyp == 1:
            host = socket.inet_ntoa(self.rfile.read(4))
        elif atyp == 3:
            host = self.rfile.read(self.rfile.read(1)[0]).decode()
        else:
            host = socket.inet_ntop(socket.AF_INET6, self.rfile.read(16))
        port = int.from_bytes(self.rfile.read(2), "big")
        try:
            upstream = socket.create_connection((HOSTS.get(host, host), port),
                                                timeout=10)
        except OSError as error:
            self.wfile.write(b"\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00")
            Upstream.log.append({"accepted": False, "why": f"dial {error}"})
            return
        self.wfile.write(b"\x05\x00\x00\x01" + socket.inet_aton("127.0.0.1")
                         + (upstream.getsockname()[1]).to_bytes(2, "big"))
        Upstream.log.append({"accepted": True, "kind": "socks5", "target": f"{host}:{port}"})
        self.pump(upstream)

    def http(self):
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = self.connection.recv(4096)
            if not chunk:
                return
            head += chunk
        lines = head.decode(errors="replace").split("\r\n")
        target = lines[0].split()[1]
        auth = next((line for line in lines
                     if line.lower().startswith("proxy-authorization:")), None)
        if auth is None:
            self.wfile.write(b"HTTP/1.1 407 Proxy Authentication Required\r\n"
                             b"Proxy-Authenticate: Basic realm=\"upstream\"\r\n"
                             b"Content-Length: 0\r\n\r\n")
            challenge = self.connection.recv(65535)
            for line in challenge.decode(errors="replace").split("\r\n"):
                if line.lower().startswith("proxy-authorization:"):
                    auth = line
        if self.server.auth:
            if not auth:
                Upstream.log.append({"accepted": False, "why": "no-credentials"})
                return
            encoded = auth.split(" ", 2)[-1].strip()
            decoded = base64.b64decode(encoded).decode()
            if tuple(decoded.split(":", 1)) != self.server.auth:
                self.wfile.write(b"HTTP/1.1 407 Proxy Authentication Required\r\n"
                                 b"Content-Length: 0\r\n\r\n")
                Upstream.log.append({"accepted": False, "why": "bad-credentials"})
                return
        host, _, port = target.rpartition(":")
        try:
            upstream = socket.create_connection(
                (HOSTS.get(host, host), int(port)), timeout=10)
        except OSError as error:
            self.wfile.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            Upstream.log.append({"accepted": False, "why": f"dial {error}"})
            return
        self.wfile.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
        Upstream.log.append({"accepted": True, "kind": "http", "target": target})
        self.pump(upstream)

    def pump(self, upstream):
        def copy_one(source, sink):
            try:
                while True:
                    chunk = source.recv(65535)
                    if not chunk:
                        break
                    sink.sendall(chunk)
            except OSError:
                pass
            try:
                sink.shutdown(socket.SHUT_WR)
            except OSError:
                pass
        down = threading.Thread(target=copy_one, args=(self.connection, upstream), daemon=True)
        up = threading.Thread(target=copy_one, args=(upstream, self.connection), daemon=True)
        down.start()
        up.start()
        up.join()
        down.join(timeout=1)

    def handle_error(self, request, client_address):
        pass


def upstream_backend(kind, auth=None):
    cls = type(f"Upstream{kind}", (Upstream,), {})
    server = Stop(("127.0.0.1", 0), cls)
    server.kind = kind
    server.auth = auth
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class Dns(socketserver.BaseRequestHandler):
    """Minimal UDP DNS responder, answering from a fixed table."""
    state = {}

    def handle(self):
        data, sock = self.request
        if len(data) < 12:
            return
        identifier, flags, questions = struct.unpack_from("!HHH", data, 0)
        offset = 12
        labels = []
        while data[offset] != 0:
            length = data[offset]
            labels.append(data[offset + 1:offset + 1 + length].decode("latin1"))
            offset += 1 + length
        name = ".".join(labels)
        offset += 1
        qtype, qclass = struct.unpack_from("!HH", data, offset)
        answer = self.state.get(name)
        header = struct.pack("!HHHHHH", identifier, 0x8180 if answer else 0x8183,
                             1, 1 if answer else 0, 0, 0)
        question = data[12:offset + 4]
        if answer is None:
            Dns.state.setdefault("asked", []).append(name)
            sock.sendto(header + question, self.client_address)
            return
        Dns.state.setdefault("asked", []).append(name)
        body = answer
        record = (b"\xc0\x0c" + struct.pack("!HHIH", 1, 1, 30, len(body)) + body)
        sock.sendto(header + question + record, self.client_address)


class StopDns(socketserver.ThreadingUDPServer):
    daemon_threads = True
    allow_reuse_address = True


def dns_backend(records):
    cls = type("DnsTable", (Dns,), {"state": {name: socket.inet_aton(ip)
                                              for name, ip in records.items()}})
    server = StopDns(("127.0.0.1", 0), cls)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class Ntp(socketserver.BaseRequestHandler):
    """NTP server whose clock is deliberately offset."""
    offset = 0

    def handle(self):
        data, sock = self.request
        if len(data) < 48:
            return
        stamp = bytearray(data[40:48])          # client transmit timestamp
        shifted = self.offset
        seconds = int.from_bytes(stamp[0:4], "big") + shifted
        fraction = int.from_bytes(stamp[4:8], "big")
        reply = bytearray(48)
        reply[0] = (4 << 3) | 4                 # LI=0, VN=4, mode=server
        reply[1] = 1                            # stratum
        reply[2] = 3                            # poll
        reply[3] = 0xE9                         # precision
        reply[4:12] = b"\x00\x00\x00\x01\x00\x00\x00\x01"
        reply[12:16] = b"LOCL"                  # reference id
        reference = int(time.time()) + 0x83AA7E80
        reply[16:24] = struct.pack("!II", reference, 0)
        reply[24:32] = bytes(stamp)             # originate (must echo the client)
        reply[32:40] = struct.pack("!II", seconds, fraction)    # receive
        reply[40:48] = struct.pack("!II", seconds, fraction)    # transmit
        sock.sendto(bytes(reply), self.client_address)


class StopNtp(socketserver.ThreadingUDPServer):
    daemon_threads = True
    allow_reuse_address = True


def ntp_backend(offset):
    cls = type("NtpOffset", (Ntp,), {"offset": offset})
    server = StopNtp(("127.0.0.1", 0), cls)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


WS_EXTENDED = """\
import http2 from 'node:http2';
import fs from 'node:fs';
import net from 'node:net';
const [port, cert, key, scheme] = process.argv.slice(2);
const options = {
  cert: fs.readFileSync(cert), key: fs.readFileSync(key),
  settings: { enableConnectProtocol: true },
};
if (scheme === 'h2') { options.allowHTTP1 = false; options.ALPNProtocols = ['h2']; }
const server = http2.createServer(options);
server.on('stream', (stream, headers) => {
  if (headers[':method'] === 'CONNECT') {
    stream.respond({ ':status': 200 });
    stream.on('data', (chunk) => stream.write(chunk));
    stream.on('end', () => stream.end());
    stream.on('error', () => {});
    return;
  }
  stream.respond({ ':status': 200, 'content-type': 'text/plain' });
  stream.end('h2-extended-plain\\n');
});
server.listen(Number(port), '127.0.0.1', () => console.log('h2x ready ' + port));
"""


def ws_extended_backend(cert, key, scheme):
    """A plain TCP HTTP/2 backend that accepts RFC 9220 extended CONNECT."""
    script = HERE / f"ws-ext-{scheme}.mjs"
    script.write_text(WS_EXTENDED, encoding="utf-8")
    log = HERE / f"ws-ext-{scheme}.log"
    port = free_port()
    spawn(["node", script, port, cert, key, scheme], log)
    if not wait_port(port):
        return 0
    return port


# --------------------------------------------------------------------------- #
# Server / agent plumbing
# --------------------------------------------------------------------------- #
SCENARIOS = []


def scenario(function):
    SCENARIOS.append(function)
    return function


def mark_chapter(title):
    say(f"\n=== {title} ===")


def scenarios():
    only = [item.strip() for item in
            os.environ.get("MATRIX2_ONLY", "").split(",") if item.strip()]
    for function in SCENARIOS:
        if only and not any(item in function.__name__ for item in only):
            continue
        mark_chapter(function.__name__)
        try:
            function()
        finally:
            cleanup()


def cleanup():
    for process in reversed(processes):
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
    deadline = time.monotonic() + 6
    for process in reversed(processes):
        if process.poll() is None:
            try:
                process.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
    processes.clear()


def report():
    passed = sum(1 for item in results if item["ok"])
    say("\n" + "=" * 72)
    say(f"extended matrix: {passed}/{len(results)} checks passed")
    for item in results:
        if not item["ok"]:
            say(f"  FAIL {item['name']}  — {item['detail']}")
    if notes:
        say("\nnotes:")
        for text in notes:
            say(f"  - {text}")
    payload = {"run": RUN, "passed": passed, "total": len(results),
               "results": results, "notes": notes}
    (HERE / f"matrix2-{RUN}.json").write_text(json.dumps(payload, indent=2), encoding="utf-8")


def bridge_block(port, quic, kcp, ws_path, extra=""):
    block = f'''[bridge]
bind = "127.0.0.1:{port}"
quic_bind = "127.0.0.1:{port}"
kcp_bind = "127.0.0.1:{kcp}"
tls = {{ cert = "{HERE}/bridge.pem", key = "{HERE}/bridge.key" }}
ws_path = "{ws_path}"

[bridge.kcp]
data_shards = 10
parity_shards = 3
mtu = 1200
send_window = 256
receive_window = 256
interval_ms = 20
max_sessions = 128
{extra}'''
    return block


def http_block(entry, hosts, extra=""):
    lines = ["[http]", f'bind = "127.0.0.1:{entry}"',
             f'tls_bind = "127.0.0.1:{entry}"', f'http3_bind = "127.0.0.1:{entry}"',
             "", "[http.certificates]"]
    for host in hosts:
        lines.append(f'"{host}" = {{ cert = "{HERE}/{host}.pem", key = "{HERE}/{host}.key" }}')
    return "\n".join(lines) + "\n" + extra


def start_server(name, config_text, expect="felucca-server ready", timeout=40):
    path = HERE / f"{name}.toml"
    path.write_text(config_text, encoding="utf-8")
    log = HERE / f"{name}.log"
    spawn([SERVER, "--config", path], log)
    if not wait_for_log(log, expect, timeout):
        say(f"--- {name}.log ---\n{log.read_text(errors='replace')[-2500:]}")
        check(f"fixture {name}: server startup", False,
              log.read_text(errors="replace")[-1000:])
        return False
    return True


def server_fingerprint(config):
    return subprocess.run([str(SERVER), "--config", str(config), "--fingerprint"],
                          cwd=str(ROOT), env=CLEAN_ENV, capture_output=True,
                          text=True, timeout=60).stdout.strip()


def start_agent(name, config_text, timeout=45, expect="tunnels registered", allow_failure=False):
    path = HERE / f"{name}.toml"
    path.write_text(config_text, encoding="utf-8")
    log = HERE / f"{name}.log"
    spawn([AGENT, "--config", path], log)
    if not wait_for_log(log, expect, timeout):
        say(f"--- {name}.log ---\n{log.read_text(errors='replace')[-1200:]}")
        if not allow_failure:
            check(f"fixture {name}: agent registration", False,
                  log.read_text(errors="replace")[-1000:])
        return False
    refused = "the server refused a tunnel"
    if refused in ANSI.sub("", log.read_text(errors="replace")):
        say(f"--- {name}.log ---\n{log.read_text(errors='replace')[-1200:]}")
        if not allow_failure:
            check(f"fixture {name}: tunnel registration", False,
                  log.read_text(errors="replace")[-1000:])
        return False
    return True


def agent_authenticated(log, key_id, timeout=45):
    return wait_for_log(log, key_id, timeout)


WS_H1 = """\
import http from 'node:http';
import crypto from 'node:crypto';
const port = Number(process.argv[2]);
const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
function send(socket, payload, opcode) {
  let header;
  if (payload.length < 126) { header = Buffer.alloc(2); header[1] = payload.length; }
  else if (payload.length < 65536) {
    header = Buffer.alloc(4); header[1] = 126; header.writeUInt16BE(payload.length, 2);
  } else {
    header = Buffer.alloc(10); header[1] = 127; header.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  header[0] = 0x80 | opcode;
  socket.write(Buffer.concat([header, payload]));
}
const server = http.createServer((request, response) => {
  response.writeHead(200, { 'content-type': 'text/plain' });
  response.end('ws-backend-http\\n');
});
server.on('upgrade', (request, socket) => {
  const key = request.headers['sec-websocket-key'];
  if (!request.url.startsWith('/ws') || !key) { socket.destroy(); return; }
  const accept = crypto.createHash('sha1').update(key + GUID).digest('base64');
  socket.write('HTTP/1.1 101 Switching Protocols\\r\\nUpgrade: websocket\\r\\n' +
    'Connection: Upgrade\\r\\nSec-WebSocket-Accept: ' + accept + '\\r\\n\\r\\n');
  let buffer = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      if (buffer.length < 2) return;
      const opcode = buffer[0] & 0x0f;
      const masked = (buffer[1] & 0x80) !== 0;
      let length = buffer[1] & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (buffer.length < 4) return;
        length = buffer.readUInt16BE(2); offset = 4;
      } else if (length === 127) {
        if (buffer.length < 10) return;
        length = Number(buffer.readBigUInt64BE(2)); offset = 10;
      }
      let mask = null;
      if (masked) {
        if (buffer.length < offset + 4) return;
        mask = buffer.subarray(offset, offset + 4); offset += 4;
      }
      if (buffer.length < offset + length) return;
      const payload = Buffer.from(buffer.subarray(offset, offset + length));
      if (mask) for (let i = 0; i < payload.length; i += 1) payload[i] ^= mask[i % 4];
      buffer = buffer.subarray(offset + length);
      if (opcode === 0x8) { socket.end(); return; }
      if (opcode === 0x9) { send(socket, payload, 0xa); continue; }
      if (opcode === 0xa) continue;
      send(socket, payload, opcode === 0x1 ? 0x1 : 0x2);
    }
  });
  socket.on('error', () => {});
});
server.listen(port, '127.0.0.1', () => console.log('ws-h1 ready ' + port));
"""

H2_WS_CLIENT = """\
# WebSocket over an HTTP/2 extended CONNECT, driven with the h2 library.
import socket, ssl, sys, time
import h2.connection, h2.config, h2.events

port, authority, path, frame = int(sys.argv[1]), sys.argv[2], sys.argv[3], \\
    sys.argv[4].encode().decode('unicode_escape').encode('latin1')

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE
ctx.set_alpn_protocols(['h2'])
sock = ctx.wrap_socket(socket.create_connection(('127.0.0.1', port), 10),
                       server_hostname=authority)
conn = h2.connection.H2Connection(
    config=h2.config.H2Configuration(client_side=True, header_encoding='utf-8'))
conn.initiate_connection()
sock.sendall(conn.data_to_send())
state = {'status': None, 'echo': b''}


def drain(seconds):
    sock.settimeout(seconds)
    out = b''
    while True:
        try:
            chunk = sock.recv(65535)
        except socket.timeout:
            break
        if not chunk:
            break
        out += chunk
        sock.settimeout(0.2)
        try:
            while True:
                more = sock.recv(65535)
                if not more:
                    break
                out += more
        except socket.timeout:
            pass
        break
    return out


def pump(data):
    for event in conn.receive_data(data):
        if isinstance(event, h2.events.ResponseReceived):
            state['status'] = dict(event.headers).get(':status')
        elif isinstance(event, h2.events.DataReceived):
            state['echo'] += event.data
            conn.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
    sock.sendall(conn.data_to_send())


pump(drain(1.0))
sid = conn.get_next_available_stream_id()
conn.send_headers(sid, [(':method', 'CONNECT'), (':scheme', 'https'),
                        (':authority', authority), (':path', path),
                        (':protocol', 'websocket'),
                        ('sec-websocket-version', '13')])
sock.sendall(conn.data_to_send())
conn.send_data(sid, frame)
sock.sendall(conn.data_to_send())
deadline = time.time() + 15
while time.time() < deadline:
    data = drain(1.0)
    if not data:
        continue
    pump(data)
    if state['echo'].startswith(frame):
        print(state['status'], state['echo'].hex())
        sys.exit(0 if state['status'] == '200' else 4)
print('TIMEOUT', state['status'], state['echo'].hex())
sys.exit(3)
"""

H3_WS_CLIENT = """\
# WebSocket over an HTTP/3 extended CONNECT, driven with aioquic.
import asyncio, ssl, sys
from aioquic.asyncio import QuicConnectionProtocol, connect
from aioquic.h3.connection import H3Connection
from aioquic.h3.events import DataReceived, HeadersReceived
from aioquic.quic.events import ConnectionTerminated, StreamDataReceived
from aioquic.quic.configuration import QuicConfiguration


class Client(QuicConnectionProtocol):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.http = H3Connection(self._quic)
        self.frames = asyncio.get_running_loop().create_future()
        self.status = 0
        self.buffer = b''

    def quic_event_received(self, event):
        for http_event in self.http.handle_event(event):
            self.http_event_received(http_event)
        if isinstance(event, ConnectionTerminated) and not self.frames.done():
            self.frames.set_exception(ConnectionError(str(event)))

    def http_event_received(self, event):
        if isinstance(event, HeadersReceived):
            for key, value in event.headers:
                if key == b':status':
                    self.status = int(value)
        elif isinstance(event, DataReceived):
            self.buffer += event.data
            if self.buffer and not self.frames.done():
                self.frames.set_result(self.buffer)


async def main():
    port, authority, path, frame = (int(sys.argv[1]), sys.argv[2], sys.argv[3],
                                    sys.argv[4].encode().decode('unicode_escape')
                                    .encode('latin1'))
    config = QuicConfiguration(is_client=True, alpn_protocols=['h3'])
    config.verify_mode = ssl.CERT_NONE
    config.server_name = authority
    async with connect('127.0.0.1', port, configuration=config,
                       create_protocol=Client) as proto:
        sid = proto._quic.get_next_available_stream_id()
        proto.http.send_headers(sid, [(b':method', b'CONNECT'), (b':scheme', b'https'),
                                      (b':authority', authority.encode()),
                                      (b':path', path.encode()),
                                      (b':protocol', b'websocket'),
                                      (b'sec-websocket-version', b'13')])
        proto.http.send_data(sid, frame, end_stream=False)
        proto.transmit()
        try:
            echo = await asyncio.wait_for(proto.frames, 15)
        except (asyncio.TimeoutError, ConnectionError) as error:
            print('TIMEOUT', type(error).__name__)
            return 3
        ok = echo.startswith(frame) and proto.status == 200
        print(proto.status, echo.hex())
        return 0 if ok else 4


sys.exit(asyncio.run(main()))
"""


@scenario
def websocket_over_h2_and_h3():
    """RFC 9220 WebSocket on an HTTP/2 entry and on an HTTP/3 entry."""
    mark_chapter("websocket over h2 / h3 entry")
    hosts = ["h1ws.test", "h2ws.test", "h3ws.test"]
    for host in hosts:
        cert_pair(host, f"/CN={host}")

    ws_h1 = free_port()
    (HERE / "ws-h1.mjs").write_text(WS_H1, encoding="utf-8")
    spawn(["node", HERE / "ws-h1.mjs", ws_h1], HERE / "ws-h1.log")
    wait_port(ws_h1)
    extended = ws_extended_backend(HERE / "bridge.pem", HERE / "bridge.key", "h2")
    if not extended:
        return
    say(f"backends: h1 websocket 127.0.0.1:{ws_h1}   extended-connect 127.0.0.1:{extended}")

    bridge = free_port()
    entry = free_port()
    vkey = secrets.token_hex(32)
    server_toml = (
        bridge_block(bridge, bridge, free_port(), "/ws")
        + "\n\n" + http_block(entry, hosts))
    for key_id in ("a-ws",):
        server_toml += f'\n[client.{key_id}]\nvkey = "{vkey}"\nallow_config_push = true\n'
    if not start_server("srv-ws", server_toml):
        return
    fingerprint = server_fingerprint(HERE / "srv-ws.toml")
    trust = f'[trust]\nmode = "pinned"\nfingerprint = "{fingerprint}"\n'

    agent_toml = f'''server = "127.0.0.1:{bridge}"
key_id = "a-ws"
vkey = "{vkey}"
transport = "tcp"
{trust}
[[tunnel]]
id = 1
mode = "http"
port = {entry}
hosts = ["h1ws.test"]
[[tunnel.backend]]
addr = "127.0.0.1:{ws_h1}"

[[tunnel]]
id = 2
mode = "http"
port = {entry}
hosts = ["h2ws.test"]
[tunnel.options.http]
backend_http2 = true
[[tunnel.backend]]
addr = "127.0.0.1:{extended}"

[[tunnel]]
id = 3
mode = "http"
port = {entry}
hosts = ["h3ws.test"]
[[tunnel.backend]]
addr = "127.0.0.1:{ws_h1}"
'''
    if not start_agent("a-ws", agent_toml):
        return

    frame = r"\x81\x05hello"
    plain = b"\x81\x05hello"
    client = HERE / "h2ws.py"
    client.write_text(H2_WS_CLIENT, encoding="utf-8")

    run = py(client, entry, "h1ws.test", "/ws", frame)
    check("websocket: HTTP/2 extended CONNECT entry -> HTTP/1.1 upgrade backend",
          run.returncode == 0 and run.stdout.split()[1:2] == [plain.hex().encode()],
          f"rc={run.returncode} {run.stdout!r} {run.stderr.decode()[-200:]}")

    run = py(client, entry, "h2ws.test", "/ws", frame)
    check("websocket: HTTP/2 entry speaks HTTP/2 extended CONNECT to the backend",
          run.returncode == 0 and run.stdout.split()[1:2] == [plain.hex().encode()],
          f"rc={run.returncode} {run.stdout!r} {run.stderr.decode()[-200:]}")

    h3 = HERE / "h3ws.py"
    h3.write_text(H3_WS_CLIENT, encoding="utf-8")
    run = py(h3, entry, "h3ws.test", "/ws", frame)
    check("websocket: HTTP/3 extended CONNECT entry -> HTTP/1.1 upgrade backend",
          run.returncode == 0 and run.stdout.split()[1:2] == [plain.hex().encode()],
          f"rc={run.returncode} {run.stdout!r} {run.stderr.decode()[-200:]}")


def http_once(port, path="/api/ping", host="x", timeout=15):
    """One HTTP/1.1 request; a refused tunnel is an empty body, not an error."""
    data = b""
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=timeout) as sock:
            sock.sendall((f"GET {path} HTTP/1.1\r\nHost: {host}\r\n"
                          "Connection: close\r\n\r\n").encode())
            while True:
                chunk = sock.recv(65535)
                if not chunk:
                    break
                data += chunk
    except OSError:
        pass
    return data.split(b"\r\n\r\n", 1)[-1] if b"\r\n\r\n" in data else b""


def hub(tag, hosts=()):
    """Ports for a private server; `tag` only names the artifacts."""
    return {"tag": tag, "hosts": list(hosts), "bridge": free_port(),
            "entry": free_port(), "kcp": free_port(),
            "vkey": secrets.token_hex(32)}


def hub_start(state, vkey, clients, hosts, bridge_extra="", http_extra=""):
    cert_hosts = list(hosts or state["hosts"]) or ["felucca.test"]
    for host in cert_hosts:
        cert_pair(host, f"/CN={host}")
    lines = [f'[bridge]', f'bind = "127.0.0.1:{state["bridge"]}"',
             f'quic_bind = "127.0.0.1:{state["bridge"]}"',
             f'kcp_bind = "127.0.0.1:{state["kcp"]}"',
             f'tls = {{ cert = "{HERE}/bridge.pem", key = "{HERE}/bridge.key" }}',
             'ws_path = "/ws"', '', '[bridge.kcp]', 'data_shards = 10',
             'parity_shards = 3', 'mtu = 1200', 'send_window = 256',
             'receive_window = 256', 'interval_ms = 20', 'max_sessions = 128',
             bridge_extra]
    text = "\n".join(lines) + "\n\n" + http_block(state["entry"], cert_hosts)
    text += http_extra + "\n"
    for key_id in sorted(clients):
        text += f'\n[client.{key_id}]\nvkey = "{vkey}"\nallow_config_push = true\n'
    return text


def agent_head(server_port, key_id, vkey, transport="tcp", fingerprint=None,
               extra=""):
    trust = (f'[trust]\nmode = "pinned"\nfingerprint = "{fingerprint}"\n'
             if fingerprint else '[trust]\nmode = "insecure"\n')
    return (f'server = "127.0.0.1:{server_port}"\nkey_id = "{key_id}"\n'
            f'vkey = "{vkey}"\ntransport = "{transport}"\n{trust}{extra}')


def log_text(name):
    path = HERE / f"{name}.log"
    return ANSI.sub("", path.read_text(errors="replace")) if path.exists() else ""


# --------------------------------------------------------------------------- #
# TLS backends
# --------------------------------------------------------------------------- #
TLS_BACKEND_AGENT = '''
[[tunnel]]
id = 1
mode = "tcp"
port = {port}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend}"
tls = true
{trust}
'''

TRUST_PINNED = '''[tunnel.backend.trust]
mode = "pinned"
fingerprint = "{pinned}"'''


def tls_backend_scenario(tag, trust_block):
    """`trust_block` may be a callable, so a pin can name the leaf in use."""
    state = hub(tag)
    if not start_server(f"srv-{tag}",
                        hub_start(state, state["vkey"], [f"a-{tag}"], [])):
        return None
    cert_pair("bepin", "/CN=bepin.test")
    backend = tls_backend("bepin")
    fingerprint = server_fingerprint(HERE / f"srv-{tag}.toml")
    port = free_port()
    if callable(trust_block):
        trust_block = trust_block()
    agent = agent_head(state["bridge"], f"a-{tag}", state["vkey"], "tcp",
                       fingerprint) + TLS_BACKEND_AGENT.format(
                           port=port, backend=backend.server_address[1],
                           trust=trust_block)
    if not start_agent(f"a-{tag}", agent):
        return None
    return http_once(port, host="bepin.test"), backend, port


@scenario
def tls_backend_pinned():
    mark_chapter("TLS backend: pinned fingerprint")
    outcome = tls_backend_scenario("bepin", lambda: TRUST_PINNED.format(
        pinned=finger(HERE / "bepin.pem")))
    if outcome is None:
        return
    body, backend, port = outcome
    try:
        info = json.loads(body)
    except ValueError:
        info = {}
    check("backend TLS: a pinned fingerprint completes the tunnel",
          info.get("backend") == "bepin" and info.get("path") == "/api/ping",
          f"{body[:200]!r}")
    check("backend TLS: the agent really negotiated TLS 1.3 to the backend",
          backend.state["version"] == "TLSv1.3",
          f"negotiated={backend.state['version']!r} "
          f"cipher={backend.state['cipher']!r}")


@scenario
def tls_backend_wrong_pin():
    mark_chapter("TLS backend: wrong fingerprint")
    outcome = tls_backend_scenario("bebad", TRUST_PINNED.format(pinned="0" * 64))
    if outcome is None:
        return
    body, backend, port = outcome
    check("backend TLS: a wrong fingerprint drops the tunnel, silently to the client",
          body == b"", f"reply {body[:160]!r}")


@scenario
def tls_backend_webpki_refusal():
    mark_chapter("TLS backend: webpki refusal")
    outcome = tls_backend_scenario("beweb", "")
    if outcome is None:
        return
    body, backend, port = outcome
    check("backend TLS: webpki mode rejects a certificate no CA vouches for",
          body == b"", f"reply {body[:160]!r}")


@scenario
def connect_then_real_tls():
    mark_chapter("HTTP CONNECT tunnel carrying a real TLS handshake")
    state = hub("ctls")
    if not start_server("srv-ctls", hub_start(state, state["vkey"], ["a-ctls"],
                                             [])):
        return
    cert_pair("bepin", "/CN=bepin.test")
    backend = tls_backend("bepin")
    fingerprint = server_fingerprint(HERE / "srv-ctls.toml")
    port = free_port()
    agent = agent_head(state["bridge"], "a-ctls", state["vkey"], "tcp",
                       fingerprint) + f'''
[[tunnel]]
id = 1
mode = "httpproxy"
port = {port}
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"
'''
    if not start_agent("a-ctls", agent):
        return
    script = HERE / "connect-tls.py"
    script.write_text('''\
# CONNECT through the proxy tunnel, then a genuine TLS handshake on the pipe.
import socket, ssl, sys
port, target = int(sys.argv[1]), int(sys.argv[2])
raw = socket.create_connection(("127.0.0.1", port), timeout=10)
raw.sendall(f"CONNECT 127.0.0.1:{target} HTTP/1.1\\r\\n"
            f"Host: 127.0.0.1:{target}\\r\\n\\r\\n".encode())
head = b""
while b"\\r\\n\\r\\n" not in head:
    chunk = raw.recv(4096)
    if not chunk:
        print("CONNECT-CLOSED")
        sys.exit(4)
    head += chunk
if b" 200" not in head.split(b"\\r\\n")[0]:
    print("CONNECT-REFUSED " + head.split(b"\\r\\n")[0].decode())
    sys.exit(4)
context = ssl.create_default_context()
context.check_hostname = False
context.verify_mode = ssl.CERT_NONE
tls = context.wrap_socket(raw, server_hostname="bepin.test")
negotiated = tls.version() or "no-tls"
tls.sendall(b"GET /api/ping HTTP/1.1\\r\\nHost: bepin.test\\r\\n"
            b"Connection: close\\r\\n\\r\\n")
body = b""
while True:
    chunk = tls.recv(65535)
    if not chunk:
        break
    body += chunk
# version() reads as None once the peer's close unwraps the socket, so it is
# captured at handshake time.
print(negotiated + " " + body.split(b"\\r\\n\\r\\n", 1)[-1].decode())
''', encoding="utf-8")
    run = py(script, port, backend.server_address[1])
    line = run.stdout.decode(errors="replace").strip()
    check("CONNECT tunnel: a real TLS handshake and HTTP request pass through",
          line.startswith("TLSv1.3") and '"bepin"' in line,
          f"rc={run.returncode} {line[:220]} {run.stderr.decode()[-200:]}")


@scenario
def proxy_authentication():
    mark_chapter("SOCKS5 and HTTP-proxy authentication")
    state = hub("pauth")
    if not start_server("srv-pauth", hub_start(state, state["vkey"], ["a-pauth"],
                                              [])):
        return
    backend = labelled_backend("pauth")
    fingerprint = server_fingerprint(HERE / "srv-pauth.toml")
    ports = {name: free_port() for name in ("socks", "http", "mixed")}
    agent = agent_head(state["bridge"], "a-pauth", state["vkey"], "tcp",
                       fingerprint)
    for tunnel_id, (name, mode) in enumerate((("socks", "socks5"),
                                              ("http", "httpproxy"),
                                              ("mixed", "mixedproxy")), start=1):
        agent += f'''
[[tunnel]]
id = {tunnel_id}
mode = "{mode}"
port = {ports[name]}
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"
[tunnel.options.auth.users]
alice = "alpine"
bob = "bunker"
'''
    if not start_agent("a-pauth", agent):
        return

    script = HERE / "socksauth.py"
    script.write_text('''\
# A SOCKS5 client with RFC 1929 credentials, plus the refusal paths.
import socket, sys
port, user, password, target = sys.argv[1:5]
port, target = int(port), int(target)
sock = socket.create_connection(("127.0.0.1", port), timeout=10)
methods = [0, 2] if user else [0]
sock.sendall(bytes([5, len(methods)]) + bytes(methods))
choice = sock.recv(2)
if choice == b"\\x05\\xff":
    print("NO-METHOD")
    sys.exit(3)
if user:
    sock.sendall(bytes([1, len(user)]) + user.encode()
                 + bytes([len(password)]) + password.encode())
    if sock.recv(2) != b"\\x01\\x00":
        print("AUTH-REFUSED")
        sys.exit(3)
sock.sendall(b"\\x05\\x01\\x00\\x01" + socket.inet_aton("127.0.0.1")
             + target.to_bytes(2, "big"))
reply = b""
while len(reply) < 10:
    chunk = sock.recv(10 - len(reply))
    if not chunk:
        break
    reply += chunk
if reply[1:2] != b"\\x00":
    print("CONNECT-REFUSED")
    sys.exit(4)
sock.sendall(b"GET /api/ping HTTP/1.1\\r\\nHost: pauth\\r\\n"
             b"Connection: close\\r\\n\\r\\n")
body = b""
while True:
    chunk = sock.recv(65535)
    if not chunk:
        break
    body += chunk
print(body.split(b"\\r\\n\\r\\n", 1)[-1].decode())
''', encoding="utf-8")
    target = backend.server_address[1]
    for name, secret in (("alice", "alpine"), ("bob", "bunker")):
        run = py(script, ports["socks"], name, secret, target)
        check(f"SOCKS5: account {name} authenticates and forwards",
              run.returncode == 0 and b'"pauth"' in run.stdout,
              f"rc={run.returncode} {run.stdout!r} {run.stderr.decode()[-160:]}")
    run = py(script, ports["socks"], "alice", "wrong", target)
    check("SOCKS5: a wrong password gets an authentication refusal",
          run.returncode == 3 and b"AUTH-REFUSED" in run.stdout,
          f"rc={run.returncode} {run.stdout!r}")
    run = py(script, ports["socks"], "carol", "x", target)
    check("SOCKS5: an unknown account is refused the same way",
          run.returncode == 3 and b"AUTH-REFUSED" in run.stdout,
          f"rc={run.returncode} {run.stdout!r}")
    run = py(script, ports["socks"], "", "", target)
    check("SOCKS5: once accounts exist, the no-auth method is withdrawn",
          run.returncode == 3 and b"NO-METHOD" in run.stdout,
          f"rc={run.returncode} {run.stdout!r}")

    with socket.create_connection(("127.0.0.1", ports["http"]), timeout=10) as sock:
        sock.sendall(b"GET http://127.0.0.1:%d/api/ping HTTP/1.1\r\n"
                     b"Host: pauth\r\nConnection: close\r\n\r\n"
                     % backend.server_address[1])
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = sock.recv(4096)
            if not chunk:
                break
            head += chunk
    check("HTTP proxy: no credentials gives 407 with a challenge",
          head.startswith(b"HTTP/1.1 407")
          and b"proxy-authenticate: basic" in head.lower(),
          f"{head[:220]!r}")
    helper = HERE / "httpauth.py"
    helper.write_text('''\
import sys, urllib.request, urllib.error
port, user, password, target = sys.argv[1:5]
opener = urllib.request.build_opener(urllib.request.ProxyHandler(
    {"http": f"http://{user}:{password}@127.0.0.1:{port}"}))
try:
    with opener.open(f"http://127.0.0.1:{target}/api/ping", timeout=10) as r:
        print(r.status, r.read().decode())
except urllib.error.HTTPError as error:
    print(error.code)
''', encoding="utf-8")
    bad = py(helper, ports["http"], "alice", "wrong", target)
    check("HTTP proxy: wrong credentials answer 407 too",
          bad.stdout.split()[:1] == [b"407"], f"{bad.stdout!r}")
    good = py(helper, ports["http"], "bob", "bunker", target)
    check("HTTP proxy: the second account authenticates and forwards",
          b'"pauth"' in good.stdout, f"rc={good.returncode} {good.stdout!r}")
    mixed = py(script, ports["mixed"], "alice", "alpine", target)
    check("mixed proxy: SOCKS credentials work beside HTTP CONNECT on one port",
          mixed.returncode == 0 and b'"pauth"' in mixed.stdout,
          f"rc={mixed.returncode} {mixed.stdout!r}")


@scenario
def proxy_protocol_egress():
    mark_chapter("PROXY protocol v1/v2 to TCP and UDP backends")
    state = hub("ppout")
    if not start_server("srv-ppout", hub_start(state, state["vkey"], ["a-ppout"],
                                              [])):
        return
    tcp = proxy_header_backend()
    udp = udp_proxy_header_backend()
    fingerprint = server_fingerprint(HERE / "srv-ppout.toml")
    ports = {name: free_port() for name in ("v1", "v2", "udp", "plain")}
    agent = agent_head(state["bridge"], "a-ppout", state["vkey"], "tcp",
                       fingerprint) + f'''
[[tunnel]]
id = 1
mode = "tcp"
port = {ports["v1"]}
bind = "127.0.0.1"
[tunnel.options]
proxy_protocol = "v1"
[[tunnel.backend]]
addr = "127.0.0.1:{tcp.server_address[1]}"

[[tunnel]]
id = 2
mode = "tcp"
port = {ports["v2"]}
bind = "127.0.0.1"
[tunnel.options]
proxy_protocol = "v2"
[[tunnel.backend]]
addr = "127.0.0.1:{tcp.server_address[1]}"

[[tunnel]]
id = 3
mode = "udp"
port = {ports["udp"]}
bind = "127.0.0.1"
[tunnel.options]
proxy_protocol = "v2"
[[tunnel.backend]]
addr = "127.0.0.1:{udp.server_address[1]}"

[[tunnel]]
id = 4
mode = "udp"
port = {ports["plain"]}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{udp.server_address[1]}"
'''
    if not start_agent("a-ppout", agent):
        return
    for version, key in (("v1", "v1"), ("v2", "v2")):
        body = http_once(ports[key], host="pp")
        try:
            info = json.loads(body)
        except ValueError:
            info = {"raw": body[:120].decode(errors="replace")}
        check(f"PROXY protocol: the {version} header reaches the TCP backend",
              info.get("version") == version and info.get("source") == "127.0.0.1"
              and info.get("destination") == "127.0.0.1"
              and int(info.get("dport", 0)) == ports[key],
              f"{body[:220]!r}")
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(3)
    reply = b""
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline and not reply:
        sock.sendto(b"pp-udp-probe", ("127.0.0.1", ports["udp"]))
        try:
            reply, _ = sock.recvfrom(65535)
        except socket.timeout:
            continue
    sock.close()
    try:
        info = json.loads(reply)
    except ValueError:
        info = {"raw": reply[:160].decode(errors="replace")}
    check("PROXY protocol: the v2 datagram header reaches the UDP backend",
          info.get("version") == "v2" and info.get("source") == "127.0.0.1"
          and info.get("dport") == str(ports["udp"]),
          f"{reply[:220]!r}")

    # Differential: the same UDP tunnel without a header must still carry the
    # datagram, so an empty reply above would mean the header, not the tunnel.
    undecorated = b""
    plain_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    plain_sock.settimeout(3)
    plain_deadline = time.monotonic() + 25
    while time.monotonic() < plain_deadline and not undecorated:
        plain_sock.sendto(b"pp-udp-plain", ("127.0.0.1", ports["plain"]))
        try:
            undecorated, _ = plain_sock.recvfrom(65535)
        except socket.timeout:
            continue
    plain_sock.close()
    try:
        plain_info = json.loads(undecorated)
    except ValueError:
        plain_info = {}
    check("PROXY protocol: a UDP tunnel without a header stays undecorated",
          plain_info.get("version") == "none"
          and plain_info.get("first", "").startswith("pp-udp-plain"),
          f"{undecorated[:220]!r}")

    # PROXY v1 has no datagram form, so the agent refuses the configuration
    # outright instead of quietly sending a header nobody can parse.
    rejected = (f'server = "127.0.0.1:{state["bridge"]}"\nkey_id = "a-ppout"\n'
                f'vkey = "{state["vkey"]}"\ntransport = "tcp"\n'
                f'[trust]\nmode = "pinned"\nfingerprint = "{fingerprint}"\n'
                f'[[tunnel]]\nid = 5\nmode = "udp"\nport = {free_port()}\n'
                f'bind = "127.0.0.1"\n[tunnel.options]\n'
                f'proxy_protocol = "v1"\n[[tunnel.backend]]\n'
                f'addr = "127.0.0.1:{udp.server_address[1]}"\n')
    start_agent("a-ppout-v1", rejected, timeout=20, expect="tunnels registered", allow_failure=True)
    v1_log = log_text("a-ppout-v1")
    check("PROXY protocol: a v1 header on a UDP tunnel is refused, not silently sent",
          "UDP supports sending PROXY v2" in v1_log, f"{v1_log[:260]!r}")


@scenario
def proxy_protocol_ingress():
    mark_chapter("PROXY protocol in front of the bridge and the HTTP entry")
    state = hub("ppin")
    if not start_server("srv-ppin", hub_start(
            state, state["vkey"], ["a-ppin"], [],
            '\n[bridge.proxy_accept]\nmode = "required"\ntrusted = ["127.0.0.1"]\n')):
        return
    backend = labelled_backend("ppin")
    relay_port = free_port()
    direct = free_port()
    relay = HERE / "proxyrelay.py"
    relay.write_text('''\
# A load balancer that prepends a PROXY v1 line, then pipes to the bridge.
import socket, sys, threading
listen, upstream = int(sys.argv[1]), int(sys.argv[2])
header = b"PROXY TCP4 203.0.113.9 198.51.100.7 51000 443\\r\\n"

def pump(a, b):
    try:
        while True:
            chunk = a.recv(65536)
            if not chunk:
                break
            b.sendall(chunk)
    except OSError:
        pass
    try:
        b.shutdown(socket.SHUT_WR)
    except OSError:
        pass

def handle(client):
    try:
        server = socket.create_connection(("127.0.0.1", upstream), timeout=15)
        server.sendall(header)
        down = threading.Thread(target=pump, args=(client, server), daemon=True)
        up = threading.Thread(target=pump, args=(server, client), daemon=True)
        down.start()
        up.start()
        up.join()
        down.join(timeout=1)
    except OSError:
        pass

listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", listen))
listener.listen(16)
print("relay ready", listen, flush=True)
while True:
    client, _ = listener.accept()
    threading.Thread(target=handle, args=(client,), daemon=True).start()
''', encoding="utf-8")
    spawn([sys.executable, relay, relay_port, state["bridge"]],
          HERE / "proxyrelay.log")

    fingerprint = server_fingerprint(HERE / "srv-ppin.toml")
    agent = agent_head(relay_port, "a-ppin", state["vkey"], "tcp",
                       fingerprint) + f'''
[[tunnel]]
id = 1
mode = "tcp"
port = {direct}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
'''
    if not start_agent("a-ppin", agent, timeout=60):
        return
    body = http_once(direct, host="pp")
    check("PROXY ingress: an agent behind a header-writing relay still connects",
          b'"ppin"' in body, f"{body[:160]!r}")

    start_agent("a-ppin-raw", agent_head(
        state["bridge"], "a-ppin", state["vkey"], "tcp", fingerprint) + f'''
[[tunnel]]
id = 2
mode = "tcp"
port = {free_port()}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
''', timeout=20, expect="tunnels registered", allow_failure=True)
    raw = log_text("a-ppin-raw")
    # The server drops the connection because no header arrived; the proof on
    # the agent side is that the bridge handshake never completes.
    check("PROXY ingress: required mode drops a bridge connection with no header",
          "tunnels registered" not in raw and "protocol error" in raw,
          f"{raw[-220:]}")

    state = hub("ppin-http")
    if not start_server("srv-ppin2", hub_start(
            state, state["vkey"], ["a-ppin"], [],
            http_extra='\n[http.proxy_accept]\nmode = "required"\n'
                       'trusted = ["127.0.0.1"]\n')):
        return
    if not start_agent("a-ppin-http", agent_head(
            state["bridge"], "a-ppin", state["vkey"], "tcp",
            server_fingerprint(HERE / "srv-ppin2.toml")) + f'''
[[tunnel]]
id = 3
mode = "http"
port = {state["entry"]}
hosts = ["ppin"]
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
'''):
        return
    with socket.create_connection(("127.0.0.1", state["entry"]), timeout=10) as sock:
        sock.sendall(b"PROXY TCP4 203.0.113.9 198.51.100.7 51000 443\r\n"
                     b"GET /api/ping HTTP/1.1\r\nHost: ppin\r\n"
                     b"Connection: close\r\n\r\n")
        body = b""
        while True:
            chunk = sock.recv(65535)
            if not chunk:
                break
            body += chunk
    try:
        info = json.loads(body.split(b"\r\n\r\n", 1)[-1])
    except ValueError:
        info = {}
    check("PROXY ingress: the HTTP entry honours a header and asserts the source",
          info.get("xff") == "203.0.113.9", f"{body[:200]!r}")

    state = hub("ppin-off")
    if not start_server("srv-ppin3", hub_start(state, state["vkey"], ["a-ppin"],
                                              [])):
        return
    if not start_agent("a-ppin-off", agent_head(
            state["bridge"], "a-ppin", state["vkey"], "tcp",
            server_fingerprint(HERE / "srv-ppin3.toml")) + f'''
[[tunnel]]
id = 4
mode = "http"
port = {state["entry"]}
hosts = ["ppin"]
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
'''):
        return
    with socket.create_connection(("127.0.0.1", state["entry"]), timeout=10) as sock:
        body = b""
        try:
            sock.sendall(b"PROXY TCP4 203.0.113.9 198.51.100.7 51000 443\r\n"
                         b"GET /api/ping HTTP/1.1\r\nHost: ppin\r\n"
                         b"Connection: close\r\n\r\n")
            while True:
                chunk = sock.recv(65535)
                if not chunk:
                    break
                body += chunk
        except ConnectionResetError:
            # Closing a rejected request with unread bytes can produce RST.
            # A subsequent valid request must still reach the expected backend.
            pass
    healthy = json.loads(http_once(state["entry"], host="ppin"))
    check("PROXY ingress: with acceptance off, a header is data, not a preface",
          (not body or body.startswith(b"HTTP/1.1 400"))
          and healthy.get("backend") == "ppin"
          and healthy.get("xff") == "127.0.0.1",
          f"rejection={body[:100]!r}, subsequent request={healthy}")


@scenario
def upstream_proxy_chain():
    mark_chapter("Upstream SOCKS5 / HTTP proxy chain, custom DNS, local_ip")
    state = hub("upchain")
    # a-up1/2/3 each carry a different upstream; a-up is the denial case.
    if not start_server("srv-upchain",
                        hub_start(state, state["vkey"],
                                  ["a-up", "a-up1", "a-up2", "a-up3"], [])):
        return
    backend = labelled_backend("upchain")
    dns = dns_backend({"bridge.test": "127.0.0.1",
                       "backend.test": "127.0.0.1"})
    dns_port = dns.server_address[1]
    socks_auth = upstream_backend("socks5", ("relay", "s3cret"))
    socks_open = upstream_backend("socks5")
    http_auth = upstream_backend("http", ("relay", "s3cret"))
    say(f"upstream: socks5+auth {socks_auth.server_address[1]}, "
        f"socks5 {socks_open.server_address[1]}, "
        f"http+auth {http_auth.server_address[1]}, DNS {dns_port}")

    fingerprint = server_fingerprint(HERE / "srv-upchain.toml")
    network_common = (f'[network]\ndns = ["127.0.0.1:{dns_port}"]\n'
                      f'local_ip = "127.0.0.1"\nforward_local_ip = true\n')

    def agent_for(key_id, proxy_block):
        return (f'server = "bridge.test:{state["bridge"]}"\n'
                f'key_id = "{key_id}"\nvkey = "{state["vkey"]}"\n'
                f'transport = "tcp"\n[trust]\nmode = "pinned"\n'
                f'fingerprint = "{fingerprint}"\n{network_common}'
                f'[network.proxy]\n{proxy_block}'
                f'[[tunnel]]\nid = 1\nmode = "tcp"\nport = %PORT%\n'
                f'bind = "127.0.0.1"\n[[tunnel.backend]]\n'
                f'addr = "backend.test:{backend.server_address[1]}"\n')

    cases = [
        ("a-up1", "socks5 with credentials, remote_dns",
         'kind = "socks5"\nserver = "127.0.0.1:%SOCKS_AUTH%"\n'
         'auth = { username = "relay", password = "s3cret" }\n'
         'remote_dns = true\n', "bridge.test"),
        ("a-up2", "socks5 with no credentials, resolving locally",
         'kind = "socks5"\nserver = "127.0.0.1:%SOCKS_OPEN%"\n'
         'remote_dns = false\n', "127.0.0.1"),
        ("a-up3", "HTTP CONNECT upstream with credentials",
         'kind = "http"\nserver = "127.0.0.1:%HTTP_AUTH%"\n'
         'auth = { username = "relay", password = "s3cret" }\n'
         'remote_dns = true\n', "bridge.test"),
    ]
    for key_id, label, block, expect_target in cases:
        block = (block.replace("%SOCKS_AUTH%", str(socks_auth.server_address[1]))
                      .replace("%SOCKS_OPEN%", str(socks_open.server_address[1]))
                      .replace("%HTTP_AUTH%", str(http_auth.server_address[1])))
        text = agent_for(key_id, block).replace("%PORT%", str(free_port()))
        if not start_agent(key_id, text, timeout=45):
            continue
        port = int(re.search(r"port = (\d+)", text).group(1))
        body = http_once(port, host="up")
        check(f"upstream chain: an agent reaches the server through {label}",
              b'"upchain"' in body, f"{body[:160]!r}")
        seen = [entry.get("target", "") for entry in Upstream.log]
        check(f"upstream chain: {label} dialled the bridge as {expect_target}",
              any(target.startswith(expect_target) for target in seen),
              f"{seen[-4:]}")

    asked = list(Dns.state.get("asked", []))
    check("custom DNS: both hostnames went through the configured resolver",
          "bridge.test" in asked and "backend.test" in asked, f"{asked[:8]}")
    check("network: local_ip with forward_local_ip still authenticates and serves",
          "tunnels registered" in log_text("a-up1"), f"{log_text('a-up1')[-200:]}")

    denied = (f'server = "bridge.test:{state["bridge"]}"\n'
              f'key_id = "a-up"\nvkey = "{state["vkey"]}"\n'
              f'transport = "tcp"\n[trust]\nmode = "pinned"\n'
              f'fingerprint = "{fingerprint}"\n{network_common}'
              f'[network.proxy]\nkind = "socks5"\n'
              f'server = "127.0.0.1:{socks_auth.server_address[1]}"\n'
              f'remote_dns = true\n'
              f'[[tunnel]]\nid = 1\nmode = "tcp"\nport = {free_port()}\n'
              f'bind = "127.0.0.1"\n[[tunnel.backend]]\n'
              f'addr = "backend.test:{backend.server_address[1]}"\n')
    start_agent("a-up-deny", denied, timeout=25, expect="tunnels registered", allow_failure=True)
    deny = log_text("a-up-deny")
    check("upstream chain: the configured credentials are required, not optional",
          "refused" in deny.lower() or "authentication" in deny.lower(),
          f"{deny[-260:]}")

    broken = dns_backend({})
    broken_port = broken.server_address[1]
    unreachable = (f'server = "bridge.test:{state["bridge"]}"\n'
                   f'key_id = "a-up"\nvkey = "{state["vkey"]}"\n'
                   f'transport = "tcp"\n[trust]\nmode = "pinned"\n'
                   f'fingerprint = "{fingerprint}"\n'
                   f'[network]\ndns = ["127.0.0.1:{broken_port}"]\n'
                   f'[[tunnel]]\nid = 1\nmode = "tcp"\nport = {free_port()}\n'
                   f'bind = "127.0.0.1"\n[[tunnel.backend]]\n'
                   f'addr = "backend.test:{backend.server_address[1]}"\n')
    start_agent("a-up-nodns", unreachable, timeout=25, expect="tunnels registered", allow_failure=True)
    nodns = log_text("a-up-nodns")
    check("custom DNS: a resolver with no answer is a visible failure",
          "tunnels registered" not in nodns and nodns.strip() != "",
          f"{nodns[-260:]}")


@scenario
def ntp_calibration():
    mark_chapter("NTP clock calibration")
    state = hub("ntp")
    if not start_server("srv-ntp", hub_start(state, state["vkey"], ["a-ntp"],
                                            [])):
        return
    backend = labelled_backend("ntp")
    server = ntp_backend(3600)
    fingerprint = server_fingerprint(HERE / "srv-ntp.toml")
    port = free_port()
    agent = agent_head(state["bridge"], "a-ntp", state["vkey"], "tcp",
                       fingerprint,
                       f'[ntp]\nserver = "127.0.0.1:'
                       f'{server.server_address[1]}"\n'
                       f'interval = "1s"\ntimeout = "500ms"\nmax_offset = "2h"\n') + f'''
[[tunnel]]
id = 1
mode = "tcp"
port = {port}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
'''
    if not start_agent("a-ntp", agent):
        return
    log = log_text("a-ntp")
    check("NTP: the agent calibrates against the configured source",
          "calibrated bridge authentication clock" in log, f"{log[-200:]}")
    measured = re.search(r"offset_ms=(\d+)", log)
    check("NTP: the measured offset matches the source's one-hour skew",
          measured is not None and abs(int(measured.group(1)) - 3_600_000) < 1000,
          f"{measured.group(0) if measured else 'no offset'} in {log[-200:]}")
    body = http_once(port, host="ntp")
    check("NTP: the tunnel still forwards after calibration",
          b'"ntp"' in body, f"{body[:120]!r}")


@scenario
def ipv6_end_to_end():
    mark_chapter("IPv6 end to end")
    bridge, entry, kcp = free_port(), free_port(), free_port()
    vkey = secrets.token_hex(32)
    cert_pair("v6test", "/CN=v6.test")
    server_toml = (f'[bridge]\nbind = "[::1]:{bridge}"\n'
                   f'quic_bind = "[::1]:{bridge}"\n'
                   f'kcp_bind = "[::1]:{kcp}"\n'
                   f'tls = {{ cert = "{HERE}/bridge.pem", key = "{HERE}/bridge.key" }}\n'
                   'ws_path = "/ws"\n\n[http]\n'
                   f'bind = "[::1]:{entry}"\ntls_bind = "[::1]:{entry}"\n'
                   f'http3_bind = "[::1]:{entry}"\n\n[http.certificates]\n'
                   f'"v6.test" = {{ cert = "{HERE}/v6test.pem", '
                   f'key = "{HERE}/v6test.key" }}\n\n'
                   f'[client.a-v6]\nvkey = "{vkey}"\nallow_config_push = true\n')
    if not start_server("srv-v6", server_toml):
        return
    fingerprint = server_fingerprint(HERE / "srv-v6.toml")
    backend = labelled_backend("v6")
    port = free_port()
    if not start_agent("a-v6", (f'server = "[::1]:{bridge}"\nkey_id = "a-v6"\n'
                                f'vkey = "{vkey}"\ntransport = "tls"\n'
                                f'[trust]\nmode = "pinned"\n'
                                f'fingerprint = "{fingerprint}"\n'
                                f'[[tunnel]]\nid = 1\nmode = "tcp"\n'
                                f'port = {port}\nbind = "::1"\n'
                                f'[[tunnel.backend]]\n'
                                f'addr = "127.0.0.1:'
                                f'{backend.server_address[1]}"\n'
                                f'[[tunnel]]\nid = 2\nmode = "http"\n'
                                f'port = {entry}\nhosts = ["v6.test"]\n'
                                f'[[tunnel.backend]]\n'
                                f'addr = "127.0.0.1:'
                                f'{backend.server_address[1]}"\n')):
        return
    with socket.socket(socket.AF_INET6, socket.SOCK_STREAM) as sock:
        sock.settimeout(10)
        sock.connect(("::1", port))
        sock.sendall(b"GET /api/ping HTTP/1.1\r\nHost: v6\r\n"
                     b"Connection: close\r\n\r\n")
        body = b""
        while True:
            chunk = sock.recv(65535)
            if not chunk:
                break
            body += chunk
    check("IPv6: agent bridge, tunnel bind and client all run over ::1",
          b'"v6"' in body, f"{body[:160]!r}")
    code, out, err = curl(["-H", "Host: v6.test", "-o", "/dev/null",
                           "-w", "%{http_code}", f"http://[::1]:{entry}/api/ping"])
    check("IPv6: the reverse proxy answers over ::1 too",
          code == 0 and out == "200", f"code={code} out={out!r} {err[:150]}")


@scenario
def load_balancing_and_health():
    mark_chapter("Load-balancing modes and active health checks")
    state = hub("lb")
    if not start_server("srv-lb", hub_start(state, state["vkey"], ["a-lb"],
                                           [])):
        return
    live = [labelled_backend(f"lb{i}") for i in range(3)]
    live_ports = [item.server_address[1] for item in live]

    def rebind(label, port):
        cls = type(f"Labelled{label}", (Labelled,), {"label": label})
        server = Stop(("127.0.0.1", port), cls)
        server.label = label
        server.health_ok = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
        return server

    BALANCE_NAMES = {"rr": "round_robin", "weighted": "weighted",
                     "least": "least_connections", "hash": "source_hash",
                     "failover": "failover"}
    ports = {name: free_port() for name in
             ("rr", "weighted", "least", "hash", "failover", "tcphealth",
              "httphealth")}
    fingerprint = server_fingerprint(HERE / "srv-lb.toml")
    agent = agent_head(state["bridge"], "a-lb", state["vkey"], "tcp", fingerprint)
    identifiers = {"rr": 1, "weighted": 2, "least": 3, "hash": 4, "failover": 5}
    for name, tunnel_id in identifiers.items():
        agent += (f'\n[[tunnel]]\nid = {tunnel_id}\nmode = "tcp"\n'
                  f'port = {ports[name]}\nbind = "127.0.0.1"\n'
                  f'balance = "{BALANCE_NAMES[name]}"\n')
        for index, address in enumerate(live_ports):
            weight = "weight = 3\n" if name == "weighted" and index == 2 else ""
            agent += f'[[tunnel.backend]]\n{weight}addr = "127.0.0.1:{address}"\n'
    agent += f'''
[[tunnel]]
id = 6
mode = "tcp"
port = {ports["tcphealth"]}
bind = "127.0.0.1"
[tunnel.health]
type = "tcp"
interval = "1s"
timeout = "500ms"
failures_to_evict = 1
successes_to_restore = 1
'''
    for address in live_ports:
        agent += f'[[tunnel.backend]]\naddr = "127.0.0.1:{address}"\n'
    agent += f'''
[[tunnel]]
id = 7
mode = "tcp"
port = {ports["httphealth"]}
bind = "127.0.0.1"
[tunnel.health]
type = "http"
path = "/healthz"
expect_status = [200]
interval = "1s"
timeout = "500ms"
failures_to_evict = 1
successes_to_restore = 1
'''
    for address in live_ports:
        agent += f'[[tunnel.backend]]\naddr = "127.0.0.1:{address}"\n'
    if not start_agent("a-lb", agent):
        return

    def spread(port, count=12):
        seen = {}
        for _ in range(count):
            body = http_once(port, host="lb")
            try:
                label = json.loads(body)["backend"]
            except (ValueError, KeyError):
                label = f"bad:{body[:40]!r}"
            seen[label] = seen.get(label, 0) + 1
        return seen

    check("round_robin: traffic spreads over every backend",
          set(spread(ports["rr"])) == {"lb0", "lb1", "lb2"},
          str(spread(ports["rr"])))
    weighted = spread(ports["weighted"])
    check("weighted: the weight-3 backend takes a larger share",
          weighted.get("lb2", 0) > weighted.get("lb0", 0)
          and weighted.get("lb0") == weighted.get("lb1"), str(weighted))
    held = []
    for _ in range(3):
        sock = socket.create_connection(("127.0.0.1", ports["least"]), timeout=10)
        sock.sendall(b"GET /api/ping HTTP/1.1\r\nHost: lb\r\n\r\n")
        held.append(sock)
    least = {}
    for sock in held:
        body = b""
        while b"\r\n\r\n" not in body:
            chunk = sock.recv(65535)
            if not chunk:
                break
            body += chunk
        while True:
            try:
                chunk = sock.recv(65535)
            except socket.timeout:
                break
            if not chunk:
                break
            body += chunk
        try:
            least[json.loads(body.split(b"\r\n\r\n", 1)[-1])["backend"]] = 1
        except (ValueError, KeyError):
            least[f"bad:{body[:40]!r}"] = 1
    for sock in held:
        sock.close()
    check("least_connections: three live connections land on three backends",
          set(least) == {"lb0", "lb1", "lb2"}, str(least))
    check("source_hash: one client address keeps landing on one backend",
          len(spread(ports["hash"])) == 1, str(spread(ports["hash"])))
    check("failover: only the first configured backend receives traffic",
          spread(ports["failover"]) == {"lb0": 12}, str(spread(ports["failover"])))

    before = spread(ports["tcphealth"])
    live[1].stop()
    after = before
    for _ in range(30):
        time.sleep(0.5)
        after = spread(ports["tcphealth"], 4)
        if set(after) == {"lb0", "lb2"}:
            break
    check("health check: a stopped backend is evicted within one interval",
          set(after) == {"lb0", "lb2"}, f"before={before} after={after}")
    live[1] = rebind("lb1", live_ports[1])
    restored = False
    final = after
    for _ in range(30):
        time.sleep(0.5)
        final = spread(ports["tcphealth"], 4)
        if "lb1" in final:
            restored = True
            break
    check("health check: a recovered backend is restored to rotation",
          restored, f"final={final}")

    live[2].health_ok = False
    eviction = {}
    for _ in range(40):
        time.sleep(0.5)
        eviction = spread(ports["httphealth"], 6)
        if set(eviction) == {"lb0", "lb1"}:
            break
    check("health check: a failing HTTP probe takes the backend out of rotation",
          set(eviction) == {"lb0", "lb1"}, f"{eviction}")
    live[2].health_ok = True
    healed = {}
    for _ in range(40):
        time.sleep(0.5)
        healed = spread(ports["httphealth"], 6)
        if set(healed) == {"lb0", "lb1", "lb2"}:
            break
    check("health check: a healthy HTTP probe restores it",
          set(healed) == {"lb0", "lb1", "lb2"}, f"{healed}")


@scenario
def tcp_half_close():
    mark_chapter("TCP half-close across the tunnel")
    state = hub("halfclose")
    if not start_server("srv-half", hub_start(state, state["vkey"], ["a-half"],
                                             [])):
        return
    backend = half_close_backend()
    fingerprint = server_fingerprint(HERE / "srv-half.toml")
    port = free_port()
    if not start_agent("a-half", agent_head(
            state["bridge"], "a-half", state["vkey"], "tcp", fingerprint) + f'''
[[tunnel]]
id = 1
mode = "tcp"
port = {port}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
'''):
        return
    payload = b"half-close-" + secrets.token_bytes(64)
    with socket.create_connection(("127.0.0.1", port), timeout=15) as sock:
        sock.sendall(b"POST /upload HTTP/1.1\r\nHost: half\r\n"
                     b"Content-Length: " + str(len(payload)).encode()
                     + b"\r\n\r\n" + payload)
        sock.shutdown(socket.SHUT_WR)
        body = b""
        while True:
            chunk = sock.recv(65535)
            if not chunk:
                break
            body += chunk
    check("half-close: the FIN the client sends arrives at the backend as EOF",
          f"saw-eof-after-{len(payload)}".encode() in body, f"{body[:160]!r}")


@scenario
def quotas_and_rate_limits():
    mark_chapter("Quotas, rate limits and connection caps")
    state = hub("limits")
    if not start_server("srv-limits", hub_start(
            state, state["vkey"], ["a-limits"], [])):
        return
    backend = labelled_backend("limits")
    fingerprint = server_fingerprint(HERE / "srv-limits.toml")
    ports = {name: free_port() for name in
             ("flow", "connections", "rate", "baseline")}
    agent = agent_head(state["bridge"], "a-limits", state["vkey"], "tcp",
                       fingerprint) + f'''
[[tunnel]]
id = 1
mode = "tcp"
port = {ports["flow"]}
bind = "127.0.0.1"
[tunnel.options.limits]
quota = {{ flow_limit = 4096 }}
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"

[[tunnel]]
id = 2
mode = "tcp"
port = {ports["connections"]}
bind = "127.0.0.1"
[tunnel.options.limits]
max_connections = 1
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"

[[tunnel]]
id = 3
mode = "tcp"
port = {ports["rate"]}
bind = "127.0.0.1"
[tunnel.options.limits]
rate_limit = 262144
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"

[[tunnel]]
id = 4
mode = "tcp"
port = {ports["baseline"]}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend.server_address[1]}"
'''
    if not start_agent("a-limits", agent):
        return

    held = socket.create_connection(("127.0.0.1", ports["connections"]), timeout=10)
    held.sendall(b"GET /api/ping HTTP/1.1\r\nHost: limits\r\n\r\n")
    first = held.recv(200)
    second = socket.create_connection(("127.0.0.1", ports["connections"]), timeout=10)
    second.sendall(b"GET /api/ping HTTP/1.1\r\nHost: limits\r\n\r\n")
    refused = b""
    try:
        while True:
            chunk = second.recv(200)
            if not chunk:
                break
            refused += chunk
    except OSError:
        pass
    held.close()
    second.close()
    check("max_connections = 1: the first connection works, the second is refused",
          b"200" in first and b'"limits"' not in refused,
          f"first={first[:60]!r} second={refused[:60]!r}")

    drained = 0
    for _ in range(8):
        drained += len(http_once(ports["flow"], path="/big", host="limits"))
    after = http_once(ports["flow"], path="/big", host="limits")
    check("quota flow_limit: traffic past the allowance is refused",
          drained > 4096 and after == b"", f"drained={drained} after={after[:80]!r}")

    def throughput(port, rounds=3):
        started = time.monotonic()
        moved = 0
        for _ in range(rounds):
            moved += len(http_once(port, path="/big", host="limits", timeout=120))
        return moved, time.monotonic() - started

    base_bytes, base_time = throughput(ports["baseline"])
    slow_bytes, slow_time = throughput(ports["rate"])
    base_rate = base_bytes / base_time
    slow_rate = slow_bytes / slow_time
    check("rate_limit: the capped tunnel moves data far slower than an uncapped one",
          slow_bytes > 262144 and slow_rate < base_rate / 3,
          f"uncapped {base_rate / 1024:.0f} KiB/s, capped {slow_rate / 1024:.0f} KiB/s "
          f"against a 256 KiB/s ceiling")


@scenario
def http2_to_http2_backend():
    mark_chapter("HTTP/2 entry to an HTTP/2 backend (plain GET)")
    state = hub("h2h2")
    if not start_server("srv-h2h2", hub_start(state, state["vkey"], ["a-h2"],
                                             ["h2be.test"])):
        return
    extended = ws_extended_backend(HERE / "bridge.pem", HERE / "bridge.key", "h2")
    if not extended:
        return
    fingerprint = server_fingerprint(HERE / "srv-h2h2.toml")
    if not start_agent("a-h2", agent_head(
            state["bridge"], "a-h2", state["vkey"], "tcp", fingerprint) + f'''
[[tunnel]]
id = 1
mode = "http"
port = {state["entry"]}
hosts = ["h2be.test"]
[tunnel.options.http]
backend_http2 = true
[[tunnel.backend]]
addr = "127.0.0.1:{extended}"
'''):
        return
    code, out, err = curl(["-H", "Host: h2be.test", "-o", "/dev/null",
                           "-w", "%{http_code} %{http_version}",
                           f"http://127.0.0.1:{state['entry']}/api/ping"])
    check("backend HTTP/2: an HTTP/1.1 entry reaches a prior-knowledge h2 backend",
          code == 0 and out.startswith("200"),
          f"code={code} out={out!r} {err[:160]}")
    script = HERE / "h2be.mjs"
    script.write_text(
        "import http2 from 'node:http2';\n"
        "const [port] = process.argv.slice(2);\n"
        "const client = http2.connect(`https://127.0.0.1:${port}`, {\n"
        "  rejectUnauthorized: false, servername: 'h2be.test', "
        "ALPNProtocols: ['h2'] });\n"
        "const timer = setTimeout(() => { console.log('TIMEOUT'); process.exit(3); }, 15000);\n"
        "client.on('error', (e) => { console.log('ERROR ' + e.message); process.exit(5); });\n"
        "const request = client.request({ ':method': 'GET', ':scheme': 'https',\n"
        "  ':authority': 'h2be.test', ':path': '/api/ping' });\n"
        "let status = 0; let body = '';\n"
        "request.on('response', (h) => { status = Number(h[':status']); });\n"
        "request.on('data', (c) => { body += c; });\n"
        "request.on('end', () => { clearTimeout(timer);\n"
        "  console.log(`${status} ${body.includes('h2-extended-plain') ? 'OK' : 'BAD'}`);\n"
        "  client.close(); process.exit(status === 200 ? 0 : 4); });\n"
        "request.end();\n", encoding="utf-8")
    run = node(script, state["entry"])
    check("backend HTTP/2: an HTTP/2 entry reaches the same h2 backend",
          run.returncode == 0 and run.stdout.startswith(b"200 OK"),
          f"rc={run.returncode} {run.stdout!r} {run.stderr.decode()[:160]}")


def main():
    for path in (SERVER, AGENT, VENV):
        if not path.exists():
            say(f"missing {path}")
            return 2
    HERE.mkdir(parents=True, exist_ok=True)
    for stale in HERE.iterdir():
        shutil.rmtree(stale, ignore_errors=True) if stale.is_dir() else stale.unlink()

    bridge_cert, bridge_key = cert_pair("bridge", "/CN=felucca-bridge")
    say(f"running feature scenarios; artifacts and logs under {HERE}")
    try:
        scenarios()
    except Exception as error:  # noqa: BLE001 - reported, never silently dropped
        import traceback
        check("scenario driver crashed", False,
              "".join(traceback.format_exception_only(type(error), error)).strip()
              + "\n" + traceback.format_exc()[-800:])
    finally:
        report()
        say(f"json: {HERE / f'matrix2-{RUN}.json'}")
    return 0 if results and all(item["ok"] for item in results) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    finally:
        cleanup()
