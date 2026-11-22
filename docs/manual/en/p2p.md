# P2P direct and relay fallback

When both ends sit behind NAT, Felucca can try UDP hole punching for a QUIC direct link and
fall back to the server relay. Application data bypasses the server; the main connection
still carries control and accounting. Server, provider agent and visitor agent must be
upgraded together.

## Server: the observation port

```toml
[bridge]
bind = "0.0.0.0:14028"

[p2p]
bind = "0.0.0.0:8025"        # Listen on this host
advertise = "203.0.113.10:8025"   # External UDP address both ends can reach, without a scheme
timeout = "5s"
max_pending = 128
```

| Field        | Default | Constraint                                               |
| ---          | ---     | ---                                                      |
| `bind`       | —       | Local listen address                                     |
| `advertise`  | —       | Externally reachable UDP address, **without** a URL scheme |
| `timeout`    | `5s`    | 500ms–10s                                                |
| `max_pending`| `128`   | 1–4096                                                   |

`advertise` must not overlap the UDP ports used by KCP, QUIC or HTTP/3. The `127.0.0.1` in
the configuration only suits demonstrations where every process runs on the same host; for
cross-network use, change it to the server's public domain name and port.

## Provider: allow direct

```toml
# provider agent
[p2p]
enabled = true
max_sessions = 64        # Default 64, maximum 1024
bind_ip = "192.0.2.10"   # Optional; fixes the local UDP IP

[[tunnel]]
id = 1
mode = "tcp"
public = false
backend = [{ addr = "127.0.0.1:3000" }]
```

```toml
# server: a share must enable direct explicitly
[share.office-web]
client = "office"
tunnel = 1
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]
direct = true
```

The server, the local provider and the share must each be **explicitly enabled** (the
server's `[p2p]`, the provider's `[p2p] enabled = true` and the share's `direct = true`). The
main connection of both ends must use TLS, WSS or QUIC with a verified certificate. When
`bind_ip` is omitted, the agent's egress IP configuration is used; it does not change the
egress used for backend dials.

## Visitor: connection preference

```toml
# visitor agent
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"
connection = "prefer_direct"    # relay | prefer_direct | direct
```

| `connection`   | Behaviour                                                                     |
| ---            | ---                                                                           |
| `relay`        | The default; uses the existing relay                                          |
| `prefer_direct`| Tries direct first, and falls back to the relay when the direct link cannot be established and no application data has been sent |
| `direct`       | Accepts direct only; closes the local connection when direct is unavailable   |

## How the link is established

1. The main connection first completes node and share-secret authentication; the server
   checks the visitor identity, the source ACLs and the connection quotas of both sides.
2. The server issues a **one-time ticket**, bound to the tunnel, source, target, UDP type,
   encoding options and PROXY information, and does not pass the provider's node key.
3. Both ends probe for observed UDP addresses using different ticket keys; the server
   confirms the return path with a short-lived, source-bound cookie, and limits the number of
   valid probes, the number of tickets and their lifetime; unknown tickets get no reply.
4. The provider keeps the same UDP socket it used for observation and sends hole-punch
   packets to the endpoint the visitor observed.
5. The visitor connects directly over QUIC TLS 1.3, pinning the fingerprint of the
   provider's ephemeral certificate, then completes a two-way HMAC of the bound ticket and
   two nonces.
6. Backend selection, health state, target ACLs, file authentication and directory
   capability handles keep reusing the original service engine.

If hole punching fails, the provider's attempt and the quotas of both sides are cleaned up
first, and only then is a relay connection allowed. A drop after forwarding begins **does
not replay application data**. A main connection drop, a disabled node, exhausted quotas or
a stopped service close the related direct links.

## Accounting and limits

- Application data travels over QUIC between the two ends and does not pass through the
  server's data relay.
- The provider keeps at most one data block awaiting accounting per direction (TCP blocks
  ≤ 16 KiB, UDP ≤ 65507 bytes).
- The server checks and schedules traffic and rate ranges for both sides before sending, and
  commits the actual byte counts after sending; duplicate sequence numbers, overlapping
  authorizations and over-quota commits are refused.
- Counting uses **decoded application bytes**, excluding probes, TLS, compression overhead,
  UDP frame headers and TCP PROXY headers.
- Direct accounting relies on reports from the authenticated provider; the server cannot
  observe direct payloads on its own. If the provider crashes or the control connection drops
  before the final report, the last sent but uncommitted blocks may not be recorded in the
  persistent counts. A normal shutdown drains through confirmation messages.
- Per-block quota requests increase the dependence on the main connection RTT, so this
  **should not** be described as an indefinite offline direct link detached from the server.

## Supported services

Private and public TCP, UDP and restricted file services are supported, along with SOCKS and
HTTP forward-proxy targets on dynamic shares. Site HTTP/SNI entries, server local and nested
routes, and targets that require separate proxy accounts do not bypass the original entry:
`prefer_direct` uses the original relay, and `direct` refuses. UDP backends with PROXY
headers enabled use the relay; TCP PROXY headers are written to the backend by the provider,
using the server-authenticated source address.

## Deployment boundaries

When destination-dependent mapping is present, when UDP is disabled, and in similar cases,
the relay is used. Real public NAT types and carrier networks still have to be validated in
the deployment environment where they run; do not treat a local simulation as a public
network result.

## Next steps

- [Private shares and visitors](private-shares.md)
- [The felucca-server module](server.md)
- [Access control, quotas and stream coding](security.md)
- Examples: [p2p-server.toml](../../../examples/p2p-server.toml) ·
[p2p-provider.toml](../../../examples/p2p-provider.toml) ·
[p2p-visitor.toml](../../../examples/p2p-visitor.toml)
