# Proxy directo

Felucca puede exponer una entrada de proxy SOCKS5 o HTTP desde detrás de un NAT. Esta página cubre los
tres modos, la autenticación por cuentas, las ACL de destino y el comportamiento con UDP.

## Tres modos

```toml
[[tunnel]]
mode = "socks5"
port = 9080
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
mode = "httpproxy"
port = 9081
bind = "127.0.0.1"

[[tunnel]]
mode = "mixedproxy"
port = 9082
bind = "127.0.0.1"
```

| `mode` | Soporte |
| --- | --- |
| `socks5` | CONNECT + UDP ASSOCIATE; IPv4/IPv6; resolución DNS del lado del agente; BIND devuelve "unsupported command" |
| `httpproxy` | Peticiones de URL absoluta + CONNECT |
| `mixedproxy` | Distingue SOCKS5 de HTTP por el inicio de la petición |

Los ejemplos de proxy directo se enlazan solo a `127.0.0.1`. Cuando necesite acceso remoto, defina
**también** una lista de permitidos de origen; de lo contrario, cualquiera que pueda alcanzar ese puerto
podrá usar su salida.

## Autenticación por cuentas

Añada una tabla de cuentas bajo el túnel actual para habilitar la autenticación:

```toml
[tunnel.options.auth.users]
alice = "replace-with-a-password"
bob = "another-password"
```

| Entrada | Autenticación | Respuesta de fallo |
| --- | --- | --- |
| SOCKS5 | Método de usuario y contraseña, **sin degradación** a anónimo | Fallo de autenticación |
| Proxy directo HTTP / CONNECT | `Proxy-Authorization: Basic` | 407 |
| Sitio del proxy inverso HTTP | `Authorization: Basic` | 401 |

Sin tabla de cuentas, se permite el acceso anónimo. Las credenciales de proxy y de sitio verificadas se
eliminan antes de enviar la petición al back-end. La configuración y los mensajes de registro contienen
las contraseñas de las cuentas, así que el puente debería usar el TLS, WSS o QUIC predeterminado;
`tcp` y `ws` son transportes en texto claro que se eligen de forma explícita.

Los túneles de archivos usan `tunnel.file.auth`, que no puede usarse junto con
`tunnel.options.auth`.

## Política de destino

Los túneles con un back-end fijo usan `target_policy` para limitar el destino real. Un proxy directo lo
necesita en particular:

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

Un `allow` vacío no impone ninguna restricción sobre qué destinos se permiten, `deny` siempre tiene
prioridad, y un `ports` vacío permite cualquier puerto distinto de cero. Las reglas admitidas son
IP/CIDR, `domain:` para un nombre de dominio exacto, `suffix:` para un sufijo (incluido el dominio raíz
y los subdominios a cualquier profundidad), `regex:` para una expresión regular y `*` para coincidir con
todos los destinos. Los nombres no distinguen mayúsculas y minúsculas, y un punto final se normaliza.

El DNS se resuelve primero, y después se comprueban uno a uno el **nombre original y la IP real**. Una
conexión usa solo las direcciones que pasan las comprobaciones y no se resuelve por segunda vez, de modo
que un dominio permitido no puede eludir una IP denegada. También se admiten GeoIP y GeoSite; véase
[Control de acceso, cuotas y codificación de flujo](security.md).

## UDP ASSOCIATE de SOCKS5

- Solo se acepta la IP del interlocutor de la conexión de control: cuando la petición nombra un puerto,
  ese puerto se fija, y en caso contrario se enlaza el puerto de origen del primer paquete UDP válido.
- Cerrar la conexión TCP de control borra todas las asociaciones de destino.
- Cada sesión permite como máximo 64 destinos, cada uno con 8 paquetes en cola, y las entradas
  inactivas se reclaman transcurridos 60 segundos.
- Un FRAG distinto de cero se descarta, y el reensamblado de fragmentos SOCKS no está implementado.

## Tasa y cuota

- `tunnel.options.limits.rate_limit` es un límite de bytes por segundo compartido por todos los túneles
  del agente y por ambos sentidos, y permite una ráfaga inicial de un segundo de tráfico.
- `quota.flow_limit` cuenta ambos sentidos, y `quota.expires_at` usa segundos Unix.
- Las cuotas se comprueban periódicamente y abortan las conexiones que están transfiriendo, así que el
  tráfico concurrente en vuelo puede superar la cuota. Los recuentos no se usan para una facturación
  estricta.

## Siguientes pasos

- [Túneles y mapeos de puertos](tunnels.md)
- [Recursos compartidos privados y visitantes](private-shares.md) — visitantes SOCKS sin puerto público
- [Directo P2P y respaldo por relé](p2p.md)
