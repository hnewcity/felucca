# Túneles y mapeos de puertos

Un túnel es la unidad básica de Felucca: una entrada pública más un conjunto de back-ends. Esta
página cubre los campos compartidos, los ocho modos, los mapeos de puertos por lotes y el protocolo
PROXY.

## Campos compartidos del túnel

```toml
[[tunnel]]
id = 1                 # Stable ID; pin it once traffic snapshots are enabled
mode = "tcp"           # Entry protocol
port = 8080            # Port opened on the server
bind = "0.0.0.0"       # IP only, no port attached
remark = "internal web app"
public = true          # false means a private share that does not take a public port
balance = "round_robin"   # Goes inside [[tunnel]], not after [[tunnel.backend]]

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`bind` toma una dirección IP, no un puerto; el puerto viene de `tunnel.port`. Omita `bind` para
escuchar en todas las interfaces. Cuando no se define `id`, se asigna un entero positivo disponible.
Una vez habilitadas las instantáneas de tráfico, defina `id` de forma explícita en lugar de confiar
en el orden del array.

## Modos

| `mode` | Comportamiento de la entrada | Origen del destino |
| --- | --- | --- |
| `tcp` | Puerto TCP público, reenvío de bytes en bruto | `backend` |
| `udp` | Puerto UDP público, se conservan los límites de datagrama | `backend` |
| `http` | Reutiliza la escucha HTTP, HTTPS o HTTP/3 del servidor y enruta por dominio y ruta | `backend` |
| `socks5` | Proxy directo SOCKS5, CONNECT + UDP ASSOCIATE | Lo define el cliente |
| `httpproxy` | Proxy directo HTTP, URL absoluta + CONNECT | Lo define el cliente |
| `mixedproxy` | Entrada mixta, distingue SOCKS5 de HTTP por el inicio de la petición | Lo define el cliente |
| `file` | Servicio de archivos y WebDAV | Un directorio del host del agente |
| `transparent` (alias `tcptrans`) | Lee del núcleo la dirección de destino original | El núcleo |

Un túnel `http` **no** abre una escucha aparte: su `port` debe coincidir con `http.bind`,
`http.tls_bind` o `http.http3_bind` del servidor, y varios túneles HTTP pueden compartir un
mismo puerto. `tunnel.bind` no tiene efecto en el enrutado HTTP. La escucha HTTP la habilita el
servidor y no está limitada por el `allowed_ports` del agente.

## Mapeos de puertos por lotes

`[[mapping]]` solo admite TCP y UDP, y mapea un rango de puertos uno a uno en orden:

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

Los dos recuentos deben ser iguales y no se permiten puertos de servidor duplicados. Los túneles
expandidos se validan junto con las entradas `[[tunnel]]` explícitas, y siguen contando para la
autorización de puertos del servidor y para el límite de túneles. `target_host` puede ser un nombre de
dominio, una dirección IPv4 o una dirección IPv6, y no puede llevar un puerto. También puede definir
`tls`/`trust`, `health`, `acl`, `options` y `target_policy`.

## Comportamiento con UDP

- Se establece una asociación distinta por cada IP:puerto de origen, y se fija a un socket de back-end
  en el lado del agente.
- Los paquetes vacíos y los límites de datagrama se conservan. El datagrama máximo es de 65507 bytes,
  y la MTU utilizable en la red real puede ser menor.
- Cada túnel permite como máximo 256 asociaciones de origen, cada una con 8 paquetes en cola. Los
  paquetes se descartan cuando una cola está llena, y una asociación se reclama tras 60 segundos de
  inactividad.
- Los datagramas se transportan por un flujo de túnel fiable, **no** por QUIC DATAGRAM, así que un
  enlace con pérdidas espera la retransmisión.
- Las estadísticas de UDP cubren solo la carga útil del datagrama, no las tramas internas de longitud.

## Protocolo PROXY

Envíe al back-end la dirección de origen original y la dirección de destino de la entrada:

```toml
[tunnel.options]
proxy_protocol = "v1"     # "off" | "v1" | "v2"
```

Para TCP, HTTP y TLS se envía una cabecera por cada conexión al back-end. UDP solo admite v2,
añadiendo una cabecera DGRAM por datagrama; los paquetes mayores de 65507 bytes incluida la cabecera
se descartan. El back-end debe ser capaz de interpretar el protocolo PROXY.

En el lado receptor, configure `proxy_accept` para la escucha propietaria:

```toml
[http.proxy_accept]
mode = "required"                    # optional | required
trusted = ["192.0.2.10", "2001:db8:1::/64"]
bridge.proxy_accept = { ... }
[tunnel.options.proxy_accept]
mode = "optional"
trusted = ["10.0.0.0/8"]
```

`optional` permite conexiones directas sin cabecera, y `required` rechaza una cabecera ausente. Cuando
active esto, `trusted` no debe estar vacío. Las ACL de origen, el hash por origen y las cabeceras de
reenvío de HTTP usan todos la dirección original verificada. Un puente y una escucha HTTP en el mismo
puerto deben configurarse con la misma política. La recepción está limitada a 5 segundos, y una escucha
UDP no acepta cabeceras PROXY.

## Codificación por flujo

```toml
[tunnel.options.payload]
compress = true     # Snappy, default false
encrypt = true      # ChaCha20-Poly1305, default false
```

Esto se aplica a TCP y UDP, a las transferencias de archivos y WebDAV, y a los flujos de datos del
proxy inverso y del proxy directo HTTP. Con la compresión activada, los datos se comprimen antes de
cifrarse. Cada flujo intercambia un nonce nuevo, y de ese vkey del agente se derivan dos claves
direccionales independientes. Cuando los dos extremos no coinciden, la negociación se rechaza, sin
respaldo automático al texto claro. Véase [Control de acceso, cuotas y codificación de flujo](security.md) para
los detalles.

## Siguientes pasos

- [Back-ends, balanceo y comprobaciones de estado](backends.md)
- [Proxy inverso HTTP/HTTPS](http-services.md)
- [Proxy directo](forward-proxy.md)
- [Servicios de archivos y WebDAV](file-services.md)
- [Destinos locales del servidor y rutas anidadas](local-services.md)
