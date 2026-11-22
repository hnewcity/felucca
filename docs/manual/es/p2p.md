# Directo P2P y respaldo por relé

Cuando ambos extremos están detrás de un NAT, Felucca puede intentar la perforación de NAT por UDP para
conseguir un enlace directo QUIC y recurrir al relé del servidor si falla. Los datos de la aplicación
evitan pasar por el servidor; la conexión principal sigue transportando el control y la contabilidad. El
servidor, el agente proveedor y el agente visitante deben actualizarse juntos.

## Servidor: el puerto de observación

```toml
[bridge]
bind = "0.0.0.0:14028"

[p2p]
bind = "0.0.0.0:8025"        # Listen on this host
advertise = "203.0.113.10:8025"   # External UDP address both ends can reach, without a scheme
timeout = "5s"
max_pending = 128
```

| Campo | Predeterminado | Restricción |
| --- | --- | --- |
| `bind` | — | Dirección de escucha local |
| `advertise` | — | Dirección UDP alcanzable desde el exterior, **sin** esquema de URL |
| `timeout` | `5s` | 500ms–10s |
| `max_pending` | `128` | 1–4096 |

`advertise` no debe solaparse con los puertos UDP que usan KCP, QUIC o HTTP/3. El `127.0.0.1` de la
configuración solo sirve para demostraciones en las que todos los procesos se ejecutan en el mismo host;
para uso entre redes, cámbielo por el nombre de dominio público del servidor y su puerto.

## Proveedor: permitir el directo

```toml
# provider agent
[p2p]
enabled = true
max_sessions = 64        # Default 64, maximum 1024
bind_ip = "192.0.2.10"   # Optional; fixes the local UDP IP

[[tunnel]]
id = 1
mode = "tcp"
public = false
backend = [{ addr = "127.0.0.1:3000" }]
```

```toml
# server: a share must enable direct explicitly
[share.office-web]
client = "office"
tunnel = 1
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]
direct = true
```

El servidor, el proveedor local y el recurso compartido deben estar cada uno **explícitamente
habilitados** (el `[p2p]` del servidor, el `[p2p] enabled = true` del proveedor y el `direct = true` del
recurso compartido). La conexión principal de ambos extremos debe usar TLS, WSS o QUIC con un
certificado verificado. Cuando se omite `bind_ip`, se usa la configuración de IP de salida del agente;
esto no cambia la salida que se usa para las marcaciones a los back-ends.

## Visitante: preferencia de conexión

```toml
# visitor agent
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"
connection = "prefer_direct"    # relay | prefer_direct | direct
```

| `connection` | Comportamiento |
| --- | --- |
| `relay` | El predeterminado; usa el relé existente |
| `prefer_direct` | Intenta primero el directo y recurre al relé cuando el enlace directo no puede establecerse y todavía no se ha enviado ningún dato de aplicación |
| `direct` | Solo acepta el directo; cierra la conexión local cuando el directo no está disponible |

## Cómo se establece el enlace

1. La conexión principal completa primero la autenticación de nodo y de secreto de recurso compartido;
   el servidor comprueba la identidad del visitante, las ACL de origen y las cuotas de conexión de
   ambos lados.
2. El servidor emite un **ticket de un solo uso**, vinculado al túnel, al origen, al destino, al tipo de
   UDP, a las opciones de codificación y a la información de PROXY, y no transmite la clave de nodo del
   proveedor.
3. Ambos extremos sondean direcciones UDP observadas usando claves de ticket distintas; el servidor
   confirma la ruta de retorno con una cookie de corta duración ligada al origen, y limita el número de
   sondeos válidos, el número de tickets y su vida útil; los tickets desconocidos no reciben respuesta.
4. El proveedor conserva el mismo socket UDP que usó para la observación y envía paquetes de perforación
   de NAT al extremo que observó el visitante.
5. El visitante se conecta directamente por QUIC TLS 1.3, fijando la huella digital del certificado
   efímero del proveedor, y después completa un HMAC bidireccional del ticket vinculado y de dos nonces.
6. La selección de back-end, el estado de salud, las ACL de destino, la autenticación de archivos y los
   descriptores de capacidad de directorio siguen reutilizando el motor de servicio original.

Si la perforación de NAT falla, primero se limpian el intento del proveedor y las cuotas de ambos lados,
y solo entonces se permite una conexión por relé. Una caída una vez iniciado el reenvío **no reproduce los
datos de la aplicación**. Una caída de la conexión principal, un nodo deshabilitado, cuotas agotadas o un
servicio detenido cierran los enlaces directos relacionados.

## Contabilidad y límites

- Los datos de la aplicación viajan por QUIC entre los dos extremos y no pasan por el relé de datos del
  servidor.
- El proveedor conserva como máximo un bloque de datos pendiente de contabilizar por sentido (TCP de
  16 KiB como máximo, UDP de 65507 bytes como máximo).
- El servidor comprueba y planifica el tráfico y los rangos de tasa de ambos lados antes de enviar, y
  confirma los recuentos reales de bytes después de enviar; los números de secuencia duplicados, las
  autorizaciones solapadas y las confirmaciones por encima de la cuota se rechazan.
- El recuento usa los **bytes de aplicación decodificados**, excluyendo sondeos, TLS, sobrecarga de
  compresión, cabeceras de trama UDP y cabeceras PROXY de TCP.
- La contabilidad directa depende de los informes del proveedor autenticado; el servidor no puede
  observar por sí solo las cargas útiles directas. Si el proveedor se cae o la conexión de control se
  corta antes del informe final, puede que los últimos bloques enviados pero no confirmados no queden
  registrados en los recuentos persistentes. Un apagado normal drena mediante mensajes de confirmación.
- Las peticiones de cuota por bloque aumentan la dependencia del RTT de la conexión principal, así que
  esto **no debería** describirse como un enlace directo sin conexión indefinido y desligado del
  servidor.

## Servicios admitidos

Se admiten los servicios TCP, UDP y de archivos restringidos, tanto privados como públicos, junto con
los destinos de proxy directo SOCKS y HTTP sobre recursos compartidos dinámicos. Las entradas HTTP/SNI de
sitio, las rutas locales y anidadas del servidor y los destinos que requieren cuentas de proxy aparte no
eluden la entrada original: `prefer_direct` usa el relé original y `direct` lo rechaza. Los back-ends UDP
con las cabeceras PROXY habilitadas usan el relé; las cabeceras PROXY de TCP las escribe el proveedor en
el back-end, usando la dirección de origen autenticada por el servidor.

## Límites de despliegue

Cuando existe un mapeo dependiente del destino, cuando UDP está deshabilitado y en casos similares, se
usa el relé. Los tipos reales de NAT público y las redes de los operadores todavía tienen que validarse
en el entorno de despliegue donde se ejecutan; no trate una simulación local como un resultado de red
pública.

## Siguientes pasos

- [Recursos compartidos privados y visitantes](private-shares.md)
- [El módulo felucca-server](server.md)
- [Control de acceso, cuotas y codificación de flujo](security.md)
- Ejemplos: [p2p-server.toml](../../../examples/p2p-server.toml) ·
[p2p-provider.toml](../../../examples/p2p-provider.toml) ·
[p2p-visitor.toml](../../../examples/p2p-visitor.toml)
