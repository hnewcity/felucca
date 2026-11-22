# Referencia

Comandos, campos, extremos, un índice de ejemplos y un glosario. Las guías explican *por qué*;
esta página enumera *qué*.

## CLI

Argumentos que comparten ambos programas:

| Argumento | Variable de entorno | Descripción |
| --- | --- | --- |
| `--build-info` | — | Imprime el nombre del programa, la versión, el protocolo y el destino de compilación |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | Archivo TOML/JSON, URL HTTP(S), JSON en línea, base64 o una URI `felucca://`; repetible en el agente |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | Expande `${VAR}` / `${VAR:-default}` |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | La variable que contiene el token Bearer de la configuración remota |
| `--check` | — | Valida y sale sin enlazar ni conectar |
| subcomando `service` | — | Gestión del servicio del sistema |
| subcomando `update` | — | Verifica e instala un binario independiente |

`felucca-server` añade:

| Argumento | Variable de entorno | Descripción |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | Ruta de la instantánea de configuración en tiempo de ejecución |
| `--fingerprint` | — | Imprime el SHA-256 del certificado `bridge.tls` |
| `--hash-password` | — | Lee una línea de contraseña de la entrada estándar e imprime un hash PBKDF2; al menos 12 bytes |
| `--generate-token` | — | Imprime un token Bearer y su `token_hash` |

`felucca-agent` añade:

| Argumento | Predeterminado | Descripción |
| --- | --- | --- |
| `--launch` | — | Alias de `--config` |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | Añade y escribe también en la salida de error estándar; el directorio padre debe existir |
| `--log-json` | — | Eventos JSON estructurados |
| `--log-max-bytes <N>` | 10 MiB | Requiere `--log-file` |
| `--log-backups <N>` | 5 | Requiere `--log-file` |
| `--log-rotate-interval <D>` | — | Por ejemplo `24h`; requiere `--log-file` |

Subcomando `service`:

| Argumento | Descripción |
| --- | --- |
| `--name <NAME>` | Nombre de la instancia del servicio |
| `--scope <user\|system>` | Su valor predeterminado es `user`; `system` solo en Windows |
| `--dry-run` | Solo imprime las acciones que se ejecutarían |
| `render` | Genera la definición del servicio; `--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | Ciclo de vida |
| `--workdir <DIR>` | Toma de forma predeterminada el directorio que contiene la configuración |
| `--binary <PATH>` | Selecciona el programa que se instala |
| `-- <ARGS>` | Argumentos adicionales del demonio |

Subcomando `update`:

| Argumento | Descripción |
| --- | --- |
| `--source <PATH_OR_URL>` | Un binario independiente, o un archivo tar.gz/zip |
| `--sha256 <HEX>` | 64 dígitos hexadecimales; la base de la confianza |
| `--check-only` | Verifica sin sustituir |
| `--expect-version <V>` | Comprobación opcional de la versión |
| `--token-env <NAME>` | La variable que contiene el token Bearer de una descarga privada |

## Estructura de la configuración

| Archivo | Contenido de nivel superior |
| --- | --- |
| servidor | `allowed_ports`, `max_tunnels`, `max_connections`, `[bridge]`, `[http]`, `[admin]`, `[state]`, `[geo]`, `[ntp]`, `[log]`, `[p2p]`, `[node]`, `[user.*]`, `[client.*]`, `[local.*]`, `[share.*]`, `[managed.*]` |
| agente | `server`, `key_id`, `vkey`, `transport`, `reconnect_min/max`, `dial_timeout`, `max_connections`, `[trust]`, `[network]`, `[mux]`, `[kcp]`, `[ntp]`, `[remote]`, `[geo]`, `[[tunnel]]`, `[[mapping]]`, `[[visitor]]`, `[p2p]`, `[log]` (el agente usa argumentos de línea de comandos) |

`allowed_ports` debe aparecer antes de la primera cabecera de tabla de TOML. Todas las tablas habilitan
`deny_unknown_fields`.

## Referencia rápida de campos de túnel

| Campo | Se aplica a los modos | Descripción |
| --- | --- | --- |
| `id` | todos | ID estable; fíjelo una vez habilitadas las instantáneas de tráfico |
| `mode` | todos | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | todos salvo privados | El puerto del servidor; `http` debe coincidir con una escucha HTTP |
| `bind` | tcp/udp/proxy/file | Solo dirección IP |
| `public` | todos | `false` significa un recurso compartido privado |
| `hosts` | http, file | Lista de dominios; se admite `*.example.com` |
| `balance` | back-ends fijos | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | back-ends fijos | `addr`, `weight`, `tls`, `trust` |
| `health` | back-ends fijos | `type`, `path`, `interval`, `timeout`, `failures_to_evict`, `successes_to_restore`, `expect_status` |
| `acl_mode`/`acl_rules` | todos | ACL de origen: off/allow/deny |
| `target_policy` | back-ends fijos y proxies directos | `allow`/`deny`/`ports` |
| `options.auth.users` | proxies directos, HTTP | Tabla de cuentas |
| `options.http.*` | http | Ruta, reescritura, cabeceras, redirecciones, CORS, páginas de error, tls_mode, backend_http2 |
| `options.payload.*` | todos | `compress`, `encrypt` |
| `options.limits.*` | todos | `max_connections`, `rate_limit`, `quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | entradas de proxy | Acepta cabeceras PROXY |
| `file` | file | `root`, `prefix`, `read_only`, `auto_index`, `auth` |

## Resumen de la API de administración

La autenticación es por cookie con Origin y CSRF, o mediante `Authorization: Bearer <token>`. Las
escrituras requieren `If-Match: "<revision>"` y un `Idempotency-Key` único. El límite del cuerpo de la
petición es de 2 MiB.

| Grupo | Rutas |
| --- | --- |
| Sesión | `/api/login`, `/api/session`, `/api/logout` |
| Recursos | `/api/users`, `/api/clients`, `/api/locals`, `/api/shares`, `/api/accounts`, cada uno con su `<id>` |
| Configuración centralizada | `/api/managed/<client>`, `/api/managed/<client>/tunnels/<id>` |
| Túneles locales del servidor | `/api/locals/<group>/tunnels/<id>` |
| Configuración completa | `/api/config`, `/api/config?secrets=1`, `/api/validate`, `/api/batch` |
| Tiempo real | `/api/events` (SSE), `/metrics`, `/api/metrics`, `/api/profile` |
| Plataforma | `/api/node/*`, `/api/platforms/<name>`, `/api/webhooks/<name>`, `/api/ws`, `/api/realtime/subscriptions` |

Véase [Interfaz de administración y configuración centralizada](management.md) e
[Integración con plataformas externas](platform.md) para todos los campos.

## Índice de configuraciones de ejemplo

| Archivo | Contenido |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | Ejemplo mínimo de servidor |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | Ejemplo mínimo de agente |
| [features-server.toml](../../../examples/features-server.toml) | Puente QUIC + HTTP/HTTPS + certificados |
| [features-agent.toml](../../../examples/features-agent.toml) | Todos los modos de proxy, balanceo de carga, comprobaciones de estado |
| [shared-server.toml](../../../examples/shared-server.toml) | Puertos TCP/UDP compartidos + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | Lado del agente y mapeo masivo de puertos |
| [acme-server.toml](../../../examples/acme-server.toml) | Servidor ACME |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL y TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | ACL por capas, cuotas, persistencia |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | Servicios de archivos |
| [network-agent.toml](../../../examples/network-agent.toml) | Proxy ascendente, DNS personalizado, NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy y cifrado independiente de flujos |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | Puente KCP |
| [local-server.toml](../../../examples/local-server.toml) | Destinos locales del servidor y reenvío anidado |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | Proveedor privado y visitante |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P y respaldo |
| [admin-server.toml](../../../examples/admin-server.toml) | Servidor de administración |
| [managed-agent.toml](../../../examples/managed-agent.toml) | Agente gestionado de forma centralizada |
| [node-server.toml](../../../examples/node-server.toml) | Plataforma externa y gestión de nodos |

Las rutas de configuración son relativas al directorio de trabajo del proceso. Los 26 ejemplos pasan
todos la validación de configuración.

## Límites numéricos

| Elemento | Límite |
| --- | --- |
| Configuración única | 4 MiB |
| Instantánea de configuración en tiempo de ejecución | 16 MiB |
| Instancias del agente | 32 |
| Saltos de anidamiento de rutas | 16 |
| Operaciones por lote | 128 elementos |
| Importación de tráfico | 128 elementos por petición |
| Registros de idempotencia conservados | 256 (256 para cada uno: acciones de llamada de retorno e importación de tráfico) |
| Cola de llamadas de retorno | `queue_max` por destino, 4096 en todo el nodo |
| WebSocket de administración directos | 16 |
| Concurrencia de WS RPC | 2 por conexión / 16 en todo el nodo |
| Suscripciones en tiempo real | 32 elementos por conexión, 32 KiB por elemento |
| Grupo de conexiones a back-ends HTTP | 8 inactivas por origen, 30 segundos de inactividad, 256 entradas en el mapa exterior |
| HTTP/3 | 128 flujos por conexión, 1 MiB por flujo / ventana de conexión de 16 MiB, cabeceras de 32 KiB |
| UDP | 65507 bytes por origen; 256 asociaciones por túnel |
| UDP de SOCKS5 | 64 destinos por sesión, 8 paquetes en cola para cada uno |
| Páginas de error | 400..599, 64 KiB por página |
| Coincidencia de dominios HTTP | Los dominios exactos tienen prioridad sobre los comodines |

## Entorno y compilación

| Elemento | Valor |
| --- | --- |
| Rust mínimo | 1.88 |
| Entorno verificado | macOS Intel, Rust/Cargo 1.98.1 |
| Pruebas del espacio de trabajo | 581 pasaron / 0 fallaron / 0 se ignoraron (2026-09-17) |
| Binarios | `target/release/felucca-server`, `target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

Ejecute solo un comando de Cargo a la vez contra el mismo directorio de destino. Termine las
compilaciones, las pruebas y rustdoc antes de editar el código fuente y las páginas incrustadas. Las
pruebas necesitan permiso para escuchar en puertos TCP/UDP locales.

## Glosario

| Término | Significado |
| --- | --- |
| agente | El proceso `felucca-agent` que se ejecuta dentro de una red privada y llama hacia afuera |
| puente | La conexión autenticada y de larga duración entre el servidor y el agente |
| cliente / `[client.<id>]` | La tabla de identidades del servidor para un agente; `key_id` es el nombre de la tabla |
| key_id | El nombre del agente en el servidor |
| vkey | El secreto del puente que comparten el agente y el servidor |
| transporte | El transporte del puente: tcp/tls/ws/wss/quic/kcp |
| confianza | Cómo verifica el agente el certificado del servidor: webpki/pinned/insecure |
| túnel | Un mapeo de un punto de entrada público a un conjunto de back-ends |
| back-end | El destino al que realmente se conecta el agente |
| mapeo | Mapeo masivo de puertos uno a uno |
| grupo local | Un grupo de servicios propios del servidor que no necesita ningún agente |
| recurso compartido | El servidor autoriza a un invitado a usar un túnel |
| visitante | Un agente que posee un secreto de recurso compartido y escucha localmente |
| proveedor | La parte que presta un servicio en un recurso compartido privado o en una configuración P2P |
| gestionado | Configuración que el servidor envía a un agente |
| mux | Multiplexación acotada sobre transportes de tipo TCP |
| FEC | Corrección directa de errores de KCP, 10 de datos + 3 de paridad por defecto |
| cuota / rate_limit | Cuotas de tráfico y límites de tasa en bytes por segundo |
| instantánea | Una persistencia atómica del tráfico o de la configuración en tiempo de ejecución |

## Documentación relacionada

- [Arquitectura](../../architecture.md) — límites de propiedad de los crates
- [Configuración y despliegue](../../configuration.md) — detalle a nivel de campo
- [P2P](../../p2p.md), [Túneles privados y acceso compartido](../../visitors.md),
  [Administración y configuración centralizada](../../management.md),
  [Nodos y plataformas externas](../../node.md), [Operaciones y distribución](../../operations.md),
  [TCP transparente](../../transparent.md)
