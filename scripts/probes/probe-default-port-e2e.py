#!/usr/bin/env python3
"""The default bridge port, end to end over real processes.

A server whose configuration omits [bridge] entirely must come up on the
default port; an agent reaching it over TLS with a pinned certificate, and one
launched from a felucca:// URI, must register tunnels and carry real bytes. A
server written with a bare port must agree, and an agent still pointed at the
previous default must never register. Everything binds 127.0.0.1 only.
Artifacts land in target/tmp/default-port-e2e unless FELUCCA_PROBE_OUTPUT says
otherwise.
"""

import json
import os
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER = ROOT / "target" / "debug" / "felucca-server"
AGENT = ROOT / "target" / "debug" / "felucca-agent"
HERE = ROOT / (os.environ.get("FELUCCA_PROBE_OUTPUT")
               or "target/tmp/default-port-e2e")
DEFAULT_PORT = 14028
OLD_DEFAULT_PORT = 8024
BULK = 4 * 1024 * 1024

ANSI = re.compile(r"\x1b\[[0-9;]*m")
FINGERPRINT = re.compile(r"fingerprint=([0-9a-f]{64})")

results = []
processes = []
listeners = []
taken_before = set()

CLEAN_ENV = {k: v for k, v in os.environ.items()
             if not k.startswith("FELUCCA_") and k.upper() not in
             ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")}
CLEAN_ENV["RUST_LOG"] = "info"


def say(message):
    print(message, flush=True)


def check(name, ok, detail=""):
    results.append({"name": name, "ok": bool(ok), "detail": str(detail)[:400]})
    say(("  PASS " if ok else "  FAIL ") + name + (f"  — {detail}" if detail else ""))
    return ok


def spawn(command, log):
    handle = log.open("wb")
    process = subprocess.Popen(
        [str(part) for part in command], cwd=str(ROOT), env=CLEAN_ENV,
        stdin=subprocess.DEVNULL, stdout=handle, stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    handle.close()
    processes.append(process)
    return process


def read_log(path):
    if not path.exists():
        return ""
    return ANSI.sub("", path.read_text(errors="replace"))


def wait_for_log(path, needle, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in read_log(path):
            return True
        time.sleep(0.1)
    return False


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def echo_backend():
    """A TCP echo service standing in for whatever sits behind the agent."""
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(64)
    listeners.append(listener)

    def serve():
        while True:
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            threading.Thread(target=pump, args=(conn,), daemon=True).start()

    def pump(conn):
        with conn:
            while True:
                try:
                    data = conn.recv(64 * 1024)
                except OSError:
                    return
                if not data:
                    return
                try:
                    conn.sendall(data)
                except OSError:
                    return

    threading.Thread(target=serve, daemon=True).start()
    return listener.getsockname()[1]


def read_exact(conn, count):
    out = bytearray()
    while len(out) < count:
        data = conn.recv(count - len(out))
        if not data:
            raise ConnectionError(f"closed after {len(out)} of {count} bytes")
        out += data
    return bytes(out)


def listening_ports(pid):
    """TCP ports this process listens on, from lsof when it is available."""
    try:
        # -a is required: without it lsof ORs the options and reports every
        # listener on the machine.
        done = subprocess.run(["lsof", "-nP", "-a", "-p", str(pid), "-iTCP",
                               "-sTCP:LISTEN"], capture_output=True, text=True,
                              timeout=20)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if done.returncode != 0:
        return None
    ports = set()
    for line in done.stdout.splitlines()[1:]:
        # A row ends with "… *:14028 (LISTEN)", so the address is not the last
        # field.
        for field in line.split():
            host, _, port = field.rpartition(":")
            if host and port.isdigit():
                ports.add(int(port))
    return ports


def server_config(clients, bind=None):
    bridge = "" if bind is None else f'[bridge]\nbind = "{bind}"\n\n'
    return bridge + "".join(
        f'[client.{name}]\nvkey = "{secret}"\nallow_config_push = true\n\n'
        for name, secret in clients.items())


def agent_toml(name, secret, fingerprint, bridge, tunnel_port, backend_port):
    return f"""server = "127.0.0.1:{bridge}"
vkey = "{secret}"
key_id = "{name}"
transport = "tls"
reconnect_min = "100ms"
reconnect_max = "200ms"
[trust]
mode = "pinned"
fingerprint = "{fingerprint}"
[[tunnel]]
mode = "tcp"
public = true
port = {tunnel_port}
bind = "127.0.0.1"
[[tunnel.backend]]
addr = "127.0.0.1:{backend_port}"
"""


def push_through_tunnel(tunnel_port, label):
    """Echo, bulk and concurrent traffic over the forwarded port."""
    with socket.create_connection(("127.0.0.1", tunnel_port), timeout=10) as client:
        client.sendall(b"felucca-default-port")
        echoed = read_exact(client, len(b"felucca-default-port"))
    check(f"{label}: bytes echo back through the tunnel",
          echoed == b"felucca-default-port", echoed[:64])

    payload = bytes((i * 7 + 13) % 256 for i in range(65536)) * (BULK // 65536)
    started = time.monotonic()
    try:
        with socket.create_connection(("127.0.0.1", tunnel_port),
                                      timeout=60) as client:
            client.settimeout(120)
            sender = threading.Thread(target=client.sendall, args=(payload,),
                                      daemon=True)
            sender.start()
            received = read_exact(client, len(payload))
            sender.join(120)
        ok = received == payload
        detail = f"{len(payload)} bytes in {time.monotonic() - started:.2f}s"
    except (OSError, ConnectionError) as error:
        ok, detail = False, repr(error)
    check(f"{label}: {BULK // 1024 // 1024} MiB bulk transfer round-trips", ok, detail)

    failures = []

    def one(index):
        try:
            with socket.create_connection(("127.0.0.1", tunnel_port),
                                          timeout=20) as client:
                client.settimeout(30)
                tag = index.to_bytes(4, "big")
                client.sendall(tag)
                if read_exact(client, 4) != tag:
                    failures.append(f"connection {index} echoed the wrong bytes")
        except OSError as error:
            failures.append(f"connection {index}: {error!r}")

    threads = [threading.Thread(target=one, args=(i,)) for i in range(8)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(60)
    check(f"{label}: 8 concurrent connections each get their own reply",
          not failures, "; ".join(failures[:3]))


def stop(process):
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
    return process.returncode


def report():
    passed = sum(1 for item in results if item["ok"])
    say(f"\n==== {passed}/{len(results)} checks passed ====")
    for item in results:
        if not item["ok"]:
            say(f"FAIL {item['name']}: {item['detail']}")
    (HERE / "default-port-e2e.json").write_text(json.dumps({
        "probe": "scripts/probes/probe-default-port-e2e.py",
        "default_port": DEFAULT_PORT,
        "previous_default_port": OLD_DEFAULT_PORT,
        "total": len(results),
        "passed": passed,
        "checks": results,
    }, indent=2), encoding="utf-8")
    say(f"artifacts: {HERE}")
    return 0 if passed == len(results) else 1


def main():
    if shutil.which("lsof") is None:
        say("note: lsof is unavailable; listener ownership checks are skipped")
    # Only the default port has to be free: something unrelated may already
    # hold the old one, which says nothing about this server.
    for port in (DEFAULT_PORT, OLD_DEFAULT_PORT):
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                taken_before.add(port)
    if DEFAULT_PORT in taken_before:
        say(f"refusing to run: something else already listens on {DEFAULT_PORT}")
        return 1
    if OLD_DEFAULT_PORT in taken_before:
        say(f"note: {OLD_DEFAULT_PORT} is held by another process, so the "
            "old-port checks describe this server's own listeners only")
    if not SERVER.is_file() or not AGENT.is_file():
        say(f"missing binaries; build them first ({SERVER}, {AGENT})")
        return 1

    shutil.rmtree(HERE, ignore_errors=True)
    HERE.mkdir(parents=True, exist_ok=True)
    backend = echo_backend()

    # ------------------------------------------------------------------ #
    # Phase 1: a bare port means "every interface", and lands on the default
    # ------------------------------------------------------------------ #
    secret = secrets.token_hex(16)
    config = HERE / "bare-port-server.toml"
    config.write_text(server_config({"a-bare": secret}, bind=str(DEFAULT_PORT)),
                      encoding="utf-8")
    log = HERE / "bare-port-server.log"
    server = spawn([SERVER, "--config", config], log)
    ready = wait_for_log(log, "felucca-server ready", 30)
    with socket.socket() as probe:
        reachable = probe.connect_ex(("127.0.0.1", DEFAULT_PORT)) == 0
    check("bare port bind comes up on the default port",
          ready and reachable, read_log(log)[-300:] if not ready else "")
    ports = listening_ports(server.pid)
    if ports is not None:
        check(f"bare port server listens on {DEFAULT_PORT} and nothing else",
              ports == {DEFAULT_PORT}, sorted(ports))
    stop(server)

    # ------------------------------------------------------------------ #
    # Phase 2: no [bridge] table at all — the documented default
    # ------------------------------------------------------------------ #
    uri_secret = secrets.token_hex(16)
    config = HERE / "server.toml"
    config.write_text(server_config({"a-toml": secret, "a-uri": uri_secret}),
                      encoding="utf-8")
    log = HERE / "server.log"
    server = spawn([SERVER, "--config", config], log)
    ready = wait_for_log(log, "felucca-server ready", 30)
    with socket.socket() as probe:
        reachable = probe.connect_ex(("127.0.0.1", DEFAULT_PORT)) == 0
    check(f"a server with no [bridge] table listens on {DEFAULT_PORT}",
          ready and reachable, read_log(log)[-300:] if not ready else "")
    if not ready:
        return report()

    # The lsof check above already pins the listener to the default port; this
    # one only says something when the old port was free to begin with.
    if OLD_DEFAULT_PORT not in taken_before:
        with socket.socket() as probe:
            old_default_free = probe.connect_ex(
                ("127.0.0.1", OLD_DEFAULT_PORT)) != 0
        check(f"the default is no longer the old port {OLD_DEFAULT_PORT}",
              old_default_free, f"{OLD_DEFAULT_PORT} answered a connection")

    ports = listening_ports(server.pid)
    if ports is not None:
        check(f"default server listens on {DEFAULT_PORT} and nothing else",
              ports == {DEFAULT_PORT}, sorted(ports))

    match = FINGERPRINT.search(read_log(log))
    if not check("the server logged its generated bridge fingerprint",
                 bool(match), "" if match else read_log(log)[-300:]):
        return report()
    fingerprint = match.group(1)

    # ------------------------------------------------------------------ #
    # Phase 3: an agent from a TOML config, pinned certificate, real traffic
    # ------------------------------------------------------------------ #
    tunnel_port = free_port()
    agent_config = HERE / "agent.toml"
    agent_config.write_text(agent_toml("a-toml", secret, fingerprint,
                                       DEFAULT_PORT, tunnel_port, backend),
                            encoding="utf-8")
    agent_log = HERE / "agent.log"
    agent = spawn([AGENT, "--config", agent_config], agent_log)

    registered = wait_for_log(agent_log, "tunnels registered", 45)
    check("the agent reached the default port and registered its tunnel",
          registered, "" if registered else read_log(agent_log)[-400:])
    seen = wait_for_log(log, "key_id=a-toml", 10)
    check("the server saw the agent connect", seen,
          "" if seen else read_log(log)[-300:])

    try:
        push_through_tunnel(tunnel_port, "toml agent")
    except (OSError, ConnectionError) as error:
        check("toml agent: traffic through the tunnel", False, repr(error))

    code = stop(agent)
    check("the agent shut down cleanly on SIGTERM", code in (0, -15),
          f"exit code {code}")
    time.sleep(1)
    with socket.socket() as probe:
        gone = probe.connect_ex(("127.0.0.1", tunnel_port)) != 0
    check("the forwarded port is released after the agent exits", gone,
          "" if gone else f"port {tunnel_port} still answers")

    # ------------------------------------------------------------------ #
    # Phase 4: an agent launched from a felucca:// URI at the default port
    # ------------------------------------------------------------------ #
    uri = (f"felucca://127.0.0.1:{DEFAULT_PORT}"
           f"?key_id=a-uri&vkey={uri_secret}&transport=tls&fingerprint={fingerprint}")
    uri_log = HERE / "uri-agent.log"
    uri_agent = spawn([AGENT, "--config", uri], uri_log)
    connected = wait_for_log(log, "key_id=a-uri", 30)
    time.sleep(3)
    alive = uri_agent.poll() is None
    check("a felucca:// launch URI reaches the default port and authenticates",
          connected and alive,
          "" if connected and alive else read_log(uri_log)[-400:])
    stop(uri_agent)

    # ------------------------------------------------------------------ #
    # Phase 5: an agent still pointed at the old port must not register
    # ------------------------------------------------------------------ #
    stale_port = free_port()
    stale_config = HERE / "stale-agent.toml"
    stale_config.write_text(agent_toml("a-toml", secret, fingerprint,
                                       OLD_DEFAULT_PORT, stale_port, backend),
                            encoding="utf-8")
    stale_log = HERE / "stale-agent.log"
    stale = spawn([AGENT, "--config", stale_config], stale_log)
    time.sleep(8)
    registered = "tunnels registered" in read_log(stale_log)
    still_retrying = "tunnel failed" in read_log(stale_log)
    check(f"an agent pointed at {OLD_DEFAULT_PORT} never registers",
          stale.poll() is None and not registered,
          (f"kept retrying: {still_retrying}" if not registered
           else "it registered a tunnel"))
    stop(stale)

    code = stop(server)
    check("the server shut down cleanly on SIGTERM", code in (0, -15),
          f"exit code {code}")

    for listener in listeners:
        listener.close()
    return report()


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    pass
