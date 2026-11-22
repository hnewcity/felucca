> 历史记录：保留原始开发背景与当时的验证结果，不作为当前开发指令或版本状态。当前架构见 `docs/architecture.md`，验证状态见 `docs/validation.md`。

# felucca architecture

## What this is

A Rust reimplementation of the nps feature set: expose services that sit behind
NAT or a firewall, by having an outbound-only **agent** hold a tunnel to a
publicly reachable **server**, which then accepts inbound traffic on the agent's
behalf.

## Decisions

**No wire compatibility with Go nps.** Confirmed with the project owner. A
felucca server pairs with a felucca agent. This buys us:

- one framing scheme instead of the Go version's per-flag string constants
  (`main` / `chan` / `vstr` / `conf` ...) sent as 4-byte ASCII tags
- typed control messages (`postcard`) instead of ad-hoc `*#*`-separated payloads
- a mux we design for backpressure from the start, rather than preserving a
  legacy frame layout

**Everything rides one multiplexed control connection.** The Go version opens a
fresh TCP connection per proxied request in several paths. We open streams on a
mux instead, so a busy tunnel costs one socket, not N.

**TCP/TLS/WS/WSS use felucca mux; QUIC uses native streams.** The data path uses
`Tunnel` / `TunnelHandle` / `TunnelStream` enums. QUIC does not carry a second mux:
its own streams already provide independent flow control.

The owner subsequently authorized every remaining item in the nps source audit,
including KCP, P2P and the admin UI. Only Go wire compatibility remains excluded.
The milestone below is historical scope, not a limit on the current task.

## First milestone scope

Target scope. Baseline paths exist, but the modules are not feature-complete
against nps; see [the source audit](../nps-parity-audit.md) for remaining subfeatures.
Passing the current tests does not establish full milestone or nps parity:

- core tunnel: handshake, auth, mux, heartbeats, reconnect
- TCP and UDP port forwarding
- HTTP and HTTPS reverse proxy, including host matching and TLS/SNI
- ACME certificate issuance, so HTTPS hosts do not need a hand-placed cert
- multiple backend targets per tunnel, with load balancing across them
- active health checking, which decides which backends are eligible
- SOCKS5 and HTTP forward proxy, with ACLs
- traffic accounting and rate limiting
- config files and CLI

Backend selection and active probing live on the agent, where local services are
reachable. Requests carry the original source IP for source hashing and ACLs.
Server listeners and HTTP host routes share admission and metering. ACME renewals
replace the SNI resolver's certificates without restarting listeners.

Deferred in the original milestone, now included in the authorized work:

- P2P hole punching (agent-to-agent direct paths)
- web admin UI

## Data path

```
                    inbound public traffic
                             |
   +-------------------------v--------------------------+
   |  felucca-server                                    |
   |                                                    |
   |  per-tunnel listener  ->  route to tunnel/host     |
   |                                |                   |
   |                     open a tunnel stream on the       |
   |                     owning agent's tunnel          |
   +--------------------------------|-------------------+
                                    | one transport conn
                                    | (tcp/tls/ws/quic)
   +--------------------------------v-------------------+
   |  felucca-agent                                     |
   |                                                    |
   |  accept stream  ->  dial the local target  ->  copy|
   +----------------------------------------------------+
                                    |
                             local service
```

The agent always dials out. The server never needs a route to the agent.
