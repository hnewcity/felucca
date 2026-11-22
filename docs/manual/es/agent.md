# El módulo felucca-agent

Esta página cubre el despliegue y la configuración de `felucca-agent`: la conexión con el servidor,
la confianza en los certificados, las reconexiones, la red de salida, los proxies ascendentes, el
registro y varias instancias.

## Una configuración mínima

```toml
server = "bridge.example.com:14028"
key_id = "agent-1"
vkey = "change-me-to-a-long-random-string"
transport = "tls"          # tcp | tls | ws | wss | quic | kcp

[trust]
mode = "pinned"
fingerprint = "0000...0000"   # from the server's --fingerprint

[[tunnel]]
mode = "tcp"
port = 8080
[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`server` admite un nombre de host o una dirección IP con un puerto. `key_id` debe coincidir con un
nombre de tabla `[client.<key_id>]` del servidor.

## Transporte y confianza

`transport` acepta `tcp`, `tls`, `ws`, `wss`, `quic` y `kcp`. Para puentes públicos prefiera
TLS/WSS/QUIC: `tcp` y `ws` autentican al interlocutor pero no cifran el tráfico, así que son
transportes en texto claro que se eligen de forma deliberada.

`[trust]` controla cómo se verifica el certificado del servidor:

| `mode` | Comportamiento |
| --- | --- |
| `webpki` | Verificación normal contra CA públicas. No defina `fingerprint` |
| `pinned` | Fija el SHA-256 del certificado completo. La firma del saludo se verifica, pero no el nombre de dominio ni el periodo de validez. Actualice la huella digital cada vez que cambie el certificado |
| `insecure` | Solo para diagnóstico temporal |

Después de configurar un PEM fijo en `bridge.tls`, obtenga la huella digital con
`felucca-server --fingerprint`.

## Conexión y reconexión

| Campo | Predeterminado | Notas |
| --- | --- | --- |
| `reconnect_min` | — | Límite inferior de la espera incremental |
| `reconnect_max` | — | Límite superior de la espera incremental |
| `dial_timeout` | — | Tiempo de espera de una sola marcación |
| `max_connections` | — | El límite de conexiones de este agente |

El agente se reconecta automáticamente tras una desconexión; el servidor vuelve a registrar las
escuchas y el estado de la sesión. Los contadores de tráfico sobreviven a las reconexiones.

## Red de salida

```toml
[network]
local_ip = "192.0.2.10"
forward_local_ip = true
dns = ["192.0.2.53:53"]

[network.proxy]
kind = "socks5"           # http (CONNECT) | socks5
server = "192.0.2.20:1080"
remote_dns = true
[network.proxy.auth]
username = "edge"
password = "replace-with-proxy-password"
```

| Campo | Notas |
| --- | --- |
| `local_ip` | Enlaza la IP de salida del puente. Debe ser una dirección que esta máquina posea realmente; no se aceptan nombres de interfaz |
| `forward_local_ip` | Las conexiones a los back-ends y las sondas de estado también usan esta dirección. De forma predeterminada solo la enlaza el puente |
| `dns` | Una lista literal de `IP:port`. Cuando está vacía se usa el resolvedor del sistema. Las respuestas se almacenan en caché según el TTL, con soporte de A/AAAA y respaldo por TCP tras un truncamiento UDP |
| `network.proxy` | Proxy ascendente. Solo transporta el TCP/TLS/WS/WSS del puente y no reenvía las peticiones a los back-ends ni el UDP de NTP |

Las ACL de destino se comprueban contra la **dirección IP que devuelve realmente la resolución**, y el
agente se conecta después a esas direcciones ya comprobadas, de modo que ninguna segunda resolución
pueda introducir una elusión. `local_ip` y las direcciones DNS deben estar en la misma familia de
direcciones. Un puente QUIC/KCP no puede usar además un proxy ascendente TCP.

## Sincronización horaria

Tanto el servidor como el agente aceptan una sección `[ntp]` para calibrar la hora de la
autenticación del puente:

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

De forma predeterminada no se consulta ningún servicio horario. `interval` va de 1 segundo a 24 horas.
`timeout` debe ser positivo, no mayor de 30 segundos y menor que `interval`. `max_offset` debe ser
positivo y no mayor de 7 días. Si la primera consulta falla, se usa la hora del sistema. La calibración
no cambia el reloj del sistema, así que la validez de los certificados TLS y la caducidad de las cuotas
siguen la hora del sistema. Debe confiar en la fuente NTP: el SNTP en claro no autentica la
fuente horaria.

Las consultas NTP del agente usan su propia configuración de DNS y de salida; el servidor usa el DNS
del sistema y la salida predeterminada.

## Recepción de configuración centralizada

```toml
[remote]
enabled = true

[remote.target_policy]
allow = ["10.0.0.0/8"]
ports = [80, 443]

[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

`remote.enabled` debe definirse de forma explícita. La conexión principal debe usar TLS/WSS/QUIC con
un certificado verificado; una conexión en texto claro no puede pedir configuración.
`remote.target_policy` es un límite estricto en la propia máquina del agente y se interseca con cada
túnel enviado. `remote.file_root` declara por adelantado capacidades de directorio con nombre, y el
servidor solo puede enviar rutas dentro de esos nombres de capacidad. Un directorio de solo lectura no
puede pasarse a escritura mediante configuración remota. Véase
[Interfaz de administración y configuración centralizada](management.md).

Con `remote.console_scope = true`, un `target_policy` en `[managed.<client-id>]` del servidor sustituye al `remote.target_policy` local, de modo que el alcance se edita desde el editor de dispositivos de la consola; si el servidor no lo indica, sigue valiendo el alcance local. Sin esta opción, una configuración enviada que incluya `target_policy` se rechaza. Las capacidades de directorio siempre siguen siendo locales. Los paquetes de conexión que genera la consola activan esta opción.

## Registro

```sh
felucca-agent -c agent.toml --log-level debug --log-file /data/agent.log --log-json
```

Definir `--log-file` habilita la rotación con archivos de 10 MiB y 5 copias archivadas de forma
predeterminada. Los archivos de registro reciben añadidos y la salida de error estándar sigue
recibiendo la salida. El directorio padre ya debe existir. `--log-max-bytes`, `--log-backups` y
`--log-rotate-interval` requieren `--log-file`. Los secretos en bruto nunca llegan al registro.

## Varias instancias

```sh
felucca-agent --config office.toml --config home.json --check
felucca-agent --config office.toml --config home.json
```

Un solo proceso admite hasta 32 instancias. Véase [Fuentes de configuración y CLI](configuration.md).

## Interruptores de P2P y de visitantes

| Tabla | Efecto |
| --- | --- |
| `[p2p] enabled = true` | Permite que este agente acepte conexiones directas como proveedor |
| `[[visitor]]` | Escucha en su propia máquina para alcanzar el recurso compartido privado de otra persona |

Véase [Directo P2P y respaldo por relé](p2p.md) y
[Recursos compartidos privados y visitantes](private-shares.md).

## Siguientes pasos

- [Túneles y mapeos de puertos](tunnels.md)
- [Back-ends, balanceo y comprobaciones de estado](backends.md)
- [Servicios, actualizaciones, registro y supervisión](operations.md) — instalar como servicio de sistema
