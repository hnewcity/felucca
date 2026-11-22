# The felucca-server module

This page covers deploying and configuring `felucca-server`: bridge listeners, the client
table, HTTP ingress, port authorization, the admin listener, server-local services and
external platform switches.

## A minimal configuration

```toml
[bridge]
bind = "0.0.0.0:14028"
ws_path = "/ws"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }

[client.agent-1]
vkey = "change-me-to-a-long-random-string"

[log]
level = "info"
```

When `bridge.tls` is omitted and runtime configuration storage is enabled (including `--init` or the admin console), the server saves its identity beside the runtime snapshot as `<snapshot>.identity.json`. Keep this file across restarts and backups. Without runtime storage the identity is temporary. `--fingerprint` reads explicit PEM files; automatic fingerprints appear in the running console and startup log.

```sh
target/release/felucca-server --config conf/server.toml --check
target/release/felucca-server --config conf/server.toml
```

## Bridge listeners

| Field | Default | Notes |
| --- | --- | --- |
| `bridge.bind` | `0.0.0.0:14028` | TCP, TLS, WS and WSS share one socket and are separated by handshake characteristics |
| `bridge.quic_bind` | none | Independent UDP address; may use the same port number as TCP |
| `bridge.kcp_bind` | none | KCP UDP listener; cannot share a UDP port with QUIC/HTTP3 |
| `bridge.ws_path` | `/ws` | WS/WSS path. Both ends must match; upgrades on other paths are refused |
| `bridge.tls` | none | `{ cert, key }`. A temporary certificate is generated when omitted |
| `bridge.cert_reload_interval` | `30s` | PEM reload interval. Reloading does not interrupt existing connections |
| `bridge.proxy_accept` | off | Accepts PROXY headers from trusted load balancers |
| `[bridge.mux]` | see below | mux windows and budgets |
| `[bridge.kcp]` | see below | KCP parameters |

`[bridge.mux]` exposes `initial_stream_window`, `max_stream_window`, `max_session_window`,
`max_send_queue`, `max_control_queue`, `max_streams`, `accept_backlog` and `open_timeout`.
The agent side has a matching `[mux]` section with the same fields. Native QUIC streams do
not use these settings.

`[bridge.kcp]` accepts `mtu`, `send_window`, `receive_window`, `interval_ms`, `nodelay`,
`fast_resend`, `congestion_control`, `session_timeout`, `data_shards`, `parity_shards` and
`max_sessions`. The default FEC setting is 10 data plus 3 parity shards, so any 3 of the 13
packets in a group can be lost and still recovered. Both ends must use the same MTU and
shard counts.

## The client table

Each agent has one `[client.<key_id>]` table. The `key_id` is the agent's name on the
server, and the agent's own `key_id` must match the table name.

| Field | Notes |
| --- | --- |
| `vkey` | Required. The shared secret used for mutual HMAC authentication. You are responsible for its length and randomness |
| `remark` | A note, shown on the admin page |
| `user` | Links a `[user.<name>]` table, used for layered quotas, ACLs and tenant permissions |
| `enabled` | `false` persistently disables and disconnects this client |
| `allow_config_push` | Whether this agent may register tunnels. Set `true` to push `[[managed.<id>.tunnel]]` configuration |
| `max_tunnels` | The tunnel limit for this agent |
| `max_connections` | The connection limit for this agent |
| `rate_limit` | A bytes-per-second limit shared by all tunnels and both directions |
| `acl_mode` / `acl_rules` | Source ACL for access to this client's public entry |
| `quota` | `{ flow_limit, expires_at }`, in bytes and Unix seconds |

A new connection with the same `key_id` replaces the old one; the old listeners stop and
release before the ports bind again. Traffic counters and bandwidth limiters survive
reconnects within the same process.

## Port authorization

`allowed_ports` is a top-level field and **must appear before the first TOML table header**:

```toml
allowed_ports = [8080, { from = 9000, to = 9100 }]
```

An empty list allows all TCP/UDP ports above 1024. `max_tunnels` and `max_connections` are
top-level global limits counted per agent.

## HTTP ingress

```toml
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"      # set to "" to disable HTTPS
http3_bind = "0.0.0.0:8443"    # optional, HTTP/3 UDP ingress
forwarded_headers = true
response_timeout = "100s"
cert_reload_interval = "30s"

[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"

[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

For full usage see [HTTP/HTTPS reverse proxy](http-services.md) and
[Certificates, SNI and ACME](certificates.md).

## Other top-level tables

| Table | Purpose | See also |
| --- | --- | --- |
| `[admin]` / `[admin.account.<name>]` | Admin listener, accounts and roles | [Admin UI and centralized configuration](management.md) |
| `[state]` | Traffic snapshot persistence | [Access control, quotas and stream coding](security.md) |
| `[user.<name>]` / `[user.<name>.limits]` | Per-user quotas and aggregation | [Access control, quotas and stream coding](security.md) |
| `[local.<name>]` | Server-local service groups | [Server-local targets and nested routes](local-services.md) |
| `[share.<name>]` | Private share authorization | [Private shares and visitors](private-shares.md) |
| `[managed.<client>]` | Centralized configuration push | [Admin UI and centralized configuration](management.md) |
| `[node]` / `[node.platform.<name>]` / `[node.webhook.<name>]` | External platforms | [External platform integration](platform.md) |
| `[p2p]` | P2P observation port | [P2P direct and relay fallback](p2p.md) |
| `[geo]` | GeoIP/GeoSite data files | [Access control, quotas and stream coding](security.md) |
| `[ntp]` | Calibrate bridge authentication time | [The felucca-agent module](agent.md) |
| `[log]` / `[log.rotation]` | Logging and rotation | [Services, updates, logging and monitoring](operations.md) |
| `allowed_ports` | Port authorization | This page |

## Useful commands

| Command | Purpose |
| --- | --- |
| `--check` | Validates the configuration and exits without binding ports |
| `--fingerprint` | Prints the SHA-256 of the `bridge.tls` certificate. Requires a fixed PEM to be configured |
| `--hash-password` | Reads one password line from stdin and prints an admin password hash. The password must be at least 12 bytes |
| `--generate-token` | Prints an admin Bearer token and its storage hash |
| `--runtime-config PATH` | Enables the runtime configuration snapshot |
| `--build-info` | Prints the program name, version, protocol and build target |

## Lifecycle

- `SIGINT` / `SIGTERM` close the listeners and the current connections, then exit.
- Changes to startup files require a process restart. Changes made through the admin
  listener go through a runtime configuration snapshot transaction, which interrupts
  forwarded connections, but the agent reconnects afterwards.
- Changing the admin binding, certificate file paths, public origin or state path
  **requires a process restart**.
- Traffic counters survive agent reconnects. They survive a server restart only when
  `[state]` is configured.

## Next steps

- [The felucca-agent module](agent.md) — bring internal services up
- [Tunnels and port mappings](tunnels.md)
- [HTTP/HTTPS reverse proxy](http-services.md)
- [Services, updates, logging and monitoring](operations.md) — systemd / launchd / Windows service
