# Primeros pasos

Use su Server público o solicite el comando de conexión a su operador. No necesita preparar un certificado, editar archivos de configuración ni instalar el agente previamente.

## Conectar un dispositivo con un comando

1. En **Dispositivos → Conectar dispositivo**, escriba un nombre. Los ajustes predeterminados permiten acceder a este dispositivo, la LAN e Internet. Para limitar el acceso, abra **Ajustes avanzados: permisos de acceso** y elija este dispositivo, la LAN o un rango personalizado.
2. Copie el comando y ejecútelo en Terminal (macOS/Linux) o PowerShell (Windows) del dispositivo. Detecta el procesador, descarga el cliente adecuado y verifica su suma de comprobación.
3. Mantenga la ventana abierta. Cuando esté en línea, seleccione **Añadir servicio**, introduzca su dirección local y use la dirección publicada. Se reconecta automáticamente tras una desconexión.
4. Para acceso privado, cree un dispositivo receptor en **Acceso privado** y ejecute su comando en el otro equipo. El receptor no necesita iniciar sesión en la consola.

El comando contiene credenciales del dispositivo: compártalo solo con el usuario previsto. Para reconectar, cierre la ventana anterior y ejecute el mismo comando. El servidor debe ofrecer un paquete compatible con el sistema del dispositivo. La descarga ZIP, los permisos de carpetas y los ajustes avanzados son opcionales.

## Ejecutar su Server

Con los programas de la distribución, inicialice una vez en un directorio con permiso de escritura:

```sh
./felucca-server --init
```

El comando crea la configuración, muestra una contraseña root aleatoria e inicia el servidor. Abra la dirección indicada. Conserve `felucca-server.toml` y todo el directorio `felucca-server.data/` al actualizar o crear copias de seguridad. Reinicie sin `--init`:

```sh
./felucca-server
```

Para un despliegue público, use `--init --admin-url https://console.example.com` detrás de un proxy HTTPS que reenvíe a `127.0.0.1:8990` conservando Host. Una dirección HTTP directa abre públicamente ese puerto; use HTTPS para acceso público. Los dispositivos necesitan acceso al puente TCP 9001 y los visitantes al puerto del servicio publicado. Consulte [despliegue y distribución de clientes](../../management.md).

## Requisitos previos

| Requisito | Detalles |
| --- | --- |
| Rust 1.88 o posterior | Para compilar; este repositorio está verificado con Rust 1.98.1 |
| Python 3 + OpenSSL | Solo lo necesita el script de la demostración local `scripts/demo-local.py` |
| Un host con IP pública | Ejecuta `felucca-server` |
| Un host en una red privada | Ejecuta `felucca-agent`, con el servicio que desea exponer |
| Permiso para enlazar TCP/UDP locales | Necesario para las pruebas; las reglas del cortafuegos las gestiona quien despliega |

Felucca no instala ningún servicio de sistema ni modifica ninguna regla de cortafuegos.

## Opción 1 — demostración local

Si solo quiere verlo funcionar en su propia máquina (página de administración más reenvío de puertos),
sin red pública:

```sh
python3 scripts/demo-local.py
```

El script compila los binarios de depuración de forma incremental, genera un certificado y
credenciales aleatorias, inicia un sitio web de prueba, el servidor y el agente, verifica el reenvío
real y luego abre por sí solo la página de la demostración. La terminal muestra la dirección de
administración, la cuenta y la contraseña. Pulse `Ctrl+C` para detener todos los servicios de la
demostración.

Toda escucha se enlaza únicamente a `127.0.0.1`, y se elige automáticamente un puerto libre cuando
un puerto predeterminado ya está ocupado. Cada ejecución guarda su configuración, sus datos de inicio
de sesión y sus registros en su propio directorio `target/local-demo/run-*`.

| Indicador | Efecto |
| --- | --- |
| `--no-open` | No abrir el navegador automáticamente |
| `--no-build` | Omitir la compilación cuando los binarios ya están compilados |

## Opción 2 — compilar

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
```

El resultado es `target/release/felucca-server` y `target/release/felucca-agent`.
`scripts/cargo-external.sh` coloca la caché de Cargo en el disco donde vive el espacio de trabajo.
Cuando tenga espacio en disco de sobra también puede invocar `cargo` directamente.

## Opción 3 — primer túnel paso a paso

### 1. Genere el certificado del puente

El certificado del puente se genera una sola vez, y la clave privada se conserva:

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

### 2. Defina la clave compartida

El `vkey` de ambos archivos debe ser la misma cadena aleatoria:

```sh
openssl rand -hex 32
```

Escríbala en `[client.agent-1].vkey` de `conf/server.local.toml` y en `vkey` de
`conf/agent.local.toml`. El `key_id` del agente debe coincidir con la entrada de la tabla
`[client.<id>]` del servidor. Cambie el `server` del agente por una dirección alcanzable del host
del servidor.

### 3. Lea la huella digital del certificado

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
```

Ponga la huella digital SHA-256 que imprime en el `[trust].fingerprint` del agente. Después compruebe
ambos archivos:

```sh
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

### 4. Inicie los dos programas

Ejecute un comando en cada una de las dos terminales:

```sh
target/release/felucca-server --config conf/server.local.toml
```

```sh
target/release/felucca-agent --config conf/agent.local.toml
```

### 5. Verifique

La configuración de ejemplo reenvía el TCP `8080` del servidor a `127.0.0.1:3000` en la
máquina del agente. Inicie en la máquina del agente un servicio que escuche en el puerto 3000 y luego
abra el puerto 8080 del servidor.

## Siguientes pasos

- [Arquitectura y conceptos](architecture.md) — qué significan puente, transporte y túnel
- [El módulo felucca-server](server.md) — la tabla de clientes, las escuchas HTTP, la autorización de puertos
- [El módulo felucca-agent](agent.md) — reconexiones, red de salida, varias instancias
- [Túneles y mapeos de puertos](tunnels.md) — modos TCP/UDP/HTTP/archivos/proxy
- [Referencia](reference.md) — todos los comandos y campos
