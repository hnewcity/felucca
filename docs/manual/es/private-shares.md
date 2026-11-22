# Recursos compartidos privados y visitantes

Un túnel privado se registra con el servidor pero no ocupa **ningún puerto público**. Otro agente, el
visitante, abre una escucha en su propia máquina y lo alcanza a través del puente autenticado y cifrado
usando un secreto de recurso compartido aparte. Use esto para exponer un servicio interno a nodos
concretos en lugar de a internet público.

## Las tres configuraciones

Cada uno de los agente proveedor, el agente visitante y el servidor cambia en un solo sitio.

### Proveedor: haga privado el túnel

```toml
[[tunnel]]
id = 1
mode = "tcp"
public = false                 # Omit port and bind
backend = [{ addr = "127.0.0.1:3000" }]

[[tunnel]]
id = 3
mode = "socks5"
public = false
[tunnel.target_policy]
allow = ["127.0.0.0/8", "::1", "10.0.0.0/8"]
```

Se admiten entradas TCP, UDP, de archivos y de proxy. **El enrutado HTTP por dominio depende de la
escucha HTTP y no puede hacerse privado**; un túnel TCP privado sí puede reenviar cualquier back-end
HTTP o HTTPS. Definir `port = 0` de forma explícita equivale a un túnel privado.

### Servidor: declare el recurso compartido

```toml
[share.office-web]
client = "office"              # Node name; for a server-local group use client = "local.<group-name>"
tunnel = 1                     # Stable tunnel ID
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]     # An empty list allows every authenticated node that holds the share secret
direct = true                  # Allow P2P direct connection attempts
```

Un `secret` tiene entre 16 y 4096 bytes de longitud, y cada recurso compartido debería usar uno
distinto. `acl_mode`/`acl_rules` comprueban la dirección de origen de las conexiones de puente de los
visitantes. Un secreto de recurso compartido por sí solo **no** puede usarse para conectarse como
cliente anónimo: un visitante sigue necesitando una identidad de nodo normal y un vkey. El servidor
puede deshabilitar un nodo de forma permanente con
`client.NAME.enabled = false`.

### Visitante: escuche localmente

```toml
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"                   # tcp | udp | socks5
max_connections = 64
udp_idle = "60s"
```

| Campo | Descripción |
| --- | --- |
| `name` | Único dentro de este agente |
| `share` | El nombre del recurso compartido en el servidor |
| `mode` | `tcp`, `udp` o `socks5` |
| `bind` | Requiere un puerto fijo; se enlaza al arrancar, y un puerto ocupado falla de inmediato |
| `max_connections` | 1–4096, predeterminado 64; conexiones TCP o sesiones de origen UDP |
| `udp_idle` | 1 segundo–1 hora, predeterminado 60 segundos; la actividad en cualquiera de los dos sentidos prolonga la sesión |
| `acl_mode` / `acl_rules` | Comprueban la dirección de origen de las conexiones a la escucha del visitante |
| `target` | Solo para recursos compartidos dinámicos, véase más abajo |

Todos los visitantes comparten además el límite de conexiones del agente y un presupuesto de 8 MiB para
el encolado UDP. Un visitante solo acepta TLS, WSS o QUIC con un certificado verificado.

El servidor aplica la ACL del recurso compartido, la ACL del visitante y la ACL de enrutado del
proveedor sobre la **dirección de origen real del puente del visitante**. Una dirección en bruto que
llegue al puerto del visitante en la LAN no se reenvía al servidor como origen de confianza.

## Lo que se obtiene

Con las configuraciones anteriores, alcanzar `127.0.0.1:18080` en la máquina del visitante llega a
`127.0.0.1:3000` en el agente de la oficina, y el UDP `127.0.0.1:15353` se corresponde con
`127.0.0.1:5353` en office.

Las conexiones y el tráfico en bruto se cuentan tanto para el visitante como para el proveedor, y
contra sus recursos de usuario y de túnel; cuando un mismo usuario es propietario de los dos extremos,
el uso de ambos se suma. Las cabeceras de longitud de UDP y los mensajes de autenticación del recurso
compartido no se cuentan como tráfico de negocio.

## Destinos dinámicos

Cuando un recurso compartido del servidor define `dynamic = true`, el visitante puede nombrar un
destino:

```toml
[[visitor]]
name = "office-proxy"
share = "office-proxy"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:11080"
mode = "socks5"
target = "internal.example:443"
```

- El proveedor debe ser un túnel SOCKS5, de proxy directo HTTP o de proxy mixto; los destinos UDP
  requieren SOCKS5 o un proxy mixto.
- La IP realmente resuelta por el proveedor y su política de destino siguen comprobándose.
- Un recurso compartido dinámico exige que la entrada de proxy **no tenga cuentas de propias**; con
  cuentas configuradas se rechaza, para que un secreto compartido no pueda eludir la autenticación
  propia del proxy.
- Sin `target`, el visitante se conecta a la entrada de servicio completa; las cuentas de archivos y de
  proxy siguen autenticando con normalidad.
- Un visitante TCP puede pasar una sesión de SOCKS CONNECT o de proxy HTTP; la dirección UDP del
  servidor que devuelve SOCKS UDP ASSOCIATE no se mapea automáticamente a una dirección local en el
  visitante.

Cuando necesite acceso SOCKS5 completo, defina el visitante con `mode = "socks5"` y referencie un
recurso compartido dinámico, sin `target`: SOCKS CONNECT y UDP ASSOCIATE se resuelven del lado del
visitante, y cada destino pasa por la autenticación del recurso compartido y por la política de destino
del proveedor. `[visitor.auth.users]` configura cuentas SOCKS locales, y cuando existen cuentas el
visitante no se degrada a la autenticación anónima. `max_connections` cuenta juntas las conexiones de
control SOCKS y las asociaciones de destino UDP, así que defínalo al menos en 2 cuando use UDP.

## Administración y tiempo de ejecución

- La página "Share access" de la administración crea, edita y elimina recursos compartidos; los secretos
  se enmascaran de forma predeterminada, y dejar el campo vacío al editar conserva el valor actual.
- Los operadores solo pueden administrar los recursos compartidos del lado del proveedor de los usuarios
  que les pertenecen; las cuentas de solo lectura no pueden hacer cambios.
- La API es `/api/shares` y `/api/shares/NAME`; las escrituras mantienen la versión de configuración y
  la clave de idempotencia de la petición.
- Tras la caída del puente, la escucha local se conserva, las sesiones existentes terminan y las
  peticiones nuevas se reanudan después de reconectar.
- Los cambios en un recurso compartido surten efecto mediante una recarga normal del Daemon, y las
  conexiones existentes se descartan.
- Esta ruta pasa por el relé del servidor; para un enlace directo véase [Directo P2P y respaldo por
  relé](p2p.md).

## Lista de verificación

1. El `--check` del servidor y el `--check` de ambos agentes pasan.
2. El `127.0.0.1:18080` del visitante devuelve el contenido del `127.0.0.1:3000` del proveedor.
3. Una conexión de visitante con el `secret` incorrecto se rechaza.
4. Los nodos fuera de `allow_clients` se rechazan.
5. El servidor no ha ganado ninguna escucha pública nueva (compárelo con `ss -ltnp`).

## Siguientes pasos

- [Directo P2P y respaldo por relé](p2p.md)
- [El módulo felucca-agent](agent.md)
- [Interfaz de administración y configuración centralizada](management.md)
- Ejemplos: [visitor-server.toml](../../../examples/visitor-server.toml) ·
[private-agent.toml](../../../examples/private-agent.toml) ·
[visitor-agent.toml](../../../examples/visitor-agent.toml)
