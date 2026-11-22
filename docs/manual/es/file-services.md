# Servicios de archivos y WebDAV

`mode = "file"` sirve archivos directamente desde el agente, sin ningún servidor HTTP aparte. Puede
usar un puerto público dedicado o aprovechar la entrada de dominio HTTP del servidor. Se admiten WebDAV,
las peticiones Range, los listados de directorio y la autenticación por cuentas.

## Puerto dedicado

```toml
[[tunnel]]
id = 100
mode = "file"
port = 9001
bind = "127.0.0.1"

[tunnel.file]
root = "public"
prefix = "/files"
read_only = true
auto_index = true

[tunnel.file.auth.users]
reader = "replace-this-password"
```

| Campo | Descripción |
| --- | --- |
| `root` | Obligatorio. Un directorio que **ya existe** en el agente, relativo al directorio de trabajo del proceso; al arrancar se abre un descriptor de directorio |
| `prefix` | Prefijo de URL |
| `read_only` | Su valor predeterminado es `true` |
| `auto_index` | Listados de directorio |
| `auth.users` | Tabla de cuentas; déjela vacía para servir de forma anónima |

Las peticiones no pueden escapar de `root` mediante `..`, rutas absolutas ni enlaces simbólicos, y los
destinos de COPY/MOVE están sujetos a la misma restricción. `root` y la configuración de las cuentas de
archivos permanecen en el agente y **no** se incluyen en el mensaje de registro del servidor.

## Entrada de dominio HTTP

Cuando defina `hosts`, los servicios de archivos usan la entrada de dominio HTTP/HTTPS/HTTP3 del
servidor, y `port` debe coincidir con esa entrada:

```toml
[[tunnel]]
mode = "file"
port = 8443
hosts = ["files.example.com"]

[tunnel.file]
root = "public"
read_only = false
```

En este modo también puede definir `tunnel.options.http.backend_http2 = true`. Las ACL de origen, las
cuotas y los límites de tasa siguen aplicándose. El modo de archivos **no** configura un back-end de
red, una política de red de destino, una salida PROXY ni un modo TLS en bruto.

## Métodos WebDAV

| Modo | Métodos permitidos |
| --- | --- |
| Solo lectura (predeterminado) | `GET`, `HEAD`, `PROPFIND`, `OPTIONS` |
| `read_only = false` | Añade `PUT`, `DELETE`, `MKCOL`, `COPY`, `MOVE`, `LOCK`, `UNLOCK` |

Los bloqueos se comparten entre las distintas conexiones de un mismo túnel de archivos y se liberan
después de que el agente se reinicie. Se admiten las peticiones Range, las peticiones condicionales, los
listados de directorio y `index.html`. Los archivos grandes se transfieren por fragmentos y usan
contrapresión en lugar de almacenarse completos en memoria.

## Archivos bajo configuración centralizada

Cuando el servidor envía un servicio de archivos, solo puede hacer referencia a una capacidad de
directorio con nombre declarada por adelantado en el agente:

```toml
# agent
[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

```toml
# server: inside [[managed.<client>.tunnel]]
[tunnel.file]
root = "documents/shared"
```

El servidor no puede enviar una ruta absoluta arbitraria. Las rutas y los enlaces simbólicos no pueden
escapar de un directorio autorizado, y un directorio de solo lectura no puede pasarse a escritura
mediante configuración remota.

## Lista de verificación

1. `felucca-agent --config agent.toml --check` pasa.
2. `curl -i http://127.0.0.1:9001/files/` muestra el listado del directorio o devuelve `index.html`.
3. Una petición con `Range: bytes=0-99` devuelve 206.
4. En modo de solo lectura, `PUT` devuelve 405.
5. `curl --path-as-is 'http://127.0.0.1:9001/files/../../etc/passwd'` se rechaza.

## Siguientes pasos

- [Túneles y mapeos de puertos](tunnels.md)
- [Proxy inverso HTTP/HTTPS](http-services.md)
- [Interfaz de administración y configuración centralizada](management.md)
- Ejemplos: [file-server.toml](../../../examples/file-server.toml) ·
  [file-agent.toml](../../../examples/file-agent.toml)
