# Interfaz de administración y configuración centralizada

`felucca-server` permite gestionar dispositivos, servicios, acceso privado y configuración. Los cambios habituales se sincronizan en línea y los servicios no modificados siguen funcionando. Cambiar la infraestructura, como las escuchas globales, recarga el reenvío; la API de administración sigue disponible.

## Conectarse sin editar la configuración

1. En **Dispositivos → Conectar dispositivo**, indique el nombre, confirme el servidor y elija el sistema y procesador. Extraiga el cliente descargado y abra `Connect.command` (macOS) o `Connect.cmd` (Windows). En Linux ejecute `sh connect.sh` en la carpeta extraída. Incluye el programa y la conexión, sin editar archivos.
2. En **Servicios → Añadir servicio**, seleccione el dispositivo, el tipo y la dirección de su aplicación. El acceso público usa un puerto o dominio del servidor; el privado permite dispositivos autorizados. Los identificadores y secretos se generan automáticamente.
3. Cada dispositivo receptor también recibe su propio paquete y usa la dirección local indicada. Para ejecutar al iniciar sesión, cierre primero la ventana de conexión y ejecute `Install.command`, `Install.cmd` o `sh install.sh`. El cliente se guarda en su carpeta personal.

La navegación principal contiene Resumen, Dispositivos, Servicios, Acceso privado, Monitorización y Ajustes. Las cuentas, cuotas, diagnósticos, escuchas e integraciones están en Ajustes. Las operaciones normales y avanzadas usan controles, sin editor JSON.

Active las conexiones directas en Ajustes para preferirlas, con retransmisión del servidor si fallan. Si están desactivadas, se usa retransmisión. Es acceso por servicio, no una VPN de todo el sistema. Compartir archivos requiere autorizar una carpeta existente en el dispositivo. Los archivos de conexión contienen secretos: protéjalos. Conserve el archivo original del receptor o cree una nueva conexión privada. Editar dispositivos, servicios y acceso no reinicia todo el reenvío. Los servicios modificados o desactivados cierran sus propias conexiones. Los demás servicios y los cambios de nombre conservan las conexiones. Los agentes antiguos solo reconectan el dispositivo afectado. Cambiar las escuchas de bridge, HTTP o P2P sigue recargando el reenvío.

Solo se ofrecen las versiones del cliente presentes en el servidor. Las versiones de macOS sin notarización de Apple se indican y pueden bloquearse tras una descarga normal. Pide una versión de distribución al administrador sin desactivar la protección del sistema.

## Habilitación

```sh
target/release/felucca-server --hash-password      # read one password line from stdin; >=12 bytes
target/release/felucca-server --generate-token     # Bearer token and its stored hash
```

```toml
[admin]
bind = "127.0.0.1:8088"
state_path = "state/runtime-config.json"
profiling = true
# required for non-loopback binds: the origin the browser uses; use https://
# unless you accept plaintext management traffic (then http://host:port)
# origin = "https://admin.example.com"

[admin.account.root]
role = "admin"
password_hash = "pbkdf2-sha256$600000$..."
```

Los archivos de configuración pueden ser TOML o JSON. El ejemplo usa la dirección de administración
`http://127.0.0.1:8088` y la cuenta `root`. La contraseña del ejemplo sirve solo para una demostración
local; sustitúyala por su propio hash en los despliegues reales.

## Cuentas y roles

| `role` | Permisos |
| --- | --- |
| `admin` | Gestiona todos los recursos y cuentas; no se define ningún campo `user` |
| `operator` | Gestiona los nodos y los túneles centralizados del `user` asignado |
| `viewer` | Acceso de solo lectura a los recursos del `user` asignado |

Las cuentas de operador y de visor deben hacer referencia a un usuario existente. Los servicios locales
del servidor pueden alcanzar directorios locales, así que solo un administrador puede modificarlos. Las
cuotas de usuario, las cuentas de administración y la configuración completa también son solo para
administradores. Conserve al menos una cuenta de administrador habilitada.
`disabled = true` desactiva una cuenta.

Las contraseñas usan PBKDF2-SHA256 con una sal aleatoria y 600000 iteraciones fijas. Las sesiones se
mantienen en memoria. Las cookies son HttpOnly y SameSite=Strict, y también se marcan como Secure cuando
configura HTTPS. Las escrituras desde el navegador exigen un Origin del mismo origen y un token CSRF.
Cambiar una contraseña, desactivar una cuenta o eliminar una cuenta invalida las sesiones antiguas, y un
reinicio del proceso también invalida las sesiones por cookie.

Para la automatización, use un token Bearer: copie el `token_hash` que imprime `--generate-token` en el
`token_hash` de la cuenta, y envíe el token en bruto con cada llamada. Las peticiones Bearer no necesitan
el token CSRF de cookie, pero las mismas comprobaciones de permisos de la cuenta se siguen aplicando.
**Nunca escriba el token en bruto en la configuración del servidor.**

## Transacciones de escritura

Cada escritura requiere dos cabeceras:

```text
If-Match: "current configuration revision"
Idempotency-Key: unique identifier for this operation
```

- Una revisión obsoleta devuelve 409.
- El mismo ID con la misma petición devuelve el resultado anterior y **no** reinicia de nuevo el plano de
  datos; el mismo ID con una petición distinta devuelve 400.
- Las 256 operaciones correctas más recientes se almacenan junto con la configuración, de modo que los
  reintentos de operaciones como DELETE siguen reconociéndose después de un reinicio.
- `durable = false` significa que la sincronización de directorios todavía no se ha confirmado.
- Editar dispositivos, servicios y acceso no reinicia todo el reenvío. Los servicios modificados o desactivados cierran sus propias conexiones. Los demás servicios y los cambios de nombre conservan las conexiones. Los agentes antiguos solo reconectan el dispositivo afectado. Cambiar las escuchas de bridge, HTTP o P2P sigue recargando el reenvío.

Cambiar la dirección de enlace de la administración, las rutas de los archivos de certificados, el origen
público o la ruta de estado requiere reiniciar el proceso. No puede cambiarlos con el servidor en
funcionamiento.

## Páginas de la interfaz

| Página | Contenido |
| --- | --- |
| Vista general | Estado de los dispositivos, servicios y conexiones visibles para esta cuenta. |
| Dispositivos | Elige un ordenador o servidor conectado para gestionar sus servicios. |
| Servicios | Elige el dispositivo y la aplicación. Felucca los conecta con la dirección elegida. |
| Acceso privado | Elige un servicio privado y genera el archivo para otro dispositivo. No necesitas un puerto público del servicio. |
| Supervisión | Consulta métricas del host, conexiones y tráfico acumulado. |
| Ajustes | Conexiones del servidor, permisos e integraciones. Las tareas diarias están en la navegación principal. |

## API

La API habla JSON, acepta cuerpos de petición de hasta 2 MiB y rechaza las peticiones de navegador de
origen cruzado. Las lecturas devuelven normalmente `{revision, config}`, y las lecturas de configuración
censuran los secretos de forma predeterminada. Cuando edite, un marcador `<redacted>` existente conserva
el valor actual de ese campo; las credenciales de recursos nuevos no pueden usar ese marcador.

| Ruta | Método y propósito |
| --- | --- |
| `/api/login` | POST `{account, password}`; el Origin es obligatorio |
| `/api/session` | GET de la identidad actual y del token CSRF |
| `/api/logout` | POST para revocar la sesión por cookie |
| `/api/overview` | GET de nodos en línea, conexiones, tráfico y túneles registrados |
| `/api/users`, `/api/users/<id>` | GET de la lista o de una sola entrada; PUT y DELETE sobre una sola entrada |
| `/api/clients`, `/api/clients/<id>` | GET de la lista o de una sola entrada; PUT y DELETE sobre una sola entrada |
| `/api/managed/<client>` | GET y PUT de toda la configuración centralizada; DELETE restaura la configuración local |
| `/api/managed/<client>/tunnels/<id>` | GET, PUT y DELETE de un solo túnel |
| `/api/locals`, `/api/locals/<id>` | GET de la lista o de una sola entrada; PUT y DELETE sobre una sola entrada |
| `/api/locals/<group>/tunnels/<id>` | PUT y DELETE de un túnel local del servidor |
| `/api/shares`, `/api/shares/<name>` | GET, PUT y DELETE de un recurso compartido privado |
| `/api/accounts`, `/api/accounts/<id>` | GET de la lista; PUT y DELETE sobre una sola entrada |
| `/api/config` | GET de la configuración completa censurada; PUT para importar una configuración completa |
| `/api/config?secrets=1` | Exportación explícita con credenciales, solo para administradores |
| `/api/validate` | POST para validar una configuración completa sin iniciar escuchas |
| `/api/batch` | POST de un lote atómico de cambios, con 128 operaciones como máximo |
| `/api/events` | GET de SSE para los cambios de revisión de configuración, con un latido de 10 segundos y recomprobación de sesión |
| `/metrics`, `/api/metrics` | GET de métricas de Prometheus, filtradas por los permisos de la cuenta |
| `/api/profile` | POST para descargar un gráfico de llamas de CPU de duración limitada |

Un lote usa una sola revisión y un solo ID de operación. Todas las operaciones se validan antes de
aplicar nada, y si alguna operación queda fuera de alcance o la configuración no es válida, no se confirma
ninguna de las operaciones precedentes:

```json
{
  "operations": [
    {
      "method": "PUT",
      "path": "/api/managed/office/tunnels/1",
      "value": {
        "id": 1,
        "enabled": false,
        "mode": "tcp",
        "port": 18080,
        "backend": [{"addr": "127.0.0.1:3000"}]
      }
    }
  ]
}
```

Cuando haga `PUT` de una cuenta puede enviar una `password`, y el servidor genera el hash. Si la omite,
se conserva el `password_hash` existente.

## Configuración centralizada

El servidor almacena en `[managed.<client-id>]` la configuración que los agentes deben ejecutar:

```toml
[[managed.office.tunnel]]
id = 1
mode = "tcp"
port = 18080
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
```

Un agente debe habilitar explícitamente `[remote] enabled = true`, y la conexión principal debe usar
TLS/WSS/QUIC autenticado. El servidor también comprueba que el puente real esté cifrado, de modo que las
conexiones en texto claro no pueden pedir configuración. Los envíos usan JSON canónico con un resumen
digest, y las credenciales de la configuración no aparecen en los registros de depuración de los mensajes
de control.

El plano de administración solo puede cambiar el contenido de los túneles. No puede enviar identidades de
agente nuevas, proxies de salida ni límites de autorización locales del servidor. Los servicios de
archivos deben tener sus capacidades de directorio con nombre configuradas por adelantado; véase
[Servicios de archivos y WebDAV](file-services.md).

La excepción: cuando un agente define `remote.console_scope = true`, el `target_policy` de `[managed.<client-id>]` sustituye su alcance de destino.

| Operación | Efecto |
| --- | --- |
| `tunnel.enabled = false` | Conserva la configuración y el ID estable, pero no registra la escucha ni inicia la sonda del back-end |
| Dejar una entrada `managed` vacía | Detiene todos los túneles centralizados de ese nodo |
| Eliminar toda la entrada `managed` | Restaura la configuración local de túneles del agente desde el arranque |

Las páginas de administración distinguen entre "publicado", "a la espera de que el nodo lo aplique" y
"deshabilitado".

## Lista de verificación

1. `felucca-server --config server.json --check` pasa.
2. Abra la dirección de administración en un navegador, inicie sesión y compruebe que no hay errores de
   JS.
3. Cree un túnel TCP y confirme que el back-end devuelve contenido de verdad.
4. `GET /api/config` no muestra secretos en texto claro.
5. Después de reiniciar el servidor, la configuración y los túneles siguen ahí, y reintentarlo con el
   mismo `Idempotency-Key` no aplica el cambio dos veces.

## Siguientes pasos

- [Integración con plataformas externas](platform.md) — deje que otro sistema gestione este nodo
- La sección `[remote]` de [el módulo felucca-agent](agent.md)
- [Servicios, actualizaciones, registro y supervisión](operations.md)
- Ejemplos: [admin-server.toml](../../../examples/admin-server.toml) ·
  [managed-agent.toml](../../../examples/managed-agent.toml)

La consola ofrece formularios estructurados para pesos y TLS de backends, comprobaciones de estado, rutas HTTP, cuentas de servicio, ACL y cuotas, servicios de archivos, grupos de túneles locales y mapeos de puertos.  La página de cuentas permite generar, reemplazar y revocar tokens de gestión; el nuevo token se descarga después de guardarlo.

La validación indica los ajustes que requieren reiniciar. La exportación proporciona una configuración inicial, una instantánea de ejecución con credenciales e instrucciones de reemplazo sin conexión. Detenga el proceso y respalde los archivos existentes primero. La instantánea tiene prioridad sobre el archivo inicial, incluida la ruta elegida con `--runtime-config`. Conserve los diarios de tráfico y plataforma; no sobrescriba la instantánea de un proceso activo.

El diagnóstico muestra tráfico por túnel, resultados existentes de comprobaciones, sesiones P2P activas y eventos recientes de retransmisión. Los administradores pueden inspeccionar certificados activos, reintentar solicitudes ACME fallidas, subir certificados y claves PEM al borrador y leer una cola de registro limitada con credenciales ocultas. Los agents antiguos sin flujo de diagnóstico opcional muestran no disponible; el reenvío continúa. Consultar resultados no inicia nuevas comprobaciones. La generación de configuraciones de agent y visitante conserva la autorización local de directorios y destinos: revise y ejecute el archivo en el agent usando allí `--check` antes de iniciar. Los comandos de servicio y actualización se ofrecen para revisión, sin ejecutarse desde la consola.
