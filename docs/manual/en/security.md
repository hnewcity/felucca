# Access control, quotas and stream coding

This page collects the security-related configuration: source ACLs, target ACLs,
GeoIP/GeoSite, layered quotas and rate limits, traffic snapshots, and the optional per-tunnel
compression and encryption.

## Source ACL

```toml
[client.agent-1]
acl_mode = "allow"                 # off | allow | deny
acl_rules = "203.0.113.0/24, 2001:db8::/32"
```

Rules are IP/CIDR entries separated by commas or newlines. The server's top-level
`acl_mode`/`acl_rules`, `[user.<name>]`, `[client.<id>]` and the tunnel rules are
**intersected**; a client associates itself with a user in its own configuration through
`user = "<name>"`, and the agent cannot change that association.

These rules apply to clients that reach a public entry, and are **not used to refuse an
agent's bridge address** — bridge identity is verified by the vkey. The rules act on the
source IP of clients reaching the proxy, not on the target address.

Source ACLs support GeoIP. `acl_rules` can also be written on `[[tunnel]]`,
`[local.<name>]` and `[[visitor]]`.

## Target policy

Enforced on the agent side, and applies to fixed backends, health probes and forward-proxy
targets:

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

| Item        | Description                                                             |
| ---         | ---                                                                     |
| `allow` empty | No restriction on allowed targets                                     |
| `deny`      | Always takes priority                                                   |
| `ports` empty | Any non-zero port is allowed                                           |
| Syntax      | IP/CIDR, `domain:`, `suffix:`, `regex:`, `*`                            |
| Geo         | `geoip:CN`, `geosite:category-ads-all`, `geosite:custom:LOCAL@fast@!ads` |

`@attribute` requires the attribute to be present, `@!attribute` requires it to be absent,
and multiple conditions are intersected. `geoip:private` covers the built-in IPv4/IPv6
private, loopback, link-local and CGNAT ranges, and needs no data file. The name and the
resolved IP are checked together; names are case-insensitive, and a trailing dot is
normalized.

A missing file, a missing group, a bad regular expression or a corrupted protobuf **is an
error**, and does not degrade into an empty rule that allows everything. Data is loaded at
startup, so restart the relevant process after updating a data file. Each end uses its own
locally declared data files, and the remote side can only reference groups and aliases.

## Geo data

```toml
[geo]
geoip = "data/geoip.dat"
geosite = "data/geosite.dat"

[geo.extra]
mycorp = "data/mycorp.dat"
```

Point these to V2Ray protobuf `.dat` files, with paths relative to the process working
directory. IP prefixes use compiled range lookups; domains support exact, suffix, keyword and
regular expression matching.

## Layered quotas

`[user.<name>.limits]`, `[tunnel.options.limits]` and
`[http.host_limit."<domain pattern>"]` accept the same fields:

```toml
max_connections = 20
rate_limit = 1048576
quota = { flow_limit = 1073741824, expires_at = 1893456000 }
```

Traffic and rate units are bytes, and `expires_at` is in Unix seconds. Leaving a field out
means no limit; rate 0 means no limit, while connections 0 explicitly blocks new connections.
Multiple clients of one user, multiple connections of one tunnel and multiple paths of one
domain share their own counters and limits.

Multiple rate layers are charged at the same time, and the waiting time takes the **strictest
layer**; a failed connection slot request returns the slots granted by the earlier layers.
HTTP domain limits are selected by the original Host/SNI, an exact domain takes priority over
a wildcard, and they are unaffected by backend Host rewriting. HTTP connection counting
covers actual backend connections; multiple HTTP/2 requests can reuse one connection.

TCP, HTTP, SNI and UDP all update the counters of every layer. TCP is checked and
rate-limited in 16 KiB blocks, and UDP is handled per datagram. Traffic currently in flight
across concurrent transfers can push the total slightly over the quota, so this is **not** an
exact byte-by-byte cut-off. HTTP counts backend protocol bytes, including headers and
protocol overhead; UDP counts the payload only. A quota aborts connections that are
transferring, and the counts are not used for strict billing.

## Traffic snapshots

```toml
[state]
path = "data/traffic.json"
save_interval = "5s"
```

Once enabled, the snapshot is atomically replaced at the interval, and the final counts are
saved on a normal exit and after connections end. The snapshot includes offline resources, so
they still count against quotas after a server restart; a crash can lose the data of the last
save interval. A single snapshot can be written by one server only. A corrupted snapshot or
one of an unknown version refuses to start up, and the original file is kept.

This is a traffic snapshot; the runtime configuration of the management UI is stored
separately in `admin.state_path` / `--runtime-config`, and the two must not use the same path.
Once snapshots are enabled, pin the `id` of your tunnels.

## Per-stream coding

```toml
[tunnel.options.payload]
compress = true
encrypt = true
```

| Option      | Implementation      | Behaviour                                                                                                                             |
| ---         | ---                 | ---                                                                                                                                   |
| `compress`  | Snappy              | Works in blocks of at most 16 KiB of raw data, and uses a compressed block only when it is smaller; the declared size is checked before decompression |
| `encrypt`   | ChaCha20-Poly1305   | Each stream exchanges a fresh nonce, and the agent's vkey verifies the identity of both sides and the full routing request; HKDF derives two independent directional keys |

With compression enabled, data is **compressed before it is encrypted**. Length headers,
sequence numbers and the end record are covered by integrity checks, and tampering, replay or
a missing end record closes the data stream. When the two ends do not match, negotiation is
refused, with no automatic fallback to plaintext. Each direction buffers one bounded record
only.

This coding layer lives inside the mux/QUIC stream and does not add a multiplexing layer of
its own; PROXY headers, HTTP/WebDAV content and UDP datagrams all go through the same coding.
Traffic quotas and rate limits are still calculated on the **raw forwarded bytes**.

Security boundary:

- Independent encryption protects the data part of routing; configuration registration and
  bridge control messages are still protected by the chosen bridge transport, so TLS, WSS or
  QUIC remains the default deployment choice.
- This PSK layer **does not provide forward secrecy on its own**.
- Compression changes the observable record size; keep `compress = false` when you need to
  hide compression characteristics.

## Secret handling

- Raw secrets never reach the logs; Debug and Display output is masked.
- Persistence must keep the secret, and masking happens only when it is displayed; when you
  add a credential field, check the masking path at the same time.
- Configuration snapshots contain the real secrets, and on Unix new files are 0600 and new
  directories 0700.
- Do not write the raw management Bearer token into the server configuration; write
  `token_hash` only.

## Next steps

- [Tunnels and port mappings](tunnels.md)
- [HTTP/HTTPS reverse proxy](http-services.md)
- [Admin UI and centralized configuration](management.md)
