# Arquitectura y conceptos

Esta página explica los componentes de Felucca, cómo fluye el tráfico y el vocabulario que usan
todas las guías posteriores. La responsabilidad de cada módulo interno se documenta aparte en
[Arquitectura](../../architecture.md).

## Forma del despliegue

```text
                    public network
 ┌──────────────────────────────────────────────────────┐
 │  felucca-server                                      │
 │  bridge   14028 (TCP/TLS/WS/WSS share one socket)    │
 │  quic     14028 (UDP, native QUIC streams)           │
 │  kcp      8025 (UDP)                                 │
 │  http     8080 / 8443 (+ http3 UDP)                  │
 │  admin    8088 (admin UI, optional)                  │
 └───────────────▲──────────────────────────────────────┘
                 │ agent dials out
        ┌─────────────────────┐
        │  felucca-agent      │    private network
        │  local service 3000 │
        └─────────────────────┘
```

El agente llama hacia afuera a un `felucca-server` en la red pública, de modo que la red privada no
necesita IP pública, ni mapeo de puertos en el router, ni cambios en el cortafuegos. Todos los puntos
de entrada públicos —puertos, dominios, proxies y archivos— pertenecen al servidor.

## Los dos programas

| Programa | Responsabilidades | Necesita puertos de entrada |
| --- | --- | --- |
| `felucca-server` | Acepta agentes, mantiene las escuchas públicas, enruta, administra, mide | Sí |
| `felucca-agent` | Llama hacia afuera desde la red privada, mantiene escuchas locales, envía las peticiones a los back-ends | No |

Ambos deben usar la misma versión de protocolo. La versión actual del protocolo de puente es 10, y el
**servidor y el agente deben actualizarse juntos**. Use `--build-info` para imprimir el nombre del
programa, la versión, el protocolo y el destino de compilación:

```sh
target/release/felucca-server --build-info
target/release/felucca-agent --build-info
```

## Conceptos clave

| Término | Significado |
| --- | --- |
| **bridge (puente)** | La conexión de larga duración entre el servidor y el agente. `bridge.bind` es la dirección que comparten TCP/TLS/WS/WSS y su valor predeterminado es `0.0.0.0:14028`; QUIC y KCP usan cada uno su propio puerto UDP |
| **identity (identidad)** | El agente se identifica con `key_id` + `vkey`; el servidor debe tener una entrada coincidente en la tabla `[client.<key_id>]` |
| **transport (transporte)** | Del lado del agente se puede elegir `tcp`, `tls`, `ws`, `wss`, `quic`, `kcp`. En la red pública prefiera TLS/WSS/QUIC |
| **trust (confianza)** | Cómo verifica el agente el certificado del servidor: `webpki` (validación por CA), `pinned` (huella digital SHA-256 fija), `insecure` (solo diagnóstico) |
| **tunnel (túnel)** | Un mapeo de un punto de entrada público a un conjunto de back-ends. `mode` decide el protocolo de entrada, `backend` decide el destino |
| **backend (back-end)** | El destino al que realmente se conecta el agente, con la forma `127.0.0.1:3000` |
| **listener (escucha)** | El socket que abre el servidor para un túnel. Una reconexión con el mismo `key_id` reemplaza la escucha anterior |
| **share (recurso compartido)** | El servidor concede un túnel registrado a un visitante con nombre sin usar un puerto público |
| **visitor (visitante)** | Otro agente que posee la clave compartida y que abre una escucha en su propia máquina para alcanzar ese recurso compartido |
| **local** | Un grupo de configuración en el que el servidor presta el servicio él mismo, sin agente |
| **mux (multiplexación)** | La multiplexación acotada sobre TCP/TLS/WS/WSS/KCP, que aporta negociación de ventana, presupuesto y contrapresión |
| **managed (gestionado)** | Configuración de túnel que el servidor envía al agente; el agente debe habilitar `[remote]` de forma explícita |

## Vida de una petición

Tomemos como ejemplo un túnel TCP:

1. Un visitante se conecta al puerto público del servidor.
2. El servidor comprueba la ACL de origen, la cuota y el permiso de conexión, y luego asigna la
   conexión al flujo del puente del agente correspondiente.
3. El agente recibe la petición, elige por balanceo de carga un back-end sano y se conecta.
4. Los datos fluyen en ambos sentidos por un flujo mux (o un flujo QUIC nativo) hasta que cualquiera
   de las dos mitades se cierra.
5. El servidor mide el tráfico y puede conservarlo entre reinicios cuando se configura `[state]` o el
   lado de administración.

Un túnel HTTP añade dos pasos: enrutado por dominio y ruta, y autenticación del sitio. Un túnel SOCKS5
añade dos más: resolución de la dirección de destino y una ACL de destino.

## Módulos internos

Como usuario normalmente no necesita ocuparse de esto, pero explican qué crate hace
qué:

| crate | Propósito |
| --- | --- |
| `felucca-core` | Configuración, identidad, reglas de acceso, tipos de medición; no depende de un runtime asíncrono |
| `felucca-mux` | Multiplexación acotada, ventanas y contrapresión |
| `felucca-proto` | Saludo, mensajes de control, protocolos de visitantes y de conexión directa |
| `felucca-transport` | Adaptación de conexiones TCP/TLS/WS/QUIC/KCP y red de salida |
| `felucca-service` | Conexión con back-ends, balanceo de carga, comprobaciones de estado y servicio restringido de archivos compartido por ambos lados |
| `felucca-server` | Aceptación de agentes, enrutado de ingreso, administración, estado en tiempo de ejecución |
| `felucca-client` | Conexión del agente, reconexiones, recepción de configuración, visitantes |
| `felucca-cli` | Los dos programas, la configuración de arranque, el registro, los servicios de sistema, las actualizaciones |

## Siguientes pasos

- [Primeros pasos](get-started.md) — retroceda un paso si todavía no tiene ningún túnel funcionando
- [Fuentes de configuración y CLI](configuration.md)
- [El módulo felucca-server](server.md) / [El módulo felucca-agent](agent.md)
