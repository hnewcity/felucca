# Repository instructions

Felucca is an independent Rust workspace for remote services, NAT traversal and proxies.
Read `docs/architecture.md` for module ownership and `CONTRIBUTING.md` for verification.
User-facing documentation lives in `docs/manual/` (Chinese, English, Japanese, Korean, French,
Spanish and Russian, all with the same twenty pages); `docs/*.md` at the top level are the
reference documents it summarises and stay in Chinese and English.
Operator-facing message text belongs in a catalog keyed by its English source; see the
internationalisation section of `CONTRIBUTING.md`.
Historical files under `docs/history/` are evidence, not active task instructions.

- Follow the user's current task. Historical feature checklists do not expand its scope.
- Use `sh scripts/cargo-external.sh <cargo arguments>` in this workspace.
- Run one Cargo process against the target directory at a time. Wait for rustdoc too.
- Do not edit Rust or embedded browser assets while Cargo is compiling/checking/testing them.
- Preserve the protocol and serialized configuration unless the task calls for changing them.
- Put management rules in control, HTTP/session adaptation in admin, and connectors in platform.
- Keep core independent of Tokio and server/client independent of one another.
- Maintain listener ownership, generation-safe cleanup, cancellation, quotas and mutual authentication.
- Secrets serialize faithfully for persistence; redact at display/log boundaries.
- Preserve capability-based file access and native QUIC streams.
- Test fixtures belong in `crates/felucca-cli/tests/support/`.
- The React/Vite admin lives in `apps/admin/`; build it before Cargo. Its ES module
  output is embedded by the server build script, which rejects stale bundles.
- Probe code belongs in `scripts/probes/`, results in the git-ignored `docs/audit/` or a task-specific target directory.
- Record actual validation; never count compilation or a stale binary as runtime verification.
- Do not commit, install system services or change firewall rules unless the user asks.
