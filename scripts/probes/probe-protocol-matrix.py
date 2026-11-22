#!/usr/bin/env python3
"""Every bridge transport and every payload protocol, over real processes.

One server binds a shared bridge TCP port (tcp/tls/ws/wss), plus QUIC and KCP
UDP sockets, plus an HTTP entry (plain/TLS/h3) on its own port. One agent
connects per bridge transport, each registering a TCP and a UDP tunnel. A second
agent carries every payload mode (reverse proxy, SOCKS5, HTTP proxy, mixed, file,
compress+encrypt tunnels). Real clients drive the traffic: curl, Node (http2 and
a raw WebSocket client), a hand-rolled SOCKS5 client and aioquic for HTTP/3.
Everything binds 127.0.0.1 only. Artifacts land in
target/tmp/protocol-matrix-audit unless FELUCCA_PROBE_OUTPUT says otherwise.
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
    or "target/tmp/protocol-matrix-audit")

TRANSPORTS = ["tcp", "tls", "ws", "wss", "quic", "kcp"]


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


H3_PYTHON = venv_python()
RUN = secrets.token_hex(6)

results = []
processes = []
used_ports = set()

CLEAN_ENV = {k: v for k, v in os.environ.items()
             if not k.startswith("FELUCCA_") and k.upper() not in
             ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")}
CLEAN_ENV["RUST_LOG"] = "info"
CLEAN_ENV["NO_COLOR"] = "1"

WS_BACKEND = """\
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
server.listen(port, '127.0.0.1', () => console.log('ws-backend ready ' + port));
"""


def say(message):
    print(message, flush=True)


def check(name, ok, detail=""):
    results.append({"name": name, "ok": bool(ok), "detail": str(detail)[:400]})
    say(("  PASS " if ok else "  FAIL ") + name + (f"  — {detail}" if detail else ""))
    return ok


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


def node(script, *arguments, timeout=60):
    return subprocess.run(["node", str(script)] + [str(item) for item in arguments],
                          cwd=str(ROOT), env=CLEAN_ENV, capture_output=True, timeout=timeout)


ANSI = re.compile(r"\x1b\[[0-9;]*m")


def wait_for_log(path, needle, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and needle in ANSI.sub("", path.read_text(errors="replace")):
            return True
        time.sleep(0.1)
    return False


def curl(arguments, timeout=30):
    command = ["curl", "-sS", "--noproxy", "*", "--max-time", str(timeout)] + [
        str(item) for item in arguments]
    done = subprocess.run(command, cwd=str(ROOT), env=CLEAN_ENV,
                          capture_output=True, timeout=timeout + 10)
    return done.returncode, done.stdout.decode(errors="replace"), done.stderr.decode(errors="replace")


def status_of(text):
    try:
        return int(text.split()[0])
    except (IndexError, ValueError):
        return -1


# --------------------------------------------------------------------------- #
# Backends
# --------------------------------------------------------------------------- #
class HttpBackend(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def reply(self, payload, content_type="application/json", status=200):
        raw = payload if isinstance(payload, bytes) else payload.encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/healthz":
            self.reply("ok", "text/plain")
        elif self.path == "/api/ping":
            self.reply(json.dumps({"run": RUN, "path": self.path,
                                   "proto": self.request_version}))
        elif self.path.startswith("/big"):
            self.reply(bytes(range(256)) * 4096, "application/octet-stream")
        else:
            self.reply(json.dumps({"run": RUN, "path": self.path}))

    def do_PUT(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.reply("mutated", "text/plain", status=405)

    def log_message(self, *args):
        pass


class UdpEcho(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        try:
            sock.sendto(data, self.client_address)
        except OSError:
            pass

    def handle_error(self, request, client_address):
        pass


class ThreadingUdp(socketserver.ThreadingUDPServer):
    daemon_threads = True
    allow_reuse_address = True
    max_packet_size = 65535


def start_backends():
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), HttpBackend)
    httpd.daemon_threads = True
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    udpd = ThreadingUdp(("127.0.0.1", 0), UdpEcho)
    threading.Thread(target=udpd.serve_forever, daemon=True).start()
    return httpd, httpd.server_address[1], udpd, udpd.server_address[1]


# --------------------------------------------------------------------------- #
# Hand-rolled clients
# --------------------------------------------------------------------------- #
def http_get(url, headers=None, timeout=20):
    request = urllib.request.Request(url, headers=headers or {})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=timeout) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


def recv_exact(sock, count):
    buffer = b""
    while len(buffer) < count:
        chunk = sock.recv(count - len(buffer))
        if not chunk:
            raise AssertionError(f"short read: {buffer!r}")
        buffer += chunk
    return buffer


def udp_roundtrip(port, payloads, timeout=5):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(timeout)
    replies = []
    try:
        for payload in payloads:
            sock.sendto(payload, ("127.0.0.1", port))
        for _ in payloads:
            try:
                replies.append(sock.recv(65535))
            except socket.timeout:
                break
    finally:
        sock.close()
    return replies


def socks5_stream(port, host, target_port, then_get=None):
    """SOCKS5 no-auth CONNECT; optionally speak an HTTP request on the pipe."""
    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    sock.settimeout(10)
    try:
        sock.sendall(b"\x05\x01\x00")
        if recv_exact(sock, 2) != b"\x05\x00":
            raise AssertionError("server refused the no-auth method")
        sock.sendall(b"\x05\x01\x00\x01" + socket.inet_aton(host)
                     + target_port.to_bytes(2, "big"))
        reply = recv_exact(sock, 10)
        if reply[1] != 0:
            raise AssertionError(f"CONNECT refused: {reply!r}")
        body = b""
        if then_get:
            sock.sendall(then_get)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                try:
                    chunk = sock.recv(65535)
                except socket.timeout:
                    break
                if not chunk:
                    break
                body += chunk
                if b'"run"' in body and b"\r\n\r\n" in body:
                    break
        return reply, body
    finally:
        sock.close()


def http_connect_tunnel(port, host, target_port, then_get):
    """HTTP CONNECT through a forward proxy, then an HTTP request on the pipe."""
    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    sock.settimeout(10)
    try:
        sock.sendall((f"CONNECT {host}:{target_port} HTTP/1.1\r\n"
                      f"Host: {host}:{target_port}\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = sock.recv(4096)
            if not chunk:
                break
            head += chunk
        first = head.split(b"\r\n")[0].decode(errors="replace")
        if " 200" not in first:
            raise AssertionError(f"CONNECT refused: {first}")
        rest = head.split(b"\r\n\r\n", 1)[1]
        sock.sendall(then_get)
        body = rest
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                chunk = sock.recv(65535)
            except socket.timeout:
                break
            if not chunk:
                break
            body += chunk
            if b'"run"' in body and b"\r\n\r\n" in body:
                break
        return first, body
    finally:
        sock.close()


def socks5_udp_associate(port, host, target_port, payload, timeout=8):
    control = socket.create_connection(("127.0.0.1", port), timeout=10)
    control.settimeout(10)
    try:
        control.sendall(b"\x05\x01\x00")
        if recv_exact(control, 2) != b"\x05\x00":
            raise AssertionError("server refused the no-auth method")
        control.sendall(b"\x05\x03\x00\x01" + socket.inet_aton("0.0.0.0") + b"\x00\x00")
        reply = recv_exact(control, 10)
        if reply[1] != 0:
            raise AssertionError(f"UDP ASSOCIATE refused: {reply!r}")
        relay = (socket.inet_ntoa(reply[4:8]), int.from_bytes(reply[8:10], "big"))
        frame = (b"\x00\x00\x00\x01" + socket.inet_aton(host)
                 + target_port.to_bytes(2, "big") + payload)
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.settimeout(timeout)
        try:
            for _ in range(3):
                sock.sendto(frame, relay)
                try:
                    data, _ = sock.recvfrom(65535)
                except socket.timeout:
                    continue
                return data[10:] if len(data) >= 10 else b""
            return None
        finally:
            sock.close()
    finally:
        control.close()


# --------------------------------------------------------------------------- #
# Main
# --------------------------------------------------------------------------- #
def main():
    for path in (SERVER, AGENT, H3_PYTHON):
        if not path.exists():
            say(f"missing {path}")
            return 2
    HERE.mkdir(parents=True, exist_ok=True)
    for stale in HERE.iterdir():
        shutil.rmtree(stale, ignore_errors=True) if stale.is_dir() else stale.unlink()

    httpd, backend_http, udpd, udp_port = start_backends()
    say(f"backends: http 127.0.0.1:{backend_http}   udp-echo 127.0.0.1:{udp_port}")

    ws_port = free_port()
    (HERE / "ws-backend.mjs").write_text(WS_BACKEND, encoding="utf-8")
    processes.append(subprocess.Popen(["node", str(HERE / "ws-backend.mjs"), str(ws_port)],
                                      env=CLEAN_ENV, start_new_session=True,
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    for _ in range(60):
        try:
            with socket.create_connection(("127.0.0.1", ws_port), timeout=1):
                break
        except OSError:
            time.sleep(0.1)
    say(f"backend: websocket 127.0.0.1:{ws_port}/ws")

    bridge = free_port()
    kcp = free_port()
    entry = free_port()
    p2p = free_port()
    vkey = secrets.token_hex(32)
    share_secret = secrets.token_hex(24)

    clients = {f"a-{name}": vkey for name in TRANSPORTS}
    clients.update({"a-payload": vkey, "a-share": vkey, "a-visitor": vkey})

    server_toml = f'''[bridge]
bind = "127.0.0.1:{bridge}"
quic_bind = "127.0.0.1:{bridge}"
kcp_bind = "127.0.0.1:{kcp}"
tls = {{ cert = "{HERE}/bridge.pem", key = "{HERE}/bridge.key" }}
ws_path = "/ws"

[bridge.kcp]
data_shards = 10
parity_shards = 3
mtu = 1200
send_window = 256
receive_window = 256
interval_ms = 20
max_sessions = 128

[http]
bind = "127.0.0.1:{entry}"
tls_bind = "127.0.0.1:{entry}"
http3_bind = "127.0.0.1:{entry}"
certificates."app.test" = {{ cert = "{HERE}/app.pem", key = "{HERE}/app.key" }}

'''
    for key_id, key in clients.items():
        push = "false" if key_id == "a-visitor" else "true"
        server_toml += f'[client.{key_id}]\nvkey = "{key}"\nallow_config_push = {push}\n\n'
    for share, tunnel in (("share-web", 1), ("share-dns", 2), ("share-proxy", 3)):
        server_toml += (f'[share.{share}]\nclient = "a-share"\ntunnel = {tunnel}\n'
                        f'secret = "{share_secret}"\nallow_clients = ["a-visitor"]\n')
        if share == "share-proxy":
            server_toml += "dynamic = true\n"
        server_toml += "direct = true\n\n"
    server_toml += (f'[p2p]\nbind = "127.0.0.1:{p2p}"\nadvertise = "127.0.0.1:{p2p}"\n'
                    f'timeout = "5s"\nmax_pending = 128\n')
    (HERE / "server.toml").write_text(server_toml, encoding="utf-8")
    for name, subject in (("bridge", "/CN=felucca-bridge"),
                          ("app", "/CN=app.test,san=DNS:app.test")):
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-days", "30", "-subj", subject, "-addext", "subjectAltName=DNS:app.test",
                        "-keyout", str(HERE / f"{name}.key"),
                        "-out", str(HERE / f"{name}.pem")], check=True, capture_output=True,
                       env=CLEAN_ENV)

    server_log = HERE / "server.log"
    fingerprint = subprocess.run([str(SERVER), "--config", str(HERE / "server.toml"),
                                  "--fingerprint"], cwd=str(ROOT), env=CLEAN_ENV,
                                 capture_output=True, text=True, timeout=60).stdout.strip()
    say(f"bridge fingerprint: {fingerprint}")
    spawn([SERVER, "--config", HERE / "server.toml"], server_log)
    if not check("server started (tcp/tls/ws/wss + quic + kcp)",
                 wait_for_log(server_log, "felucca-server ready", 30)):
        say(server_log.read_text(errors="replace")[-800:])
        report()
        return 1

    # ------------------------------------------------------------------ #
    # Phase A: one agent per bridge transport
    # ------------------------------------------------------------------ #
    tunnels = {name: {"tcp": free_port(), "udp": free_port()} for name in TRANSPORTS}
    trust_block = f'[trust]\nmode = "pinned"\nfingerprint = "{fingerprint}"\n'
    for name in TRANSPORTS:
        # KCP lives on its own UDP socket; every other transport shares the bridge port.
        endpoint = kcp if name == "kcp" else bridge
        lines = [f'server = "127.0.0.1:{endpoint}"',
                 f'key_id = "a-{name}"',
                 f'vkey = "{vkey}"',
                 f'transport = "{name}"',
                 trust_block,
                 f'[[tunnel]]\nid = 1\nmode = "tcp"\nport = {tunnels[name]["tcp"]}\nbind = "127.0.0.1"',
                 '[[tunnel.backend]]', f'addr = "127.0.0.1:{backend_http}"\n',
                 f'[[tunnel]]\nid = 2\nmode = "udp"\nport = {tunnels[name]["udp"]}\nbind = "127.0.0.1"',
                 '[[tunnel.backend]]', f'addr = "127.0.0.1:{udp_port}"\n']
        if name == "kcp":
            lines.insert(5, "[kcp]\ndata_shards = 10\nparity_shards = 3\nmtu = 1200\n"
                            "send_window = 256\nreceive_window = 256\ninterval_ms = 20\n")
        (HERE / f"agent-{name}.toml").write_text("\n".join(lines), encoding="utf-8")
        agent_env = dict(CLEAN_ENV)
        if name == "kcp":
            agent_env["RUST_LOG"] = ("info,felucca_transport::kcp=debug,"
                                     "felucca_client::agent=debug")
        spawn([AGENT, "--config", HERE / f"agent-{name}.toml"],
              HERE / f"agent-{name}.log", agent_env)
    say("\n[phase A] bridge transports")
    for name in TRANSPORTS:
        agent_log = HERE / f"agent-{name}.log"
        detail = ANSI.sub("", agent_log.read_text(errors="replace"))[-300:] \
            if agent_log.exists() else ""
        if not check(f"transport {name}: agent authenticated",
                     wait_for_log(server_log, f"key_id=a-{name}", 60), detail):
            say(f"  --- agent-{name}.log tail ---\n{detail}")
            continue
        status, _, body = http_get(f"http://127.0.0.1:{tunnels[name]['tcp']}/api/ping")
        check(f"transport {name}: TCP tunnel forwards HTTP",
              status == 200 and RUN.encode() in body, f"status={status} body={body[:100]!r}")
        payloads = [b"", b"ping", b"x" * 1400, secrets.token_bytes(8192)]
        if sys.platform.startswith("linux"):
            payloads.append(secrets.token_bytes(60000))
        replies = udp_roundtrip(tunnels[name]["udp"], payloads)
        check(f"transport {name}: UDP tunnel preserves datagrams",
              replies == payloads,
              f"sent {[len(item) for item in payloads]} got {[len(item) for item in replies]}")

    # ------------------------------------------------------------------ #
    # Phase B: payload protocols over the TLS bridge
    # ------------------------------------------------------------------ #
    say("\n[phase B] payload protocols (bridge transport = tls)")
    ports = {key: free_port() for key in
             ("tcp", "udp", "socks5", "httpproxy", "mixed", "file", "codec")}
    visitor = {key: free_port() for key in ("tcp", "socks5", "udp", "direct")}
    pin_block = f'[trust]\nmode = "pinned"\nfingerprint = "{fingerprint}"\n'
    (HERE / "files").mkdir()
    (HERE / "files" / "hello.txt").write_text("felucca-file-service\n", encoding="utf-8")

    payload_toml = f'''server = "127.0.0.1:{bridge}"
key_id = "a-payload"
vkey = "{vkey}"
transport = "tls"
{trust_block}
[[tunnel]]
id = 10
mode = "tcp"
port = {ports["tcp"]}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend_http}"

[[tunnel]]
id = 11
mode = "udp"
port = {ports["udp"]}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{udp_port}"

[[tunnel]]
id = 12
mode = "http"
port = {entry}
hosts = ["app.test"]
[[tunnel.backend]]
addr = "127.0.0.1:{backend_http}"

[[tunnel]]
id = 18
mode = "http"
port = {entry}
hosts = ["ws.test"]
[[tunnel.backend]]
addr = "127.0.0.1:{ws_port}"

[[tunnel]]
id = 13
mode = "socks5"
port = {ports["socks5"]}
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
id = 14
mode = "httpproxy"
port = {ports["httpproxy"]}
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
id = 15
mode = "mixedproxy"
port = {ports["mixed"]}
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
id = 16
mode = "file"
port = {ports["file"]}
bind = "127.0.0.1"
[tunnel.file]
root = "{HERE}/files"
read_only = true
auto_index = true
[tunnel.file.auth.users]
reader = "filepass"

[[tunnel]]
id = 17
mode = "tcp"
port = {ports["codec"]}
bind = "127.0.0.1"
[tunnel.options.payload]
compress = true
encrypt = true
[[tunnel.backend]]
addr = "127.0.0.1:{backend_http}"
'''
    (HERE / "agent-payload.toml").write_text(payload_toml, encoding="utf-8")
    spawn([AGENT, "--config", HERE / "agent-payload.toml"], HERE / "agent-payload.log")
    if not check("payload agent authenticated", wait_for_log(server_log, "key_id=a-payload", 45)):
        report()
        return 1

    code, out, err = curl(["-H", "Host: app.test", "-o", "/dev/null",
                           "-w", "%{http_code} %{http_version}",
                           f"http://127.0.0.1:{entry}/api/ping"])
    check("reverse proxy: HTTP/1.1 entry -> tunnel -> backend",
          code == 0 and out.startswith("200 1.1"), f"code={code} out={out!r} {err[:200]}")

    (HERE / "ws-client.mjs").write_text(
        "import net from 'node:net';\n"
        "import crypto from 'node:crypto';\n"
        "const [port, host, payload] = process.argv.slice(2);\n"
        "const key = crypto.randomBytes(16).toString('base64');\n"
        "const timer = setTimeout(() => { console.log('TIMEOUT'); process.exit(3); }, 20000);\n"
        "const sock = net.connect(Number(port), '127.0.0.1', () => {\n"
        "  sock.write('GET /ws HTTP/1.1\\r\\n' +\n"
        "    `Host: ${host}\\r\\n` +\n"
        "    'Upgrade: websocket\\r\\nConnection: Upgrade\\r\\n' +\n"
        "    `Sec-WebSocket-Key: ${key}\\r\\nSec-WebSocket-Version: 13\\r\\n\\r\\n`);\n"
        "});\n"
        "let handshake = Buffer.alloc(0);\n"
        "let stream = Buffer.alloc(0);\n"
        "let upgraded = false;\n"
        "function encode(payload) {\n"
        "  const mask = crypto.randomBytes(4);\n"
        "  const body = Buffer.from(payload);\n"
        "  let header;\n"
        "  if (body.length < 126) { header = Buffer.alloc(2); header[1] = 0x80 | body.length; }\n"
        "  else { header = Buffer.alloc(4); header[1] = 0x80 | 126; header.writeUInt16BE(body.length, 2); }\n"
        "  header[0] = 0x81;\n"
        "  for (let i = 0; i < body.length; i += 1) body[i] ^= mask[i % 4];\n"
        "  return Buffer.concat([header, mask, body]);\n"
        "}\n"
        "function onFrame(buffer) {\n"
        "  if (buffer.length < 2) return;\n"
        "  const opcode = buffer[0] & 0x0f;\n"
        "  let length = buffer[1] & 0x7f;\n"
        "  let offset = 2;\n"
        "  if (length === 126) { if (buffer.length < 4) return; length = buffer.readUInt16BE(2); offset = 4; }\n"
        "  if (buffer.length < offset + length) return;\n"
        "  const data = buffer.subarray(offset, offset + length).toString();\n"
        "  clearTimeout(timer);\n"
        "  if (opcode === 0x1 && data === payload) { console.log('ECHO-OK'); process.exit(0); }\n"
        "  console.log('ECHO-BAD ' + data);\n"
        "  process.exit(4);\n"
        "}\n"
        "sock.on('data', (chunk) => {\n"
        "  if (!upgraded) {\n"
        "    handshake = Buffer.concat([handshake, chunk]);\n"
        "    const text = handshake.toString('latin1');\n"
        "    const end = text.indexOf('\\r\\n\\r\\n');\n"
        "    if (end === -1) return;\n"
        "    const status = text.slice(0, text.indexOf('\\r\\n'));\n"
        "    if (!status.includes('101')) { console.log('NO-UPGRADE ' + status); process.exit(5); }\n"
        "    upgraded = true;\n"
        "    sock.write(encode(payload));\n"
        "    const rest = handshake.subarray(end + 4);\n"
        "    if (rest.length) onFrame(rest);\n"
        "    return;\n"
        "  }\n"
        "  stream = Buffer.concat([stream, chunk]);\n"
        "  onFrame(stream);\n"
        "});\n"
        "sock.on('error', (error) => { console.log('ERROR ' + error.message); process.exit(6); });\n",
        encoding="utf-8")
    ws_run = node(HERE / "ws-client.mjs", entry, "ws.test", "matrix-" + secrets.token_hex(4))
    check("reverse proxy: WebSocket upgrade relays frames both ways",
          ws_run.returncode == 0 and b"ECHO-OK" in ws_run.stdout,
          f"rc={ws_run.returncode} {ws_run.stdout!r} {ws_run.stderr.decode()[:200]}")

    (HERE / "h2-get.mjs").write_text(
        "import http2 from 'node:http2';\n"
        "const [run, port] = process.argv.slice(2);\n"
        "const client = http2.connect(`https://127.0.0.1:${port}`, {\n"
        "  rejectUnauthorized: false, servername: 'app.test', ALPNProtocols: ['h2'],\n"
        "});\n"
        "const timer = setTimeout(() => { console.log('TIMEOUT'); process.exit(3); }, 20000);\n"
        "client.on('error', (error) => { console.log('ERROR ' + error.message); process.exit(5); });\n"
        "const request = client.request({\n"
        "  ':method': 'GET', ':scheme': 'https', ':authority': 'app.test', ':path': '/api/ping',\n"
        "});\n"
        "let body = '';\n"
        "let status = 0;\n"
        "request.on('response', (headers) => { status = Number(headers[':status']); });\n"
        "request.on('data', (chunk) => { body += chunk; });\n"
        "request.on('end', () => {\n"
        "  clearTimeout(timer);\n"
        "  const alpn = client.socket?.alpnProtocol || 'none';\n"
        "  console.log(`${status} ${alpn} ${body.includes(run) ? 'RUN-OK' : 'RUN-BAD'}`);\n"
        "  client.close();\n"
        "  process.exit(status === 200 && body.includes(run) ? 0 : 4);\n"
        "});\n"
        "request.end();\n",
        encoding="utf-8")
    h2_run = node(HERE / "h2-get.mjs", RUN, entry)
    check("reverse proxy: HTTP/2 over TLS entry (ALPN h2)",
          h2_run.returncode == 0 and b" h2 " in h2_run.stdout,
          f"rc={h2_run.returncode} {h2_run.stdout!r} {h2_run.stderr.decode()[:200]}")

    (HERE / "h3-get.py").write_text(
        "import asyncio, ssl, sys\n"
        "from aioquic.asyncio import QuicConnectionProtocol, connect\n"
        "from aioquic.h3.connection import H3Connection\n"
        "from aioquic.h3.events import DataReceived, HeadersReceived\n"
        "from aioquic.quic.events import ConnectionTerminated\n"
        "from aioquic.quic.configuration import QuicConfiguration\n"
        "\n"
        "class Client(QuicConnectionProtocol):\n"
        "    def __init__(self, *a, **kw):\n"
        "        super().__init__(*a, **kw)\n"
        "        self.http = H3Connection(self._quic)\n"
        "        self.done = self._loop.create_future()\n"
        "        self.status = 0\n"
        "        self.body = b''\n"
        "\n"
        "    def http_event_received(self, event):\n"
        "        if isinstance(event, HeadersReceived):\n"
        "            for key, value in event.headers:\n"
        "                if key == b':status':\n"
        "                    self.status = int(value)\n"
        "        elif isinstance(event, DataReceived):\n"
        "            self.body += event.data\n"
        "        if event.stream_ended:\n"
        "            self.done.set_result(None)\n"
        "\n"
        "    def quic_event_received(self, event):\n"
        "        for http_event in self.http.handle_event(event):\n"
        "            self.http_event_received(http_event)\n"
        "        if isinstance(event, ConnectionTerminated) and not self.done.done():\n"
        "            self.done.set_exception(ConnectionError(str(event)))\n"
        "\n"
        "async def main():\n"
        "    run, port = sys.argv[1], int(sys.argv[2])\n"
        "    config = QuicConfiguration(is_client=True, alpn_protocols=['h3'])\n"
        "    config.verify_mode = ssl.CERT_NONE\n"
        "    config.server_name = 'app.test'\n"
        "    async with connect('127.0.0.1', port, configuration=config,\n"
        "                     create_protocol=Client) as proto:\n"
        "        stream_id = proto._quic.get_next_available_stream_id()\n"
        "        proto.http.send_headers(stream_id, [\n"
        "            (b':method', b'GET'), (b':scheme', b'https'),\n"
        "            (b':authority', b'app.test'), (b':path', b'/api/ping')], end_stream=True)\n"
        "        proto.transmit()\n"
        "        try:\n"
        "            await asyncio.wait_for(proto.done, 10)\n"
        "        except asyncio.TimeoutError:\n"
        "            pass\n"
        "        print(proto.status)\n"
        "        print(proto.body.decode())\n"
        "\n"
        "asyncio.run(main())\n",
        encoding="utf-8")
    h3_run = subprocess.run([str(H3_PYTHON), str(HERE / "h3-get.py"), RUN, str(entry)],
                            cwd=str(ROOT), env=CLEAN_ENV, capture_output=True, timeout=90)
    h3_out = h3_run.stdout.decode(errors="replace")
    check("reverse proxy: HTTP/3 (QUIC) entry -> tunnel -> backend",
          h3_run.returncode == 0 and h3_out.startswith("200") and RUN in h3_out,
          f"rc={h3_run.returncode} out={h3_out[:160]!r} err={h3_run.stderr.decode()[-260:]}")

    request = (f"GET /api/ping HTTP/1.1\r\nHost: 127.0.0.1:{backend_http}\r\n"
               f"Connection: close\r\n\r\n").encode()
    try:
        _, body = socks5_stream(ports["socks5"], "127.0.0.1", backend_http, request)
        check("SOCKS5: CONNECT tunnelled to backend",
              body.startswith(b"HTTP/1.1 200") and RUN.encode() in body, f"{body[:120]!r}")
    except Exception as error:  # noqa: BLE001
        check("SOCKS5: CONNECT tunnelled to backend", False, repr(error))

    try:
        echoed = socks5_udp_associate(ports["socks5"], "127.0.0.1", udp_port,
                                      b"udp-assoc-" + secrets.token_hex(3).encode())
        check("SOCKS5: UDP ASSOCIATE relays datagrams",
              echoed is not None and echoed.startswith(b"udp-assoc-"), f"{echoed!r}")
    except Exception as error:  # noqa: BLE001
        check("SOCKS5: UDP ASSOCIATE relays datagrams", False, repr(error))

    code, out, err = curl(["-x", f"http://127.0.0.1:{ports['httpproxy']}", "-o", "/dev/null",
                           "-w", "%{http_code}", f"http://127.0.0.1:{backend_http}/api/ping"])
    check("HTTP forward proxy: absolute-URL request", code == 0 and out == "200",
          f"code={code} out={out!r} {err[:200]}")

    try:
        first, body = http_connect_tunnel(ports["httpproxy"], "127.0.0.1", backend_http, request)
        check("HTTP forward proxy: CONNECT tunnels TCP",
              body.startswith(b"HTTP/1.1 200") and RUN.encode() in body,
              f"{first} {body[:100]!r}")
    except Exception as error:  # noqa: BLE001
        check("HTTP forward proxy: CONNECT tunnels TCP", False, repr(error))

    try:
        _, body = socks5_stream(ports["mixed"], "127.0.0.1", backend_http, request)
        check("mixed proxy: SOCKS5 on the shared port",
              body.startswith(b"HTTP/1.1 200") and RUN.encode() in body, f"{body[:120]!r}")
    except Exception as error:  # noqa: BLE001
        check("mixed proxy: SOCKS5 on the shared port", False, repr(error))
    code, out, err = curl(["-x", f"http://127.0.0.1:{ports['mixed']}", "-o", "/dev/null",
                           "-w", "%{http_code}", f"http://127.0.0.1:{backend_http}/api/ping"])
    check("mixed proxy: HTTP absolute-URL on the shared port", code == 0 and out == "200",
          f"code={code} out={out!r} {err[:200]}")

    for label, port in (("plain TCP tunnel", ports["tcp"]),
                        ("compress+encrypt tunnel", ports["codec"])):
        code, out, err = curl(["-o", "/dev/null", "-w", "%{http_code} %{size_download}",
                               f"http://127.0.0.1:{port}/big"])
        check(f"{label}: 1 MiB transfer", code == 0 and out.startswith("200 1048576"),
              f"code={code} out={out!r} {err[:200]}")

    payloads = [b"", b"udp", b"y" * 2048, secrets.token_bytes(8192)]
    if sys.platform.startswith("linux"):
        payloads.append(secrets.token_bytes(60000))
    replies = udp_roundtrip(ports["udp"], payloads)
    check("UDP tunnel: empty/large/multiple datagrams preserved",
          replies == payloads,
          f"sent {[len(item) for item in payloads]} got {[len(item) for item in replies]}")

    status, _, body = http_get(f"http://127.0.0.1:{ports['file']}/hello.txt",
                               {"Authorization": "Basic "
                                + base64.b64encode(b"reader:filepass").decode()})
    check("file service: authenticated download",
          status == 200 and body == b"felucca-file-service\n", f"status={status} {body[:80]!r}")
    code, out, err = curl(["-u", "reader:filepass", "-H", "Range: bytes=0-6", "-o", "/dev/null",
                           "-w", "%{http_code} %{size_download}",
                           f"http://127.0.0.1:{ports['file']}/hello.txt"])
    check("file service: Range request returns partial content",
          code == 0 and out.startswith("206 7"), f"code={code} out={out!r} {err[:200]}")
    code, out, err = curl(["-u", "reader:filepass", "-X", "PUT", "--data-binary", "x",
                           "-o", "/dev/null", "-w", "%{http_code}",
                           f"http://127.0.0.1:{ports['file']}/hello.txt"])
    check("file service: read_only rejects writes",
          code == 0 and status_of(out) in (400, 403, 405),
          f"code={code} out={out!r} {err[:200]}")
    code, out, err = curl(["--path-as-is", "-u", "reader:filepass", "-o", "/dev/null",
                           "-w", "%{http_code}",
                           f"http://127.0.0.1:{ports['file']}/../server.toml"])
    check("file service: directory traversal refused",
          code == 0 and status_of(out) in (400, 403, 404),
          f"code={code} out={out!r} {err[:200]}")
    code, out, err = curl(["-o", "/dev/null", "-w", "%{http_code}",
                           f"http://127.0.0.1:{ports['file']}/hello.txt"])
    check("file service: anonymous request rejected",
          code == 0 and status_of(out) in (401, 403), f"code={code} out={out!r} {err[:200]}")

    code, out, err = curl(["-u", "reader:filepass", "-X", "PROPFIND", "-H", "Depth: 1",
                           "-o", "/dev/null", "-w", "%{http_code}",
                           f"http://127.0.0.1:{ports['file']}/"])
    check("file service: WebDAV PROPFIND lists the directory",
          code == 0 and out == "207", f"code={code} out={out!r} {err[:200]}")
    code, headers, _ = curl(["-u", "reader:filepass", "-X", "OPTIONS", "-D", "-",
                             "-o", "/dev/null", f"http://127.0.0.1:{ports['file']}/"])
    check("file service: WebDAV OPTIONS advertises DAV",
          code == 0 and "dav" in headers.lower(), f"code={code} dav header: "
          f"{[line for line in headers.splitlines() if 'dav' in line.lower()][:2]}")

    # ------------------------------------------------------------------ #
    # Phase C: private share + visitor
    # ------------------------------------------------------------------ #
    say("\n[phase C] private share and visitor")
    share_toml = f'''server = "127.0.0.1:{bridge}"
key_id = "a-share"
vkey = "{vkey}"
transport = "tls"
{pin_block}
[p2p]
enabled = true
max_sessions = 64

[[tunnel]]
id = 1
mode = "tcp"
public = false
[[tunnel.backend]]
addr = "127.0.0.1:{backend_http}"

[[tunnel]]
id = 2
mode = "udp"
public = false
[[tunnel.backend]]
addr = "127.0.0.1:{udp_port}"

[[tunnel]]
id = 3
mode = "socks5"
public = false
[tunnel.target_policy]
allow = ["127.0.0.0/8", "::1"]
'''
    (HERE / "agent-share.toml").write_text(share_toml, encoding="utf-8")
    spawn([AGENT, "--config", HERE / "agent-share.toml"], HERE / "agent-share.log")
    if not check("provider agent authenticated",
                 wait_for_log(server_log, "key_id=a-share", 45)):
        report()
        return 1

    visitor_toml = f'''server = "127.0.0.1:{bridge}"
key_id = "a-visitor"
vkey = "{vkey}"
transport = "tls"
[trust]
mode = "pinned"
fingerprint = "{fingerprint}"
[[visitor]]
name = "share-web"
share = "share-web"
secret = "{share_secret}"
bind = "127.0.0.1:{visitor['tcp']}"
mode = "tcp"
connection = "prefer_direct"

[[visitor]]
name = "share-web-strict"
share = "share-web"
secret = "{share_secret}"
bind = "127.0.0.1:{visitor['direct']}"
mode = "tcp"
connection = "direct"

[[visitor]]
name = "share-proxy"
share = "share-proxy"
secret = "{share_secret}"
bind = "127.0.0.1:{visitor['socks5']}"
mode = "socks5"
connection = "prefer_direct"

[[visitor]]
name = "share-dns"
share = "share-dns"
secret = "{share_secret}"
bind = "127.0.0.1:{visitor['udp']}"
mode = "udp"
connection = "prefer_direct"
'''
    (HERE / "agent-visitor.toml").write_text(visitor_toml, encoding="utf-8")
    spawn([AGENT, "--config", HERE / "agent-visitor.toml"], HERE / "agent-visitor.log")

    ready = False
    last = ""
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            status, _, body = http_get(f"http://127.0.0.1:{visitor['tcp']}/api/ping")
            last = f"status={status} body={body[:80]!r}"
            if status == 200 and RUN.encode() in body:
                ready = True
                break
        except Exception as error:  # noqa: BLE001
            last = repr(error)
        time.sleep(0.3)
    check("visitor: TCP private share reaches provider backend", ready, last)

    try:
        _, body = socks5_stream(visitor["socks5"], "127.0.0.1", backend_http, request)
        check("visitor: SOCKS5 private share reaches provider backend",
              body.startswith(b"HTTP/1.1 200") and RUN.encode() in body, f"{body[:120]!r}")
    except Exception as error:  # noqa: BLE001
        check("visitor: SOCKS5 private share reaches provider backend", False, repr(error))

    echoed = udp_roundtrip(visitor["udp"], [b"visitor-udp", b"z" * 2048])
    check("visitor: UDP private share preserves datagrams",
          [len(item) for item in echoed] == [11, 2048] and echoed[0:1] == [b"visitor-udp"],
          f"{[len(item) for item in echoed]}")

    # Direct-only visitor: no relay fallback, so a pass proves the hole-punched path.
    ready = False
    last = ""
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            status, _, body = http_get(f"http://127.0.0.1:{visitor['direct']}/api/ping")
            last = f"status={status} body={body[:80]!r}"
            if status == 200 and RUN.encode() in body:
                ready = True
                break
        except Exception as error:  # noqa: BLE001
            last = repr(error)
        time.sleep(0.3)
    check("visitor: connection=direct reaches the provider with no relay fallback",
          ready, last)

    report()
    return 0 if all(item["ok"] for item in results) else 1


def report():
    passed = sum(1 for item in results if item["ok"])
    say(f"\n==== {passed}/{len(results)} checks passed ====")
    for item in results:
        if not item["ok"]:
            say(f"FAIL {item['name']}: {item['detail']}")
    (HERE / f"matrix-{RUN}.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    say(f"artifacts: {HERE}")


def cleanup():
    for process in reversed(processes):
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                pass
    deadline = time.monotonic() + 6
    for process in reversed(processes):
        if process.poll() is None:
            try:
                process.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    pass


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        cleanup()
