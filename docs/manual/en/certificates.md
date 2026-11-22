# Certificates, SNI and ACME

This page covers the three kinds of certificates: the bridge certificate, static website
certificates, and certificates issued and renewed automatically by ACME.

## Bridge certificate

```toml
[bridge]
bind = "0.0.0.0:14028"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }
cert_reload_interval = "30s"
```

When `bridge.tls` is omitted and runtime configuration storage is enabled (including `--init` or the admin console), the server saves its identity beside the runtime snapshot as `<snapshot>.identity.json`. Keep this file across restarts and backups. Without runtime storage the identity is temporary. `--fingerprint` reads explicit PEM files; automatic fingerprints appear in the running console and startup log.

PEM files are reloaded on the `cert_reload_interval` (30 seconds by default). New TLS/WSS and
QUIC connections pick up the new certificate, and existing connections are not interrupted.
If a certificate or key update is incomplete, the previous value is kept. An agent that uses
`pinned` trust must update its fingerprint before the next reconnect; an agent that validates
against the correct CA does not need to pin a particular certificate.

```sh
target/release/felucca-server --config conf/server.toml --fingerprint
```

On the agent side you have two choices:

```toml
[trust]
mode = "pinned"
fingerprint = "<from --fingerprint>"

[trust]
mode = "webpki"        # CA-issued bridge certificate
```

## Website certificates

```toml
[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.certificates."*.example.com"]
cert = "conf/wildcard.pem"
key = "conf/wildcard.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"
```

Exact matches take priority over wildcards, and wildcards over `default_tls`. If nothing
matches, the handshake is rejected. A conflict between SNI and the HTTP Host returns 421.
Static certificates are re-read every 30 seconds by default, which you can adjust with
`http.cert_reload_interval`. A certificate is replaced only once it matches its private key,
and a failure keeps the old certificate in place.

HTTP/3 and HTTP/HTTPS share the same SNI resolver, including ACME renewal and hot reloading
of static PEM files.

## ACME issuance

```toml
[http.acme]
email = "admin@example.com"
accept_tos = true
challenge = "http-01"          # http-01 | tls-alpn-01
cache_dir = "state/acme"
staging = false
domains = ["app.example.com"]
```

| Field | Description |
| --- | --- |
| `accept_tos` | Set to `true` only after you accept the CA's terms |
| `challenge` | `http-01` needs public TCP 80 to reach `http.bind`. `tls-alpn-01` needs public TCP 443 to reach `http.tls_bind` directly, in which case `http.bind` can be closed |
| `staging` | Verify connectivity with staging first; this pins Let's Encrypt staging |
| `directory` | Point to a different CA; the default is Let's Encrypt production |
| `cache_dir` | Directory for the account and certificate cache. **Keep and protect it** |
| `domains` | Explicitly limits which names are issued. When empty, names are derived every minute from the registered exact domains, and wildcards are skipped |

The account is stored in `account.json`, and each domain's certificate chain and private key
live together in one JSON file that is replaced atomically. On Unix, files use mode 0600 and
new cache directories 0700. Certificates are restored from the cache on startup. Renewal is
calculated from the certificate's actual validity, with a lead time of no more than one third
of that validity. Checks run every minute, failures are retried after an hour, and the old
certificate stays in use while renewal fails. A successful issuance is swapped in hot and
does not interrupt existing TLS connections.

**ACME only manages HTTP/HTTPS reverse proxy certificates. It does not change the bridge
certificate or an agent's pinned fingerprint.**

### EAB and ZeroSSL

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

ZeroSSL can use a manual EAB, or switch to API key mode:

```toml
[http.acme]
directory = "https://acme.zerossl.com/v2/DV90"
staging = false
zerossl_api_key = "your-account-key"
```

API key mode obtains an EAB when the account is first registered and then issues
certificates through ACME. **You cannot configure a manual `eab` at the same time.** No new
EAB is requested once an account cache exists. The API key and the secrets in the responses
are not written to error messages.

## Deployment notes

- Neither ACME challenge **supports wildcard certificates**. For wildcard names, use static
  certificates issued externally.
- When your application listens on 8080/8443, map 80/443 to those ports from outside.
- Until issuance succeeds, the TLS handshake for that domain can fail; HTTP-01 still provides
  validation.
- A TLS-terminating proxy in front must forward the challenge TLS as-is and return the
  challenge certificate only for ALPN values that contain `acme-tls/1`.
- Use a new `cache_dir` when you switch CA or switch between staging and production.
- The repository drives real CSR and challenge flows against a local simulated CA. Real
  issuance from a public CA still has to be verified in your own domain environment.

## Next steps

- [HTTP/HTTPS reverse proxy](http-services.md)
- [The felucca-server module](server.md)
- [Services, updates, logging and monitoring](operations.md)
