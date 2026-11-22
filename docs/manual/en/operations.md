# Services, updates, logging and monitoring

This page covers registering Felucca with a service manager, updating binaries safely,
log rotation, metrics and CPU profiling.

## Installing as a system service

Both programs use the same subcommands, and `--name` supports multiple instances:

```sh
felucca-server service --name office render --config /srv/felucca/server.toml
felucca-server service --name office --dry-run install --config /srv/felucca/server.toml
felucca-server service --name office install --config /srv/felucca/server.toml
felucca-server service --name office start
felucca-server service --name office status
felucca-server service --name office restart
felucca-server service --name office stop
felucca-server service --name office uninstall
```

| Platform | Manager | Default scope |
| --- | --- | --- |
| Linux | systemd | `user`; `--scope system` is machine-level |
| macOS | launchd | `user`; `--scope system` is machine-level |
| Windows | SCM | `system` only |

`render --platform systemd|launchd|windows` shows you the definition for any supported host
without assembling commands through a shell. `--workdir` defaults to the directory that holds
the configuration, and relative paths inside the configuration are interpreted from there.
`--binary` selects which program is installed. Extra daemon arguments go after `--`, for example
`-- --log-file /srv/felucca/agent.log`.

Installing **does not overwrite** an existing service with the same name, and it does not start
the service immediately. On Linux it enables the service; on macOS it bootstraps at the next
login/startup or when you run `start` explicitly. A user systemd service that must be present at
boot requires the deployer to enable lingering, and a macOS user service depends on a graphical
login session. Machine-level services run as the service manager's default identity, so edit the
run user to match your deployment. On Windows the install identity is
`NT AUTHORITY\LocalService`, and the deployer must grant it the corresponding permissions on the
configuration, log, state and file root directories.

Uninstalling keeps the executable, configuration, logs, certificates and runtime state.

Scope of what has been verified: the repository tests definition generation, macOS plist parsing,
isolated copy updates and real CPU profiling. A Linux arm64 server image was also built and
run in OrbStack, with management login, monitoring, bridge TLS and persistence across container
recreation verified; see the deployment record.
System-service installation and Linux/Windows service managers remain unverified.

## Updating binaries

Get the SHA-256 of a standalone executable from a trusted release channel, then verify or
replace the running program:

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --expect-version 0.1.0
felucca-agent  update --source https://releases.example/felucca-agent --sha256 <64-hex-digest>
```

The digest is the **basis of trust** and should come from a trusted release manifest. The input
must be a standalone binary, so unpack the tar.gz/zip first. HTTPS downloads verify certificates
and do not follow redirects; for private downloads use `--token-env VARIABLE_NAME`, and
credentials are never written to logs. Downloads are limited to 512 MiB and 5 minutes. The
`--version` probe, bounded to 10 seconds, runs only after the digest matches, and it checks the
server/agent type and the optional version. Failures clean up temporary files, and an update lock
prevents concurrent replacement. The replacement keeps the original file permissions.

Afterwards you must run `service ... restart` explicitly; already running processes keep using
the old version. Updating inside a container does not work on a read-only image filesystem —
replace the image instead.

When the protocol version changes, the server and the agent must be upgraded together. The
packaging script actually runs `--build-info` and refuses to mix binaries with different
protocol versions or targets.

## Logging

On the server side:

```toml
[log]
level = "info"
file = "/data/felucca.log"
json = true

[log.rotation]
max_bytes = 10485760
max_files = 5
interval = "24h"
```

Output goes to both the file and stderr. Rotation happens between complete events, so a size
threshold may allow one extra event. Archives are numbered `.1` to `.5`, with `.1` the newest.
`interval` is measured from the moment the current file is opened, and a restart starts the
period again. An exclusive lock prevents multiple processes from rotating the same path, and new
log and lock files are 0600 on Unix.

**Changing the server log configuration requires a restart; the admin plane rejects online
changes to these settings.** The agent uses command-line arguments instead; see
[the felucca-agent module](agent.md).

## Monitoring

`GET /metrics` and `GET /api/metrics` return Prometheus text. Use a management account Bearer
token or a valid session, because there is **no anonymous metrics endpoint**, and metrics respect
the visibility of the account's user.

They cover service health, the configuration revision, node online/enabled state, connection
counts, cumulative node traffic, tunnel readiness and cumulative tunnel traffic. Traffic counts
reuse the existing persisted MeterStore, so they survive a server restart once you enable
`[state]`. The admin UI's Monitoring page shows host CPU, physical memory, filesystem capacity and network rates as interactive line charts, without Prometheus text or setup instructions. Samples arrive every five seconds and stay in memory for one hour; process restarts clear history. Select a 5-minute, 15-minute or 1-hour window. Hover, click to pin, or use arrow keys to inspect exact sample times, local timezone and values. Drag the range slider or use the zoom/reset buttons to zoom all four charts together; live updates preserve the selected absolute time window. Cards show minimum, sample mean and peak for visible observations, and the network legend toggles receive/send series. Host metrics are admin-only; other roles see traffic and connections for their permitted resources. The structured endpoint is `GET /api/monitor?window=900` (supported windows: 300, 900, 3600 seconds).

## CPU profiling

After you set `[admin] profiling = true`, an admin can call `POST /api/profile` with the JSON
body `{"seconds":5}`, or download a CPU flame graph from the Monitoring page. Linux and macOS are
supported, and other platforms refuse to enable it. Each run takes 1 to 30 seconds at 49 Hz, only
one task is allowed in the process at a time, and output is capped at 8 MiB. A dropped HTTP
request does not release the sampling lock early, and stopping the daemon cancels the sampling.
When there are no samples during an idle period, an explanatory SVG is returned. Release builds
keep function symbols for profiling but drop debug line information; line-level lookup requires a
build you produce with debug info.

## Docker

```sh
docker build --target server -t felucca-server:local .
docker build --target agent  -t felucca-agent:local .

docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

Images default to UID/GID 10001. Mount the configuration directory read-only, and allow the
image to write to the data directory. Open the additional TCP/UDP ports you need explicitly in
your configuration, and bind to addresses that are reachable from inside the container.
Multi-architecture images can be built with Buildx using the Dockerfile's native targets.
`STOPSIGNAL` is SIGTERM, which matches the program's shutdown path.

### Local OrbStack

Start OrbStack, then run `sh scripts/run-orbstack.sh` from the repository root. This builds
and deploys only the server, including the admin UI. The host needs Docker Compose, Python 3
and OpenSSL. Open `http://127.0.0.1:8088` and sign in as `root` using the random password
in `conf/orbstack/credentials.json`. The management listener binds only to loopback.

Credentials and the persistent bridge certificate live in the Git-ignored `conf/orbstack/`
directory. Configuration and traffic snapshots live in the `felucca_server-data` volume.
The credentials file records the initial password; saved admin configuration takes precedence
over the bootstrap TOML. Running the script again builds current sources and updates the
container while preserving existing credentials, certificates and data.

The container restarts with the Docker engine unless manually stopped. No clients are configured
initially. Bridge TCP and QUIC use port 14028. Compose uses OrbStack host networking, so
additional tunnel ports need no individual Docker port mappings. Avoid ports already used
by other local programs.

```sh
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

`down` keeps the data volume. Adding `-v` deletes persisted configuration and traffic data.

## Packaging and CI

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

Source builds require Node.js 22.12+ (24 recommended). The React + Vite admin in `apps/admin/`
is embedded into the server binary; deployments do not require Node.js. Rebuild the frontend
after changing it: Cargo rejects missing or stale bundles.

Artifacts contain the two programs, examples, documentation, a per-binary digest manifest and an
archive digest. Binaries that cannot run on the packaging host are rejected by default, and
cross-platform CI packages on each target platform. The GitHub workflow runs checks, tests and
archives for Linux amd64/arm64, macOS arm64 and Windows amd64, and separately verifies the Rust
1.88 minimum version and the Docker build. The workflow uploads job artifacts only; it does not
create a GitHub release and does not push images.

## Verification checklist

```sh
felucca-server service --name office render --config conf/server.toml
felucca-server --config conf/server.toml --fingerprint
felucca-agent  --config conf/agent.toml  --check
```

`--fingerprint` requires the configuration to reference fixed PEM files. `--check` does not mean
that backends, DNS, certificate files or public ports are already available.

## Next steps

- [The felucca-server module](server.md)
- [Admin UI and centralized configuration](management.md)
- [Reference](reference.md)
