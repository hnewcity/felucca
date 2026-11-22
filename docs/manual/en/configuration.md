# Configuration sources and CLI

This page covers where configuration comes from, how it is passed, which environment
variables apply, and how to validate without starting. Field meanings live in the module
guides.

## Configuration files

Both programs accept TOML and JSON. Relative paths resolve against the process working
directory, so `--workdir` and the systemd/launchd start directory both matter.

```sh
felucca-server --config /srv/felucca/server.toml
felucca-agent  --config /srv/felucca/agent.toml
```

Without `--config`, the programs read `felucca-server.toml` / `felucca-agent.toml` from the
working directory. `-c` is the short form of `--config`; the agent also accepts the alias
`--launch`.

All configuration uses `deny_unknown_fields`: an unknown field is an error and is never
silently ignored.

## Accepted sources

`--config` accepts four forms:

| Form | Example | Details |
| --- | --- | --- |
| Local file | `conf/server.toml` | TOML or JSON |
| HTTP(S) URL | `https://cfg.example/edge.json` | Waits at most 15 seconds and requires a 200; does not follow redirects and does not accept compressed responses; the URL accepts no userinfo and no fragment, and query parameters can carry a signed download address |
| Inline JSON | `'{"bridge":{"bind":"0.0.0.0:14028"}}'` | Passes a JSON object directly |
| base64 | `base64:eyJicmlkZ2Ui...` | Standard base64 or base64url without padding |

A single configuration may be at most 4 MiB. `--config-token-env NAME` reads a Bearer token
from an environment variable for private download addresses; the credential is never written
to the log. The URL, the token and the inline body are never printed as the configuration
source name.

In a remote configuration, certificate, Geo data and file root paths still point to paths on
the **agent machine**.

## Agent launch URI

`felucca-agent` also accepts a short URI. On its own it only connects, and local tunnels still need TOML/JSON; with `remote=1` the server supplies the tunnels and the access range, so one command is enough:

```sh
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&transport=tls'
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&fingerprint=SHA256_HEX&remote=1'
```

| Field | Required | Details |
| --- | --- | --- |
| Port | Yes | Must be non-zero |
| `vkey` | Yes | The shared key; the value must be percent-encoded |
| `key_id` | No | Uses the default identity when omitted |
| `transport` | No | `tcp`/`tls`/`ws`/`wss`/`quic`/`kcp` |
| `fingerprint` | No | Enables pinned certificate SHA-256 validation |
| `ws_path` | No | The WebSocket path |
| `remote` | No | `1` hands services and the access range to the server: enables `remote.enabled` and `remote.console_scope`, and allows only local addresses until the server sends a range |

Unknown fields and repeated fields are errors.

## Multiple agent instances

`--config` may be repeated, up to 32 instances in one process:

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://cfg.example/edge.json' --config-token-env CONFIG_TOKEN
```

Each instance keeps its own connections, backoff, routes, probes and connection limits. All
configuration is validated and initialized first, and only then does connecting begin; a
repeated `server` + `key_id` combination is an error. When one server is unreachable the
other instances keep working, and a signal to exit closes every instance.

## Environment expansion

Off by default. Set `--expand-env` (or `FELUCCA_EXPAND_ENV=true`) and resolved string values
are expanded:

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}` requires the variable to exist. `${VAR:-default}` uses the default when the variable
is missing or empty. `$$` is a literal `$`. Expansion runs once, does not touch field names,
and does not convert strings to numbers. A result larger than 4 MiB is rejected.

## Runtime configuration snapshot

```sh
felucca-server --config bootstrap.toml --runtime-config conf/runtime.json
```

You can also set the path with `FELUCCA_RUNTIME_CONFIG`. After the first successful start,
the normalized configuration is saved atomically, and later starts read the snapshot first.
`--check` and `--fingerprint` read the same snapshot. If the file is corrupted the start
fails and does not fall back to the start file. **Do not** set it to the same path as the
traffic file of `[state].path`.

The snapshot is at most 16 MiB and contains real secrets; on Unix a new file gets permission
0600 and a new directory 0700, and a file lock prevents another daemon from using it at the
same time.

Changing configuration from the admin UI uses the same mechanism — see
[Management and centralized configuration](management.md).

## Validation and logging

```sh
felucca-server --config conf/server.toml --check
felucca-agent  --config conf/agent.toml  --check
```

`--check` validates the TOML/JSON structure, field constraints, ports, ACLs and Geo data. It
does **not** connect to backends, **not** verify public DNS, and **not** inspect the contents
of certificate files. Certificate contents and bind conflicts are checked at start time.

Logging flags for the agent:

```sh
felucca-agent -c agent.toml --log-level info \
  --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

`RUST_LOG` overrides the log level on both sides. Server logging is written in the
configured `[log]` table, and changing it needs a restart.

## Environment variables

| Variable | Effect |
| --- | --- |
| `FELUCCA_CONFIG` | The same as `--config` (repeatable on the agent) |
| `FELUCCA_RUNTIME_CONFIG` | The same as the server's `--runtime-config` |
| `FELUCCA_EXPAND_ENV` | The same as `--expand-env` |
| `FELUCCA_CONFIG_TOKEN_ENV` | The name of the variable holding the remote configuration Bearer token |
| `RUST_LOG` | Overrides the log filter level |

## Next steps

- [The felucca-server module](server.md)
- [The felucca-agent module](agent.md)
- [System services, updates, logging and monitoring](operations.md)
