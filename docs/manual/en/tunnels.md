# Tunnels and port mappings

A tunnel is the basic unit of Felucca: one public entry plus a set of backends. This page
covers the shared fields, the eight modes, batch port mappings and the PROXY protocol.

## Shared tunnel fields

```toml
[[tunnel]]
id = 1                 # Stable ID; pin it once traffic snapshots are enabled
mode = "tcp"           # Entry protocol
port = 8080            # Port opened on the server
bind = "0.0.0.0"       # IP only, no port attached
remark = "internal web app"
public = true          # false means a private share that does not take a public port
balance = "round_robin"   # Goes inside [[tunnel]], not after [[tunnel.backend]]

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`bind` takes an IP address, not a port; the port comes from `tunnel.port`. Omit `bind` to
listen on all interfaces. When `id` is not set, an available positive integer is assigned.
Once traffic snapshots are enabled, set `id` explicitly instead of relying on array order.

## Modes

| `mode` | Entry behaviour | Target source |
| --- | --- | --- |
| `tcp` | Public TCP port, raw byte forwarding | `backend` |
| `udp` | Public UDP port, datagram boundaries preserved | `backend` |
| `http` | Reuses the server's HTTP, HTTPS or HTTP/3 listener and routes by domain and path | `backend` |
| `socks5` | SOCKS5 forward proxy, CONNECT + UDP ASSOCIATE | Set by the client |
| `httpproxy` | HTTP forward proxy, absolute URL + CONNECT | Set by the client |
| `mixedproxy` | Mixed entry, distinguishes SOCKS5 from HTTP by the start of the request | Set by the client |
| `file` | File serving and WebDAV | A directory on the agent host |
| `transparent` (alias `tcptrans`) | Reads the original destination address from the kernel | The kernel |

An `http` tunnel does **not** open a separate listener: its `port` must match the server's
`http.bind`, `http.tls_bind` or `http.http3_bind`, and several HTTP tunnels can share one
port. `tunnel.bind` has no effect on HTTP routing. The HTTP listener is enabled by the
server, and is not limited by the agent's `allowed_ports`.

## Batch port mappings

`[[mapping]]` supports TCP and UDP only, and maps a range of ports one to one in order:

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

The two counts must be equal, and duplicate server ports are not allowed. The expanded
tunnels are validated together with explicit `[[tunnel]]` entries, and still count against
the server's port authorization and the tunnel limit. `target_host` can be a domain name, an
IPv4 address or an IPv6 address, and cannot carry a port. You can also set `tls`/`trust`,
`health`, `acl`, `options` and `target_policy`.

## UDP behaviour

- A separate association is established for each source IP:port, and pinned to one backend
  socket on the agent side.
- Empty packets and datagram boundaries are preserved. The maximum datagram is 65507 bytes,
  and the usable MTU on the real network may be lower.
- Each tunnel allows at most 256 source associations, each queuing 8 packets. Packets are
  dropped when a queue is full, and an association is reclaimed after 60 seconds idle.
- Datagrams are carried over a reliable tunnel stream, **not** QUIC DATAGRAM, so a lossy link
  waits for retransmission.
- UDP statistics cover the datagram payload only, not the internal length frames.

## PROXY protocol

Send the original source address and the entry destination address to the backend:

```toml
[tunnel.options]
proxy_protocol = "v1"     # "off" | "v1" | "v2"
```

For TCP, HTTP and TLS, one header is sent per backend connection. UDP supports v2 only,
appending a DGRAM header per datagram; packets larger than 65507 bytes including the header
are dropped. The backend must be able to parse the PROXY protocol.

On the receiving side, configure `proxy_accept` for the owning listener:

```toml
[http.proxy_accept]
mode = "required"                    # optional | required
trusted = ["192.0.2.10", "2001:db8:1::/64"]
bridge.proxy_accept = { ... }
[tunnel.options.proxy_accept]
mode = "optional"
trusted = ["10.0.0.0/8"]
```

`optional` allows direct connections without a header, and `required` rejects a missing
header. When you enable this, `trusted` must not be empty. Source ACLs, source hashing and
HTTP forwarded headers all use the verified original address. A bridge and an HTTP listener
on the same port must be configured with the same policy. Receiving is limited to 5 seconds,
and a UDP listener does not accept PROXY headers.

## Per-stream coding

```toml
[tunnel.options.payload]
compress = true     # Snappy, default false
encrypt = true      # ChaCha20-Poly1305, default false
```

This applies to TCP and UDP, file and WebDAV transfers, HTTP reverse-proxy and forward-proxy
data streams. With compression enabled, data is compressed before it is encrypted. Each
stream exchanges a fresh nonce, and two independent directional keys are derived from that
agent's vkey. When the two ends do not match, negotiation is refused, with no automatic
fallback to plaintext. See [Access control, quotas and stream coding](security.md) for
details.

## Next steps

- [Backends, balancing and health checks](backends.md)
- [HTTP/HTTPS reverse proxy](http-services.md)
- [Forward proxy](forward-proxy.md)
- [File services and WebDAV](file-services.md)
- [Server-local targets and nested routes](local-services.md)
