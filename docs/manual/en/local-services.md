# Server-local targets and nested routes

`[local.<name>]` lets the server provide services itself, with no agent and no bridge
credentials. Local tunnel backends can also use aliases that enter another registered
tunnel, including tunnels on an agent, which composes into nested routes.

## A local service group

```toml
[user.office]
[user.office.limits]
max_connections = 100

[local.office]
user = "office"
dial_timeout = "10s"
[local.office.network]
local_ip = "127.0.0.1"
forward_local_ip = true

[[local.office.tunnel]]
id = 1
mode = "tcp"
port = 9001
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
[local.office.tunnel.health]
type = "tcp"
interval = "10s"
timeout = "2s"
```

Each group can set `user`, `acl_mode`/`acl_rules`, `limits`, `network` and `dial_timeout`,
and can use the same TCP/UDP, HTTP/SNI, proxy, file, load balancing, probe and ACL
configuration as an agent. When you run local services only, you can omit the client table.

A local group is recorded as `local.<name>` in the registry and traffic snapshots, and
**this namespace cannot be used as the key_id of an external agent**. A local group has no
bridge, so upstream proxy configuration is rejected. Only `forward_local_ip = true` uses the
egress IP for the backend. File roots, certificates and backend configuration are prepared at
startup, and a listening conflict makes that service fail to start.

## Backend aliases

| Address | Behaviour |
| --- | --- |
| `tunnel://local.office/7` | Enters the service entry of another registered local tunnel |
| `tunnel://agent-1/7` | Enters a registered tunnel on an online agent |
| `file://agent-1/7` | Same as above, but requires the registered target to be `file` mode; directory boundaries, read-only state and file authentication are preserved |
| `bridge://tcp`, `tls`, `ws`, `wss` | Enters the matching bridge entry; the agent still needs mutual authentication, and TLS uses the bridge certificate |
| `bridge://http` | Enters the server's HTTP entry, where Host, site authentication and permission checks continue to run |
| `bridge://web` | Connects to the management endpoint, which authenticates in the same way |

Tunnel numbers must be explicit and stable. A local group with a `user` can only reference
targets of that same user; a group without a `user` is managed by the server operator and can
reference across users. `bridge` aliases are allowed only for operator groups. A remote agent
and dynamic proxy requests cannot use an alias as a network target.

Aliases do not take an extra `backend.tls`, a network health probe or an entry target ACL:
permissions are enforced by each hop's target. An HTTPS/SNI alias can pass TLS data directly,
and the target site terminates or passes it through. UDP can only nest into UDP, and other
modes use the stream entry. SOCKS5/HTTP proxy authentication is not skipped.

## Loop control and accounting

- Every hop keeps the original source IP and port and the routing history.
- A repeated `(client, id)` or more than **16 hops** is rejected immediately.
- The HTTP connection pool is also isolated by history, so a reused connection cannot bypass
  the loop check.
- Every tunnel actually passed through gets its own connection allowance and records traffic;
  when the same user or group passes through multiple hops, the aggregate quota is also
  charged for each hop.
- UDP accounting covers the message payload and excludes internal length frames.

## Common patterns

| Scenario | How |
| --- | --- |
| Expose a service from the server host as well | `[local.ops]` + `[[local.ops.tunnel]]` |
| Add TLS offload in front of an internal service | An HTTP tunnel in a local group with `tls_mode = "offload"` |
| Add another entry on top of an internal service, such as site authentication | An outer local group with the backend written as `tunnel://agent-1/7` |
| Reuse management endpoint authentication from an internal service | Write the backend as `bridge://web` |

## Next steps

- [Tunnels and port mappings](tunnels.md)
- [The felucca-server module](server.md)
- [Access control, quotas and stream coding](security.md)
- Examples: [local-server.toml](../../../examples/local-server.toml)
