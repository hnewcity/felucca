# Felucca documentation index · English

[中文](../README.md) · English · [日本語](ja/README.md) · [한국어](ko/README.md) · [Français](fr/README.md) · [Español](es/README.md) · [Русский](ru/README.md)

This is the user documentation for Felucca: from a first working tunnel, through how to
configure and use every module, to the command, field, endpoint and glossary references.
It describes **how to use Felucca**, not how it is implemented.

## Get Started

| Page | Contents |
| --- | --- |
| [Get started](get-started.md) | Installation, the local demo, and a first tunnel by hand |
| [Architecture and concepts](architecture.md) | The parts, the traffic path, and a glossary |
| [Configuration sources and CLI](configuration.md) | Config sources, environment variables, multiple instances, `--check` |

## Guides

| Page | Contents |
| --- | --- |
| [The felucca-server module](server.md) | Bridge, client table, HTTP ingress, port authorization |
| [The felucca-agent module](agent.md) | Connection, trust, reconnects, egress networking, upstream proxy |
| [Tunnels and port mappings](tunnels.md) | The eight modes, `[[mapping]]`, UDP, PROXY protocol |
| [Backends, balancing and health checks](backends.md) | Backend fields, the five strategies, eviction and recovery |
| [HTTP/HTTPS reverse proxy](http-services.md) | Domain and path routing, rewriting, CORS, SNI, HTTP/2, HTTP/3 |
| [Certificates, SNI and ACME](certificates.md) | Bridge certificates, static certificates, issuance and renewal |
| [Forward proxy](forward-proxy.md) | SOCKS5 / HTTP / mixed entries, accounts, target ACLs |
| [File services and WebDAV](file-services.md) | Directory capabilities, read-only control, Range, WebDAV methods |
| [Server-local targets and nested routes](local-services.md) | `[local.*]`, backend aliases, 16-hop loop control |
| [Private shares and visitors](private-shares.md) | TCP/UDP/file/proxy access without a public port |
| [P2P direct and relay fallback](p2p.md) | One-time tickets, hole punching, QUIC direct, fallback |
| [Access control, quotas and stream coding](security.md) | Source and target ACLs, GeoIP, layered quotas, compression and encryption |
| [Admin UI and centralized configuration](management.md) | Admin UI, account roles, API, configuration push |
| [External platform integration](platform.md) | Platform tokens, reverse WSS RPC, signed callbacks, traffic import |
| [Services, updates, logging and monitoring](operations.md) | systemd/launchd/SCM, updates, log rotation, metrics |

## Reference

| Page | Contents |
| --- | --- |
| [Reference](reference.md) | CLI, field quick reference, admin API, example index, numeric limits, glossary |

## Related documentation

- [Architecture](../../architecture.md) — crate ownership boundaries
- [Configuration and deployment](../../configuration.md) — field-level detail
- [Implementation status](../../implementation-status.md) · [Validation](../../validation.md)

The reference documents one level up are kept in Chinese and English.
