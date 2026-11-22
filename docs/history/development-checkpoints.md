> 历史记录：保留原始开发背景与当时的验证结果，不作为当前开发指令或版本状态。当前架构见 `docs/architecture.md`，验证状态见 `docs/validation.md`。

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

felucca is a NAT-traversal and reverse-proxy system in Rust (2024 edition, tokio), a
from-scratch reimplementation of the feature set of Go [nps](https://github.com/djylb/nps).
A local checkout of the Go source is useful as a behaviour reference,
but **there is no wire compatibility** — a felucca server pairs only with a felucca
agent. Confirmed with the project owner; don't reintroduce Go framing to "stay
compatible".

`docs/port/architecture.md` holds the design decisions and the first-milestone scope.

## Commands

```sh
cargo build --release              # felucca-server + felucca-agent binaries
cargo test --workspace             # unit + transport + end-to-end suites
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

cargo test -p felucca-mux                       # one crate
cargo test -p felucca-mux --lib credit::        # one module's tests
cargo test -p felucca-cli --test end_to_end     # the integration suite
cargo test -p felucca-mux --test session        # mux integration tests

target/release/felucca-server --config examples/felucca-server.toml --check
target/release/felucca-server --config examples/felucca-server.toml --fingerprint
target/release/felucca-agent  --config examples/felucca-agent.toml  --check
```

`--check` validates config and exits without binding. `--fingerprint` prints the
configured bridge cert's SHA-256, which is the value an agent pins for a self-signed bridge.
It requires persistent `bridge.tls` PEM files; ephemeral certificates change on restart.
`RUST_LOG` overrides the configured log level.

The local Homebrew toolchain is Rust 1.98.1 using LLVM 22.1.8. It works without
DYLD_LIBRARY_PATH overrides. The workspace minimum Rust version is 1.88 (rcgen
and let-chain syntax). Local socket tests require network permissions in a sandbox.
The internal disk filled during an earlier build. Use `sh scripts/cargo-external.sh <cargo args>` for
Cargo commands; this sets CARGO_HOME and TMPDIR under the external target directory.
The existing registry was copied there without deleting the original.
Do not run multiple Cargo processes against one target directory concurrently.

## Crate graph

Dependencies flow strictly one way:

```
core  ->  proto  ->  mux
      \         \->  transport  ->  server  ->  cli
       \                        \-> client  ->/
```

| crate | responsibility |
|---|---|
| `felucca-core` | config model, errors, ids, ACL, metering, rate limiting, `Secret` |
| `felucca-proto` | control messages, handshake, length-delimited codec, **both** handshake halves |
| `felucca-mux` | credit-based multiplexer over any reliable ordered duplex stream |
| `felucca-transport` | tcp / tls / ws / quic listeners and connectors, protocol sniffing |
| `felucca-server` | tunnel registry, per-tunnel listeners, metered splicing |
| `felucca-service` | shared backend pool, dialing, probes and confined files |
| `felucca-client` | agent control loop and bridge dialing |
| `felucca-cli` | the two binaries plus the end-to-end tests |

`felucca-core` is deliberately runtime-free: `RateLimiter` returns an `Option<Duration>`
rather than sleeping, so nothing in core needs tokio.

The handshake **driver** (`proto::session::{accept, connect}`) lives in `felucca-proto`,
not `felucca-server`, because `felucca-client` needs the agent half and cannot depend on
the server.

## Load-bearing design decisions

These cost real debugging to arrive at. Changing them needs a reason.

**QUIC does not carry mux.** `TransportKind::needs_mux()` is false only for Quic; QUIC
already multiplexes with its own flow control. Stacking mux on top gives two credit
systems in series plus head-of-line blocking, which negates the reason to use QUIC.

**The mux outbound channel holds budget permits.** DATA uses a byte semaphore and
control frames use a separate count budget. Control-budget overflow terminates the
session; never silently drop a window update. Credit returns cannot exceed reserved
outstanding bytes. Receive advertisements and dynamic growth share a runtime budget.
Tiny received frames are coalesced into segment-sized chunks.

**The agent picks the backend.** `StreamRequest::Routed` carries the tunnel id and
original source IP (for ACL and source hashing). A dynamic target is only allowed
for SOCKS5/HTTP proxy modes. Legacy target-only requests are rejected. Protocol v6
requires both ends to upgrade together.

**`Role` decides stream-id parity** — `Client` odd (from 1), `Server` even (from 2) — so
both ends can open streams concurrently without a negotiation round trip.

**OPEN/ACK negotiate receive windows.** An opener may send one 16 KiB segment
optimistically, so small initial requests do not wait for a round trip. ACK advertises
the peer's actual window and makes `open_timeout` meaningful. `WindowGrow` is distinct
from returned credit. This supersedes the original no-ACK design because different
window settings and unacknowledged opens were audit findings.

**`Stream`'s `Drop` sends `CloseReason::Normal`, not `Cancelled`.** Write-then-drop is the
ordinary Rust idiom and means what TCP `close()` means.

**The release profile omits `panic = "abort"` on purpose**, so a panicking connection task
cannot take the whole daemon with it.

**Frame sizes are ours, not Go's**: mux header 9 bytes, `MAX_SEGMENT` 16 KiB (Go: 13 /
4085), control-plane `MAX_FRAME_LEN` 1 MiB.

## Security-relevant invariants

- The handshake is **mutual**. The agent verifies `AuthResult::Ok { server_mac, .. }`;
  without it a MITM could accept the connection, answer OK without knowing the vkey and
  read the whole tunnel. `Direction::{ClientProof, ServerProof}` is mixed into the
  transcript so the client's own proof can't be reflected back at it.
- `accept` verifies against a placeholder key on an unknown `key_id`, so response timing
  cannot enumerate valid ids.
- `Verification::Pinned` still delegates signature verification to rustls. Pinning changes
  *which* certificate is accepted, never *whether* the peer proved it holds the key.
- Always fetch the rustls provider via the local `provider()` helper.
  `CryptoProvider::get_default()` returns `None` until something builds a config, so an
  agent that only ever builds client configs would fail at startup without the explicit
  `install_default`.
- `Secret` redacts in `Debug` and `Display` but its `Serialize` writes the real value —
  redacting there would destroy a user's key the first time a config was rewritten.

## Config conventions

Every config struct carries `#[serde(deny_unknown_fields)]`. A typo like `reconect_max`
must be an error, not a silently defaulted field; keep this on new structs.

Two distinct bind parsers, and picking the wrong one is a real bug this repo already hit:
- `parse_bind` — a bare port or a full `host:port`.
- `parse_bind_host(host, port)` — a bare host plus a separately configured port. Rejects
  a host that carries its own port (two sources of truth) and rejects hostnames
  (ambiguous resolution).

Validation actively rejects footguns rather than accepting them: zero `reconnect_min`
(busy loop), inverted backoff, backends on target-choosing tunnel modes, zero weights,
health-checked UDP tunnels, health timeout ≥ interval, ACL allow-lists that are empty
(would deny everything), CIDR prefixes with host bits set. Follow that pattern.

Traffic-counter JSON field names are `inlet_bytes` / `export_bytes` to match Go nps, so a
migrated deployment reports identically. Don't "fix" them.

**Test gotcha:** in TOML, bare keys written after a `[client.agent-1]` header land *inside*
that table. A server-config test that puts top-level keys after a table section will fail
for the wrong reason.

## Naming collision to watch

`felucca_transport::Tunnel` (a transport connection with its handle) and
`felucca_server::Tunnel` (a registered forwarding entry) are different types. Import one
and qualify the other.

## Current state

All six bridge transports, mux, TCP/UDP, proxy/site multi-account authentication,
SOCKS5 CONNECT/UDP ASSOCIATE, HTTP/HTTPS routing and rewriting, SNI passthrough and
offload, HTTP/2 front/backends and pooling, HTTP/3 ingress, static certificate reload, ACME
HTTP-01/TLS-ALPN-01/EAB/ZeroSSL API, GeoIP/GeoSite, global/user/client/tunnel source ACLs,
user/client/tunnel/host metering and limits, atomic traffic snapshots, batch mappings
and PROXY v1/v2 are implemented. This is NOT complete nps parity. The user authorized
ALL remaining audit items, including Web admin, P2P and KCP. Only Go wire
compatibility is excluded. Do not ask for priorities or treat an intermediate
test run as completion. Track current status in docs/implementation-status.md;
docs/nps-parity-audit.md preserves the historical baseline.

Shared TCP ingress uses felucca/6 ALPN for direct TLS/WSS agents and bounded HTTP
classification for WS. WSS through standard HTTP reverse proxies uses the reserved
upgrade path with http/1.1 ALPN. PROXY headers are accepted only from configured
trusted peers; source ACLs run after that validation. HTTP pooling additionally
keys on source port when emitting PROXY headers. ACME challenge certificates
require a dedicated signer resolver because webpki rejects the required critical
extension even during a key consistency check; compare SPKI explicitly instead.

The audit reproduced and fixed public TCP RST leaking a backend/connection slot
(copy uses try_join: preserve clean EOF, cancel on errors) and u32 weighted cursor
wrap (now u64). Keep their regression tests. The audit baseline was 358 tests;
the latest KCP/FEC regression passed 480 tests, clippy and fmt.
Current v5 release passed all 16 example checks, real process persistence, and
lossy KCP with 10+3 FEC, encrypted/compressed 2 MiB TCP and 50 KB UDP.
See docs/audit/kcp-runtime.json. Server local routes are the next work in progress.

ACME tests use a local authority and real HTTP-01 requests/CSR signatures, not a
public CA. SOCKS5 does not support BIND. Admin UI and P2P remain pending.
Quota cutoff checks each block/datagram and periodically ends idle relays, with
in-flight overshoot. `[state]` enables atomic traffic snapshots across server restarts.
Stable explicit tunnel ids preserve attribution when config order changes.
CLI shutdown drains connections before saving the final snapshot. Runtime
configuration persistence is implemented in Daemon/config_store.
File mode serves HTTP/WebDAV over a virtual agent stream. With hosts it registers
as an HTTP route; otherwise as TCP. Private FileConfig is never sent to the server.
Agent::new now returns Result and initializes the directory before connecting.
ConfinedFs uses cap-std for every path operation; do not replace it with
canonicalize-then-open. COPY exclusively creates its destination after the DAV
handler has removed an overwrite target, preventing raced source aliases from
being truncated. File service authentication is agent-side under file.auth.

WebSocket conversion supports HTTP/1, HTTP/2 and HTTP/3 ingress to HTTP/1 or
HTTP/2 backends. The HTTP/1 key/accept challenge is validated separately on each
hop. H2/H3 use extended CONNECT, successful status and stream data instead.
`vendor/h3` adds the WebSocket protocol token; `vendor/h3-quinn` fixes cancellation
of a pending receive (the original moved the stream out and then unwrapped None).
See each FELUCCA-PATCH.md. Shared bridge QUIC/HTTP3 UDP uses ALPN and distinct
certificate resolvers; bridge reload must preserve HTTP3 in the endpoint config.

`Pool::select` returns a backend even when every member is marked down, on purpose.
Backend leases live for the entire proxied connection. Registration replacement
waits for old listeners to stop, and late cleanup cannot remove a newer session.
Health probes and per-session tasks are cancelled by their owning JoinSets.

Network provides per-agent DNS caching, source IP binding and bridge-only
HTTP CONNECT/SOCKS5 proxies. Backends and probes resolve through the same network,
apply target ACLs to the actual addresses, and dial without re-resolving.
NTP calibrates only bridge authentication time, never OS/TLS/quota time.

PayloadStream adds per-route Snappy blocks and optional ChaCha20-Poly1305.
Fresh per-stream nonces and mutual HMAC bind the exact Routed request and flags;
HKDF derives directional keys. Authenticate length headers and FIN; reject
replay, truncation and decompression above 16 KiB. Raw routes allocate no record
state. Meter plaintext and wrap PROXY headers as payload. It buffers at most one
record each direction; callers flush/shutdown normally. Never put another mux
over QUIC. Protocol v7 includes PayloadOptions in registration.
Do not cancel a partially read UDP length-prefixed frame just because another
direction becomes ready: run the two copy directions as long-lived futures.

Nothing is committed yet — the whole tree is untracked, and no commit was requested.

KCP uses a bounded owned driver over connected UDP, then mux. On macOS use send
for connected sockets, not send_to (EISCONN). Drop drains accepted writes for up
to 5s instead of aborting; the listener owns and cancels all drivers. FEC raw
data is delivered immediately, parity does not delay a partial group. Bound
reconstruction cache and deduplicate completed groups before allocating shards.

The local/nested + Daemon v6 baseline passed 499 tests, fmt and clippy.
On-disk release is v6, with real state/local/KCP probes in docs/audit.
Startup signals are registered before Daemon::start: the real local probe caught
SIGTERM arriving before the old lazy signal future was first polled.
Local groups use reserved local.NAME keys and bounded duplex channels, no embedded
agent or extra mux. RouteTrace carries source and caps at 16 pairs; HTTP pool keys
include it. SessionGuard retires canceled authenticated sessions.

Source protocol is now v7. ManagedConfig uses canonical JSON inside a redacted
Secret control envelope. Both agent opt-in and an encrypted server-side bridge
are required. Operator target ACLs intersect all remote route and health ACLs.
Remote file names resolve under preopened RemoteAuthority directory capabilities;
never replace this with canonicalize-then-open or allow remote write escalation.
Remote editing passed 6 end-to-end tests after the 501-test full baseline; clippy passed.

Admin API/UI passed 513 full tests, 5 final admin tests, clippy and fmt.
Release v7 built; real Chrome + state/local/KCP probes passed. Results in docs/audit.
Source is now v8 with private tunnels/secret visitors; visitor tests are running.
The old target/tmp/visitor-stage has already been promoted: DO NOT re-copy it.
Daemon owns a management listener separate from forwarding generations. Apply
prepares replacement/rollback, drains old listeners, atomically commits config
and a bounded operation history. CAS and account-scoped idempotency keys prevent
double application, including DELETE retries after restart. ConfigStore uses an
exclusive lock, 0600 snapshots and atomic rename; preserve traffic/bridge identity.
bridge://web now reaches authenticated Admin; bridge://http is the old reverse
proxy portal. CLI adds --hash-password (stdin) and --generate-token.
The current release is v7; do not claim v8 visitors passed release checks yet.
P2P/secret/transparent mode, multinode/reverse WS/callback and ops remain authorized
and unfinished. Nothing is committed; no commit was requested.

V8 visitor source passed 526 tests (visitor-workspace-final.log), clippy and fmt.
Private/public routes, shares API/UI, disabled clients, TCP/UDP and local SOCKS5
visitors are integrated. Shared SOCKS parsers live in felucca-service/socks.rs.
Socks UDP is terminated locally; every target uses a signed dynamic share request,
provider destination ACL, both client quotas/meters. Proxy accounts cannot be bypassed.
Visitor queues share 8MiB bytes; RAII generation cancellation tears down local proxy
controls on bridge disconnect. Debug real-process and Chrome probes passed.
Release v8 is building: DO NOT edit workspace code until that build ends.
Transparent TCP is staged in target/tmp/transparent-stage, NOT promoted.
original-hashes.json must match before promotion. Stage includes protocol v9,
Linux socket2 original-destination/TPROXY, Apple/FreeBSD PF bounded ioctl adapter.
PF unsafe is restricted to one documented syscall; native smoke test does not imply
actual firewall redirect validation. Do not alter system firewall rules.

V8 release built and verified: admin Chrome, visitor process, state/local/KCP.
Release server 23,494,160 bytes SHA256 38adb41d9bc08f610a3a08a6f5d360463a2e549d1196505be32defc227752451;
agent 11,497,232 SHA256 434562bc7bd9fe9887db0ccb16c783b24958d7634621242ea056310dc19d7b20.
Transparent stage HAS NOW BEEN PROMOTED; do not recopy old stage files.
Source protocol v9 and direct TLS ALPN felucca/9. Compile passed, clippy running.
Tests must freeze source until docs tests finish (editing during a previous test
run caused a source/rlib mismatch at rustdoc, then a clean full v8 rerun passed).
No actual firewall/NAT changes or Linux runtime test have been performed.

Current v9 source includes completed logging rotation, authenticated Prometheus,
optional CPU profiling and UI monitoring. Full 535 tests passed before profiling.
Real pprof busy-worker test passed: opt-in/RBAC, bounded concurrent sample,
cancel-on-daemon-stop and semaphore reuse. pprof 0.14.1 Linux/macOS only.
CLI maintenance added systemd/launchd/Windows SCM, service render/dry-run,
atomic SHA256-verified update via self-replace. Native maintenance subprocess tests
passed, including macOS plutil and a replacement on an isolated copy, never project
binaries. SHA256 switched to existing ring implementation for debug performance;
all-target clippy passes. FULL TESTS currently running: maintenance-workspace.log.
Freeze workspace Rust files until all test and rustdoc processes finish.
Stage target/tmp/maintenance-stage HAS BEEN PROMOTED and is now stale; never recopy.
New Dockerfile, check.yml workflow and package.py are unexecuted on external
platforms. --build-info reports actual program/version/protocol/build target.
Release strip changed to debuginfo (retain symbols for profiler); on-disk release
STILL verified v8. Need build v9 release and rerun process/UI/packaging probes.
P2P, multinode, reverse WS and callback remain authorized and NOT IMPLEMENTED.
Continue autonomously; no user questions or final partial milestone handoff.

Full v9 maintenance regression now PASSED 539 tests, clippy/fmt clean.
maintenance-workspace-final.log includes all rustdoc. Initial full run had one
KCP impairment test timeout: held reorder packet had no time bound, trapping
last congestion-window ACK. Added 10ms max reorder hold; loss/dupe rules and
20s deadline unchanged. Isolated 17.11s old -> 4.27s new, full rerun passed.
V9 RELEASE BUILD RUNNING session 38370 (maintenance-release.log), jobs=2.
DO NOT edit workspace Rust/embedded UI until it completes. P2P code is staged
only in target/tmp/p2p-stage/rendezvous.rs and not wired/tested yet.
New admin browser probe tests monitor metrics and actual CPU SVG download.

During v9 release build, debug real Chrome probe passed monitor metrics + actual
SVG download, shares, CRUD, restart, zero JS errors. Results separately under
target/tmp/v9-browser-debug (not overwriting audited v8 release results). Monitor
screenshot visually inspected. Debug package.py generated archive and actual v9
manifest; target/tmp/v9-debug-dist. Log process probe passed 23 JSON events, two
archives, exclusive writer and SIGTERM 0. Its initial fixture needed explicit
100ms dial_timeout to produce events; product logging code was unchanged.
P2P staged rendezvous.rs/peer.rs passed 4 standalone rustc smoke tests with cached
debug dependencies: cookie/auth/expiry/budget and observed socket -> pinned QUIC
2MiB halfclose. They are NOT in workspace or counted in 539 tests yet.


## Continuation checkpoint: P2P v10 integration (active work)

User continues to authorize ALL remaining nps parity work; do not stop at this milestone or ask priorities.
No commits, actual system-service installation, Docker runs or public CA/NAT deployment performed.
Toolchain rust/cargo1.98.1 works; external cache script scripts/cargo-external.sh. No concurrent Cargo/build mutations.

v9 release completed and all actual Chrome/state/local/visitor/KCP/logging/package probes passed.
Server SHA256 f363281c730eab6d1b2b50cb94762f0813f8d5b1155b8409c7f78c943719e04f (30,108,200bytes).
Agent SHA256 485cca6b250052aea307a600b55fab263ab8cfe67c3300fe39ee08c44f0181f9 (14,671,208bytes).
Archive dist/felucca-0.1.0-x86_64-apple-darwin-release.tar.gz verified by extraction and executing build-info, audit/package-runtime.json.
Docs validation/implementation-status corrected from stale v8/building wording.

P2P source now integrated in workspace, protocol/min10 and bridge ALPN felucca/10 (release on disk remains v9).
core/config/peer.rs fields added to ServerConfig/AgentConfig/ShareConfig/VisitorConfig; default relay and opt-ins on broker/provider/share.
transport/rendezvous.rs authenticated cookie-based UDP observations and peer.rs same-socket pinned TLS1.3QUIC/hole-punch.
proto/peer.rs ticket mutual HMAC, exact route, direction/sequence checked CreditLedger; visitor proofs bind direct policy and full offer.
client/peer.rs + server/peer.rs implement prepare/observe/connect/commit, plaintext per-block grants/commits, FIN drain and fallback before app data.
Agent dispatcher reads request once, Services shared serve_request/serve_direct reuse backend policies/LB/file caps, provider slots released before Finished ack.
Client secure bridge provenance prevents direct ticket disclosure over plain transports. Broker takes/drops listener on shutdown.
Services emits authenticated TCP PROXY header itself, excluded from meters; unsupported direct entrypoints relay or fail according to visitor policy.
Visitor framing pumps remain live through setup timeout, park at Abort/Rejected boundary before reusing the stream for relay.
Control disconnect cancels direct tasks; counted grant only becomes traffic on matching Commit; last unreported chunk on crash documented.

11 focused direct e2e tests pass: target/tmp/p2p-e2e-fourth.log (4.61s). Includes 2MiB raw+FLP half-close, UDPempty50k,
pre/post-offer fallback, file auth/readonly/capability, dynamic resolved-IP ACL/proxy auth, per-side quota/rate, cap1 reuse,
RST/provider disconnect/UDP listener release, and two simulated port-filtered NAT mappings carrying 2MiB pinned QUIC.
Initial UDP fixture used native socket default tiny macOS send buffer; fixed fixture to existing transport::udp::bind, no product change needed.
Last small fixes: broker max_pending returns relay eligibility instead of hard failure; relay-only visitor keeps original dial timeout.
One extra proto offer-tampering test added. UI share checkbox direct, examples/p2p-{server,provider,visitor}.toml, docs/p2p.md added.

ONGOING verification session49652: fmt -> clippy(alltargets denywarnings) -> test --workspace --locked.
Clippy passed (target/tmp/p2p-clippy-final.log). Workspace log target/tmp/p2p-workspace-final.log still running at this checkpoint.
Expected count558 if all pass; count actual result, wait INCLUDING rustdoc before source changes or release build.
No v10 release built yet. Need native P2P actual-process probe and v10 browser/regression checks/package after release.

Next authorized work: multi-platform node control/reverse WS/callback persisted retries/history/replay/webhook CRUD, then full nps re-audit.
Independent staging target/tmp/node-stage/design.md and config.rs created during source freeze; NOT promoted, compiled or accepted.
Reference nps web/service/node_{control,protocol,runtime,platform}.go + docs/reference/server-config-node.md.
Plan uses existing Admin/Daemon authority & CAS/idempotency, reserved platform principals and current-config reauth, TLS reverse WS,
bounded signed callback journal and scoped metadata; details in staging design. Do not confuse config skeleton with completed node functionality.


## Latest checkpoint: node control implemented; native release building

P2P v10 completed 558 tests + release process probes. Source now additionally includes node/platform control.
Node modules config/node.rs and server/admin/node/{mod,api,store,ws,callback,operations,reports}.rs are LIVE, not staging.
Ignore stale target/tmp/node-stage files. Platform token direct/reverse/dual + scoped RBAC, pinned WSS, bounded RPC,
heartbeat/reconnect, signed callbacks, 0600 atomic journal/epoch/history, queues+pagination+run generations,
manual replay/delete; traffic snapshot v2 imports counters+idempotency atomically, v1 still used without imports;
persistent kick disables clients, sync reuses Daemon activation, ephemeral threshold traffic reports; UI + examples/node-server.toml.
8 node e2e + 5 persistence tests + 3 config tests. Full workspace574 passed with rustdoc
(target/tmp/node-workspace-first.log), clippy clean (target/tmp/node-clippy-final.log), fmt clean.
Real debug process + Chrome probe passed (target/tmp/node-process-debug.log and target/tmp/node-debug-audit/node-runtime.json):
WSS, platform scopes, signed callbacks/restart/replay, independent actor traffic imports, running-agent kick, webhook UI, 0 JS errors, SIGTERM[0,0,0].
New script docs/audit/probe-node.py, docs/node.md and README updated.
Release build session1366 ONGOING, log target/tmp/node-release.log. Never edit embedded UI/Rust while Cargo runs.
Next: await release; run node/P2P/visitor/admin/state/local/KCP/logging probes against release; persist actual results
(probe-state.py only prints, must explicitly write docs/audit/state-runtime.json! old file still had22examples).
Then package + verify extract hashes/execution, update docs/validation and implementation-status final release data.
No commits. Keep working on all previously authorized missing items; don't ask user priorities.
