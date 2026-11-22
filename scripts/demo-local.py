#!/usr/bin/env python3
"""Start a loopback-only Felucca demo using Python's standard library."""

import argparse
import contextlib
import datetime
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


ROOT = Path(__file__).resolve().parents[1]


def say(message):
    print(message, flush=True)


def reserve_port(preferred):
    sock = socket.socket()
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("127.0.0.1", preferred))
    except OSError:
        sock.bind(("127.0.0.1", 0))
    return sock


def page(admin_url, backend_port, tunnel_port):
    return """<!doctype html>
<html lang="zh-CN"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Felucca 本机演示</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#f2f5f8;color:#182c40;
font:16px/1.7 system-ui,-apple-system,sans-serif;padding:8vh 24px}
main{max-width:760px;margin:auto;background:white;border:1px solid #dae3ea;
border-radius:24px;padding:clamp(24px,6vw,56px);box-shadow:0 16px 60px #182c400a}
.tag{color:#087c60;font-weight:650}h1{font-size:clamp(28px,5vw,42px);
line-height:1.25;letter-spacing:-1px;margin:18px 0}p{color:#526578}
.route{padding:20px;background:#f4f7fa;border-radius:12px;margin:28px 0}
.actions{display:flex;gap:12px;flex-wrap:wrap}
a,button{font:inherit;border:1px solid #c9d6df;border-radius:10px;
padding:10px 18px;text-decoration:none;color:#182c40;background:white;cursor:pointer}
button{background:#126a53;border-color:#126a53;color:white}
button:disabled{opacity:.5}#result{min-height:3em;white-space:pre-wrap}
.hint{font-size:14px;margin-bottom:0}
</style>
<main>
<div class="tag">FELUCCA · 本机演示</div>
<h1>你的第一条隧道已经跑通。</h1>
<p>这个页面来自本机测试网站，通过 Felucca 转发后送达浏览器。</p>
<div class="route">浏览器 → 服务端 :TUNNEL_PORT → TLS 隧道
→ agent → 测试网站 :BACKEND_PORT</div>
<div class="actions"><button id="test" type="button">再发一次测试请求</button>
<a href="ADMIN_URL" target="_blank" rel="noopener">打开管理页面 ↗</a></div>
<p id="result" role="status">点击按钮，确认请求仍然可以通过隧道。</p>
<p class="hint">管理账号和密码在启动脚本的终端里。
可以在管理页面查看 office 节点与隧道，或停用隧道后再次点击测试。</p>
</main>
<script>
const button = document.querySelector('#test');
const result = document.querySelector('#result');
button.onclick = async () => {
  button.disabled = true;
  result.textContent = '正在通过隧道请求…';
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 5000);
  const start = performance.now();
  try {
    const response = await fetch('/api/ping', {
      cache: 'no-store', signal: controller.signal
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const data = await response.json();
    result.textContent = `转发成功 · ${Math.round(performance.now()-start)} ms\\n后端时间：${data.time}`;
  } catch (error) {
    result.textContent = '请求失败：请检查节点是否在线、隧道是否启用。';
  } finally {
    clearTimeout(timer);
    button.disabled = false;
  }
};
</script></html>
""".replace("ADMIN_URL", admin_url).replace(
        "BACKEND_PORT", str(backend_port)
    ).replace("TUNNEL_PORT", str(tunnel_port))


def main():
    parser = argparse.ArgumentParser(description="一键启动 Felucca 本机演示；Ctrl+C 停止。")
    parser.add_argument("--no-open", action="store_true", help="不自动打开浏览器")
    parser.add_argument("--no-build", action="store_true", help="复用已有的 debug 二进制")
    args = parser.parse_args()
    for command in (["openssl"] if args.no_build else ["openssl", "cargo", "npm"]):
        if not shutil.which(command):
            parser.error(f"找不到 {command}，请先安装后重试。")

    children = []
    reservations = []
    backend = None
    backend_thread = None
    run_dir = None
    # Do not let a previous deployment's CLI environment select demo configuration.
    env = os.environ.copy()
    for name in ("FELUCCA_CONFIG", "FELUCCA_RUNTIME_CONFIG", "FELUCCA_EXPAND_ENV",
                 "FELUCCA_CONFIG_TOKEN_ENV"):
        env.pop(name, None)

    def stop_requested(signum, frame):
        raise KeyboardInterrupt

    def run(command, **kwargs):
        # Track short-lived commands too, so interruption also cleans up a build.
        with tempfile.TemporaryFile() as output:
            process = subprocess.Popen(
                [str(arg) for arg in command], cwd=ROOT, env=env,
                stdin=subprocess.PIPE if "input" in kwargs else subprocess.DEVNULL,
                stdout=output, stderr=subprocess.STDOUT, start_new_session=True,
            )
            children.append(process)
            process.communicate(kwargs.get("input"), timeout=120)
            output.seek(0)
            text = output.read().decode("utf-8", errors="replace").strip()
            if process.returncode:
                raise RuntimeError(text or f"命令失败：{command[0]}")
            return text

    def start(binary, config, log):
        with log.open("wb") as output:
            process = subprocess.Popen(
                [str(binary), "--config", str(config)], cwd=ROOT, env=env,
                stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        children.append(process)
        return process

    def check_processes():
        for process in children:
            if process.poll() not in (None, 0):
                raise RuntimeError(f"子进程异常退出，退出码 {process.returncode}")
        for process in services:
            if process.poll() is not None:
                raise RuntimeError("服务进程已停止。")

    # Explicitly bypass shell/system HTTP proxies for loopback readiness checks.
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def wait_http(url, predicate):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            check_processes()
            try:
                with http.open(url, timeout=1) as response:
                    body = response.read()
                    if response.status == 200 and predicate(body):
                        return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.2)
        raise RuntimeError(f"等待服务就绪超时：{url}")

    services = []
    old_umask = os.umask(0o077)
    signal.signal(signal.SIGTERM, stop_requested)
    try:
        if not args.no_build:
            say("正在构建 React 管理端…")
            run(["npm", "--prefix", str(ROOT / "apps/admin"), "ci"])
            run(["npm", "--prefix", str(ROOT / "apps/admin"), "run", "build"])
            say("正在编译本机演示程序（首次运行可能需要几分钟）…")
            build = subprocess.Popen(
                ["sh", str(ROOT / "scripts/cargo-external.sh"), "build", "--locked",
                 "-p", "felucca-cli", "--bins", "--target-dir", str(ROOT / "target")],
                cwd=ROOT, env=env, start_new_session=True,
            )
            children.append(build)
            if build.wait():
                raise RuntimeError("编译失败，请查看上面的 Cargo 输出。")
        server_bin = ROOT / "target/debug/felucca-server"
        agent_bin = ROOT / "target/debug/felucca-agent"
        if not all(binary.is_file() for binary in (server_bin, agent_bin)):
            raise RuntimeError("未找到 debug 二进制，请去掉 --no-build 后重新运行。")

        demo_root = ROOT / "target/local-demo"
        demo_root.mkdir(parents=True, exist_ok=True)
        run_dir = Path(tempfile.mkdtemp(prefix="run-", dir=demo_root))
        say(f"本次演示目录：{run_dir}")
        reservations = [reserve_port(port) for port in (14028, 8990, 18080)]
        bridge_port, admin_port, tunnel_port = [
            sock.getsockname()[1] for sock in reservations
        ]
        run_id = secrets.token_hex(16)

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/api/ping":
                    content = json.dumps({
                        "run_id": run_id,
                        "time": datetime.datetime.now().astimezone().isoformat(),
                    }).encode()
                    content_type = "application/json"
                elif self.path == "/":
                    content = html.encode()
                    content_type = "text/html; charset=utf-8"
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(content)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                with contextlib.suppress(BrokenPipeError, ConnectionResetError):
                    self.wfile.write(content)

            def log_message(self, format, *args):
                pass

        try:
            backend = ThreadingHTTPServer(("127.0.0.1", 3000), Handler)
        except OSError:
            backend = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        backend_port = backend.server_port
        admin_url = f"http://127.0.0.1:{admin_port}"
        tunnel_url = f"http://127.0.0.1:{tunnel_port}"
        html = page(admin_url, backend_port, tunnel_port)

        say("正在生成证书、节点密钥和管理密码…")
        password = "felucca-" + secrets.token_urlsafe(9)
        password_hash = run([server_bin, "--hash-password"],
                            input=(password + "\n").encode())
        cert, key = run_dir / "bridge.pem", run_dir / "bridge.key"
        run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-days", "30", "-subj", "/CN=felucca-local-demo",
             "-keyout", key, "-out", cert])
        vkey = secrets.token_hex(32)
        server_config = {
            "bridge": {"bind": f"127.0.0.1:{bridge_port}",
                       "tls": {"cert": str(cert), "key": str(key)}},
            "admin": {
                "bind": f"127.0.0.1:{admin_port}",
                "state_path": str(run_dir / "runtime-config.json"),
                "account": {"root": {"role": "admin", "password_hash": password_hash}},
            },
            "state": {"path": str(run_dir / "traffic.json"), "save_interval": "5s"},
            "user": {"office": {"limits": {"max_connections": 128}}},
            "client": {"office": {"user": "office", "vkey": vkey,
                                  "allow_config_push": False}},
            "managed": {"office": {"tunnel": [{
                "id": 1, "mode": "tcp", "port": tunnel_port, "bind": "127.0.0.1",
                "backend": [{"addr": f"127.0.0.1:{backend_port}"}],
            }]}},
        }
        server_path, agent_path = run_dir / "server.json", run_dir / "agent.json"
        server_path.write_text(json.dumps(server_config, indent=2), encoding="utf-8")
        fingerprint = run([server_bin, "--config", server_path, "--fingerprint"])
        agent_config = {
            "server": f"127.0.0.1:{bridge_port}", "key_id": "office", "vkey": vkey,
            "transport": "tls", "trust": {"mode": "pinned", "fingerprint": fingerprint},
            "remote": {"enabled": True, "console_scope": True, "target_policy": {
                "allow": ["127.0.0.0/8", "::1"], "ports": [backend_port],
            }},
        }
        agent_path.write_text(json.dumps(agent_config, indent=2), encoding="utf-8")
        run([server_bin, "--config", server_path, "--check"])
        run([agent_bin, "--config", agent_path, "--check"])
        (run_dir / "login.txt").write_text(
            f"{admin_url}\n账号：root\n密码：{password}\n", encoding="utf-8"
        )

        backend_thread = threading.Thread(target=backend.serve_forever, daemon=True)
        backend_thread.start()
        for sock in reservations:
            sock.close()
        say("正在启动 server、agent，并验证真实转发…")
        services.append(start(server_bin, server_path, run_dir / "server.log"))
        wait_http(admin_url + "/", lambda body: b"<html" in body.lower()
                  or b"<!doctype" in body.lower())
        services.append(start(agent_bin, agent_path, run_dir / "agent.log"))
        wait_http(tunnel_url + "/api/ping", lambda body: run_id.encode() in body)

        say(f"""
✓ 本机隧道已验证成功

  演示页面：{tunnel_url}
  管理页面：{admin_url}
  管理账号：root
  管理密码：{password}

  转发路径：:{tunnel_port} → server → TLS → agent → :{backend_port}
  命令验证：curl --noproxy '*' {tunnel_url}/api/ping
  配置日志：{run_dir}

可在管理页面查看 office 节点、停用或启用隧道，再点击演示页面的测试按钮。
每次运行创建一套新演示配置；本次修改和日志保留在上面的目录。
保持本终端运行，按 Ctrl+C 停止全部演示服务。
""")
        if not args.no_open:
            try:
                webbrowser.open(tunnel_url)
            except webbrowser.Error:
                say("未能自动打开浏览器，请复制上面的演示地址。")
        while True:
            check_processes()
            time.sleep(0.5)
    except KeyboardInterrupt:
        say("\n正在停止本机演示…")
        return 0
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        say(f"\n启动或运行失败：{error}")
        if run_dir:
            for log in sorted(run_dir.glob("*.log")):
                say(f"\n{log.name}（末尾日志）：")
                say("\n".join(log.read_text(errors="replace").splitlines()[-20:]))
        return 1
    finally:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        for process in reversed(children):
            if process.poll() is None:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        for process in reversed(children):
            if process.poll() is None:
                try:
                    process.wait(timeout=max(0, deadline - time.monotonic()))
                except subprocess.TimeoutExpired:
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        if backend_thread:
            backend.shutdown()
            backend_thread.join(timeout=2)
        if backend:
            backend.server_close()
        for sock in reservations:
            sock.close()
        os.umask(old_umask)


if __name__ == "__main__":
    sys.exit(main())
