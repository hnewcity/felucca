# Forward proxy

Felucca can expose a SOCKS5 or HTTP proxy entry from behind NAT. This page covers the three
modes, account authentication, target ACLs and UDP behaviour.

## Three modes

```toml
[[tunnel]]
mode = "socks5"
port = 9080
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
mode = "httpproxy"
port = 9081
bind = "127.0.0.1"

[[tunnel]]
mode = "mixedproxy"
port = 9082
bind = "127.0.0.1"
```

| `mode` | Support |
| --- | --- |
| `socks5` | CONNECT + UDP ASSOCIATE; IPv4/IPv6; DNS resolution on the agent side; BIND returns "unsupported command" |
| `httpproxy` | Absolute URL requests + CONNECT |
| `mixedproxy` | Tells SOCKS5 and HTTP apart by the start of the request |

The forward proxy examples bind to `127.0.0.1` only. When you need remote access, **also** set
a source allowlist; otherwise anyone who can reach that port can use your egress.

## Account authentication

Add an account table under the current tunnel to enable authentication:

```toml
[tunnel.options.auth.users]
alice = "replace-with-a-password"
bob = "another-password"
```

| Entry | Authentication | Failure response |
| --- | --- | --- |
| SOCKS5 | Username and password method, with **no downgrade** to anonymous | Authentication failure |
| HTTP forward proxy / CONNECT | `Proxy-Authorization: Basic` | 407 |
| HTTP reverse proxy site | `Authorization: Basic` | 401 |

Without an account table, anonymous access is allowed. Verified proxy and site credentials
are removed before the request is sent to the backend. Configuration and registration
messages contain account passwords, so the bridge should use the default TLS, WSS or QUIC;
`tcp` and `ws` are explicitly chosen plaintext transports.

File tunnels use `tunnel.file.auth`, which cannot be used together with
`tunnel.options.auth`.

## Target policy

Tunnels with a fixed backend use `target_policy` to limit the actual target. A forward proxy
needs it in particular:

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

An empty `allow` places no restriction on which targets are allowed, `deny` always takes
priority, and an empty `ports` allows any non-zero port. Supported rules are IP/CIDR,
`domain:` for an exact domain name, `suffix:` for a suffix (including the root domain and
subdomains at any depth), `regex:` for a regular expression, and `*` to match all targets.
Names are case-insensitive, and a trailing dot is normalized.

DNS is resolved first, then the **original name and the actual IP** are checked one by one. A
connection uses only the addresses that pass the checks and is not resolved a second time, so
an allowed domain cannot bypass a denied IP. GeoIP and GeoSite are also supported; see
[Access control, quotas and stream coding](security.md).

## SOCKS5 UDP ASSOCIATE

- Only the IP of the control connection peer is accepted: when the request names a port, that
  port is fixed, and otherwise the source port of the first valid UDP packet is bound.
- Closing the control TCP connection clears all target associations.
- Each session allows at most 64 targets, each queueing 8 packets, and idle entries are
  reclaimed after 60 seconds.
- A non-zero FRAG is dropped, and SOCKS fragment reassembly is not implemented.

## Rate and quota

- `tunnel.options.limits.rate_limit` is a bytes-per-second limit shared by all of the agent's
  tunnels and by both directions, and it allows an initial burst of one second of traffic.
- `quota.flow_limit` counts both directions, and `quota.expires_at` uses Unix seconds.
- Quotas are checked periodically and abort connections that are transferring, so concurrent
  in-flight traffic can exceed the quota. The counts are not used for strict billing.

## Next steps

- [Tunnels and port mappings](tunnels.md)
- [Private shares and visitors](private-shares.md) — SOCKS visitors with no public port
- [P2P direct and relay fallback](p2p.md)
