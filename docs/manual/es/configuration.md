# Fuentes de configuración y CLI

Esta página explica de dónde viene la configuración, cómo se pasa, qué variables de entorno
se aplican y cómo validarla sin arrancar. El significado de cada campo está en las guías de los
módulos.

## Archivos de configuración

Ambos programas aceptan TOML y JSON. Las rutas relativas se resuelven contra el directorio de
trabajo del proceso, así que tanto `--workdir` como el directorio de arranque de systemd/launchd
importan.

```sh
felucca-server --config /srv/felucca/server.toml
felucca-agent  --config /srv/felucca/agent.toml
```

Sin `--config`, los programas leen `felucca-server.toml` / `felucca-agent.toml` del directorio de
trabajo. `-c` es la forma corta de `--config`; el agente además acepta el alias
`--launch`.

Toda la configuración usa `deny_unknown_fields`: un campo desconocido es un error y nunca se
ignora en silencio.

## Fuentes admitidas

`--config` acepta cuatro formas:

| Forma | Ejemplo | Detalles |
| --- | --- | --- |
| Archivo local | `conf/server.toml` | TOML o JSON |
| URL HTTP(S) | `https://cfg.example/edge.json` | Espera como máximo 15 segundos y exige un 200; no sigue redirecciones y no acepta respuestas comprimidas; la URL no admite userinfo ni fragmento, y los parámetros de consulta pueden llevar una dirección de descarga firmada |
| JSON en línea | `'{"bridge":{"bind":"0.0.0.0:14028"}}'` | Pasa un objeto JSON directamente |
| base64 | `base64:eyJicmlkZ2Ui...` | base64 estándar o base64url sin relleno |

Una sola configuración puede ocupar como máximo 4 MiB. `--config-token-env NAME` lee un token
Bearer de una variable de entorno para las direcciones de descarga privadas; la credencial nunca se
escribe en el registro. La URL, el token y el cuerpo en línea nunca se imprimen como nombre de la
fuente de configuración.

En una configuración remota, las rutas de certificados, de datos de Geo y de la raíz de archivos
siguen apuntando a rutas de la **máquina del agente**.

## URI de arranque del agente

`felucca-agent` también acepta una URI corta. Por sí sola solo conecta y los túneles locales siguen necesitando TOML/JSON; con `remote=1` el servidor aporta los túneles y el alcance de acceso, así que basta un comando:

```sh
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&transport=tls'
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&fingerprint=SHA256_HEX&remote=1'
```

| Campo | Obligatorio | Detalles |
| --- | --- | --- |
| Puerto | Sí | Debe ser distinto de cero |
| `vkey` | Sí | La clave compartida; el valor debe ir codificado en porcentaje |
| `key_id` | No | Usa la identidad predeterminada cuando se omite |
| `transport` | No | `tcp`/`tls`/`ws`/`wss`/`quic`/`kcp` |
| `fingerprint` | No | Habilita la validación por huella digital SHA-256 del certificado |
| `ws_path` | No | La ruta del WebSocket |
| `remote` | No | `1` deja servicios y alcance de acceso en manos del servidor: activa `remote.enabled` y `remote.console_scope`, y solo permite direcciones locales hasta que el servidor envíe un alcance |

Los campos desconocidos y los campos repetidos son errores.

## Varias instancias del agente

`--config` puede repetirse, hasta 32 instancias en un mismo proceso:

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://cfg.example/edge.json' --config-token-env CONFIG_TOKEN
```

Cada instancia mantiene sus propias conexiones, su espera incremental, sus rutas, sus sondas y sus
límites de conexión. Toda la configuración se valida y se inicializa primero, y solo entonces
empieza la conexión; una combinación repetida de `server` + `key_id` es un error. Cuando un servidor
no es alcanzable, las demás instancias siguen funcionando, y una señal de salida cierra todas las
instancias.

## Expansión de entorno

Desactivada de forma predeterminada. Defina `--expand-env` (o `FELUCCA_EXPAND_ENV=true`) y los
valores de cadena resueltos se expanden:

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}` exige que la variable exista. `${VAR:-default}` usa el valor predeterminado cuando la
variable falta o está vacía. `$$` es un `$` literal. La expansión se ejecuta una sola vez, no toca
nombres de campos y no convierte cadenas en números. Un resultado mayor de 4 MiB se rechaza.

## Instantánea de configuración en tiempo de ejecución

```sh
felucca-server --config bootstrap.toml --runtime-config conf/runtime.json
```

También puede definir la ruta con `FELUCCA_RUNTIME_CONFIG`. Tras el primer arranque correcto, la
configuración normalizada se guarda de forma atómica, y los arranques posteriores leen primero la
instantánea. `--check` y `--fingerprint` leen la misma instantánea. Si el archivo está corrupto, el
arranque falla y no se vuelve al archivo de arranque. **No** la defina con la misma ruta que el
archivo de tráfico de `[state].path`.

La instantánea ocupa como máximo 16 MiB y contiene secretos reales; en Unix un archivo nuevo recibe
el permiso 0600 y un directorio nuevo el 0700, y un bloqueo de archivo impide que otro demonio la
use al mismo tiempo.

Cambiar la configuración desde la interfaz de administración usa el mismo mecanismo — véase
[Administración y configuración centralizada](management.md).

## Validación y registro

```sh
felucca-server --config conf/server.toml --check
felucca-agent  --config conf/agent.toml  --check
```

`--check` valida la estructura TOML/JSON, las restricciones de los campos, los puertos, las ACL y
los datos de Geo. **No** conecta con los back-ends, **no** verifica el DNS público y **no** inspecciona
el contenido de los archivos de certificados. El contenido de los certificados y los conflictos de
enlace se comprueban en el momento del arranque.

Indicadores de registro para el agente:

```sh
felucca-agent -c agent.toml --log-level info \
  --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

`RUST_LOG` anula el nivel de registro en ambos lados. El registro del servidor se escribe en la
tabla `[log]` configurada, y cambiarla requiere un reinicio.

## Variables de entorno

| Variable | Efecto |
| --- | --- |
| `FELUCCA_CONFIG` | Equivale a `--config` (repetible en el agente) |
| `FELUCCA_RUNTIME_CONFIG` | Equivale al `--runtime-config` del servidor |
| `FELUCCA_EXPAND_ENV` | Equivale a `--expand-env` |
| `FELUCCA_CONFIG_TOKEN_ENV` | El nombre de la variable que contiene el token Bearer de la configuración remota |
| `RUST_LOG` | Anula el nivel del filtro de registro |

## Siguientes pasos

- [El módulo felucca-server](server.md)
- [El módulo felucca-agent](agent.md)
- [Servicios de sistema, actualizaciones, registro y supervisión](operations.md)
