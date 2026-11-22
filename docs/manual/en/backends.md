# Backends, balancing and health checks

This page covers backend configuration, the five balancing strategies, and how health
checks evict and restore backends.

## Backends

```toml
[[tunnel]]
mode = "tcp"
port = 9000
bind = "127.0.0.1"
balance = "round_robin"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
weight = 2

[[tunnel.backend]]
addr = "backend.internal:3001"
weight = 1
tls = true
[tunnel.backend.trust]
mode = "pinned"
fingerprint = "<backend SHA-256>"
```

| Field | Description |
| --- | --- |
| `addr` | Required. A hostname or IP address plus a port |
| `weight` | A positive integer weight used by the `weighted` strategy |
| `tls` | Enables TLS to the backend. WebPKI validation is used by default, and the SNI comes from the hostname in the address |
| `trust` | For self-signed backends, use `mode = "pinned"` plus `fingerprint` |

`balance` belongs to `[[tunnel]]` and must **not** be written after `[[tunnel.backend]]`.
Health checks and data connections use the same TLS configuration.

## Balancing strategies

| Strategy | How a backend is chosen |
| --- | --- |
| `round_robin` | Rotates through the healthy backends |
| `weighted` | Distributes connections by the positive integer `weight` |
| `least_connections` | Picks the backend with the fewest connections currently open |
| `source_hash` | Picks a backend stably from the original user IP address |
| `failover` | Picks the first healthy backend in configuration order |

The choice happens on the **agent**. The count is released when a backend connection closes
or is cancelled. Weights distribute connection counts, not byte counts.

Behaviour details:

- When every backend is marked unhealthy, one is still attempted, so that a misconfigured
  probe cannot make the whole tunnel unavailable.
- A single connection failure reports the error and does **not** retry another backend
  within the same request.
- `source_hash` uses the original address verified by the source ACL or the PROXY protocol.

## Health checks

```toml
[tunnel.health]
type = "http"                # "tcp" | "http"
path = "/healthz"
interval = "10s"
timeout = "2s"
failures_to_evict = 3
successes_to_restore = 2
expect_status = [200, 204]
```

| Field | Description |
| --- | --- |
| `type = "tcp"` | Only checks that a connection can be established |
| `type = "http"` | Requests the given `path`, and accepts 2xx by default |
| `expect_status` | Lists the acceptable status codes explicitly |
| `interval` | The probe interval |
| `timeout` | The timeout for a single probe |
| `failures_to_evict` | How many consecutive failures evict a backend |
| `successes_to_restore` | How many consecutive successes restore a backend |

An HTTP probe uses the backend address as its Host. Consecutive failures evict a backend and
consecutive successes restore it, and both are logged.

UDP tunnels and dynamic-target proxies (socks5/httpproxy/mixedproxy) should **not** be given
TCP or HTTP backend probes, because they have no fixed backend. The same applies to file
mode, where the target is determined by the directory capability handle.

## HTTP backend pooling

The backend connection pool for HTTP tunnels is isolated per client session, tunnel, source
IP, target and HTTP origin: each origin allows at most 8 idle connections with a 30 second
idle timeout, and the outer table holds at most 256 entries. When sending a PROXY header is
enabled, the pool is also isolated per source port. Multiple HTTP/2 requests can share one
connection.

`response_timeout` (server `[http]`) limits the time from sending a request to receiving the
backend's response headers; raise it accordingly for slow uploads of large files. Response
bodies use backpressure and are not buffered whole.

## Next steps

- [Tunnels and port mappings](tunnels.md)
- [HTTP/HTTPS reverse proxy](http-services.md)
- [Access control, quotas and stream coding](security.md)
