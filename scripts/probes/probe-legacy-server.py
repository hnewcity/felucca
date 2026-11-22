#!/usr/bin/env python3
"""Verify a current local agent against a retained, unmodified old server image."""
import argparse
import hashlib
import http.cookiejar
import http.server
import json
from pathlib import Path
import secrets
import socket
import subprocess
import threading
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True, mode=0o700)
docker = ["docker", "--context", "orbstack"]
container = "felucca-compat-" + uuid.uuid4().hex[:12]
agent = ROOT / "target/debug/felucca-agent"
server = ROOT / "target/debug/felucca-server"


def command(argv, **kwargs):
    return subprocess.run(argv, capture_output=True, check=True, **kwargs).stdout


def free_port():
    # Use non-ephemeral ports for explicit host-network listener verification.
    for _ in range(100):
        port = 20000 + secrets.randbelow(10000)
        try:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", port))
            return port
        except OSError:
            pass
    raise RuntimeError("no free test port")


def until(check, label):
    deadline = time.monotonic() + 25
    last = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, ValueError) as error:
            last = error
        time.sleep(0.1)
    raise RuntimeError(f"{label} timed out: {last}")


class Backend(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        body = b"felucca-legacy-server\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


backend = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Backend)
threading.Thread(target=backend.serve_forever, daemon=True).start()
admin_port, bridge_port, service_port = [free_port() for _ in range(3)]
origin = f"http://127.0.0.1:{admin_port}"
cookie = http.cookiejar.CookieJar()
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                                     urllib.request.HTTPCookieProcessor(cookie))
password, key = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
csrf = None


def api(path, value=None, method=None, revision=None):
    headers = {"Origin": origin, "Content-Type": "application/json"}
    if csrf:
        headers["X-Felucca-CSRF"] = csrf
    if revision is not None:
        headers.update({"If-Match": f'"{revision}"', "Idempotency-Key": str(uuid.uuid4())})
    request = urllib.request.Request(origin + path, method=method, headers=headers,
                                     data=None if value is None else json.dumps(value).encode())
    with opener.open(request, timeout=5) as response:
        return json.load(response)


process = None
try:
    cert, private_key = output / "bridge.pem", output / "bridge.key"
    command(["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
             "-nodes", "-days", "2", "-subj", "/CN=localhost", "-keyout", str(private_key),
             "-out", str(cert)])
    private_key.chmod(0o600)
    password_hash = command([str(server), "--hash-password"], input=(password + "\n").encode()).decode().strip()
    configuration = {
        "bridge": {"bind": f"0.0.0.0:{bridge_port}", "tls": {"cert": str(cert), "key": str(private_key)}},
        "admin": {"bind": f"127.0.0.1:{admin_port}", "state_path": str(output / "runtime.json"),
                  "account": {"root": {"role": "admin", "password_hash": password_hash}}},
        "client": {"legacy-device": {"vkey": key}},
        "managed": {"legacy-device": {"tunnel": [{
            "id": 1, "mode": "tcp", "port": service_port, "bind": "127.0.0.1",
            "backend": [{"addr": f"127.0.0.1:{backend.server_port}"}],
        }]}},
    }
    server_file = output / "server.json"
    server_file.write_text(json.dumps(configuration))
    server_file.chmod(0o600)
    # Only this private fixture is mounted. Root is confined to this disposable container;
    # no host service, firewall, credential store or live server configuration is changed.
    command([*docker, "run", "-d", "--rm", "--name", container, "--network", "host",
             "--user", "0:0", "--mount", f"type=bind,source={output},target={output}",
             args.image, "--config", str(server_file)])
    until(lambda: opener.open(origin, timeout=2).status == 200, "old server")
    session = api("/api/login", {"account": "root", "password": password})
    csrf = session["csrf"]
    launch = api("/api/launch")
    config = {"server": f"127.0.0.1:{bridge_port}", "key_id": "legacy-device", "vkey": key,
              "transport": "tls", "trust": {"mode": "pinned", "fingerprint": launch["fingerprint"]},
              "remote": {"enabled": True, "target_policy": {"allow": ["ip:127.0.0.1/32"]}}}
    agent_file = output / "agent.json"
    agent_file.write_text(json.dumps(config))
    agent_file.chmod(0o600)
    with (output / "agent.log").open("wb") as log:
        process = subprocess.Popen([str(agent), "--config", str(agent_file)], stdout=log, stderr=log)
        def online():
            return next((row for row in api("/api/overview")["clients"] if row["online"]), None)
        until(online, "current agent registration")
        def forwarded():
            with opener.open(f"http://127.0.0.1:{service_port}/", timeout=3) as response:
                return response.read() == b"felucca-legacy-server\n"
        until(forwarded, "initial forwarding registration")
        import http.client
        connection = http.client.HTTPConnection("127.0.0.1", service_port, timeout=3)
        held = None
        for _ in range(12):
            connection.request("GET", "/")
            response = connection.getresponse()
            assert response.status == 200 and response.read() == b"felucca-legacy-server\n"
            assert connection.sock is not None
            held = held or connection.sock
            assert connection.sock is held, "HTTP client had to reconnect"
            time.sleep(0.5)
        connection.close()
        logged = subprocess.run([*docker, "logs", container], capture_output=True)
        logs = (logged.stdout + logged.stderr).decode()
        assert logs.count("agent connected") == 1, "agent repeatedly reconnected"
        overview = api("/api/overview")
        value = configuration["client"]["legacy-device"] | {"remark": "renamed on old server"}
        api("/api/clients/legacy-device", value, method="PUT", revision=overview["revision"])
        until(forwarded, "legacy reload recovery")
    report = {
        "status": "passed", "old_server_image": args.image,
        "old_server_image_id": command([*docker, "image", "inspect", args.image,
                                       "--format", "{{.Id}}"]).decode().strip(),
        "agent_sha256": hashlib.sha256(agent.read_bytes()).hexdigest(),
        "checks": ["current agent remains on one session with legacy v10 server",
                   "12 real HTTP requests over one held connection without reconnect",
                   "legacy full reload reconnects and restores forwarding"],
    }
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
finally:
    if process is not None:
        process.terminate()
        process.wait(timeout=10)
    logged = subprocess.run([*docker, "logs", container], capture_output=True)
    (output / "server.log").write_bytes(logged.stdout + logged.stderr)
    subprocess.run([*docker, "stop", "--time", "10", container], capture_output=True)
    backend.shutdown()
    backend.server_close()
