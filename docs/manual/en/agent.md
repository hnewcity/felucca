# The felucca-agent module

This page covers deploying and configuring `felucca-agent`: connecting to the server,
certificate trust, reconnects, egress networking, upstream proxies, logging and multiple
instances.

## A minimal configuration

```toml
server = "bridge.example.com:14028"
key_id = "agent-1"
vkey = "change-me-to-a-long-random-string"
transport = "tls"          # tcp | tls | ws | wss | quic | kcp

[trust]
mode = "pinned"
fingerprint = "0000...0000"   # from the server's --fingerprint

[[tunnel]]
mode = "tcp"
port = 8080
[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`server` takes a hostname or IP address with a port. `key_id` must match a
`[client.<key_id>]` table name on the server.

## Transport and trust

`transport` accepts `tcp`, `tls`, `ws`, `wss`, `quic` and `kcp`. For public bridges prefer
TLS/WSS/QUIC: `tcp` and `ws` authenticate the peer but do not encrypt the traffic, so they
are plaintext transports you choose deliberately.

`[trust]` controls how the server certificate is verified:

| `mode` | Behaviour |
| --- | --- |
| `webpki` | Normal verification against public CAs. Do not set `fingerprint` |
| `pinned` | Pins the SHA-256 of the whole certificate. The handshake signature is verified, but the domain name and validity period are not. Update the fingerprint whenever the certificate changes |
| `insecure` | For temporary diagnostics only |

After you configure a fixed PEM in `bridge.tls`, get the fingerprint with
`felucca-server --fingerprint`.

## Connection and reconnection

| Field | Default | Notes |
| --- | --- | --- |
| `reconnect_min` | — | Lower bound of the backoff |
| `reconnect_max` | — | Upper bound of the backoff |
| `dial_timeout` | — | Timeout for a single dial |
| `max_connections` | — | The connection limit for this agent |

The agent reconnects automatically after a disconnect; the server re-registers the
listeners and session state. Traffic counters survive reconnects.

## Egress networking

```toml
[network]
local_ip = "192.0.2.10"
forward_local_ip = true
dns = ["192.0.2.53:53"]

[network.proxy]
kind = "socks5"           # http (CONNECT) | socks5
server = "192.0.2.20:1080"
remote_dns = true
[network.proxy.auth]
username = "edge"
password = "replace-with-proxy-password"
```

| Field | Notes |
| --- | --- |
| `local_ip` | Binds the bridge egress IP. It must be an address actually owned by this machine; interface names are not accepted |
| `forward_local_ip` | Backend connections and health probes also use this address. By default only the bridge binds it |
| `dns` | A literal list of `IP:port`. When empty, the system resolver is used. Responses are cached by TTL, with A/AAAA support and a TCP fallback after UDP truncation |
| `network.proxy` | Upstream proxy. It carries bridge TCP/TLS/WS/WSS only and does not forward backend requests or NTP UDP |

Target ACLs are checked against the **IP address actually returned by resolution**, and the
agent then connects to those checked addresses, so no second resolution can introduce a
bypass. `local_ip` and the DNS addresses must be in the same address family. A QUIC/KCP
bridge cannot also use a TCP upstream proxy.

## Time synchronization

Both the server and the agent accept an `[ntp]` section to calibrate bridge
authentication time:

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

By default no time service is queried. `interval` ranges from 1 second to 24 hours.
`timeout` must be positive, no more than 30 seconds, and smaller than `interval`.
`max_offset` must be positive and no more than 7 days. If the first query fails, the system
time is used. Calibration does not change the system clock, so TLS certificate validity and
quota expiry still follow system time. You must trust the NTP source: plain SNTP does not
authenticate the time source.

The agent's NTP queries use its own DNS and egress configuration; the server uses system
DNS and the default egress.

## Receiving centralized configuration

```toml
[remote]
enabled = true

[remote.target_policy]
allow = ["10.0.0.0/8"]
ports = [80, 443]

[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

`remote.enabled` must be set explicitly. The main connection must use TLS/WSS/QUIC with a
verified certificate; a plaintext connection cannot request configuration.
`remote.target_policy` is a hard boundary on the agent's own machine and is intersected
with each pushed tunnel. `remote.file_root` declares named directory capabilities in
advance, and the server can only push paths within those capability names. A read-only
directory cannot be upgraded to writable through remote configuration. See
[Admin UI and centralized configuration](management.md).

With `remote.console_scope = true`, a `target_policy` in the server's `[managed.<client-id>]` replaces the local `remote.target_policy`, so the range can be edited from the device editor in the console; without one the local range still applies. Without the opt-in, a managed configuration carrying `target_policy` is refused. Directory capabilities always stay local. Connection packages generated by the console enable this option.

## Logging

```sh
felucca-agent -c agent.toml --log-level debug --log-file /data/agent.log --log-json
```

Setting `--log-file` enables rotation with 10 MiB files and 5 archived copies by default.
Log files are appended to and stderr still receives output. The parent directory must
already exist. `--log-max-bytes`, `--log-backups` and `--log-rotate-interval` require
`--log-file`. Raw secrets never reach the log.

## Multiple instances

```sh
felucca-agent --config office.toml --config home.json --check
felucca-agent --config office.toml --config home.json
```

A single process supports up to 32 instances. See [Configuration sources and CLI](configuration.md).

## P2P and visitor switches

| Table | Effect |
| --- | --- |
| `[p2p] enabled = true` | Lets this agent accept direct connections as a provider |
| `[[visitor]]` | Listens on your own machine to reach someone else's private share |

See [P2P direct and relay fallback](p2p.md) and
[Private shares and visitors](private-shares.md).

## Next steps

- [Tunnels and port mappings](tunnels.md)
- [Backends, balancing and health checks](backends.md)
- [Services, updates, logging and monitoring](operations.md) — install as a system service
