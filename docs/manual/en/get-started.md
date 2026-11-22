# Get started

Use your own public Server or ask its operator for a connection command. You do not need to prepare a certificate, edit a configuration file or install an agent first.

## Connect a device with one command

1. In **Devices → Connect device**, enter a name. The default settings allow this device, the LAN and the Internet. To restrict access, open **Advanced settings: access permissions** and choose this device, the LAN or a custom range.
2. Copy the command and run it in Terminal (macOS/Linux) or PowerShell (Windows) on the device. It detects the processor, downloads the matching client and checks its checksum.
3. Keep the window open. When the device is online, select **Add service**, enter its local address and use the published address. Disconnections retry automatically.
4. For private access, create a receiving device in **Private access** and run its command on the other computer. The recipient needs no console login.

The command contains device credentials: share it only with the intended user. Rerun it to reconnect; close an existing connection window first. The server must provide a matching package for the device’s OS. Manual ZIP downloads, folder permissions and advanced settings remain optional.

## Run your own Server

With the release binaries, initialize once in a writable directory:

```sh
./felucca-server --init
```

The command creates the configuration, prints a random root login and starts the server. Open the displayed management address. Keep `felucca-server.toml` and the entire `felucca-server.data/` directory when upgrading or backing up. Restart without `--init`:

```sh
./felucca-server
```

For a public deployment, use `--init --admin-url https://console.example.com` behind an HTTPS reverse proxy forwarding to `127.0.0.1:8990` and preserving Host. With a direct HTTP address, initialization binds that port publicly; use HTTPS for public access. Devices need access to bridge TCP 9001 and visitors need access to the published service port. See [deployment and client distribution](../../management.md).

## Prerequisites

| Requirement | Details |
| --- | --- |
| Rust 1.88 or later | For building; this repository is verified on Rust 1.98.1 |
| Python 3 + OpenSSL | Needed only by the local demo script `scripts/demo-local.py` |
| A host with a public IP | Runs `felucca-server` |
| A host on a private network | Runs `felucca-agent`, with the service you want to expose |
| Permission to bind local TCP/UDP | Needed for testing; firewall rules are managed by whoever deploys |

Felucca installs no system service and changes no firewall rules.

## Option 1 — local demo

If you only want to see it work on your own machine (admin page plus port forwarding), with
no public network:

```sh
python3 scripts/demo-local.py
```

The script builds the debug binaries incrementally, generates a certificate and random
credentials, starts a test website, the server and the agent, verifies real forwarding, and
then opens the demo page by itself. The terminal shows the admin address, the account and
the password. Press `Ctrl+C` to stop all demo services.

Every listener binds to `127.0.0.1` only, and a free port is chosen automatically when a
default port is already taken. Each run keeps its configuration, login details and logs in
its own `target/local-demo/run-*` directory.

| Flag | Effect |
| --- | --- |
| `--no-open` | Do not open the browser automatically |
| `--no-build` | Skip the build when the binaries are already built |

## Option 2 — build

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
```

The result is `target/release/felucca-server` and `target/release/felucca-agent`.
`scripts/cargo-external.sh` places the Cargo cache on the disk where the workspace lives.
When you have plenty of disk space you can also call `cargo` directly.

## Option 3 — first tunnel by hand

### 1. Generate the bridge certificate

You generate the bridge certificate once, and you keep the private key:

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

### 2. Set the shared key

The `vkey` in both files must be the same random string:

```sh
openssl rand -hex 32
```

Write it into `[client.agent-1].vkey` in `conf/server.local.toml` and into `vkey` in
`conf/agent.local.toml`. The agent's `key_id` must match the server's `[client.<id>]` table
entry. Change the agent's `server` to a reachable address of the server host.

### 3. Read the certificate fingerprint

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
```

Put the SHA-256 fingerprint it prints into the agent's `[trust].fingerprint`. Then check
both files:

```sh
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

### 4. Start both programs

Run one command in each of two terminals:

```sh
target/release/felucca-server --config conf/server.local.toml
```

```sh
target/release/felucca-agent --config conf/agent.local.toml
```

### 5. Verify

The example configuration forwards TCP `8080` on the server to `127.0.0.1:3000` on the
agent machine. Start a service on the agent machine that listens on 3000, then open port
8080 on the server.

## Next steps

- [Architecture and concepts](architecture.md) — what bridge, transport and tunnel mean
- [The felucca-server module](server.md) — the client table, HTTP listeners, port authorization
- [The felucca-agent module](agent.md) — reconnects, egress networking, multiple instances
- [Tunnels and port mappings](tunnels.md) — TCP/UDP/HTTP/file/proxy modes
- [Reference](reference.md) — every command and field
