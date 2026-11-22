#!/usr/bin/env python3
"""Real server/agent, WSS platform, signed callbacks, restart, and Chrome UI.

Uses installed Python websockets and playwright with local Google Chrome.
All fixtures bind loopback; credentials/configuration are temporary and private.
"""
import asyncio
import hashlib
import hmac
import http.client
import http.server
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import socketserver
import ssl
import subprocess
import tempfile
import threading
import time

from websockets.asyncio.server import serve
from playwright.async_api import async_playwright

ROOT = Path(__file__).resolve().parents[2]
PROFILE = os.environ.get("FELUCCA_PROBE_PROFILE", "release")
OUTPUT = ROOT / os.environ.get("FELUCCA_PROBE_OUTPUT", "docs/audit")
BIN = {role: ROOT / "target" / PROFILE / f"felucca-{role}" for role in ("server", "agent")}


def port():
    with socket.socket() as stream:
        stream.bind(("127.0.0.1", 0))
        return stream.getsockname()[1]


async def until(check, label, timeout=20):
    end = time.monotonic() + timeout
    last = None
    while time.monotonic() < end:
        try:
            result = await check()
            if result:
                return result
        except (OSError, ValueError) as error:
            last = error
        await asyncio.sleep(0.05)
    raise AssertionError(f"timed out: {label}; {last}")


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while data := self.request.recv(65536):
            self.request.sendall(data)


class EchoServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


class Platform:
    def __init__(self, token):
        self.token, self.socket = token, None
        self.pending, self.events = {}, []
        self.connections, self.requests = 0, 0

    async def handler(self, socket):
        assert socket.request.headers["Authorization"] == f"Bearer {self.token}"
        hello = json.loads(await socket.recv())
        assert hello["type"] == "hello" and hello["node"] == "probe-node"
        assert socket.subprotocol == "felucca-node-v1"
        self.socket = socket
        self.connections += 1
        try:
            async for raw in socket:
                data = json.loads(raw)
                if data["type"] == "ping":
                    await socket.send(json.dumps({"type": "pong", "nonce": data["nonce"]}))
                elif data["type"] == "response":
                    future = self.pending.pop(data["id"], None)
                    if future is not None and not future.done():
                        future.set_result(data)
                else:
                    self.events.append(data)
        except Exception:
            # Reconfiguration intentionally closes transports without a WS close frame.
            pass
        finally:
            if self.socket is socket:
                self.socket = None

    async def rpc(self, method, path, body=None, revision=0, key=None):
        async def connected():
            return self.socket
        socket = await until(connected, "reverse platform connection")
        self.requests += 1
        identity = f"rpc-{self.requests}"
        future = asyncio.get_running_loop().create_future()
        self.pending[identity] = future
        request = {"type": "request", "id": identity, "method": method, "path": path}
        if body is not None:
            request.update(body=body, headers={"if-match": f'"{revision}"',
                                               "idempotency-key": key or identity})
        await socket.send(json.dumps(request))
        return await asyncio.wait_for(future, 15)


async def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    temp = Path(tempfile.mkdtemp(prefix="node-probe-", dir=ROOT / "target/tmp"))
    children, exits, js_errors, callbacks = [], [], [], []
    callback_success = threading.Event()
    signing = secrets.token_hex(32)
    password = "node-browser-management-password"
    success = False

    class Callback(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            timestamp = self.headers["X-Felucca-Timestamp"]
            identity = self.headers["X-Felucca-Event-Id"]
            payload = f"felucca-callback-v1\n{timestamp}\n{identity}\n".encode() + body
            expected = "v1=" + hmac.new(signing.encode(), payload, hashlib.sha256).hexdigest()
            valid = hmac.compare_digest(self.headers["X-Felucca-Signature"], expected)
            callbacks.append({"id": identity, "valid": valid, "event": json.loads(body)})
            self.send_response(200 if callback_success.is_set() else 503)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *_):
            pass

    callback = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Callback)
    echo = EchoServer(("127.0.0.1", 0), Echo)
    for fixture in (callback, echo):
        threading.Thread(target=fixture.serve_forever, daemon=True).start()

    def command(role, args, text=None):
        return subprocess.run([str(BIN[role]), *args], input=text, text=True,
                              capture_output=True, check=True).stdout.strip()

    def save(name, data):
        path = temp / name
        path.write_text(json.dumps(data))
        path.chmod(0o600)
        return path

    def start(role, path):
        with (temp / f"{role}-{len(children)}.log").open("wb") as log:
            child = subprocess.Popen([str(BIN[role]), "--config", str(path)],
                                     stdout=log, stderr=subprocess.STDOUT)
        children.append(child)
        return child

    async def stop(child):
        child.terminate()
        code = await asyncio.to_thread(child.wait, 20)
        assert code == 0, code
        exits.append(code)

    try:
        credential = json.loads(command("server", ["--generate-token"]))
        password_hash = command("server", ["--hash-password"], password + "\n")
        platform_token, tenant_token, vkey = [secrets.token_hex(32) for _ in range(3)]
        bridge, admin, public = [port() for _ in range(3)]
        cert, key = temp / "cert.pem", temp / "key.pem"
        openssl = temp / "openssl.cnf"
        openssl.write_text("[req]\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\n[ext]\nsubjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
        subprocess.run(["/usr/bin/openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-sha256", "-days", "1", "-config", str(openssl), "-subj", "/CN=127.0.0.1",
                        "-keyout", str(key), "-out", str(cert)], check=True, capture_output=True)
        der = subprocess.run(["/usr/bin/openssl", "x509", "-in", str(cert), "-outform", "DER"],
                             check=True, capture_output=True).stdout
        fingerprint = hashlib.sha256(der).hexdigest()
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(cert, key)
        platform = Platform(platform_token)
        async with serve(platform.handler, "127.0.0.1", 0, ssl=tls,
                         subprotocols=["felucca-node-v1"], compression=None, close_timeout=1) as ws:
            ws_port = ws.sockets[0].getsockname()[1]
            config = {
                "bridge": {"bind": f"127.0.0.1:{bridge}", "tls": {"cert": str(cert), "key": str(key)}},
                "admin": {"bind": f"127.0.0.1:{admin}", "state_path": str(temp / "runtime.json"),
                          "account": {"root": {"role": "admin", "password_hash": password_hash,
                                               "token_hash": credential["token_hash"]}}},
                "state": {"path": str(temp / "traffic.json")},
                "user": {"team-a": {}, "team-b": {}},
                "client": {"office": {"user": "team-a", "vkey": vkey, "allow_config_push": True,
                                      "quota": {"flow_limit": 1048576}},
                           "foreign": {"user": "team-b", "vkey": secrets.token_hex(32)}},
                "node": {"id": "probe-node", "state_path": str(temp / "node.json"),
                         "usage_interval": "1s", "traffic_report_step_bytes": 1,
                         "platform": {
                             "reverse": {"token": platform_token, "scope": "account", "user": "team-a",
                                         "mode": "reverse", "reverse_url": f"wss://127.0.0.1:{ws_port}/control",
                                         "trust": {"mode": "pinned", "fingerprint": fingerprint}, "heartbeat": "1s"},
                             "direct": {"token": tenant_token, "scope": "account", "user": "team-a"}},
                         "webhook": {"events": {"user": "team-a", "delivery": {
                             "url": f"http://127.0.0.1:{callback.server_port}/events", "signing_key": signing,
                             "retry_max": 1, "retry_backoff": "100ms", "queue_max": 10}}}},
            }
            config_path = save("server.json", config)
            agent_path = save("agent.json", {
                "server": f"127.0.0.1:{bridge}", "transport": "tls", "key_id": "office", "vkey": vkey,
                "trust": {"mode": "pinned", "fingerprint": fingerprint},
                "reconnect_min": "50ms", "reconnect_max": "100ms",
                "tunnel": [{"id": 7, "mode": "tcp", "port": public, "bind": "127.0.0.1",
                            "backend": [{"addr": f"127.0.0.1:{echo.server_address[1]}"}]}],
            })
            server = start("server", config_path)
            agent = start("agent", agent_path)

            async def api(path, method="GET", body=None, token=None, revision=0, key=None):
                def request():
                    client = http.client.HTTPConnection("127.0.0.1", admin, timeout=10)
                    headers = {"Authorization": f"Bearer {token or credential['token']}"}
                    data = None if body is None else json.dumps(body)
                    if method != "GET":
                        headers.update({"Content-Type": "application/json", "If-Match": f'"{revision}"',
                                        "Idempotency-Key": key or secrets.token_hex(16)})
                    try:
                        client.request(method, path, data, headers)
                        response = client.getresponse()
                        return response.status, json.loads(response.read())
                    finally:
                        client.close()
                return await asyncio.to_thread(request)

            async def ready():
                code, data = await api("/api/overview")
                return code == 200 and any(c["id"] == "office" and c["online"] for c in data["clients"]) and len(data["tunnels"]) == 1
            await until(ready, "agent and tunnel")
            assert (await api("/api/clients", token=platform_token))[0] == 401
            assert (await api("/api/clients/foreign", token=tenant_token))[0] == 404
            reply = await platform.rpc("GET", "/api/clients")
            assert reply["status"] == 200 and len(reply["body"]["config"]) == 1
            assert (await platform.rpc("GET", "/api/clients/foreign"))["status"] == 404
            plaintext = b"node-control-echo"

            def transfer():
                with socket.create_connection(("127.0.0.1", public), timeout=5) as io:
                    io.sendall(plaintext)
                    io.shutdown(socket.SHUT_WR)
                    output = b""
                    while chunk := io.recv(65536):
                        output += chunk
                    assert output == plaintext
            await asyncio.to_thread(transfer)
            report = {"items": [{"client": "office", "inlet_bytes": 30, "export_bytes": 40}]}
            imported = await platform.rpc("POST", "/api/node/traffic", report, key="import-001")
            assert imported["status"] == 200 and imported["body"]["durable"]
            repeated = await platform.rpc("POST", "/api/node/traffic", report, key="import-001")
            assert repeated["body"]["replayed"]
            async def traffic_event():
                return any(e.get("ephemeral") and e.get("event", {}).get("kind") == "client.traffic.reported"
                           for e in platform.events)
            await until(traffic_event, "live WSS traffic report")

            async def failed():
                _, data = await api("/api/node/callbacks")
                return next((item for queue in data["queues"] for item in queue["items"] if item["failed"]), None)
            item = await until(failed, "durable callback failure")
            assert item["attempts"] == 2 and item["last_status"] == 503
            event_id = item["id"]
            assert sum(e["id"] == event_id for e in callbacks) == 2
            epoch = (await api("/api/node/status"))[1]["epoch"]
            await stop(server)
            server = start("server", config_path)
            await until(ready, "agent reconnect after restart")
            assert (await api("/api/node/status"))[1]["epoch"] == epoch
            assert event_id in json.dumps((await api("/api/node/callbacks"))[1])
            assert (await api("/api/node/traffic", "POST", report, token=tenant_token, key="import-001"))[1]["replayed"] is False
            # HTTP direct and WSS reverse are distinct actors; an import retry
            # must use the same platform identity. Undoing a billed delta is not supported.
            assert (await platform.rpc("POST", "/api/node/traffic", report, key="import-001"))["body"]["replayed"]
            callback_success.set()
            replay_body = {"ids": [event_id]}
            assert (await api("/api/node/callbacks/replay", "POST", replay_body, key="replay-001"))[0] == 200
            async def delivered():
                return event_id not in json.dumps((await api("/api/node/callbacks"))[1])
            await until(delivered, "manual callback replay")
            assert (await api("/api/node/callbacks/replay", "POST", replay_body, key="replay-001"))[1]["replayed"]

            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(
                    executable_path=os.environ.get("FELUCCA_CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
                    headless=True)
                page = await browser.new_page(viewport={"width": 1360, "height": 960})
                page.on("pageerror", lambda error: js_errors.append(str(error)))
                # These assertions name Chinese labels, so the console is pinned to
                # Chinese instead of following the browser's language.
                await page.goto(f"http://127.0.0.1:{admin}")
                await page.evaluate('window.localStorage.setItem("felucca.locale","zh")')
                await page.reload()
                await page.get_by_label("账号", exact=True).fill("root")
                await page.get_by_label("密码", exact=True).fill(password)
                await page.get_by_role("button", name="登录", exact=True).click()
                await page.get_by_role("button", name="平台连接", exact=True).click()
                await page.get_by_role("heading", name="回调投递", exact=True).wait_for()
                browser_ws = await page.evaluate("""async () => {
                    const auth = await (await fetch('/api/session')).json();
                    const existing = await (await fetch('/api/clients/office')).json();
                    existing.config.remark = 'written through browser WebSocket';
                    return await new Promise((resolve, reject) => {
                        const ws = new WebSocket(`ws://${location.host}/api/ws`, ['felucca-node-v1']);
                        const timer = setTimeout(() => {ws.close(); reject(new Error('browser WS timeout'));}, 15000);
                        let written = false, callback = false;
                        const finish = () => {if (written && callback) {clearTimeout(timer); ws.close(); resolve({cookie_write:true,temporary_subscription:true});}};
                        ws.onerror = () => {clearTimeout(timer); reject(new Error('browser WS handshake failed'));};
                        ws.onmessage = ({data}) => {
                            const message = JSON.parse(data);
                            if (message.type === 'ping') ws.send(JSON.stringify({type:'pong',nonce:message.nonce}));
                            if (message.type === 'hello') ws.send(JSON.stringify({
                                type:'request',id:'subscribe',method:'PUT',path:'/api/realtime/subscriptions/browser',
                                body:{selector:{event_names:['client.updated'],clients:['office']},content:{body_template:{name:'{{event.client}}'}}}
                            }));
                            if (message.type === 'response' && message.id === 'subscribe') {
                                if (message.status !== 200) {clearTimeout(timer);ws.close();reject(new Error('browser subscription rejected'));return;}
                                ws.send(JSON.stringify({type:'request',id:'write',method:'PUT',path:'/api/clients/office',
                                    headers:{'if-match':`"${existing.revision}"`,'idempotency-key':'browser-ws-write','x-felucca-csrf':auth.csrf},
                                    body:existing.config}));
                            }
                            if (message.type === 'response' && message.id === 'write') {
                                if (message.status !== 200) {clearTimeout(timer);ws.close();reject(new Error('browser WS write rejected'));return;}
                                written=true;finish();
                            }
                            if (message.type === 'callback' && message.subscription === 'browser') {
                                if (message.body.name !== 'office') {clearTimeout(timer);ws.close();reject(new Error('wrong template content'));return;}
                                callback=true;finish();
                            }
                        };
                    });
                }""")
                assert browser_ws == {"cookie_write": True, "temporary_subscription": True}
                await page.get_by_role("button", name="刷新", exact=True).click()
                await page.get_by_role("heading", name="回调投递", exact=True).wait_for()
                await page.screenshot(path=str(OUTPUT / "node-desktop.png"), full_page=True)
                await page.get_by_role("button", name="事件回调", exact=True).click()
                await page.get_by_role("button", name="新增", exact=True).click()
                await page.get_by_label("名称", exact=True).fill("browser-hook")
                await page.get_by_label("所属用户（完整权限留空）", exact=True).fill("team-a")
                await page.get_by_label("接收地址", exact=True).fill(f"http://127.0.0.1:{callback.server_port}/browser")
                await page.get_by_label("签名密钥（至少 16 字节）", exact=True).fill(secrets.token_hex(32))
                await page.get_by_label("启用", exact=True).uncheck()
                await page.get_by_role("button", name="保存", exact=True).click()
                await page.get_by_role("row").filter(has_text="browser-hook").wait_for()
                await page.get_by_role("button", name="管理平台", exact=True).click()
                await page.get_by_role("row").filter(has_text="reverse").wait_for()
                assert not js_errors, js_errors
                await browser.close()

            code, view = await api("/api/overview")
            revision = view["revision"]
            kicked = await api("/api/node/kick", "POST", {"ids": ["office"]}, revision=revision, key="kick-001")
            assert kicked[0] == 200, kicked
            async def offline():
                _, view = await api("/api/overview")
                return any(c["id"] == "office" and not c["online"] and not c["enabled"] for c in view["clients"])
            await until(offline, "persistent kick")
            await asyncio.sleep(0.5)
            assert await offline() and agent.poll() is None
            await stop(agent)
            await stop(server)
            counters = json.loads((temp / "traffic.json").read_text())
            for resource, counts in counters["counters"]:
                if resource["kind"] in ("client", "user") and resource.get("name") in ("office", "team-a"):
                    assert counts == {"inlet_bytes": len(plaintext) + 60, "export_bytes": len(plaintext) + 80}, counts
            assert all(e["valid"] for e in callbacks)
            result = {
                "profile": PROFILE, "build": json.loads(command("server", ["--build-info"])),
                "wss_connections": platform.connections, "wss_rpc_requests": platform.requests,
                "reverse_token_http_refused": True, "tenant_isolation": True,
                "signed_callback_requests": len(callbacks), "failed_attempts": 2,
                "queue_and_epoch_survived_restart": True, "manual_replay_idempotent": True,
                "traffic_import_atomic_and_retry_safe": True, "ephemeral_traffic_report": True,
                "kick_prevented_running_agent_reconnect": True, "browser_webhook_created": True,
                "browser_javascript_errors": js_errors, "sigterm_exit_codes": exits,
                "browser_cookie_websocket": browser_ws,
                "binaries": {role: {"bytes": binary.stat().st_size,
                                    "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
                             for role, binary in BIN.items()},
            }
            (OUTPUT / "node-runtime.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
            success = True
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=5)
        callback.shutdown()
        echo.shutdown()
        callback.server_close()
        echo.server_close()
        if success:
            shutil.rmtree(temp)
        else:
            print(f"probe failed; fixture logs retained at {temp}")


if __name__ == "__main__":
    asyncio.run(main())
