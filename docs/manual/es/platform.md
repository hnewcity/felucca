# Integración con plataformas externas

`[node]` permite que un plano de control externo gestione este nodo: puede llamar a la API de
administración con un token aparte, o el nodo puede llamar hacia afuera y aceptar peticiones a través de
un WebSocket inverso. Los cambios de configuración también pueden enviarse como llamadas de retorno
firmadas, y se puede importar tráfico externo. Este es el protocolo de control propio de Felucca; no es
compatible a nivel de cable con nps.

## Habilitación y ámbitos

```toml
[node]
id = "edge-a"
state_path = "state/node-events.json"
changes_window = 1024
usage_interval = "10s"

[node.platform.panel]
enabled = true
token = "replace-with-a-random-platform-token"
scope = "account"
user = "office"
mode = "direct"
```

| Campo | Valores y significado |
| --- | --- |
| `scope` | `full` (gestión completa; no puede definir además `user`) o `account` (debe definir un `user` existente, y solo puede gestionar los clientes y túneles de ese usuario y leer el estado de las llamadas de retorno de este inquilino) |
| `mode` | `direct` (el token puede acceder a la API HTTP pública de administración), `reverse` (solo RPC por WS inverso; usar este token directamente contra la API HTTP de administración devuelve 401) o `dual` (ambos sentidos) |
| `enabled = false` | Los dos puntos de entrada dejan de funcionar y las conexiones inversas existentes se cierran |

Los tokens deben tener al menos 16 bytes, y cada plataforma debe usar uno distinto. Por HTTP, envíe
`Authorization: Bearer <token>`. Las identidades de plataforma son independientes de las cuentas de
administración locales. Cambiar un token, su ámbito o la configuración de conexión reemplaza la conexión
inversa; las peticiones de una conexión anterior se siguen comprobando contra la configuración actual.

Las direcciones públicas deben usar HTTPS/WSS; el HTTP/WS en texto claro solo se admite para direcciones
de bucle local literales. WSS verifica las CA públicas de forma predeterminada, y en su lugar puede usar
`trust = { mode = "pinned", fingerprint = "..." }`. La confianza insegura no se admite. Las llamadas de
retorno HTTP verifican las CA públicas y no siguen redirecciones.

## API de gestión directa

Las escrituras usan JSON, `If-Match: "<configuration revision>"` y un `Idempotency-Key` único. La
autenticación por cookie del navegador exige además un Origin y un token CSRF.

| Extremo | Comportamiento |
| --- | --- |
| `GET /api/node/status`, `/api/node/registration` | ID del nodo, ID de arranque, época persistida, versión, ventana de eventos y estado visible de la conexión con la plataforma |
| `GET /api/node/usage` | Estado en línea de los clientes, recuentos de conexiones y bytes bidireccionales acumulados dentro del ámbito actual |
| `GET /api/node/changes?after=<epoch>:<sequence>&limit=100` | La ventana de cambios persistida; `limit` va de 1 a 512 |
| `GET /api/node/callbacks?target=...&offset=0&limit=100` | Colas de llamadas de retorno visibles, estadísticas de entrega y entradas paginadas |
| `POST /api/node/callbacks/replay` | `{"ids":["delivery-uuid"]}`; borra el estado de fallo y vuelve a entregar |
| `POST /api/node/callbacks/delete` | Elimina solo las entradas visibles especificadas |
| `POST /api/node/kick` | `{"ids":["client-name"]}`; desactiva y desconecta los clientes de forma persistente, bloqueando las reconexiones inmediatas |
| `POST /api/node/sync` | El administrador reactiva la configuración persistida local y borra el estado de tiempo de ejecución, las cachés y las conexiones antiguos |
| `POST /api/node/traffic` | Importa deltas de tráfico externos, acumulando atómicamente los contadores del cliente y del usuario propietario |
| `GET/PUT/DELETE /api/platforms/<name>` | Gestión de plataformas por parte del administrador |
| `GET/PUT/DELETE /api/webhooks/<name>` | Creación, modificación y eliminación por parte del administrador de webhooks persistidos |

Los cambios de configuración por lotes, la exportación completa y la restauración reutilizan `/api/batch`
y `GET/PUT /api/config`. Sync y kick reutilizan la misma transacción de configuración y el mismo
historial de idempotencia, y aumentan la revisión de configuración. Las acciones de llamada de retorno y
la importación de tráfico no cambian la revisión de configuración.

### Importación de tráfico

Debe configurar `state.path`:

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

Una petición transporta como máximo 128 elementos. Los recuentos de bytes deben ser enteros sin signo, y
varios elementos del mismo cliente se fusionan primero. No se escribe nada hasta que todas las
comprobaciones de permisos pasan, y si alguna entrada queda fuera de ámbito o no es válida, se rechaza el
lote completo. Importe tráfico que **no esté ya medido por el plano de datos de este nodo**; reimportar
tráfico que el nodo ya midió lo cuenta dos veces. La primera importación actualiza la instantánea a la
versión 2, que las compilaciones anteriores no pueden leer.

## Protocolo WSS inverso v1

Una vez que habilita `[node]`, `GET /api/ws` acepta una mejora a WebSocket HTTP/1.1. Los clientes nativos
usan un token Bearer. Un navegador puede usar su cookie de inicio de sesión del mismo origen, o pasar el
token en el subprotocolo `["felucca-node-v1", "felucca-token.<unpadded base64url token>"]`. El servidor
solo selecciona `felucca-node-v1` y no acepta autenticación mediante parámetros de URL. Mantiene como
máximo 16 WebSocket de administración directos al mismo tiempo.

El nodo marca `reverse_url`, y el saludo transporta el token Bearer de la plataforma y el subprotocolo
`felucca-node-v1`. La plataforma debe seleccionar ese subprotocolo en su respuesta. Todos los mensajes de
aplicación son texto JSON.

El nodo envía `hello` primero (node, boot, epoch, cursor, max_message, max_inflight,
heartbeat_ms), y la plataforma envía después las peticiones:

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

Las respuestas tienen la forma
`{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}`. El RPC reutiliza la
autenticación, el filtrado por inquilino, el CAS y el comportamiento de idempotencia de los gestores de
administración normales. Puede pasar If-Match, Idempotency-Key, Accept y X-Felucca-CSRF, pero no puede
anular Host ni Authorization. El inicio de sesión, el cierre de sesión, SSE, el muestreo de CPU y la
propia mejora a WebSocket no pasan por el RPC.

| Límite | Valor |
| --- | --- |
| Mensaje máximo | 2 MiB |
| RPC concurrentes por conexión | 2 |
| RPC concurrentes en todo el nodo | 16; las peticiones sobrantes devuelven 429 |
| ID de petición en vuelo repetido | Se cierra la conexión |
| Tiempo de espera de la petición | 504; reintente las escrituras con la misma clave de idempotencia |

Cualquiera de los dos lados puede enviar `{"type":"ping","nonce":1}`. El nodo no sobrescribe un ping sin
respuesta, y cierra y reconecta si no llega un pong coincidente en más de dos periodos de latido,
esperando de 1 a 30 segundos. Los cambios de configuración envían un evento, y se envía una resincronización
cuando una suscripción se queda atrás. Una vez que define `usage_interval`, el nodo envía mensajes de uso
filtrados por inquilino periódicamente.

## Llamadas de retorno firmadas

```toml
[node.webhook.events]
user = "office"

[node.webhook.events.delivery]
enabled = true
url = "https://panel.example/felucca/events"
signing_key = "replace-with-a-random-callback-signing-key"
timeout = "5s"
retry_max = 2
retry_backoff = "2s"
queue_max = 100
```

Cabeceras de la petición:

| Cabecera | Contenido |
| --- | --- |
| `X-Felucca-Event-Id` | El UUID de entrega; es el mismo en los reintentos automáticos y en las repeticiones manuales |
| `X-Felucca-Timestamp` | La hora Unix en milisegundos de esta petición |
| `X-Felucca-Signature` | `v1=` seguido del HMAC-SHA256 en hexadecimal en minúsculas |

La entrada de firma es el prefijo UTF-8 concatenado directamente con los **bytes en bruto del cuerpo de
la petición**:

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

El receptor debería verificar la firma, comprobar la ventana temporal y deduplicar por ID de evento. Solo
un 2xx cuenta como éxito. `retry_max` es el número de intentos adicionales más allá de la primera
entrega, y `retry_backoff` es un intervalo fijo. La entrega es **al menos una vez**: si el nodo sale antes
de que una confirmación correcta se escriba en disco, un reinicio puede volver a entregar el mismo ID. Las
llamadas de retorno llevan solo metadatos de cambios, no secretos de configuración; después de recibir un
evento, obtenga los recursos que necesite con la API autorizada en ese momento.

`queue_max` limita las entradas pendientes y fallidas persistidas por destino. Cuando se supera el límite,
las entradas más antiguas se descartan y se cuentan como descartadas; `0` no conserva ninguna entrada
fallida final. El máximo de todo el nodo es de 4096 entradas. Deshabilitar una suscripción conserva su cola
existente. Eliminar la suscripción, o cambiar la URL de recepción, la clave de firma, los filtros o las
plantillas, borra la cola antigua.

La ventana de eventos registra `node.started`, `config.changed`, `config.resync`, y los metadatos de
creación, actualización y eliminación de clientes, usuarios, túneles centralizados y dominios. Las consultas
se filtran por inquilino. Un cursor caducado, demasiado adelantado o con una época que no coincide devuelve
410/reset_required; en ese caso obtenga primero la configuración actual y luego reanude la suscripción desde
la posición devuelta. Esta ventana acotada no conserva diffs completos de configuración y no es un registro
de auditoría a largo plazo.

## Suscripciones y plantillas en tiempo real

Tanto las conexiones WebSocket directas como las inversas pueden usar estas rutas **solo de WS RPC**:
`GET /api/realtime/subscriptions`, y `GET/PUT/DELETE /api/realtime/subscriptions/<name>`. Las suscripciones
no modifican la configuración persistida y no requieren If-Match. Cada conexión permite como máximo 32
suscripciones de 32 KiB como máximo cada una, y todas se liberan cuando la conexión se cae.

```json
{
  "enabled": true,
  "selector": {"event_names":["client.updated"],"clients":["office"]},
  "content": {
    "body_template": {"message":"node {{event.client}} updated","event":"{{event}}"},
    "header_templates": {"x-event-name":"{{event.kind}}"}
  }
}
```

Las plantillas son valores JSON y no ejecutan scripts. Admiten `{{node}}`, `{{id}}`, `{{epoch}}`,
`{{event}}` y `{{event.kind}}`, resource, action, user, client, tunnel, host, target,
sequence, revision, timestamp_ms y traffic. Cuando una cadena completa es un único marcador, el tipo JSON
original se conserva. Las plantillas no pueden anular la autenticación, las cookies, las cabeceras HTTP de
longitud/conexión ni las cabeceras de firma `X-Felucca-*`.

## Notificaciones de tráfico

`traffic_report_interval` (100ms–1h) y `traffic_report_step_bytes` habilitan las notificaciones del
tráfico importado. Una notificación `client.traffic.reported` se envía en la primera importación de un
cliente que tiene una cuota de tráfico, y después cuando se alcanza el umbral temporal o de bytes. La
notificación lleva `ephemeral: true`, de modo que no entra en el historial de cambios, ni en la cola de
fallos, ni en los reintentos. Los receptores lentos pueden perdérsela; lea `/api/node/usage` cuando
necesite los datos completos.

## Lista de verificación

1. `GET /api/node/status` devuelve el ID del nodo, la época y el estado de conexión con la plataforma.
2. Una conexión WSS inversa completa un ciclo de ida y vuelta `hello` → `request` → `response`.
3. El receptor de llamadas de retorno verifica las firmas correctamente, y repetir el mismo ID de evento no
   produce ningún efecto de negocio duplicado.
4. Después de un `kick`, el cliente queda desconectado y no puede reconectar de inmediato.
5. Después de una importación de tráfico, los bytes acumulados en `/api/node/usage` cambian de inmediato.

## Siguientes pasos

- [Interfaz de administración y configuración centralizada](management.md)
- [El módulo felucca-server](server.md)
- Ejemplos: [node-server.toml](../../../examples/node-server.toml)
