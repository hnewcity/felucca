# Proxy inverso HTTP/HTTPS

Esta página cubre cómo exponer servicios HTTP internos por dominio y ruta: enrutado, reescritura de
cabeceras y de Host, redirecciones, CORS, páginas de error, SNI, HTTP/2 y HTTP/3, puertos compartidos y
cabeceras de reenvío.

## Registre una ruta HTTP

```toml
# Server side
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"

# Agent side
[[tunnel]]
mode = "http"
port = 8080                       # Must match http.bind or http.tls_bind
hosts = ["app.example.test", "*.example.test"]
balance = "round_robin"

[tunnel.options.http]
path = "/api/"
scheme = "any"
rewrite_prefix = "/v2/"
host = "backend.internal"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

Un túnel `http` no abre ninguna escucha aparte; `port` decide a qué escucha HTTP se engancha, y es
alcanzable tanto por HTTP como por HTTPS.

## Reglas de enrutado

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"        # any | http | https
```

Prioridad de coincidencia: **primero dominio exacto, luego el prefijo de ruta más largo, luego esquema
exacto**. Los nombres de dominio no distinguen mayúsculas y minúsculas, y un punto final se normaliza.

| Patrón | Coincide con |
| --- | --- |
| Exacto `app.example.test` | Solo con ese dominio |
| Comodín `*.example.test` | Coincide con `a.example.test`, pero **no** con `example.test` ni con `a.b.example.test` |

Los registros duplicados con el mismo dominio, la misma ruta y el mismo protocolo se rechazan. Una
petición sin ruta coincidente devuelve 404. Puede registrar varias rutas para un mismo dominio y
asignarlas a túneles distintos.

## Opciones de petición y respuesta

```toml
[tunnel.options.http]
rewrite_prefix = "/v2/"
host = "backend.internal"
cors = true
redirect = "https://new.example{path}"
force_https = true
request_headers = { x-source = "{remote_ip}", x-remove = "" }
response_headers = { x-proxy = "felucca" }
error_pages = { "502" = "<h1>Service unavailable</h1>" }
```

| Opción | Comportamiento |
| --- | --- |
| `rewrite_prefix` | Reescribe el prefijo y conserva el resto de la ruta y la cadena de consulta |
| `host` | Reescribe el Host que se envía al back-end |
| `request_headers` | Plantillas de cabeceras; un valor vacío elimina esa cabecera |
| `response_headers` | Plantillas de cabeceras de respuesta |
| `cors` | Devuelve `Access-Control-Allow-Origin: *`, y un preflight válido devuelve 204 directamente. Un preflight no requiere cuenta del sitio, pero la petición real sí requiere autenticación |
| `redirect` | Devuelve 307 |
| `force_https` | Devuelve 301, y el destino usa el puerto de la escucha HTTPS del servidor (el 443 se omite). Debe existir una escucha HTTPS cuando lo registre |
| `error_pages` | Reemplaza el cuerpo de la respuesta según el código de estado. Admite de 400 a 599, con 64 KiB como máximo por página |

Las plantillas admiten `{host}`, `{path}` (incluida la cadena de consulta), `{scheme}`,
`{remote_ip}` y `{remote_port}`. Los valores se toman de la petición original, y el resultado de una
sustitución no se expande de nuevo. Las cabeceras de límite de mensaje, como Content-Length y
Transfer-Encoding, y las cabeceras hop-by-hop no se pueden reescribir; el Host tiene su propio campo.

La autenticación del sitio usa `Authorization: Basic`, y un fallo devuelve 401. Las credenciales
verificadas no se reenvían al back-end.

## Modos TLS

```toml
[tunnel.options.http]
tls_mode = "http"        # http | passthrough | offload
```

| Modo | Comportamiento |
| --- | --- |
| `http` | El predeterminado. Un proxy inverso HTTPS ordinario |
| `passthrough` | Pasa el flujo TLS en bruto al back-end según el SNI, y el certificado del back-end se muestra directamente a los visitantes. En este modo `backend.tls` debe estar deshabilitado |
| `offload` | Termina el TLS con el certificado SNI del servidor y luego envía contenido TCP arbitrario al back-end |

`passthrough` y `offload` ocupan el SNI completo, y no se pueden combinar con opciones de ruta o de
cabeceras HTTP, ni con la autenticación del sitio.

## Puertos y protocolos

| Capacidad | Descripción |
| --- | --- |
| Entrada | HTTP/1.1, HTTP/2, HTTP/3 |
| Back-end | HTTP/1.1, HTTP/2 (`backend_http2 = true`) |
| ALPN | Las entradas TLS admiten `h2` y `http/1.1`; las entradas en texto claro admiten HTTP/2 por conocimiento previo |
| WebSocket | Se convierte automáticamente entre los tres tipos de entrada y los dos tipos de back-end. Una entrada HTTP/1.1 devuelve 101, mientras que HTTP/2 o HTTP/3 usa el CONNECT extendido `:protocol = websocket` |
| HTTP/3 | Requiere `http.http3_bind`. Cada conexión QUIC permite como máximo 128 flujos bidireccionales, con una ventana de recepción de 1 MiB por flujo y de 16 MiB por conexión. Se cierra tras 30 segundos de inactividad, y las cabeceras de petición están limitadas a 32 KiB |

HTTP/3 solo es un protocolo de entrada pública; los back-ends siguen usando HTTP/1.1 o HTTP/2. Una
respuesta HTTPS normal lleva una cabecera `Alt-Svc`, y puede anularla con `response_headers`
cuando el puerto asignado externamente sea distinto.

## Compartir puertos

`bridge.bind`, `http.bind` y `http.tls_bind` pueden ser todos el **mismo** IP:puerto, y un solo
socket se demultiplexa por las características del saludo: los agentes conectados directamente por
TLS/WSS usan el ALPN `felucca/9`, mientras que los sitios web usan `h2`/`http/1.1` o ningún ALPN. Un
simple `GET /ws` sigue siendo una petición de sitio web, y solo una mejora a WebSocket correcta entra
en el puente. El puente QUIC y HTTP/3 también pueden compartir una sola dirección UDP,
demultiplexados por ALPN, con certificados del puente y certificados de sitios web que se seleccionan
y renuevan por separado.

## Direcciones de los clientes

De forma predeterminada, las cabeceras `Forwarded`, `X-Forwarded-*` y `X-Real-IP` que envía el cliente
se eliminan, y después `X-Forwarded-For`, `X-Real-IP`, `X-Forwarded-Host` y `X-Forwarded-Proto` se
definen a partir de la dirección de origen directa o del origen PROXY verificado por
`http.proxy_accept`. Defina `forwarded_headers = false` para desactivar esta reescritura, lo que
conviene a los despliegues en los que usted mismo controla toda la cadena de proxies. Las cabeceras
hop-by-hop, y las cabeceras nombradas en `Connection`, no se pasan al siguiente salto.

## Siguientes pasos

- [Certificados, SNI y ACME](certificates.md)
- [Back-ends, balanceo y comprobaciones de estado](backends.md)
- [Servicios de archivos y WebDAV](file-services.md) — entrada de archivos por dominio
- [Control de acceso, cuotas y codificación de flujo](security.md) — cuotas y ACL por dominio
