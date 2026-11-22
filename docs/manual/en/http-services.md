# HTTP/HTTPS reverse proxy

This page covers exposing internal HTTP services by domain and path: routing, header and
Host rewriting, redirects, CORS, error pages, SNI, HTTP/2 and HTTP/3, shared ports and
forwarded headers.

## Register an HTTP route

```toml
# Server side
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"

# Agent side
[[tunnel]]
mode = "http"
port = 8080                       # Must match http.bind or http.tls_bind
hosts = ["app.example.test", "*.example.test"]
balance = "round_robin"

[tunnel.options.http]
path = "/api/"
scheme = "any"
rewrite_prefix = "/v2/"
host = "backend.internal"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

An `http` tunnel opens no separate listener; `port` decides which HTTP listener it attaches
to, and it is reachable over both HTTP and HTTPS.

## Routing rules

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"        # any | http | https
```

Matching priority: **exact domain, then longest path prefix, then exact scheme**.
Domain names are case-insensitive, and a trailing dot is normalized.

| Pattern | Matches |
| --- | --- |
| Exact `app.example.test` | That domain only |
| Wildcard `*.example.test` | Matches `a.example.test`, but **not** `example.test` or `a.b.example.test` |

Duplicate registrations with the same domain, path and protocol are rejected. A request with
no matching route returns 404. You can register several paths for the same domain and assign
them to different tunnels.

## Request and response options

```toml
[tunnel.options.http]
rewrite_prefix = "/v2/"
host = "backend.internal"
cors = true
redirect = "https://new.example{path}"
force_https = true
request_headers = { x-source = "{remote_ip}", x-remove = "" }
response_headers = { x-proxy = "felucca" }
error_pages = { "502" = "<h1>Service unavailable</h1>" }
```

| Option | Behaviour |
| --- | --- |
| `rewrite_prefix` | Rewrites the prefix and keeps the remaining path and query string |
| `host` | Rewrites the Host sent to the backend |
| `request_headers` | Header templates; an empty value removes that header |
| `response_headers` | Response header templates |
| `cors` | Returns `Access-Control-Allow-Origin: *`, and a valid preflight returns 204 directly. A preflight does not require a site account, but the actual request still requires authentication |
| `redirect` | Returns 307 |
| `force_https` | Returns 301, with the target using the server's HTTPS listener port (443 is omitted). An HTTPS listener must exist when you register it |
| `error_pages` | Replaces the response body by status code. Supports 400..599, with at most 64 KiB per page |

Templates support `{host}`, `{path}` (including the query string), `{scheme}`,
`{remote_ip}` and `{remote_port}`. Values are taken from the original request, and a
substitution result is not expanded again. Message boundary headers such as Content-Length
and Transfer-Encoding, and hop-by-hop headers, cannot be rewritten; Host has its own field.

Site authentication uses `Authorization: Basic`, and a failure returns 401. Verified
credentials are not forwarded to the backend.

## TLS modes

```toml
[tunnel.options.http]
tls_mode = "http"        # http | passthrough | offload
```

| Mode | Behaviour |
| --- | --- |
| `http` | The default. An ordinary HTTPS reverse proxy |
| `passthrough` | Passes the raw TLS stream to the backend by SNI, and the backend certificate is shown directly to visitors. In this mode `backend.tls` must be disabled |
| `offload` | Terminates TLS with the server's SNI certificate, then sends arbitrary TCP content to the backend |

`passthrough` and `offload` occupy the full SNI, and cannot be combined with HTTP path or
header options, or with site authentication.

## Ports and protocols

| Capability | Description |
| --- | --- |
| Entry | HTTP/1.1, HTTP/2, HTTP/3 |
| Backend | HTTP/1.1, HTTP/2 (`backend_http2 = true`) |
| ALPN | TLS entries support `h2` and `http/1.1`; plaintext entries support HTTP/2 prior knowledge |
| WebSocket | Automatically converted across the three entry types and the two backend types. An HTTP/1.1 entry returns 101, while HTTP/2 or HTTP/3 uses the `:protocol = websocket` extended CONNECT |
| HTTP/3 | Requires `http.http3_bind`. Each QUIC connection allows at most 128 bidirectional streams, with a receive window of 1 MiB per stream and 16 MiB per connection. It closes after 30 seconds idle, and request headers are limited to 32 KiB |

HTTP/3 is only a public entry protocol; backends still use HTTP/1.1 or HTTP/2. A normal
HTTPS response carries an `Alt-Svc` header, and you can override it with `response_headers`
when the externally mapped port differs.

## Sharing ports

`bridge.bind`, `http.bind` and `http.tls_bind` can all be the **same** IP:port, and one
socket demultiplexes by handshake characteristics: directly connected TLS/WSS agents use the
`felucca/9` ALPN, while websites use `h2`/`http/1.1` or no ALPN. An ordinary `GET /ws` is
still a website request, and only a correct WebSocket upgrade enters the bridge. QUIC
bridging and HTTP/3 can also share one UDP address, demultiplexed by ALPN, with bridge
certificates and website certificates selected and renewed separately.

## Client addresses

By default, client-supplied `Forwarded`, `X-Forwarded-*` and `X-Real-IP` headers are
removed, and then `X-Forwarded-For`, `X-Real-IP`, `X-Forwarded-Host` and
`X-Forwarded-Proto` are set from the direct source address or from the PROXY source verified
by `http.proxy_accept`. Set `forwarded_headers = false` to turn this rewriting off, which
suits deployments where you control the full proxy chain yourself. Hop-by-hop headers, and
headers named in `Connection`, are not passed on to the next hop.

## Next steps

- [Certificates, SNI and ACME](certificates.md)
- [Backends, balancing and health checks](backends.md)
- [File services and WebDAV](file-services.md) — file entry by domain
- [Access control, quotas and stream coding](security.md) — domain quotas and ACLs
