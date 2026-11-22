# File services and WebDAV

`mode = "file"` serves files directly from the agent, with no separate HTTP server. It can
use a dedicated public port or ride on the server's HTTP domain entry. WebDAV, Range
requests, directory listings and per-account authentication are supported.

## Dedicated port

```toml
[[tunnel]]
id = 100
mode = "file"
port = 9001
bind = "127.0.0.1"

[tunnel.file]
root = "public"
prefix = "/files"
read_only = true
auto_index = true

[tunnel.file.auth.users]
reader = "replace-this-password"
```

| Field | Description |
| --- | --- |
| `root` | Required. A directory that **already exists** on the agent, relative to the process working directory; a directory handle is opened at startup |
| `prefix` | URL prefix |
| `read_only` | Defaults to `true` |
| `auto_index` | Directory listings |
| `auth.users` | Account table; leave it empty to serve anonymously |

Requests cannot escape `root` through `..`, absolute paths or symbolic links, and COPY/MOVE
targets are held to the same constraint. `root` and the file account configuration stay on
the agent and are **not** put into the server registration message.

## HTTP domain entry

When you set `hosts`, file services use the server's HTTP/HTTPS/HTTP3 domain entry, and
`port` must match that entry:

```toml
[[tunnel]]
mode = "file"
port = 8443
hosts = ["files.example.com"]

[tunnel.file]
root = "public"
read_only = false
```

In this mode you can also set `tunnel.options.http.backend_http2 = true`. Source ACLs, quotas
and rate limits continue to apply. File mode does **not** configure a network backend, target
network policy, PROXY output or raw TLS mode.

## WebDAV methods

| Mode | Allowed methods |
| --- | --- |
| Read-only (default) | `GET`, `HEAD`, `PROPFIND`, `OPTIONS` |
| `read_only = false` | Adds `PUT`, `DELETE`, `MKCOL`, `COPY`, `MOVE`, `LOCK`, `UNLOCK` |

Locks are shared across the multiple connections of the same file tunnel and are released
after the agent restarts. Range requests, conditional requests, directory listings and
`index.html` are supported. Large files are transferred in chunks and use backpressure rather
than being cached whole.

## Files under centralized configuration

When the server pushes a file service, it can only reference a named directory capability
declared in advance on the agent:

```toml
# agent
[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

```toml
# server: inside [[managed.<client>.tunnel]]
[tunnel.file]
root = "documents/shared"
```

The server cannot push an arbitrary absolute path. Paths and symbolic links cannot escape an
authorized directory, and a read-only directory cannot be upgraded to writable through remote
configuration.

## Verification checklist

1. `felucca-agent --config agent.toml --check` passes.
2. `curl -i http://127.0.0.1:9001/files/` lists the directory or returns `index.html`.
3. A request with `Range: bytes=0-99` returns 206.
4. In read-only mode, `PUT` returns 405.
5. `curl --path-as-is 'http://127.0.0.1:9001/files/../../etc/passwd'` is rejected.

## Next steps

- [Tunnels and port mappings](tunnels.md)
- [HTTP/HTTPS reverse proxy](http-services.md)
- [Admin UI and centralized configuration](management.md)
- Examples: [file-server.toml](../../../examples/file-server.toml) ·
  [file-agent.toml](../../../examples/file-agent.toml)
