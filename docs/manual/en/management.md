# Admin UI and centralized configuration

`felucca-server` includes device, service, private access and configuration management. Daily device, service and access changes sync online while unchanged services keep running. Infrastructure changes such as global listeners still reload forwarding; the admin API stays available.

## Get connected without editing configuration

1. Open **Devices → Connect device**, enter a name, confirm the server address and select the target system and processor. Download and extract the client package, then open `Connect.command` (macOS) or `Connect.cmd` (Windows). On Linux run `sh connect.sh` inside the extracted folder. The program and connection are included; no separate binary or file editing is needed. Online status follows the real connection.
2. Open **Services → Add service**. Choose the device and service type, then enter the address of the application on that device. Choose public access for a server port/domain, or private access for authorized receiving devices. Names, device keys, service IDs and sharing secrets are handled by the console.
3. Private receiving devices also use their own client package and the displayed local access address. To run after sign-in, first close any existing connection window, then run `Install.command`, `Install.cmd` or `sh install.sh` from the package. It saves the client to a personal directory; no service definition needs to be written.

The main navigation contains Overview, Devices, Services, Private access, Monitor and Settings. Settings holds accounts, quotas, diagnostics, server listeners and external integrations. Routine tasks and advanced options use controls rather than a JSON editor; downloaded files and backups remain machine-readable configurations.

Direct connections can be enabled in Settings. Receiving devices prefer direct connections when the server is configured for them, with server relay as fallback; otherwise the server uses relay. This does not create a system-wide VPN. File access must be authorized by selecting an existing folder during device connection. The connection file contains credentials: keep it private. Replacing an existing receiving device requires its original file or a new private connection. Device, service and access edits do not restart all forwarding. Changed or disabled services end their own connections. Unchanged services and device/service renaming keep connections. Older agents reconnect only the affected device. Bridge, HTTP, P2P listener and other infrastructure changes still reload forwarding.

Only client builds present on the server are offered. macOS builds without Apple notarization are identified and may be blocked after a normal download; ask the administrator for a release build instead of disabling system protection.

## Enabling

```sh
target/release/felucca-server --hash-password      # read one password line from stdin; >=12 bytes
target/release/felucca-server --generate-token     # Bearer token and its stored hash
```

```toml
[admin]
bind = "127.0.0.1:8088"
state_path = "state/runtime-config.json"
profiling = true
# required for non-loopback binds: the origin the browser uses; use https://
# unless you accept plaintext management traffic (then http://host:port)
# origin = "https://admin.example.com"

[admin.account.root]
role = "admin"
password_hash = "pbkdf2-sha256$600000$..."
```

Configuration files can be TOML or JSON. The example uses the admin address
`http://127.0.0.1:8088` and the account `root`. The example password is for a local demo
only; replace it with your own hash in real deployments.

## Accounts and roles

| `role` | Permissions |
| --- | --- |
| `admin` | Manage all resources and accounts; no `user` field is set |
| `operator` | Manage the nodes and centralized tunnels of the assigned `user` |
| `viewer` | Read-only access to the assigned `user`'s resources |

Operator and viewer accounts must reference an existing user. Server-local services can reach
local directories, so only an admin may modify them. User quotas, admin accounts and the full
configuration are also admin-only. Keep at least one enabled admin account.
`disabled = true` deactivates an account.

Passwords use PBKDF2-SHA256 with a random salt and a fixed 600000 iterations. Sessions are
kept in memory. Cookies are HttpOnly and SameSite=Strict, and are also marked Secure when you
configure HTTPS. Browser writes require a same-origin Origin and a CSRF token. Changing a
password, disabling an account or deleting an account invalidates old sessions, and a process
restart also invalidates cookie sessions.

For automation, use a Bearer token: copy the `token_hash` printed by `--generate-token` into
the account's `token_hash`, and send the raw token with each call. Bearer requests do not need
the cookie CSRF token, but the same account permission checks still apply. **Never write the
raw token into the server configuration.**

## Write transactions

Every write requires two headers:

```text
If-Match: "current configuration revision"
Idempotency-Key: unique identifier for this operation
```

- A stale revision returns 409.
- The same ID with the same request returns the previous result and does **not** restart the
  data plane again; the same ID with a different request returns 400.
- The most recent 256 successful operations are stored with the configuration, so retries of
  operations such as DELETE are still recognized after a restart.
- `durable = false` means directory synchronization has not been confirmed yet.
- Device, service and access edits do not restart all forwarding. Changed or disabled services end their own connections. Unchanged services and device/service renaming keep connections. Older agents reconnect only the affected device. Bridge, HTTP, P2P listener and other infrastructure changes still reload forwarding.

Changing the admin bind address, certificate file paths, public origin or state path requires a
process restart. You cannot change them while the server is running.

## UI pages

| Page | Contents |
| --- | --- |
| Overview | Device, service and connection status visible to this account. |
| Devices | Your connected computers and servers. Choose a device to manage its services. |
| Services | Choose a device and the application you want to access. Felucca connects it to the address you choose. |
| Private access | Choose a private service and generate a connection file for another device. No public service port is needed. |
| Monitoring | Inspect host metrics, connection counts and cumulative traffic. |
| Settings | Server connections, access management and integrations. Everyday device and service tasks stay in the main navigation. |

## API

The API speaks JSON, accepts request bodies up to 2 MiB, and rejects cross-origin browser
requests. Reads normally return `{revision, config}`, and configuration reads redact secrets by
default. When you edit, an existing `<redacted>` placeholder keeps the current value of that
field; credentials for new resources cannot use this placeholder.

| Path | Method and purpose |
| --- | --- |
| `/api/login` | POST `{account, password}`; Origin is required |
| `/api/session` | GET the current identity and CSRF token |
| `/api/logout` | POST to revoke the cookie session |
| `/api/overview` | GET online nodes, connections, traffic and registered tunnels |
| `/api/users`, `/api/users/<id>` | GET the list or a single entry; PUT and DELETE on a single entry |
| `/api/clients`, `/api/clients/<id>` | GET the list or a single entry; PUT and DELETE on a single entry |
| `/api/managed/<client>` | GET, PUT the whole centralized configuration; DELETE restores the local configuration |
| `/api/managed/<client>/tunnels/<id>` | GET, PUT, DELETE a single tunnel |
| `/api/locals`, `/api/locals/<id>` | GET the list or a single entry; PUT and DELETE on a single entry |
| `/api/locals/<group>/tunnels/<id>` | PUT, DELETE a server-local tunnel |
| `/api/shares`, `/api/shares/<name>` | GET, PUT, DELETE a private share |
| `/api/accounts`, `/api/accounts/<id>` | GET the list; PUT and DELETE on a single entry |
| `/api/config` | GET the redacted full configuration; PUT to import a full configuration |
| `/api/config?secrets=1` | Admin-only explicit export including credentials |
| `/api/validate` | POST to validate a full configuration without starting listeners |
| `/api/batch` | POST an atomic batch of changes, at most 128 operations |
| `/api/events` | GET SSE for configuration revision changes, with a 10 second heartbeat and session re-check |
| `/metrics`, `/api/metrics` | GET Prometheus metrics, filtered by account permissions |
| `/api/profile` | POST to download a time-limited CPU flame graph |

A batch uses one revision and one operation ID. All operations are validated before anything is
applied, and if any operation is out of scope or the configuration is invalid, none of the
preceding operations are committed:

```json
{
  "operations": [
    {
      "method": "PUT",
      "path": "/api/managed/office/tunnels/1",
      "value": {
        "id": 1,
        "enabled": false,
        "mode": "tcp",
        "port": 18080,
        "backend": [{"addr": "127.0.0.1:3000"}]
      }
    }
  ]
}
```

When you `PUT` an account you can send a `password`, and the server generates the hash. If you
omit it, the existing `password_hash` is kept.

## Centralized configuration

The server stores the configuration that agents must run under `[managed.<client-id>]`:

```toml
[[managed.office.tunnel]]
id = 1
mode = "tcp"
port = 18080
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
```

An agent must explicitly enable `[remote] enabled = true`, and the main connection must use
authenticated TLS/WSS/QUIC. The server also checks that the actual bridge is encrypted, so
plaintext connections cannot request configuration. Pushes use canonical JSON with a digest,
and credentials in the configuration do not appear in control message debug logs.

The admin plane can change tunnel contents only. It cannot push new agent identities, exit
proxies or server-local authorization boundaries. File services must have their named directory
capabilities configured in advance; see [File services and WebDAV](file-services.md).

The exception: once an agent sets `remote.console_scope = true`, the `target_policy` in `[managed.<client-id>]` replaces its destination range.

| Operation | Effect |
| --- | --- |
| `tunnel.enabled = false` | Keeps the configuration and the stable ID, but does not register the listener or start the backend probe |
| Keep an empty `managed` entry | Stops all centralized tunnels for that node |
| Delete the whole `managed` entry | Restores the agent's local tunnel configuration from startup |

The management pages distinguish "published", "waiting for the node to apply" and "disabled".

## Verification checklist

1. `felucca-server --config server.json --check` passes.
2. Open the admin address in a browser, sign in, and check that there are no JS errors.
3. Create a TCP tunnel and confirm the backend really returns content.
4. `GET /api/config` shows no plaintext secrets.
5. After restarting the server, the configuration and tunnels are still there, and retrying with
   the same `Idempotency-Key` does not apply the change twice.

## Next steps

- [External platform integration](platform.md) — let another system manage this node
- The `[remote]` section of [the felucca-agent module](agent.md)
- [Services, updates, logging and monitoring](operations.md)
- Examples: [admin-server.toml](../../../examples/admin-server.toml) ·
  [managed-agent.toml](../../../examples/managed-agent.toml)

The console now provides structured configuration forms for backend weights and TLS, health checks, HTTP routing, service accounts, ACLs and quotas, file services, local tunnel groups and port mappings.  The account page can generate, replace and revoke management tokens; a new token is downloaded after it is saved.

Validation reports settings that require a restart. The restart export provides a startup configuration and a runtime snapshot, including credentials, with offline replacement instructions. Stop the process and back up the existing files first. The runtime snapshot takes precedence over the startup file, including a path selected with `--runtime-config`. Keep traffic and platform journals; do not overwrite a running process's snapshot.

Diagnostics shows per-tunnel traffic, existing backend probe results, active P2P sessions and recent relay events. Administrators can inspect active certificates, retry failed ACME requests, upload PEM certificate/key files into a draft configuration, and read a bounded, credential-redacted log tail. Older agents without the optional diagnostics stream report unavailable; forwarding continues. Probe results are read without initiating extra health checks. Agent and visitor configuration generation preserves local directory and target authorization: review and run the generated file on the agent, using `--check` there before starting. Service and update commands are provided for review, not executed by the console.
