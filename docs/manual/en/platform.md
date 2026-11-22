# External platform integration

`[node]` lets an external control plane manage this node: it can call the admin API with a
separate token, or the node can dial out and accept requests over a reverse WebSocket.
Configuration changes can also be pushed as signed callbacks, and external traffic can be
imported. This is Felucca's own control protocol; it is not wire-compatible with nps.

## Enabling and scopes

```toml
[node]
id = "edge-a"
state_path = "state/node-events.json"
changes_window = 1024
usage_interval = "10s"

[node.platform.panel]
enabled = true
token = "replace-with-a-random-platform-token"
scope = "account"
user = "office"
mode = "direct"
```

| Field | Values and meaning |
| --- | --- |
| `scope` | `full` (full management; you cannot also set `user`) or `account` (you must set an existing `user`, and you can only manage that user's clients and tunnels and read this tenant's callback state) |
| `mode` | `direct` (the token may access the public admin HTTP API), `reverse` (reverse WS RPC only; using this token directly against the admin HTTP API returns 401), or `dual` (both directions) |
| `enabled = false` | Both entry points stop working and existing reverse connections are closed |

Tokens must be at least 16 bytes, and each platform must use a different one. Over HTTP, send
`Authorization: Bearer <token>`. Platform identities are separate from local admin accounts.
Changing a token, its scope or the connection configuration replaces the reverse connection;
requests on an older connection are still checked against the current configuration.

Public addresses must use HTTPS/WSS; plaintext HTTP/WS is allowed only for literal loopback
addresses. WSS verifies public CAs by default, and you can instead use
`trust = { mode = "pinned", fingerprint = "..." }`. Insecure trust is not allowed. HTTP callbacks
verify public CAs and do not follow redirects.

## Direct management API

Writes use JSON, `If-Match: "<configuration revision>"` and a unique `Idempotency-Key`. Browser
cookie authentication additionally requires an Origin and a CSRF token.

| Endpoint | Behavior |
| --- | --- |
| `GET /api/node/status`, `/api/node/registration` | Node ID, boot ID, persisted epoch, version, event window, and visible platform connection state |
| `GET /api/node/usage` | Client online state, connection counts and cumulative bidirectional bytes within the current scope |
| `GET /api/node/changes?after=<epoch>:<sequence>&limit=100` | The persisted change window; `limit` is 1–512 |
| `GET /api/node/callbacks?target=...&offset=0&limit=100` | Visible callback queues, delivery statistics and paged entries |
| `POST /api/node/callbacks/replay` | `{"ids":["delivery-uuid"]}`; clears the failure state and delivers again |
| `POST /api/node/callbacks/delete` | Deletes only the specified visible entries |
| `POST /api/node/kick` | `{"ids":["client-name"]}`; persistently deactivates and disconnects the clients, blocking immediate reconnects |
| `POST /api/node/sync` | Admin re-activates the local persisted configuration and clears old runtime state, caches and connections |
| `POST /api/node/traffic` | Imports external traffic deltas, accumulating client and owning user counters atomically |
| `GET/PUT/DELETE /api/platforms/<name>` | Admin management of platforms |
| `GET/PUT/DELETE /api/webhooks/<name>` | Admin create, modify and delete of persisted webhooks |

Batch configuration changes, full export and restore reuse `/api/batch` and
`GET/PUT /api/config`. Sync and kick reuse the same configuration transaction and idempotency
history, and increase the configuration revision. Callback actions and traffic import do not
change the configuration revision.

### Traffic import

You must configure `state.path`:

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

A request carries at most 128 items. Byte counts must be unsigned integers, and multiple items
for the same client are merged first. Nothing is written until all permission checks pass, and
if any entry is out of scope or invalid the whole batch is rejected. Import traffic that is
**not already measured by this node's data plane**; re-importing traffic the node already
measured counts it twice. The first import upgrades the snapshot to version 2, which earlier
builds cannot read.

## Reverse WSS protocol v1

Once you enable `[node]`, `GET /api/ws` accepts an HTTP/1.1 WebSocket upgrade. Native clients
use a Bearer token. A browser can use its same-origin login cookie, or pass the token in the
subprotocol `["felucca-node-v1", "felucca-token.<unpadded base64url token>"]`. The server only
selects `felucca-node-v1` and does not accept authentication through URL parameters. It keeps at
most 16 direct admin WebSockets at the same time.

The node dials `reverse_url`, and the handshake carries the platform Bearer token and the
`felucca-node-v1` subprotocol. The platform must select that subprotocol in its response. All
application messages are JSON text.

The node sends `hello` first (node, boot, epoch, cursor, max_message, max_inflight,
heartbeat_ms), and the platform then sends requests:

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

Responses look like
`{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}`. RPC reuses the
authentication, tenant filtering, CAS and idempotency behavior of the normal admin handlers. You
may pass If-Match, Idempotency-Key, Accept and X-Felucca-CSRF, but you cannot override Host or
Authorization. Login, logout, SSE, CPU sampling and the WebSocket upgrade itself do not go
through RPC.

| Limit | Value |
| --- | --- |
| Maximum message | 2 MiB |
| Concurrent RPC per connection | 2 |
| Concurrent RPC node-wide | 16; excess requests return 429 |
| Repeated in-flight request ID | The connection is closed |
| Request timeout | 504; retry writes with the same idempotency key |

Either side may send `{"type":"ping","nonce":1}`. The node does not overwrite an unanswered ping,
and it closes and reconnects if no matching pong arrives within more than two heartbeat periods,
backing off by 1–30 seconds. Configuration changes send an event, and a resync is sent when a
subscription falls behind. Once you set `usage_interval`, the node sends tenant-filtered usage
messages periodically.

## Signed callbacks

```toml
[node.webhook.events]
user = "office"

[node.webhook.events.delivery]
enabled = true
url = "https://panel.example/felucca/events"
signing_key = "replace-with-a-random-callback-signing-key"
timeout = "5s"
retry_max = 2
retry_backoff = "2s"
queue_max = 100
```

Request headers:

| Header | Contents |
| --- | --- |
| `X-Felucca-Event-Id` | The delivery UUID; it stays the same across automatic retries and manual replays |
| `X-Felucca-Timestamp` | The Unix time in milliseconds for this request |
| `X-Felucca-Signature` | `v1=` followed by the lowercase hex HMAC-SHA256 |

The signing input is the UTF-8 prefix concatenated directly with the **raw request body bytes**:

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

The receiver should verify the signature, check the time window and deduplicate by event ID.
Only 2xx counts as success. `retry_max` is the number of extra attempts beyond the first
delivery, and `retry_backoff` is a fixed interval. Delivery is **at least once**: if the node
exits before a successful acknowledgement is written to disk, a restart may deliver the same ID
again. Callbacks carry change metadata only, not configuration secrets; after receiving an event,
fetch the resources you need with the currently authorized API.

`queue_max` limits the persisted pending and failed entries per target. When the limit is
exceeded, the oldest entries are dropped and counted as dropped; `0` keeps no final failed
entries. The node-wide maximum is 4096 entries. Disabling a subscription keeps its existing
queue. Deleting the subscription, or changing the receiving URL, signing key, filters or
templates, clears the old queue.

The event window records `node.started`, `config.changed`, `config.resync`, and create, update
and delete metadata for clients, users, centralized tunnels and domains. Queries are filtered by
tenant. An expired, too-far-ahead or epoch-mismatched cursor returns 410/reset_required; in that
case fetch the current configuration first and then resume the subscription from the returned
position. This bounded window does not keep full configuration diffs and is not a long-term audit
log.

## Realtime subscriptions and templates

Both direct and reverse WebSocket connections can use these **WS RPC only** paths:
`GET /api/realtime/subscriptions`, and `GET/PUT/DELETE /api/realtime/subscriptions/<name>`.
Subscriptions do not modify persisted configuration and do not require If-Match. Each connection
allows at most 32 subscriptions of at most 32 KiB each, and all of them are released when the
connection drops.

```json
{
  "enabled": true,
  "selector": {"event_names":["client.updated"],"clients":["office"]},
  "content": {
    "body_template": {"message":"node {{event.client}} updated","event":"{{event}}"},
    "header_templates": {"x-event-name":"{{event.kind}}"}
  }
}
```

Templates are JSON values and run no scripts. They support `{{node}}`, `{{id}}`, `{{epoch}}`,
`{{event}}`, and `{{event.kind}}`, resource, action, user, client, tunnel, host, target,
sequence, revision, timestamp_ms and traffic. When a whole string is a single placeholder, the
original JSON type is preserved. Templates cannot override authentication, cookies, HTTP
length/connection headers or the `X-Felucca-*` signature headers.

## Traffic notifications

`traffic_report_interval` (100ms–1h) and `traffic_report_step_bytes` enable notifications for
imported traffic. A `client.traffic.reported` notification is sent on the first import for a
client that has a traffic quota, and afterwards when the time or byte threshold is reached. The
notification carries `ephemeral: true`, so it does not enter the changes history, the failure
queue or retries. Slow receivers may miss it; read `/api/node/usage` when you need the complete
data.

## Verification checklist

1. `GET /api/node/status` returns the node ID, epoch and platform connection state.
2. A reverse WSS connection completes a `hello` → `request` → `response` round trip.
3. The callback receiver verifies signatures successfully, and replaying the same event ID
   produces no duplicate business effect.
4. After a `kick`, the client is disconnected and cannot reconnect immediately.
5. After a traffic import, the cumulative bytes in `/api/node/usage` change immediately.

## Next steps

- [Admin UI and centralized configuration](management.md)
- [The felucca-server module](server.md)
- Examples: [node-server.toml](../../../examples/node-server.toml)
