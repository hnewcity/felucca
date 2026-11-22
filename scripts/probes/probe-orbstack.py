#!/usr/bin/env python3
"""Verify the local server deployment, optionally recreating its container."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import tempfile
import time
from datetime import datetime, timezone

ROOT = pathlib.Path(__file__).resolve().parents[2]
DOCKER = ["docker", "--context", "orbstack"]
COMPOSE = DOCKER + ["compose", "-f", str(ROOT / "compose.orbstack.yaml")]


def command(args, **kwargs):
    return subprocess.run(
        args, cwd=ROOT, check=True, text=True, capture_output=True, **kwargs
    ).stdout.strip()


def inspect():
    return json.loads(command(DOCKER + ["inspect", "felucca-server"]))[0]


def fingerprint():
    return command(DOCKER + [
        "exec", "felucca-server", "felucca-server",
        "--config", "/etc/felucca/server.toml", "--fingerprint",
    ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--recreate", action="store_true",
                        help="recreate the server and verify volume persistence")
    parser.add_argument("--capabilities", action="store_true",
                        help="verify the current console capability APIs and local UI bundle")
    args = parser.parse_args()
    credentials = json.loads((ROOT / "conf/orbstack/credentials.json").read_text())
    origin = credentials["url"]
    output = ROOT / "target/tmp/orbstack-deploy"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output) as temporary:
        cookie = str(pathlib.Path(temporary) / "cookies")
        csrf = None

        def request(path, data=None):
            # The management listener is loopback-only on OrbStack's host network.
            argv = [
                "curl", "--noproxy", "*", "--silent", "--show-error",
                "--max-time", "15", "--cookie", cookie, "--cookie-jar", cookie,
                "--write-out", "\n%{http_code}", origin + path,
            ]
            if data is not None:
                argv += ["--header", "Content-Type: application/json",
                         "--header", "Origin: " + origin, "--data-binary", "@-"]
                if csrf is not None:
                    argv += ["--header", "X-Felucca-CSRF: " + csrf]
            result = command(argv, input=json.dumps(data) if data is not None else None)
            body, status = result.rsplit("\n", 1)
            return int(status), body

        def get(path):
            status, body = request(path)
            assert status == 200, f"{path}: HTTP {status}"
            return json.loads(body)

        def login():
            nonlocal csrf
            status, body = request("/api/login", {
                "account": credentials["account"], "password": credentials["password"],
            })
            assert status == 200, f"login: HTTP {status}"
            session = json.loads(body)
            assert session["principal"]["role"] == "admin"
            csrf = session["csrf"]
            assert get("/api/session")["principal"]["account"] == credentials["account"]

        status, html = request("/")
        assert status == 200, f"admin page: HTTP {status}"
        assets = re.findall(r'(?:src|href)="(/assets/[^"]+)"', html)
        assert any(path.endswith(".js") for path in assets)
        assert any(path.endswith(".css") for path in assets)
        for path in assets:
            assert request(path)[0] == 200, f"asset failed: {path}"
        assert request("/api/overview")[0] == 401
        login()
        overview = get("/api/overview")
        assert overview["healthy"], "server reports unhealthy"
        configuration = get("/api/config")
        get("/api/monitor?window=300")
        capabilities = {}
        if args.capabilities:
            launch = get("/api/launch")
            diagnostics = get("/api/diagnostics")
            certificates = get("/api/certificates")
            logs = get("/api/logs")
            status, plan = request("/api/validate", configuration["config"])
            assert status == 200, f"configuration plan: HTTP {status}"
            assert json.loads(plan)["valid"]
            assert json.loads(plan)["restart_fields"] == []
            assert diagnostics["revision"] == configuration["revision"]
            assert any(certificate["name"] == "bridge"
                       for certificate in certificates["diagnostics"]["certificates"])
            assert "configured" in logs
            bundle = next(path for path in assets if path.endswith(".js"))
            local_bundle = ROOT / "apps/admin/dist" / bundle.lstrip("/")
            assert local_bundle.is_file(), "deployed bundle differs from current local build"
            status, javascript = request(bundle)
            assert status == 200
            digest = hashlib.sha256(javascript.encode()).hexdigest()
            assert digest == hashlib.sha256(local_bundle.read_bytes()).hexdigest(), \
                "deployed JavaScript does not match the locally verified bundle"
            assert "/api/diagnostics" in javascript and "/api/config/export" in javascript
            assert launch["fingerprint"] == fingerprint()
            capabilities = {
                "launch_metadata": True,
                "scoped_diagnostics": True,
                "certificate_metadata": True,
                "log_endpoint": True,
                "restart_aware_validation": True,
                "verified_local_frontend_match": True,
                "frontend_sha256": digest,
            }
        before = inspect()
        assert before["Config"]["User"] == "10001:10001"
        assert before["HostConfig"]["ReadonlyRootfs"]
        assert before["HostConfig"]["NetworkMode"] == "host"
        assert before["State"]["Health"]["Status"] == "healthy"
        bridge_fingerprint = fingerprint()
        command([
            "openssl", "s_client", "-brief", "-connect", "127.0.0.1:14028",
            "-servername", "felucca-server", "-verify_return_error",
            "-CAfile", str(ROOT / "conf/orbstack/config/bridge.pem"),
        ], input="", timeout=15)
        volume = next(m["Name"] for m in before["Mounts"] if m["Destination"] == "/data")
        command(DOCKER + ["exec", "felucca-server", "test", "-s", "/data/runtime-config.json"])
        deadline = time.monotonic() + 15
        while True:
            try:
                command(DOCKER + ["exec", "felucca-server", "test", "-s", "/data/traffic.json"])
                break
            except subprocess.CalledProcessError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.5)

        if args.recreate:
            command(COMPOSE + ["up", "-d", "--force-recreate", "--wait",
                               "--wait-timeout", "120", "server"])
            assert request("/api/session")[0] == 401, "old session survived restart"
            login()
            assert get("/api/overview")["healthy"]
            assert get("/api/config") == configuration, "configuration changed on recreation"
            assert fingerprint() == bridge_fingerprint, "bridge certificate changed"
            after = inspect()
            assert before["Id"] != after["Id"], "container was not recreated"
            assert before["Image"] == after["Image"]
            assert any(m.get("Name") == volume for m in after["Mounts"])
        else:
            after = before
        image = json.loads(command(DOCKER + ["image", "inspect", after["Image"]]))[0]
        report = {
            "validated_at": datetime.now(timezone.utc).isoformat(),
            "url": origin,
            "image": after["Config"]["Image"],
            "image_id": after["Image"],
            "platform": image["Os"] + "/" + image["Architecture"],
            "image_size_bytes": image["Size"],
            "container_id": after["Id"],
            "health": after["State"]["Health"]["Status"],
            "runtime_user": after["Config"]["User"],
            "network_mode": after["HostConfig"]["NetworkMode"],
            "data_volume": volume,
            "checks": {
                "loopback_http_access": True,
                "embedded_js_css": assets,
                "unauthenticated_api_rejected": True,
                "admin_login_and_session": True,
                "overview_healthy": True,
                "monitor_api": True,
                "bridge_tls_handshake_verified": True,
                "configuration_snapshot_present": True,
                "traffic_snapshot_present": True,
                "container_recreated": args.recreate,
                "configuration_and_certificate_preserved": args.recreate,
                **capabilities,
            },
        }
        (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
