# Servicios, actualizaciones, registro y supervisión

Esta página cubre cómo registrar Felucca en un gestor de servicios, cómo actualizar los binarios de forma
segura, la rotación de registros, las métricas y el perfilado de CPU.

## Instalación como servicio del sistema

Ambos programas usan los mismos subcomandos, y `--name` admite varias instancias:

```sh
felucca-server service --name office render --config /srv/felucca/server.toml
felucca-server service --name office --dry-run install --config /srv/felucca/server.toml
felucca-server service --name office install --config /srv/felucca/server.toml
felucca-server service --name office start
felucca-server service --name office status
felucca-server service --name office restart
felucca-server service --name office stop
felucca-server service --name office uninstall
```

| Plataforma | Gestor | Ámbito predeterminado |
| --- | --- | --- |
| Linux | systemd | `user`; `--scope system` es a nivel de máquina |
| macOS | launchd | `user`; `--scope system` es a nivel de máquina |
| Windows | SCM | solo `system` |

`render --platform systemd|launchd|windows` le muestra la definición de cualquier host admitido sin
tener que montar comandos a través de un shell. `--workdir` toma de forma predeterminada el directorio
que contiene la configuración, y las rutas relativas de dentro de la configuración se interpretan desde
ahí. `--binary` selecciona qué programa se instala. Los argumentos adicionales del demonio van después
de `--`, por ejemplo `-- --log-file /srv/felucca/agent.log`.

La instalación **no sobrescribe** un servicio existente con el mismo nombre, y no inicia el servicio de
inmediato. En Linux habilita el servicio; en macOS lo lanza en el siguiente inicio de sesión o arranque, o
cuando ejecuta `start` explícitamente. Un servicio de usuario de systemd que deba estar presente en el
arranque requiere que quien despliega habilite el lingering, y un servicio de usuario de macOS depende de
una sesión gráfica de inicio de sesión. Los servicios a nivel de máquina se ejecutan con la identidad
predeterminada del gestor de servicios, así que edite el usuario de ejecución para que coincida con su
despliegue. En Windows la identidad de instalación es
`NT AUTHORITY\LocalService`, y quien despliega debe concederle los permisos correspondientes sobre los
directorios de configuración, registro, estado y raíz de archivos.

La desinstalación conserva el ejecutable, la configuración, los registros, los certificados y el estado en
tiempo de ejecución.

Alcance de lo verificado: el repositorio prueba la generación de definiciones, el análisis de plists de
macOS, las actualizaciones de copias aisladas y el perfilado real de CPU. También se construyó y ejecutó
una imagen de servidor Linux arm64 en OrbStack, verificando el inicio de sesión de administración, la
supervisión, el TLS del puente y la persistencia tras recrear el contenedor; véase el
registro de despliegue.
La instalación como servicio del sistema y los gestores de servicios de Linux/Windows siguen sin
verificarse.

## Actualización de binarios

Obtenga el SHA-256 de un ejecutable independiente de un canal de publicación de confianza y después
verifique o sustituya el programa en ejecución:

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --expect-version 0.1.0
felucca-agent  update --source https://releases.example/felucca-agent --sha256 <64-hex-digest>
```

El resumen digest es la **base de la confianza** y debería provenir de un manifiesto de publicación de
confianza. La entrada debe ser un binario independiente, así que desempaquete primero el tar.gz/zip. Las
descargas HTTPS verifican los certificados y no siguen redirecciones; para descargas privadas use
`--token-env VARIABLE_NAME`, y las credenciales nunca se escriben en los registros. Las descargas están
limitadas a 512 MiB y 5 minutos. La sonda `--version`, acotada a 10 segundos, solo se ejecuta después de
que el resumen digest coincida, y comprueba el tipo de servidor/agente y la versión opcional. Los fallos
limpian los archivos temporales, y un bloqueo de actualización impide la sustitución concurrente. La
sustitución conserva los permisos originales del archivo.

Después debe ejecutar `service ... restart` de forma explícita; los procesos que ya están en ejecución
siguen usando la versión antigua. La actualización dentro de un contenedor no funciona sobre un sistema de
archivos de imagen de solo lectura — sustituya la imagen en su lugar.

Cuando cambia la versión del protocolo, el servidor y el agente deben actualizarse juntos. El script de
empaquetado ejecuta realmente `--build-info` y se niega a mezclar binarios con versiones de protocolo o
destinos distintos.

## Registro

Del lado del servidor:

```toml
[log]
level = "info"
file = "/data/felucca.log"
json = true

[log.rotation]
max_bytes = 10485760
max_files = 5
interval = "24h"
```

La salida va tanto al archivo como a la salida de error estándar. La rotación ocurre entre eventos
completos, así que un umbral de tamaño puede admitir un evento adicional. Los archivos se numeran de `.1`
a `.5`, siendo `.1` el más reciente. `interval` se mide desde el momento en que se abre el archivo actual,
y un reinicio empieza el periodo de nuevo. Un bloqueo exclusivo impide que varios procesos roten la misma
ruta, y los archivos nuevos de registro y de bloqueo son 0600 en Unix.

**Cambiar la configuración de registro del servidor requiere un reinicio; el plano de administración
rechaza los cambios en línea de estos ajustes.** El agente usa en su lugar argumentos de línea de
comandos; véase [el módulo felucca-agent](agent.md).

## Supervisión

`GET /metrics` y `GET /api/metrics` devuelven texto de Prometheus. Use un token Bearer de una cuenta de
administración o una sesión válida, porque **no existe ningún extremo de métricas anónimo**, y las métricas
respetan la visibilidad del usuario de la cuenta.

Cubren el estado del servicio, la revisión de configuración, el estado en línea/habilitado de los nodos,
los recuentos de conexiones, el tráfico acumulado del nodo, la disponibilidad de los túneles y el tráfico
acumulado por túnel. Los recuentos de tráfico reutilizan el MeterStore persistido que ya existe, así que
sobreviven a un reinicio del servidor una vez que habilita `[state]`. La página Supervisión de la interfaz
de administración muestra la CPU del host, la memoria física, la capacidad del sistema de archivos y las
tasas de red como gráficos de líneas interactivos, sin texto de Prometheus ni instrucciones de
configuración. Las muestras llegan cada cinco segundos y se conservan en memoria durante una hora; los
reinicios del proceso borran el historial. Seleccione una ventana de 5 minutos, 15 minutos o 1 hora. Pase
el cursor, haga clic para fijar o use las flechas del teclado para inspeccionar la hora exacta de la
muestra, la zona horaria local y los valores. Arrastre el control de rango o use los botones de zoom y de
restablecimiento para hacer zoom en los cuatro gráficos a la vez; las actualizaciones en vivo conservan la
ventana de tiempo absoluta seleccionada. Las tarjetas muestran el mínimo, la media de las muestras y el
pico de las observaciones visibles, y la leyenda de red alterna las series de recepción y de envío. Las
métricas del host son solo para administradores; los demás roles ven el tráfico y las conexiones de los
recursos que les están permitidos. El extremo estructurado es `GET /api/monitor?window=900` (ventanas
admitidas: 300, 900 y 3600 segundos).

## Perfilado de CPU

Después de definir `[admin] profiling = true`, un administrador puede llamar a `POST /api/profile` con el
cuerpo JSON `{"seconds":5}`, o descargar un gráfico de llamas de CPU desde la página Supervisión. Se
admiten Linux y macOS, y el resto de plataformas se niega a habilitarlo. Cada ejecución dura de 1 a 30
segundos a 49 Hz, solo se permite una tarea a la vez en el proceso, y la salida está limitada a 8 MiB. Una
petición HTTP abandonada no libera antes de tiempo el bloqueo de muestreo, y detener el demonio cancela el
muestreo. Cuando no hay muestras durante un periodo de inactividad, se devuelve un SVG explicativo. Las
compilaciones de release conservan los símbolos de función para el perfilado pero descartan la información
de líneas de depuración; la consulta a nivel de línea requiere una compilación que usted produzca con
información de depuración.

## Docker

```sh
docker build --target server -t felucca-server:local .
docker build --target agent  -t felucca-agent:local .

docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

Las imágenes usan por defecto el UID/GID 10001. Monte el directorio de configuración en modo de solo
lectura y permita que la imagen escriba en el directorio de datos. Abra explícitamente en su configuración
los puertos TCP/UDP adicionales que necesite, y enlácese a direcciones que sean alcanzables desde dentro
del contenedor. Las imágenes multiarquitectura pueden construirse con Buildx usando los destinos nativos
del Dockerfile. `STOPSIGNAL` es SIGTERM, lo que coincide con la ruta de apagado del programa.

### OrbStack local

Inicie OrbStack y después ejecute `sh scripts/run-orbstack.sh` desde la raíz del repositorio. Esto
compila y despliega solo el servidor, incluida la interfaz de administración. El host necesita Docker
Compose, Python 3 y OpenSSL. Abra `http://127.0.0.1:8088` e inicie sesión como `root` con la contraseña
aleatoria de `conf/orbstack/credentials.json`. La escucha de administración se enlaza solo al bucle local.

Las credenciales y el certificado del puente persistente viven en el directorio `conf/orbstack/`,
ignorado por Git. La configuración y las instantáneas de tráfico viven en el volumen
`felucca_server-data`. El archivo de credenciales registra la contraseña inicial; la configuración de
administración guardada tiene prioridad sobre el TOML de arranque. Ejecutar el script de nuevo compila las
fuentes actuales y actualiza el contenedor conservando las credenciales, los certificados y los datos
existentes.

El contenedor se reinicia con el motor de Docker salvo que se detenga manualmente. Al principio no hay
ningún cliente configurado. El TCP y el QUIC del puente usan el puerto 14028. Compose usa la red de host de
OrbStack, así que los puertos de túnel adicionales no necesitan mapeos de puerto Docker individuales.
Evite los puertos que ya usen otros programas locales.

```sh
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

`down` conserva el volumen de datos. Añadir `-v` elimina la configuración persistida y los datos de
tráfico.

## Empaquetado y CI

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

Las compilaciones desde las fuentes requieren Node.js 22.12+ (se recomienda 24). La interfaz de
administración en React + Vite de `apps/admin/` se incrusta en el binario del servidor; los despliegues no
necesitan Node.js. Vuelva a compilar el frontend después de cambiarlo: Cargo rechaza los paquetes ausentes
o caducados.

Los artefactos contienen los dos programas, los ejemplos, la documentación, un manifiesto de resúmenes
digest por binario y un resumen digest del archivo. Los binarios que no pueden ejecutarse en el host de
empaquetado se rechazan por defecto, y la CI multiplataforma empaqueta en cada plataforma de destino. El
flujo de trabajo de GitHub ejecuta comprobaciones, pruebas y archivos para Linux amd64/arm64, macOS arm64 y
Windows amd64, y verifica aparte la versión mínima de Rust 1.88 y la compilación de Docker. El flujo de
trabajo solo sube los artefactos de los jobs; no crea ninguna release de GitHub y no publica imágenes.

## Lista de verificación

```sh
felucca-server service --name office render --config conf/server.toml
felucca-server --config conf/server.toml --fingerprint
felucca-agent  --config conf/agent.toml  --check
```

`--fingerprint` requiere que la configuración referencie archivos PEM fijos. `--check` no significa que los
back-ends, el DNS, los archivos de certificados o los puertos públicos estén ya disponibles.

## Siguientes pasos

- [El módulo felucca-server](server.md)
- [Interfaz de administración y configuración centralizada](management.md)
- [Referencia](reference.md)
