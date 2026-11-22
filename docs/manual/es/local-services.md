# Destinos locales del servidor y rutas anidadas

`[local.<name>]` permite que el servidor preste servicios él mismo, sin agente y sin credenciales de
puente. Los back-ends de los túneles locales también pueden usar alias que entran en otro túnel
registrado, incluidos túneles de un agente, lo que se compone en rutas anidadas.

## Un grupo de servicios locales

```toml
[user.office]
[user.office.limits]
max_connections = 100

[local.office]
user = "office"
dial_timeout = "10s"
[local.office.network]
local_ip = "127.0.0.1"
forward_local_ip = true

[[local.office.tunnel]]
id = 1
mode = "tcp"
port = 9001
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
[local.office.tunnel.health]
type = "tcp"
interval = "10s"
timeout = "2s"
```

Cada grupo puede definir `user`, `acl_mode`/`acl_rules`, `limits`, `network` y `dial_timeout`,
y puede usar la misma configuración de TCP/UDP, HTTP/SNI, proxy, archivos, balanceo de carga, sondas y
ACL que un agente. Cuando solo ejecute servicios locales, puede omitir la tabla de clientes.

Un grupo local se registra como `local.<name>` en el registro y en las instantáneas de tráfico, y
**este espacio de nombres no puede usarse como key_id de un agente externo**. Un grupo local no tiene
puente, así que la configuración de proxy ascendente se rechaza. Solo `forward_local_ip = true` usa la
IP de salida para el back-end. Las raíces de archivos, los certificados y la configuración de back-ends
se preparan al arrancar, y un conflicto de escucha hace que ese servicio no llegue a iniciarse.

## Alias de back-end

| Dirección | Comportamiento |
| --- | --- |
| `tunnel://local.office/7` | Entra en la entrada de servicio de otro túnel local registrado |
| `tunnel://agent-1/7` | Entra en un túnel registrado de un agente en línea |
| `file://agent-1/7` | Igual que el anterior, pero exige que el destino registrado sea de modo `file`; se conservan los límites de directorio, el estado de solo lectura y la autenticación de archivos |
| `bridge://tcp`, `tls`, `ws`, `wss` | Entra en la entrada de puente correspondiente; el agente sigue necesitando autenticación mutua, y TLS usa el certificado del puente |
| `bridge://http` | Entra en la entrada HTTP del servidor, donde el Host, la autenticación del sitio y las comprobaciones de permisos siguen ejecutándose |
| `bridge://web` | Se conecta al extremo de administración, que autentica del mismo modo |

Los números de túnel deben ser explícitos y estables. Un grupo local con un `user` solo puede referenciar
destinos de ese mismo usuario; un grupo sin `user` lo gestiona el operador del servidor y puede
referenciar a través de usuarios. Los alias `bridge` solo se permiten en grupos de operador. Un agente
remoto y las peticiones de proxy dinámico no pueden usar un alias como destino de red.

Los alias no aceptan un `backend.tls` adicional, una sonda de estado de red ni una ACL de destino de
entrada: los permisos los aplica el destino de cada salto. Un alias HTTPS/SNI puede pasar los datos TLS
directamente, y el sitio de destino los termina o los deja pasar. UDP solo puede anidarse en UDP, y los
demás modos usan la entrada de flujo. La autenticación del proxy SOCKS5/HTTP no se omite.

## Control de bucles y contabilidad

- Cada salto conserva la IP y el puerto de origen originales y el historial de enrutado.
- Un `(client, id)` repetido o más de **16 saltos** se rechaza de inmediato.
- El grupo de conexiones HTTP también está aislado por historial, de modo que una conexión reutilizada
  no puede eludir la comprobación de bucles.
- Cada túnel que realmente se atraviesa obtiene su propio permiso de conexión y registra el tráfico;
  cuando el mismo usuario o grupo atraviesa varios saltos, la cuota agregada también se cobra por cada
  salto.
- La contabilidad de UDP cubre la carga útil del mensaje y excluye las tramas internas de longitud.

## Patrones habituales

| Escenario | Cómo |
| --- | --- |
| Exponer también un servicio del host del servidor | `[local.ops]` + `[[local.ops.tunnel]]` |
| Añadir terminación TLS delante de un servicio interno | Un túnel HTTP en un grupo local con `tls_mode = "offload"` |
| Añadir otra entrada sobre un servicio interno, como autenticación del sitio | Un grupo local exterior con el back-end escrito como `tunnel://agent-1/7` |
| Reutilizar la autenticación del extremo de administración desde un servicio interno | Escriba el back-end como `bridge://web` |

## Siguientes pasos

- [Túneles y mapeos de puertos](tunnels.md)
- [El módulo felucca-server](server.md)
- [Control de acceso, cuotas y codificación de flujo](security.md)
- Ejemplos: [local-server.toml](../../../examples/local-server.toml)
