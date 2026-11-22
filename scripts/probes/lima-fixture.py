#!/usr/bin/env python3
"""Task-owned Lima fixtures and process groups for probe-lima-network.mjs.

Run only in the disposable probe directory, without services or firewall edits.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import socketserver
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def private_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.chmod(0o600)


def process_birth(pid):
    # Start time distinguishes an owned process from a later reuse of its PID.
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]


def start(directory, name, command):
    if not name.replace("-", "").isalnum():
        raise ValueError("invalid process label")
    pidfile = directory / f"{name}.pid.json"
    if pidfile.exists():
        record = json.loads(pidfile.read_text())
        try:
            if process_birth(record["pid"]) == record["birth"]:
                raise RuntimeError("owned process is still running")
        except FileNotFoundError:
            pass
    with (directory / f"{name}.log").open("wb") as log:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log,
                                   stderr=subprocess.STDOUT, cwd=directory,
                                   start_new_session=True)
    record = {"pid": process.pid, "birth": process_birth(process.pid)}
    private_json(pidfile, record)
    print(json.dumps(record))


def stop(directory):
    records = []
    for path in directory.glob("*.pid.json"):
        record = json.loads(path.read_text())
        try:
            if process_birth(record["pid"]) == record["birth"]:
                os.killpg(record["pid"], signal.SIGTERM)
                records.append(record)
        except ProcessLookupError:
            pass
        except FileNotFoundError:
            pass
    time.sleep(1)
    for record in records:
        try:
            if process_birth(record["pid"]) == record["birth"]:
                os.killpg(record["pid"], signal.SIGKILL)
        except (ProcessLookupError, FileNotFoundError):
            pass
    print(json.dumps({"stopped_owned_groups": len(records)}))


def backend(directory):
    marker = "felucca-lima-" + os.urandom(12).hex()
    video = (directory / "sample.mp4").read_bytes()
    range_lock = threading.Lock()

    class Http(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_HEAD(self):
            self.do_GET()

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path == "/sample.mp4":
                body, mime = video, "video/mp4"
            elif path == "/video":
                body = ("<!doctype html><html><meta charset=utf-8><title>Felucca Lima video</title>"
                        "<body style='background:#132235;color:white;font:20px sans-serif'>"
                        "<h1>Felucca: Mac → Lima</h1><p>H.264 / AAC · seek test</p>"
                        "<video id=video controls muted playsinline preload=metadata "
                        "width=960 src='/sample.mp4'></video></body></html>").encode()
                mime = "text/html; charset=utf-8"
            elif path == "/":
                body, mime = marker.encode(), "text/plain"
            else:
                self.send_error(404)
                return
            status, start_byte, end_byte = 200, 0, len(body) - 1
            requested = self.headers.get("Range")
            if requested and path == "/sample.mp4":
                try:
                    first, last = requested.removeprefix("bytes=").split("-")
                    if first:
                        start_byte = int(first)
                        end_byte = min(int(last), end_byte) if last else end_byte
                    else:
                        start_byte = max(0, len(body) - int(last))
                    if not 0 <= start_byte <= end_byte < len(body):
                        raise ValueError("range")
                except ValueError:
                    self.send_error(416)
                    return
                status = 206
                with range_lock, (directory / "ranges.jsonl").open("a") as log:
                    log.write(json.dumps({"range": requested, "start": start_byte,
                                          "end": end_byte}) + "\n")
            self.send_response(status)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(end_byte - start_byte + 1))
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Cache-Control", "no-store")
            if status == 206:
                self.send_header("Content-Range", f"bytes {start_byte}-{end_byte}/{len(body)}")
            self.end_headers()
            if self.command != "HEAD":
                try:
                    self.wfile.write(body[start_byte:end_byte + 1])
                except (BrokenPipeError, ConnectionResetError):
                    pass

        def log_message(self, *args):
            pass

    class Tcp(socketserver.BaseRequestHandler):
        def handle(self):
            while data := self.request.recv(65536):
                self.request.sendall(data)
            self.request.sendall(b"\nHALF-CLOSE-ACK\n")

    class Udp(socketserver.BaseRequestHandler):
        def handle(self):
            data, sock = self.request
            sock.sendto(data, self.client_address)

    http = ThreadingHTTPServer(("127.0.0.1", 0), Http)
    tcp = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Tcp)
    udp = socketserver.UDPServer(("127.0.0.1", 0), Udp)
    udp.max_packet_size = 65535
    http.daemon_threads = tcp.daemon_threads = True
    for server in (http, tcp, udp):
        threading.Thread(target=server.serve_forever, daemon=True).start()
    private_json(directory / "backend.json", {
        "marker": marker, "http": http.server_port, "tcp": tcp.server_address[1],
        "udp": udp.server_address[1], "video_bytes": len(video),
        "video_sha256": hashlib.sha256(video).hexdigest(),
        "platform": platform.platform(),
    })
    signal.pause()


if __name__ == "__main__":
    os.umask(0o077)
    action, folder, *arguments = sys.argv[1:]
    directory = Path(folder).resolve()
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    if action == "start":
        start(directory, arguments[0], arguments[1:])
    elif action == "stop":
        stop(directory)
    elif action == "backend":
        backend(directory)
    else:
        raise ValueError("unknown action")
