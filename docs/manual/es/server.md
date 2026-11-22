# El módulo felucca-server

Esta página cubre el despliegue y la configuración de `felucca-server`: escuchas del puente, la tabla
de clientes, el ingreso HTTP, la autorización de puertos, la escucha de administración, los servicios
locales del servidor y los interruptores de plataformas externas.

## Una configuración mínima

```toml
[bridge]
bind = "0.0.0.0:14028"
ws_path = "/ws"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }

[client.agent-1]
vkey = "change-me-to-a-long-random-string"

[log]
level = "info"
```

Si se omite `bridge.tls` y se activa el almacenamiento de configuración (incluidos `--init` o la consola), el servidor guarda su identidad junto a la instantánea en `<snapshot>.identity.json`. Conserve este archivo al reiniciar y respaldar. Sin almacenamiento, la identidad es temporal. `--fingerprint` lee archivos PEM explícitos; las huellas automáticas aparecen en la consola activa y en el registro de inicio.

```sh
target/release/felucca-server --config conf/server.toml --check
target/release/felucca-server --config conf/server.toml
```

## Escuchas del puente

| Campo | Predeterminado | Notas |
| --- | --- | --- |
| `bridge.bind` | `0.0.0.0:14028` | TCP, TLS, WS y WSS comparten un solo socket y se distinguen por las características del saludo |
| `bridge.quic_bind` | ninguno | Dirección UDP independiente; puede usar el mismo número de puerto que TCP |
| `bridge.kcp_bind` | ninguno | Escucha UDP de KCP; no puede compartir un puerto UDP con QUIC/HTTP3 |
| `bridge.ws_path` | `/ws` | Ruta de WS/WSS. Ambos extremos deben coincidir; las mejoras por otras rutas se rechazan |
| `bridge.tls` | ninguno | `{ cert, key }`. Se genera un certificado temporal cuando se omite |
| `bridge.cert_reload_interval` | `30s` | Intervalo de recarga del PEM. La recarga no interrumpe las conexiones existentes |
| `bridge.proxy_accept` | desactivado | Acepta cabeceras PROXY de balanceadores de carga de confianza |
| `[bridge.mux]` | véase abajo | ventanas y presupuestos de mux |
| `[bridge.kcp]` | véase abajo | parámetros de KCP |

`[bridge.mux]` expone `initial_stream_window`, `max_stream_window`, `max_session_window`,
`max_send_queue`, `max_control_queue`, `max_streams`, `accept_backlog` y `open_timeout`.
El lado del agente tiene una sección `[mux]` correspondiente con los mismos campos. Los flujos QUIC
nativos no usan estos ajustes.

`[bridge.kcp]` acepta `mtu`, `send_window`, `receive_window`, `interval_ms`, `nodelay`,
`fast_resend`, `congestion_control`, `session_timeout`, `data_shards`, `parity_shards` y
`max_sessions`. El ajuste FEC predeterminado es 10 fragmentos de datos más 3 de paridad, de modo que
se pueden perder 3 cualesquiera de los 13 paquetes de un grupo y aun así recuperarlos. Ambos extremos
deben usar la misma MTU y el mismo número de fragmentos.

## La tabla de clientes

Cada agente tiene una tabla `[client.<key_id>]`. El `key_id` es el nombre del agente en el
servidor, y el `key_id` propio del agente debe coincidir con el nombre de la tabla.

| Campo | Notas |
| --- | --- |
| `vkey` | Obligatorio. El secreto compartido que se usa para la autenticación HMAC mutua. De su longitud y su aleatoriedad es usted responsable |
| `remark` | Una nota, que se muestra en la página de administración |
| `user` | Vincula una tabla `[user.<name>]`, que se usa para cuotas por capas, ACL y permisos de inquilino |
| `enabled` | `false` deshabilita y desconecta este cliente de forma persistente |
| `allow_config_push` | Si este agente puede registrar túneles. Defina `true` para enviar configuración `[[managed.<id>.tunnel]]` |
| `max_tunnels` | El límite de túneles de este agente |
| `max_connections` | El límite de conexiones de este agente |
| `rate_limit` | Un límite de bytes por segundo compartido por todos los túneles y ambos sentidos |
| `acl_mode` / `acl_rules` | ACL de origen para el acceso a la entrada pública de este cliente |
| `quota` | `{ flow_limit, expires_at }`, en bytes y en segundos Unix |

Una conexión nueva con el mismo `key_id` reemplaza a la anterior; las escuchas antiguas se detienen y
liberan antes de que los puertos se enlacen de nuevo. Los contadores de tráfico y los limitadores de
ancho de banda sobreviven a las reconexiones dentro de un mismo proceso.

## Autorización de puertos

`allowed_ports` es un campo de nivel superior y **debe aparecer antes de la primera cabecera de tabla
de TOML**:

```toml
allowed_ports = [8080, { from = 9000, to = 9100 }]
```

Una lista vacía permite todos los puertos TCP/UDP por encima de 1024. `max_tunnels` y `max_connections`
son límites globales de nivel superior que se cuentan por agente.

## Ingreso HTTP

```toml
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"      # set to "" to disable HTTPS
http3_bind = "0.0.0.0:8443"    # optional, HTTP/3 UDP ingress
forwarded_headers = true
response_timeout = "100s"
cert_reload_interval = "30s"

[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"

[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

Para el uso completo véase [Proxy inverso HTTP/HTTPS](http-services.md) y
[Certificados, SNI y ACME](certificates.md).

## Otras tablas de nivel superior

| Tabla | Propósito | Véase también |
| --- | --- | --- |
| `[admin]` / `[admin.account.<name>]` | Escucha de administración, cuentas y roles | [Interfaz de administración y configuración centralizada](management.md) |
| `[state]` | Persistencia de la instantánea de tráfico | [Control de acceso, cuotas y codificación de flujo](security.md) |
| `[user.<name>]` / `[user.<name>.limits]` | Cuotas y agregación por usuario | [Control de acceso, cuotas y codificación de flujo](security.md) |
| `[local.<name>]` | Grupos de servicios locales del servidor | [Destinos locales del servidor y rutas anidadas](local-services.md) |
| `[share.<name>]` | Autorización de recursos compartidos privados | [Recursos compartidos privados y visitantes](private-shares.md) |
| `[managed.<client>]` | Envío de configuración centralizada | [Interfaz de administración y configuración centralizada](management.md) |
| `[node]` / `[node.platform.<name>]` / `[node.webhook.<name>]` | Plataformas externas | [Integración con plataformas externas](platform.md) |
| `[p2p]` | Puerto de observación P2P | [Directo P2P y respaldo por relé](p2p.md) |
| `[geo]` | Archivos de datos GeoIP/GeoSite | [Control de acceso, cuotas y codificación de flujo](security.md) |
| `[ntp]` | Calibrar la hora de la autenticación del puente | [El módulo felucca-agent](agent.md) |
| `[log]` / `[log.rotation]` | Registro y rotación | [Servicios, actualizaciones, registro y supervisión](operations.md) |
| `allowed_ports` | Autorización de puertos | Esta página |

## Comandos útiles

| Comando | Propósito |
| --- | --- |
| `--check` | Valida la configuración y sale sin enlazar puertos |
| `--fingerprint` | Imprime el SHA-256 del certificado `bridge.tls`. Requiere tener configurado un PEM fijo |
| `--hash-password` | Lee una línea de contraseña de la entrada estándar e imprime un hash de contraseña de administración. La contraseña debe tener al menos 12 bytes |
| `--generate-token` | Imprime un token Bearer de administración y su hash de almacenamiento |
| `--runtime-config PATH` | Habilita la instantánea de configuración en tiempo de ejecución |
| `--build-info` | Imprime el nombre del programa, la versión, el protocolo y el destino de compilación |

## Ciclo de vida

- `SIGINT` / `SIGTERM` cierran las escuchas y las conexiones actuales, y después salen.
- Los cambios en los archivos de arranque requieren reiniciar el proceso. Los cambios hechos a través
  de la escucha de administración pasan por una transacción de instantánea de configuración en tiempo
  de ejecución, que interrumpe las conexiones reenviadas, pero el agente se reconecta después.
- Cambiar el enlace de administración, las rutas de los archivos de certificados, el origen público o
  la ruta de estado **requiere reiniciar el proceso**.
- Los contadores de tráfico sobreviven a las reconexiones del agente. Solo sobreviven a un reinicio
  del servidor cuando se configura `[state]`.

## Siguientes pasos

- [El módulo felucca-agent](agent.md) — ponga en marcha los servicios internos
- [Túneles y mapeos de puertos](tunnels.md)
- [Proxy inverso HTTP/HTTPS](http-services.md)
- [Servicios, actualizaciones, registro y supervisión](operations.md) — systemd / launchd / servicio de Windows
