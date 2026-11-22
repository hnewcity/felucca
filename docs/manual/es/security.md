# Control de acceso, cuotas y codificación de flujo

Esta página reúne la configuración relacionada con la seguridad: ACL de origen, ACL de destino,
GeoIP/GeoSite, cuotas por capas y límites de tasa, instantáneas de tráfico y la compresión y el cifrado
opcionales por túnel.

## ACL de origen

```toml
[client.agent-1]
acl_mode = "allow"                 # off | allow | deny
acl_rules = "203.0.113.0/24, 2001:db8::/32"
```

Las reglas son entradas IP/CIDR separadas por comas o por saltos de línea. Los `acl_mode`/`acl_rules` de
nivel superior del servidor, `[user.<name>]`, `[client.<id>]` y las reglas del túnel se
**intersecan**; un cliente se asocia con un usuario en su propia configuración mediante
`user = "<name>"`, y el agente no puede cambiar esa asociación.

Estas reglas se aplican a los clientes que llegan a una entrada pública, y **no se usan para rechazar la
dirección del puente de un agente** — la identidad del puente la verifica el vkey. Las reglas actúan
sobre la IP de origen de los clientes que llegan al proxy, no sobre la dirección de destino.

Las ACL de origen admiten GeoIP. `acl_rules` también puede escribirse en `[[tunnel]]`,
`[local.<name>]` y `[[visitor]]`.

## Política de destino

Se aplica del lado del agente y cubre los back-ends fijos, las sondas de estado y los destinos del proxy
directo:

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

| Elemento | Descripción |
| --- | --- |
| `allow` vacío | No hay restricción sobre los destinos permitidos |
| `deny` | Siempre tiene prioridad |
| `ports` vacío | Se permite cualquier puerto distinto de cero |
| Sintaxis | IP/CIDR, `domain:`, `suffix:`, `regex:`, `*` |
| Geo | `geoip:CN`, `geosite:category-ads-all`, `geosite:custom:LOCAL@fast@!ads` |

`@attribute` exige que el atributo esté presente, `@!attribute` exige que esté ausente, y varias
condiciones se intersecan. `geoip:private` cubre los rangos IPv4/IPv6 privados, de bucle local, de
enlace local y de CGNAT incluidos de fábrica, y no necesita ningún archivo de datos. El nombre y la IP
resuelta se comprueban juntos; los nombres no distinguen mayúsculas y minúsculas, y un punto final se
normaliza.

Un archivo ausente, un grupo ausente, una expresión regular incorrecta o un protobuf corrupto **son un
error**, y no se degradan a una regla vacía que permita todo. Los datos se cargan al arrancar, así que
reinicie el proceso correspondiente después de actualizar un archivo de datos. Cada extremo usa sus
propios archivos de datos declarados localmente, y el lado remoto solo puede referenciar grupos y alias.

## Datos de Geo

```toml
[geo]
geoip = "data/geoip.dat"
geosite = "data/geosite.dat"

[geo.extra]
mycorp = "data/mycorp.dat"
```

Apunte estos campos a archivos `.dat` protobuf de V2Ray, con rutas relativas al directorio de trabajo
del proceso. Los prefijos IP usan búsquedas de rangos compiladas; los dominios admiten coincidencia
exacta, por sufijo, por palabra clave y por expresión regular.

## Cuotas por capas

`[user.<name>.limits]`, `[tunnel.options.limits]` y
`[http.host_limit."<domain pattern>"]` aceptan los mismos campos:

```toml
max_connections = 20
rate_limit = 1048576
quota = { flow_limit = 1073741824, expires_at = 1893456000 }
```

Las unidades de tráfico y de tasa son bytes, y `expires_at` está en segundos Unix. Omitir un campo
significa que no hay límite; una tasa de 0 significa que no hay límite, mientras que 0 conexiones bloquea
explícitamente las conexiones nuevas. Varios clientes de un usuario, varias conexiones de un túnel y
varias rutas de un dominio comparten sus propios contadores y límites.

Las distintas capas de tasa se cobran al mismo tiempo, y el tiempo de espera toma la **capa más
estricta**; una petición fallida de hueco de conexión devuelve los huecos concedidos por las capas
anteriores. Los límites de dominio HTTP se seleccionan por el Host/SNI original, un dominio exacto tiene
prioridad sobre un comodín, y no se ven afectados por la reescritura del Host en el back-end. El recuento
de conexiones HTTP cubre las conexiones reales a los back-ends; varias peticiones HTTP/2 pueden reutilizar
una misma conexión.

TCP, HTTP, SNI y UDP actualizan todos los contadores de todas las capas. TCP se comprueba y se limita en
tasa en bloques de 16 KiB, y UDP se trata por datagrama. El tráfico en vuelo en transferencias
concurrentes puede llevar el total ligeramente por encima de la cuota, así que esto **no** es un corte
exacto byte a byte. HTTP cuenta los bytes del protocolo hacia el back-end, incluidas las cabeceras y la
sobrecarga del protocolo; UDP cuenta solo la carga útil. Una cuota aborta las conexiones que están
transfiriendo, y los recuentos no se usan para una facturación estricta.

## Instantáneas de tráfico

```toml
[state]
path = "data/traffic.json"
save_interval = "5s"
```

Una vez habilitadas, la instantánea se reemplaza de forma atómica en cada intervalo, y los recuentos
finales se guardan en una salida normal y después de que las conexiones terminen. La instantánea incluye
los recursos fuera de línea, de modo que siguen contando contra las cuotas después de un reinicio del
servidor; un fallo brusco puede perder los datos del último intervalo de guardado. Una sola instantánea
solo puede escribirla un servidor. Una instantánea corrupta o de una versión desconocida impide el
arranque, y el archivo original se conserva.

Esto es una instantánea de tráfico; la configuración en tiempo de ejecución de la interfaz de
administración se almacena aparte en `admin.state_path` / `--runtime-config`, y las dos no deben usar la
misma ruta. Una vez habilitadas las instantáneas, fije el `id` de sus túneles.

## Codificación por flujo

```toml
[tunnel.options.payload]
compress = true
encrypt = true
```

| Opción | Implementación | Comportamiento |
| --- | --- | --- |
| `compress` | Snappy | Funciona en bloques de como máximo 16 KiB de datos en bruto, y usa un bloque comprimido solo cuando es más pequeño; el tamaño declarado se comprueba antes de la descompresión |
| `encrypt` | ChaCha20-Poly1305 | Cada flujo intercambia un nonce nuevo, y el vkey del agente verifica la identidad de ambos lados y la petición de enrutado completa; HKDF deriva dos claves direccionales independientes |

Con la compresión activada, los datos se **comprimen antes de cifrarse**. Las cabeceras de longitud, los
números de secuencia y el registro final están cubiertos por comprobaciones de integridad, y una
manipulación, una reproducción o un registro final ausente cierran el flujo de datos. Cuando los dos
extremos no coinciden, la negociación se rechaza, sin respaldo automático al texto claro. Cada sentido
almacena en memoria un solo registro acotado.

Esta capa de codificación vive dentro del flujo mux/QUIC y no añade una capa de multiplexación propia;
las cabeceras PROXY, el contenido HTTP/WebDAV y los datagramas UDP pasan todos por la misma codificación.
Las cuotas de tráfico y los límites de tasa se siguen calculando sobre los **bytes reenviados en bruto**.

Límite de seguridad:

- El cifrado independiente protege la parte de datos del enrutado; el registro de configuración y los
  mensajes de control del puente siguen protegidos por el transporte de puente elegido, así que TLS, WSS
  o QUIC siguen siendo la opción de despliegue predeterminada.
- Esta capa PSK **no proporciona por sí sola confidencialidad hacia adelante**.
- La compresión cambia el tamaño observable del registro; mantenga `compress = false` cuando necesite
  ocultar las características de la compresión.

## Manejo de secretos

- Los secretos en bruto nunca llegan a los registros; la salida de Debug y Display está enmascarada.
- La persistencia debe conservar el secreto, y el enmascaramiento solo ocurre al mostrarlo; cuando añada
  un campo de credenciales, compruebe al mismo tiempo la ruta de enmascaramiento.
- Las instantáneas de configuración contienen los secretos reales, y en Unix los archivos nuevos son
  0600 y los directorios nuevos 0700.
- No escriba el token Bearer de administración en bruto en la configuración del servidor; escriba
  únicamente `token_hash`.

## Siguientes pasos

- [Túneles y mapeos de puertos](tunnels.md)
- [Proxy inverso HTTP/HTTPS](http-services.md)
- [Interfaz de administración y configuración centralizada](management.md)
