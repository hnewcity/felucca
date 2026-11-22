# Reference

Commands, fields, endpoints, an example index and a glossary. The guides explain *why*;
this page lists *what*.

## CLI

Arguments shared by both programs:

| Argument | Environment variable | Description |
| --- | --- | --- |
| `--build-info` | — | Prints the program name, version, protocol and build target |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | TOML/JSON file, HTTP(S) URL, inline JSON, base64 or a `felucca://` URI; repeatable for the agent |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | Expands `${VAR}` / `${VAR:-default}` |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | The variable that holds the Bearer token for remote configuration |
| `--check` | — | Validates and exits without binding or connecting |
| `service` subcommand | — | System service management |
| `update` subcommand | — | Verifies and installs a standalone binary |

`felucca-server` adds:

| Argument | Environment variable | Description |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | Path of the runtime configuration snapshot |
| `--fingerprint` | — | Prints the SHA-256 of the `bridge.tls` certificate |
| `--hash-password` | — | Reads one password line from stdin and prints a PBKDF2 hash; at least 12 bytes |
| `--generate-token` | — | Prints a Bearer token and its `token_hash` |

`felucca-agent` adds:

| Argument | Default | Description |
| --- | --- | --- |
| `--launch` | — | Alias for `--config` |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | Appends and also writes to stderr; the parent directory must exist |
| `--log-json` | — | Structured JSON events |
| `--log-max-bytes <N>` | 10 MiB | Requires `--log-file` |
| `--log-backups <N>` | 5 | Requires `--log-file` |
| `--log-rotate-interval <D>` | — | For example `24h`; requires `--log-file` |

`service` subcommand:

| Argument | Description |
| --- | --- |
| `--name <NAME>` | Service instance name |
| `--scope <user\|system>` | Defaults to `user`; `system` only on Windows |
| `--dry-run` | Only prints the actions that would run |
| `render` | Generates the service definition; `--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | Lifecycle |
| `--workdir <DIR>` | Defaults to the directory that holds the configuration |
| `--binary <PATH>` | Selects the program to install |
| `-- <ARGS>` | Extra daemon arguments |

`update` subcommand:

| Argument | Description |
| --- | --- |
| `--source <PATH_OR_URL>` | A standalone binary, or a tar.gz/zip archive |
| `--sha256 <HEX>` | 64 hex digits; the basis of trust |
| `--check-only` | Verifies without replacing |
| `--expect-version <V>` | Optional version assertion |
| `--token-env <NAME>` | The variable that holds the Bearer token for a private download |

## Configuration layout

| File | Top-level contents |
| --- | --- |
| server | `allowed_ports`, `max_tunnels`, `max_connections`, `[bridge]`, `[http]`, `[admin]`, `[state]`, `[geo]`, `[ntp]`, `[log]`, `[p2p]`, `[node]`, `[user.*]`, `[client.*]`, `[local.*]`, `[share.*]`, `[managed.*]` |
| agent | `server`, `key_id`, `vkey`, `transport`, `reconnect_min/max`, `dial_timeout`, `max_connections`, `[trust]`, `[network]`, `[mux]`, `[kcp]`, `[ntp]`, `[remote]`, `[geo]`, `[[tunnel]]`, `[[mapping]]`, `[[visitor]]`, `[p2p]`, `[log]` (the agent uses command-line arguments) |

`allowed_ports` must appear before the first TOML table header. All tables enable
`deny_unknown_fields`.

## Tunnel field quick reference

| Field | Applies to modes | Description |
| --- | --- | --- |
| `id` | all | Stable ID; fix it once traffic snapshots are enabled |
| `mode` | all | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | all except private | The server port; `http` must match an HTTP listener |
| `bind` | tcp/udp/proxy/file | IP address only |
| `public` | all | `false` means a private share |
| `hosts` | http, file | Domain list; `*.example.com` is supported |
| `balance` | fixed backends | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | fixed backends | `addr`, `weight`, `tls`, `trust` |
| `health` | fixed backends | `type`, `path`, `interval`, `timeout`, `failures_to_evict`, `successes_to_restore`, `expect_status` |
| `acl_mode`/`acl_rules` | all | Source ACL: off/allow/deny |
| `target_policy` | fixed backends and forward proxies | `allow`/`deny`/`ports` |
| `options.auth.users` | forward proxies, HTTP | Account table |
| `options.http.*` | http | Path, rewriting, headers, redirects, CORS, error pages, tls_mode, backend_http2 |
| `options.payload.*` | all | `compress`, `encrypt` |
| `options.limits.*` | all | `max_connections`, `rate_limit`, `quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | proxy entries | Accepts PROXY headers |
| `file` | file | `root`, `prefix`, `read_only`, `auto_index`, `auth` |

## Admin API summary

Authentication is by cookie with Origin and CSRF, or by `Authorization: Bearer <token>`. Writes
require `If-Match: "<revision>"` and a unique `Idempotency-Key`. The request body limit is 2 MiB.

| Group | Paths |
| --- | --- |
| Session | `/api/login`, `/api/session`, `/api/logout` |
| Resources | `/api/users`, `/api/clients`, `/api/locals`, `/api/shares`, `/api/accounts`, each with its `<id>` |
| Centralized configuration | `/api/managed/<client>`, `/api/managed/<client>/tunnels/<id>` |
| Server-local tunnels | `/api/locals/<group>/tunnels/<id>` |
| Whole configuration | `/api/config`, `/api/config?secrets=1`, `/api/validate`, `/api/batch` |
| Realtime | `/api/events` (SSE), `/metrics`, `/api/metrics`, `/api/profile` |
| Platform | `/api/node/*`, `/api/platforms/<name>`, `/api/webhooks/<name>`, `/api/ws`, `/api/realtime/subscriptions` |

See [Admin UI and centralized configuration](management.md) and
[External platform integration](platform.md) for the full fields.

## Example configuration index

| File | Contents |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | Minimal server example |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | Minimal agent example |
| [features-server.toml](../../../examples/features-server.toml) | QUIC bridge + HTTP/HTTPS + certificates |
| [features-agent.toml](../../../examples/features-agent.toml) | All proxy modes, load balancing, health checks |
| [shared-server.toml](../../../examples/shared-server.toml) | Shared TCP/UDP ports + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | Agent side and bulk port mapping |
| [acme-server.toml](../../../examples/acme-server.toml) | ACME server |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL and TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | Layered ACLs, quotas, persistence |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | File services |
| [network-agent.toml](../../../examples/network-agent.toml) | Upstream proxy, custom DNS, NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy and independent stream encryption |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | KCP bridge |
| [local-server.toml](../../../examples/local-server.toml) | Server-local targets and nested forwarding |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | Private provider and visitor |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P and fallback |
| [admin-server.toml](../../../examples/admin-server.toml) | Management server |
| [managed-agent.toml](../../../examples/managed-agent.toml) | Centrally managed agent |
| [node-server.toml](../../../examples/node-server.toml) | External platform and node management |

Configuration paths are relative to the process working directory. All 26 examples pass
configuration validation.

## Numeric limits

| Item | Limit |
| --- | --- |
| Single configuration | 4 MiB |
| Runtime configuration snapshot | 16 MiB |
| Agent instances | 32 |
| Nested routing hops | 16 |
| Batch operations | 128 items |
| Traffic import | 128 items per request |
| Idempotency records kept | 256 (256 each for callback actions and traffic import) |
| Callback queue | `queue_max` per target, 4096 node-wide |
| Direct admin WebSockets | 16 |
| WS RPC concurrency | 2 per connection / 16 node-wide |
| Realtime subscriptions | 32 items per connection, 32 KiB per item |
| HTTP backend connection pool | 8 idle per origin, 30 seconds idle, 256 entries in the outer map |
| HTTP/3 | 128 streams per connection, 1 MiB per stream / 16 MiB connection window, 32 KiB headers |
| UDP | 65507 bytes per source; 256 associations per tunnel |
| SOCKS5 UDP | 64 destinations per session, 8 packets queued for each |
| Error pages | 400..599, 64 KiB per page |
| HTTP domain matching | Exact domains take priority over wildcards |

## Environment and build

| Item | Value |
| --- | --- |
| Minimum Rust | 1.88 |
| Verified environment | macOS Intel, Rust/Cargo 1.98.1 |
| Workspace tests | 581 passed / 0 failed / 0 ignored (2026-09-17) |
| Binaries | `target/release/felucca-server`, `target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

Run only one Cargo command at a time against the same target directory. Finish builds, tests and
rustdoc before you edit source code and embedded pages. Tests need permission to listen on local
TCP/UDP ports.

## Glossary

| Term | Meaning |
| --- | --- |
| agent | The `felucca-agent` process that runs inside a private network and dials out |
| bridge | The authenticated long-lived connection between the server and the agent |
| client / `[client.<id>]` | The server's identity table for one agent; `key_id` is the table name |
| key_id | The agent's name on the server |
| vkey | The bridge secret shared by the agent and the server |
| transport | The bridge transport: tcp/tls/ws/wss/quic/kcp |
| trust | How the agent verifies the server certificate: webpki/pinned/insecure |
| tunnel | A mapping from a public entry point to a set of backends |
| backend | The target the agent actually connects to |
| mapping | Bulk one-to-one port mapping |
| local group | A group of the server's own services that needs no agent |
| share | The server authorizes a guest to use one tunnel |
| visitor | An agent that holds a share secret and listens locally |
| provider | The party that provides a service in a private share or a P2P setup |
| managed | Configuration the server pushes to an agent |
| mux | Bounded multiplexing over TCP-like transports |
| FEC | KCP forward error correction, 10 data + 3 parity by default |
| quota / rate_limit | Traffic quotas and byte-per-second rate limits |
| snapshot | One atomic persistence of traffic or runtime configuration |

## Related documentation

- [Architecture](../../architecture.md) — crate ownership boundaries
- [Configuration and deployment](../../configuration.md) — field-level detail
- [P2P](../../p2p.md), [Private tunnels and shared access](../../visitors.md),
  [Management and centralized configuration](../../management.md),
  [Nodes and external platforms](../../node.md), [Operations and distribution](../../operations.md),
  [Transparent TCP](../../transparent.md)
