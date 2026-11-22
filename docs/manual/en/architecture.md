# Architecture and concepts

This page explains the parts of Felucca, how traffic flows, and the vocabulary used by
every later guide. Internal module ownership is documented separately in
[Architecture](../../architecture.md).

## Deployment shape

```text
                    public network
 ┌──────────────────────────────────────────────────────┐
 │  felucca-server                                      │
 │  bridge   14028 (TCP/TLS/WS/WSS share one socket)    │
 │  quic     14028 (UDP, native QUIC streams)           │
 │  kcp      8025 (UDP)                                 │
 │  http     8080 / 8443 (+ http3 UDP)                  │
 │  admin    8088 (admin UI, optional)                  │
 └───────────────▲──────────────────────────────────────┘
                 │ agent dials out
        ┌─────────────────────┐
        │  felucca-agent      │    private network
        │  local service 3000 │
        └─────────────────────┘
```

The agent dials out to a `felucca-server` on the public network, so the private network needs
no public IP, no port mapping on the router, and no firewall change. Every public entry
point — ports, domains, proxies and files — belongs to the server.

## The two programs

| Program | Responsibilities | Needs inbound ports |
| --- | --- | --- |
| `felucca-server` | Accepts agents, holds public listeners, routes, manages, meters | Yes |
| `felucca-agent` | Dials out from the private network, holds local listeners, sends backend requests | No |

Both must use the same protocol version. The current bridge protocol version is 10, and the
**server and agent must be upgraded together**. Use `--build-info` to print the program name,
version, protocol and build target:

```sh
target/release/felucca-server --build-info
target/release/felucca-agent --build-info
```

## Key concepts

| Term | Meaning |
| --- | --- |
| **bridge** | The long-lived connection between server and agent. `bridge.bind` is the address shared by TCP/TLS/WS/WSS and defaults to `0.0.0.0:14028`; QUIC and KCP each use their own UDP port |
| **identity** | The agent is identified by `key_id` + `vkey`; the server must have a matching `[client.<key_id>]` table entry |
| **transport** | The agent side can choose `tcp`, `tls`, `ws`, `wss`, `quic`, `kcp`. On the public network prefer TLS/WSS/QUIC |
| **trust** | How the agent verifies the server certificate: `webpki` (CA validation), `pinned` (fixed SHA-256 fingerprint), `insecure` (diagnostics only) |
| **tunnel** | A mapping from one public entry point to a set of backends. `mode` decides the entry protocol, `backend` decides the target |
| **backend** | The target the agent actually connects to, in the form `127.0.0.1:3000` |
| **listener** | The socket the server opens for a tunnel. A reconnect with the same `key_id` replaces the old listener |
| **share** | The server grants a registered tunnel to a named visitor without using a public port |
| **visitor** | Another agent holding the shared key, which opens a listener on its own machine to reach that share |
| **local** | A configuration group where the server provides the service itself, with no agent |
| **mux** | The bounded multiplexing above TCP/TLS/WS/WSS/KCP, providing window negotiation, budget and backpressure |
| **managed** | Tunnel configuration the server pushes to the agent; the agent must enable `[remote]` explicitly |

## Life of a request

Take a TCP tunnel as an example:

1. A visitor connects to the public port on the server.
2. The server checks the source ACL, the quota and the connection allowance, then assigns the
   connection to the bridge stream of the matching agent.
3. The agent receives the request, picks a healthy backend by load balancing, and connects.
4. Data flows both ways over a mux stream (or a native QUIC stream) until either half closes.
5. The server meters the traffic, and can keep it across restarts when `[state]` or the admin
   side is configured.

An HTTP tunnel adds two steps: domain/path routing and site authentication. A SOCKS5 tunnel
adds two more: target address resolution and a target ACL.

## Internal modules

As a user you normally do not need to care about these, but they explain which crate does
what:

| crate | Purpose |
| --- | --- |
| `felucca-core` | Configuration, identity, access rules, metering types; does not depend on an async runtime |
| `felucca-mux` | Bounded multiplexing, windows and backpressure |
| `felucca-proto` | Handshake, control messages, visitor and direct-connection protocols |
| `felucca-transport` | TCP/TLS/WS/QUIC/KCP connection adaptation and egress networking |
| `felucca-service` | Backend connection, load balancing, health checks and restricted file serving shared by both sides |
| `felucca-server` | Agent acceptance, ingress routing, management, runtime state |
| `felucca-client` | Agent connection, reconnects, configuration reception, visitors |
| `felucca-cli` | The two programs, startup configuration, logging, system services, updates |

## Next steps

- [Get started](get-started.md) — go back a step if you have no working tunnel yet
- [Configuration sources and CLI](configuration.md)
- [The felucca-server module](server.md) / [The felucca-agent module](agent.md)
