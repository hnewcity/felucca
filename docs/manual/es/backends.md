# Back-ends, balanceo y comprobaciones de estado

Esta página cubre la configuración de los back-ends, las cinco estrategias de balanceo y cómo las
comprobaciones de estado desalojan y restauran back-ends.

## Back-ends

```toml
[[tunnel]]
mode = "tcp"
port = 9000
bind = "127.0.0.1"
balance = "round_robin"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
weight = 2

[[tunnel.backend]]
addr = "backend.internal:3001"
weight = 1
tls = true
[tunnel.backend.trust]
mode = "pinned"
fingerprint = "<backend SHA-256>"
```

| Campo | Descripción |
| --- | --- |
| `addr` | Obligatorio. Un nombre de host o una dirección IP más un puerto |
| `weight` | Un peso entero positivo que usa la estrategia `weighted` |
| `tls` | Habilita TLS hacia el back-end. De forma predeterminada se usa la validación WebPKI, y el SNI sale del nombre de host de la dirección |
| `trust` | Para back-ends autofirmados, use `mode = "pinned"` más `fingerprint` |

`balance` pertenece a `[[tunnel]]` y **no** debe escribirse después de `[[tunnel.backend]]`.
Las comprobaciones de estado y las conexiones de datos usan la misma configuración TLS.

## Estrategias de balanceo

| Estrategia | Cómo se elige un back-end |
| --- | --- |
| `round_robin` | Rota entre los back-ends sanos |
| `weighted` | Distribuye las conexiones según el `weight` entero positivo |
| `least_connections` | Elige el back-end con menos conexiones abiertas en ese momento |
| `source_hash` | Elige un back-end de forma estable a partir de la dirección IP original del usuario |
| `failover` | Elige el primer back-end sano en el orden de configuración |

La elección ocurre en el **agente**. El recuento se libera cuando una conexión al back-end se cierra o
se cancela. Los pesos distribuyen recuentos de conexión, no recuentos de bytes.

Detalles de comportamiento:

- Cuando todos los back-ends están marcados como no sanos, aun así se intenta uno, para que una sonda
  mal configurada no deje todo el túnel indisponible.
- Un fallo de conexión aislado comunica el error y **no** reintenta con otro back-end
  dentro de la misma petición.
- `source_hash` usa la dirección original verificada por la ACL de origen o por el protocolo PROXY.

## Comprobaciones de estado

```toml
[tunnel.health]
type = "http"                # "tcp" | "http"
path = "/healthz"
interval = "10s"
timeout = "2s"
failures_to_evict = 3
successes_to_restore = 2
expect_status = [200, 204]
```

| Campo | Descripción |
| --- | --- |
| `type = "tcp"` | Solo comprueba que se puede establecer una conexión |
| `type = "http"` | Pide la `path` indicada y acepta 2xx de forma predeterminada |
| `expect_status` | Enumera de forma explícita los códigos de estado aceptables |
| `interval` | El intervalo de la sonda |
| `timeout` | El tiempo de espera de una sola sonda |
| `failures_to_evict` | Cuántos fallos consecutivos desalojan un back-end |
| `successes_to_restore` | Cuántos éxitos consecutivos restauran un back-end |

Una sonda HTTP usa la dirección del back-end como su Host. Los fallos consecutivos desalojan un
back-end y los éxitos consecutivos lo restauran, y ambos casos se registran.

Los túneles UDP y los proxies de destino dinámico (socks5/httpproxy/mixedproxy) **no** deberían llevar
sondas de back-end TCP o HTTP, porque no tienen un back-end fijo. Lo mismo se aplica al modo de
archivos, donde el destino lo determina el descriptor de capacidad de directorio.

## Agrupación de back-ends HTTP

El grupo de conexiones a back-ends de los túneles HTTP está aislado por sesión de cliente, túnel, IP
de origen, destino y origen HTTP: cada origen permite como máximo 8 conexiones inactivas con un tiempo
de inactividad de 30 segundos, y la tabla exterior guarda como máximo 256 entradas. Cuando se habilita
el envío de una cabecera PROXY, el grupo también se aísla por puerto de origen. Varias peticiones
HTTP/2 pueden compartir una misma conexión.

`response_timeout` (`[http]` del servidor) limita el tiempo transcurrido desde el envío de una petición
hasta la recepción de las cabeceras de respuesta del back-end; auméntelo cuando corresponda para
subidas lentas de archivos grandes. Los cuerpos de respuesta usan contrapresión y no se almacenan en
memoria por completo.

## Siguientes pasos

- [Túneles y mapeos de puertos](tunnels.md)
- [Proxy inverso HTTP/HTTPS](http-services.md)
- [Control de acceso, cuotas y codificación de flujo](security.md)
