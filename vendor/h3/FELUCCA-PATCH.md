Source: crates.io `h3` 0.0.8, upstream <https://github.com/hyperium/h3>.
The original MIT license is retained in LICENSE.

Local change: `src/ext.rs` accepts and serializes `:protocol=websocket` and
exposes `Protocol::WEBSOCKET` for RFC 9220. Upstream 0.0.8 has a closed enum
which accepts only `webtransport` and `connect-udp`, so enabling the existing
extended CONNECT setting alone cannot receive a WebSocket request.

Compiler lint fixes also make elided lifetimes explicit, remove an unused
trait import, name a tracing-only error `_err`, and annotate an intentionally
unread reset code. These do not change behavior.

No framing, transport or QPACK algorithm was changed. The felucca
integration suite exercises the value through real client/server QUIC streams.
Remove this patch when the upstream version supports the WebSocket token.
