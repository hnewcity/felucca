#!/bin/sh
# Build and start the server in the local OrbStack Docker engine.
set -eu
felucca_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$felucca_root"
export BUILDX_CONFIG="${BUILDX_CONFIG:-$felucca_root/target/orbstack-buildx}"
mkdir -p "$BUILDX_CONFIG"
mkdir -p conf/orbstack/agents
if [ -n "${FELUCCA_LOCAL_AGENT:-}" ]; then
    python3 scripts/stage-agent.py "$FELUCCA_LOCAL_AGENT" --output conf/orbstack/agents
fi
docker --context orbstack info >/dev/null
docker --context orbstack compose -f compose.orbstack.yaml build server

python3 - <<'PY'
import json
import os
import pathlib
import secrets
import subprocess

root = pathlib.Path("conf/orbstack")
config = root / "config"
server = config / "server.toml"
credentials = root / "credentials.json"
root.mkdir(parents=True, exist_ok=True, mode=0o700)
config.mkdir(exist_ok=True, mode=0o755)

if not server.exists():
    if credentials.exists():
        password = json.loads(credentials.read_text())["password"]
    else:
        password = secrets.token_urlsafe(24)
        with credentials.open("x") as output:
            os.chmod(credentials, 0o600)
            json.dump({
                "url": "http://127.0.0.1:8088",
                "account": "root",
                "password": password,
            }, output, indent=2)
            output.write("\n")
    password_hash = subprocess.run(
        ["docker", "--context", "orbstack", "run", "--rm", "-i",
         "felucca-server:local", "--hash-password"],
        input=password + "\n", text=True, capture_output=True, check=True,
    ).stdout.strip()
    # The enclosing host directory is private. Container UID 10001 needs to
    # read the read-only bind mount; the plaintext password is outside it.
    if not (config / "bridge.pem").exists() or not (config / "bridge.key").exists():
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "ec",
            "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-days", "3650",
            "-subj", "/CN=felucca-server",
            "-addext", "basicConstraints=critical,CA:FALSE",
            "-addext", "keyUsage=critical,digitalSignature",
            "-addext", "extendedKeyUsage=serverAuth",
            "-addext", "subjectAltName=DNS:felucca-server,DNS:felucca.orb.local,DNS:localhost,IP:127.0.0.1",
            "-keyout", str(config / "bridge.key"),
            "-out", str(config / "bridge.pem"),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    for name in ("bridge.pem", "bridge.key"):
        (config / name).chmod(0o644)
    server.write_text(f'''[bridge]
bind = "0.0.0.0:14028"
quic_bind = "0.0.0.0:14028"
tls = {{ cert = "/etc/felucca/bridge.pem", key = "/etc/felucca/bridge.key" }}

[admin]
bind = "127.0.0.1:8088"
state_path = "/data/runtime-config.json"

[admin.account.root]
role = "admin"
password_hash = "{password_hash}"

[state]
path = "/data/traffic.json"
save_interval = "5s"

[log]
level = "info"
''')
    server.chmod(0o644)
print(f"Local login details: {credentials.resolve()}")
PY

docker --context orbstack compose -f compose.orbstack.yaml run --rm --no-deps server --config /etc/felucca/server.toml --check
docker --context orbstack compose -f compose.orbstack.yaml up -d --wait --wait-timeout 120 server
printf '%s\n' 'Admin URL: http://127.0.0.1:8088'
