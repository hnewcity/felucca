#!/usr/bin/env python3
"""Resume an existing loopback demo after rebuilding, preserving its credentials/state."""
import argparse
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", type=Path)
    args = parser.parse_args()
    run_dir = args.run_dir.resolve()
    config = json.loads((run_dir / "server.json").read_text())
    agent = json.loads((run_dir / "agent.json").read_text())
    admin_bind = config["admin"]["bind"]
    if not admin_bind.startswith("127.0.0.1:"):
        parser.error("Only loopback demo configurations are supported")
    backend_port = agent["remote"]["target_policy"]["ports"][0]
    tunnel_port = next(iter(config["managed"].values()))["tunnel"][0]["port"]
    origin = f"http://{admin_bind}"
    spec = importlib.util.spec_from_file_location("demo", ROOT / "scripts/demo-local.py")
    demo = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(demo)
    html = demo.page(origin, backend_port, tunnel_port).encode()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/api/ping":
                body = json.dumps({"time": time.strftime("%Y-%m-%dT%H:%M:%S%z")}).encode()
                content_type = "application/json"
            elif self.path == "/":
                body, content_type = html, "text/html; charset=utf-8"
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            with contextlib.suppress(BrokenPipeError, ConnectionResetError):
                self.wfile.write(body)

        def log_message(self, *_args):
            pass

    def interrupted(*_args):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    children = []
    backend = ThreadingHTTPServer(("127.0.0.1", backend_port), Handler)
    thread = threading.Thread(target=backend.serve_forever, daemon=True)
    thread.start()
    env = {k: v for k, v in os.environ.items() if not k.startswith("FELUCCA_")}
    try:
        for name in ("server", "agent"):
            with (run_dir / f"{name}.log").open("ab") as log:
                children.append(subprocess.Popen(
                    [str(ROOT / f"target/debug/felucca-{name}"), "--config", str(run_dir / f"{name}.json")],
                    cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                ))
        http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        deadline = time.monotonic() + 30
        while True:
            if any(child.poll() is not None for child in children):
                raise RuntimeError("Demo process exited; inspect logs in the existing run directory")
            try:
                with http.open(origin, timeout=1) as response:
                    assert response.status == 200
                with http.open(f"http://127.0.0.1:{tunnel_port}/api/ping", timeout=1) as response:
                    assert "time" in json.load(response)
                break
            except OSError:
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.2)
        print(f"Preview ready: {origin}/#monitor; real tunnel verified; existing credentials and configuration preserved.", flush=True)
        while all(child.poll() is None for child in children):
            time.sleep(0.5)
    except KeyboardInterrupt:
        pass
    finally:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
        for child in reversed(children):
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        backend.shutdown()
        backend.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
