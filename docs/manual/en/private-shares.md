# Private shares and visitors

A private tunnel registers with the server but occupies **no public port**. Another agent,
the visitor, opens a listener on its own machine and reaches it over the authenticated,
encrypted bridge using a separate share secret. Use this to expose an internal service to
specific nodes rather than to the public internet.

## The three configs

Each of the provider agent, the visitor agent and the server changes in one place.

### Provider: make the tunnel private

```toml
[[tunnel]]
id = 1
mode = "tcp"
public = false                 # Omit port and bind
backend = [{ addr = "127.0.0.1:3000" }]

[[tunnel]]
id = 3
mode = "socks5"
public = false
[tunnel.target_policy]
allow = ["127.0.0.0/8", "::1", "10.0.0.0/8"]
```

TCP, UDP, file and proxy entries are supported. **HTTP domain routing relies on the HTTP
listener and cannot be made private**; a private TCP tunnel can forward any HTTP or HTTPS
backend. Setting `port = 0` explicitly is equivalent to a private tunnel.

### Server: declare the share

```toml
[share.office-web]
client = "office"              # Node name; for a server-local group use client = "local.<group-name>"
tunnel = 1                     # Stable tunnel ID
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]     # An empty list allows every authenticated node that holds the share secret
direct = true                  # Allow P2P direct connection attempts
```

A `secret` is 16–4096 bytes long, and each share should use a different one.
`acl_mode`/`acl_rules` check the source address of visitor bridge connections. A share secret
alone **cannot** be used to connect as an anonymous client: a visitor still needs a normal
node identity and a vkey. The server can permanently disable a node with
`client.NAME.enabled = false`.

### Visitor: listen locally

```toml
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"                   # tcp | udp | socks5
max_connections = 64
udp_idle = "60s"
```

| Field                | Description                                                              |
| ---                  | ---                                                                      |
| `name`               | Unique within this agent                                                 |
| `share`              | The server's share name                                                  |
| `mode`               | `tcp`, `udp` or `socks5`                                                 |
| `bind`               | Requires a fixed port; it binds at startup, and a busy port fails at once |
| `max_connections`    | 1–4096, default 64; TCP connections or UDP source sessions               |
| `udp_idle`           | 1 second–1 hour, default 60 seconds; activity in either direction extends the session |
| `acl_mode` / `acl_rules` | Check the source address of connections to the visitor listener |
| `target`             | Dynamic shares only, see below                                           |

All visitors also share the agent's connection limit and an 8 MiB UDP queueing budget. A
visitor accepts TLS, WSS or QUIC with a verified certificate only.

The server applies the share ACL, the visitor ACL and the provider's routing ACL on the
**actual source address of the visitor bridge**. A raw address that reaches the visitor port
on the LAN is not forwarded to the server as a trusted source.

## What you get

With the configurations above, reaching `127.0.0.1:18080` on the visitor machine arrives at
`127.0.0.1:3000` on the office agent, and UDP `127.0.0.1:15353` corresponds to
`127.0.0.1:5353` on office.

Connections and raw traffic count against both the visitor and the provider, and against
their user and tunnel resources; when one user owns both ends, the usage of both is added
up. UDP length headers and share authentication messages are not counted as business
traffic.

## Dynamic targets

When a server share sets `dynamic = true`, the visitor can name a target:

```toml
[[visitor]]
name = "office-proxy"
share = "office-proxy"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:11080"
mode = "socks5"
target = "internal.example:443"
```

- The provider must be a SOCKS5, HTTP forward proxy or mixed proxy tunnel; UDP targets
  require SOCKS5 or a mixed proxy.
- The provider's actual resolved IP and target policy are still checked.
- A dynamic share requires that the proxy entry has **no separate proxy accounts**; with
  accounts configured it is refused, so that a shared secret cannot bypass the proxy's own
  authentication.
- Without `target`, the visitor connects to the full service entry; file and proxy accounts
  still authenticate normally.
- A TCP visitor can pass a SOCKS CONNECT or an HTTP proxy session through; the server UDP
  address returned by SOCKS UDP ASSOCIATE is not automatically mapped to a local address on
  the visitor.

When you need full SOCKS5 access, set the visitor to `mode = "socks5"` and reference a
dynamic share, with no `target`: SOCKS CONNECT and UDP ASSOCIATE are resolved on the visitor
side, and every destination goes through share authentication and the provider's target
policy. `[visitor.auth.users]` configures local SOCKS accounts, and when accounts exist the
visitor does not downgrade to anonymous authentication. `max_connections` counts SOCKS
control connections and UDP target associations together, so set it to at least 2 when you
use UDP.

## Management and runtime

- The management "Share access" page creates, edits and deletes shares; secrets are masked by
  default, and leaving the field empty while editing keeps the current value.
- Operators can only manage the provider-side shares of the users they own; read-only accounts
  cannot make changes.
- The API is `/api/shares` and `/api/shares/NAME`; writes keep the configuration version and
  the idempotency request key.
- After the bridge drops, the local listener is kept, existing sessions finish, and new
  requests resume after reconnecting.
- Changes to a share take effect through a normal Daemon reload, and existing connections are
  dropped.
- This path goes through the server relay; for a direct link see [P2P direct and relay
  fallback](p2p.md).

## Verification checklist

1. The server's `--check` and both agents' `--check` pass.
2. The visitor's `127.0.0.1:18080` returns the content of the provider's `127.0.0.1:3000`.
3. A visitor connection with the wrong `secret` is refused.
4. Nodes outside `allow_clients` are refused.
5. The server gained no new public listener (compare with `ss -ltnp`).

## Next steps

- [P2P direct and relay fallback](p2p.md)
- [The felucca-agent module](agent.md)
- [Admin UI and centralized configuration](management.md)
- Examples: [visitor-server.toml](../../../examples/visitor-server.toml) ·
[private-agent.toml](../../../examples/private-agent.toml) ·
[visitor-agent.toml](../../../examples/visitor-agent.toml)
