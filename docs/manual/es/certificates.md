# Certificados, SNI y ACME

Esta página cubre los tres tipos de certificados: el certificado del puente, los certificados
estáticos de los sitios web y los certificados que ACME emite y renueva automáticamente.

## Certificado del puente

```toml
[bridge]
bind = "0.0.0.0:14028"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }
cert_reload_interval = "30s"
```

Si se omite `bridge.tls` y se activa el almacenamiento de configuración (incluidos `--init` o la consola), el servidor guarda su identidad junto a la instantánea en `<snapshot>.identity.json`. Conserve este archivo al reiniciar y respaldar. Sin almacenamiento, la identidad es temporal. `--fingerprint` lee archivos PEM explícitos; las huellas automáticas aparecen en la consola activa y en el registro de inicio.

Los archivos PEM se recargan según el `cert_reload_interval` (30 segundos de forma predeterminada). Las
conexiones TLS/WSS y QUIC nuevas adoptan el certificado nuevo, y las conexiones existentes no se
interrumpen. Si la actualización de un certificado o de una clave queda incompleta, se conserva el
valor anterior. Un agente que use la confianza `pinned` debe actualizar su huella digital antes de la
siguiente reconexión; un agente que valide contra la CA correcta no necesita fijar un certificado
concreto.

```sh
target/release/felucca-server --config conf/server.toml --fingerprint
```

Del lado del agente tiene dos opciones:

```toml
[trust]
mode = "pinned"
fingerprint = "<from --fingerprint>"

[trust]
mode = "webpki"        # CA-issued bridge certificate
```

## Certificados de los sitios web

```toml
[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.certificates."*.example.com"]
cert = "conf/wildcard.pem"
key = "conf/wildcard.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"
```

Las coincidencias exactas tienen prioridad sobre los comodines, y los comodines sobre `default_tls`. Si
nada coincide, el saludo se rechaza. Un conflicto entre el SNI y el Host de HTTP devuelve 421. Los
certificados estáticos se releen cada 30 segundos de forma predeterminada, algo que puede ajustar con
`http.cert_reload_interval`. Un certificado solo se reemplaza cuando coincide con su clave privada, y
un fallo deja vigente el certificado anterior.

HTTP/3 y HTTP/HTTPS comparten el mismo resolutor de SNI, incluida la renovación ACME y la recarga en
caliente de los archivos PEM estáticos.

## Emisión ACME

```toml
[http.acme]
email = "admin@example.com"
accept_tos = true
challenge = "http-01"          # http-01 | tls-alpn-01
cache_dir = "state/acme"
staging = false
domains = ["app.example.com"]
```

| Campo | Descripción |
| --- | --- |
| `accept_tos` | Defínalo como `true` solo después de aceptar las condiciones de la CA |
| `challenge` | `http-01` necesita el TCP 80 público para llegar a `http.bind`. `tls-alpn-01` necesita el TCP 443 público para llegar directamente a `http.tls_bind`, en cuyo caso `http.bind` puede estar cerrado |
| `staging` | Verifique primero la conectividad con staging; esto fija el staging de Let's Encrypt |
| `directory` | Apunte a una CA distinta; el valor predeterminado es la producción de Let's Encrypt |
| `cache_dir` | Directorio de la caché de cuenta y de certificados. **Consérvelo y protéjalo** |
| `domains` | Limita explícitamente qué nombres se emiten. Cuando está vacío, los nombres se derivan cada minuto de los dominios exactos registrados, y los comodines se omiten |

La cuenta se guarda en `account.json`, y la cadena de certificados y la clave privada de cada dominio
viven juntas en un único archivo JSON que se reemplaza de forma atómica. En Unix, los archivos usan el
modo 0600 y los directorios de caché nuevos el 0700. Los certificados se restauran desde la caché al
arrancar. La renovación se calcula a partir de la validez real del certificado, con una antelación no
mayor de un tercio de esa validez. Las comprobaciones se ejecutan cada minuto, los fallos se reintentan
al cabo de una hora, y el certificado anterior sigue en uso mientras la renovación falle. Una emisión
correcta se sustituye en caliente y no interrumpe las conexiones TLS existentes.

**ACME solo gestiona los certificados del proxy inverso HTTP/HTTPS. No cambia el certificado del
puente ni la huella digital fijada por un agente.**

### EAB y ZeroSSL

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

ZeroSSL puede usar un EAB manual, o pasar al modo de clave de API:

```toml
[http.acme]
directory = "https://acme.zerossl.com/v2/DV90"
staging = false
zerossl_api_key = "your-account-key"
```

El modo de clave de API obtiene un EAB cuando la cuenta se registra por primera vez y luego emite los
certificados a través de ACME. **No puede configurar un `eab` manual al mismo tiempo.** No se solicita
ningún EAB nuevo una vez que existe una caché de cuenta. La clave de API y los secretos de las
respuestas no se escriben en los mensajes de error.

## Notas de despliegue

- Ninguno de los dos desafíos ACME **admite certificados comodín**. Para nombres comodín, use
  certificados estáticos emitidos externamente.
- Cuando su aplicación escuche en 8080/8443, mapee 80/443 hacia esos puertos desde el exterior.
- Hasta que la emisión tenga éxito, el saludo TLS de ese dominio puede fallar; HTTP-01 sigue
  proporcionando la validación.
- Un proxy que termine el TLS delante debe reenviar el TLS del desafío tal cual y devolver el
  certificado del desafío solo para los valores de ALPN que contengan `acme-tls/1`.
- Use un `cache_dir` nuevo cuando cambie de CA o cambie entre staging y producción.
- El repositorio ejecuta flujos reales de CSR y de desafío contra una CA local simulada. La emisión
  real desde una CA pública todavía debe verificarse en su propio entorno de dominio.

## Siguientes pasos

- [Proxy inverso HTTP/HTTPS](http-services.md)
- [El módulo felucca-server](server.md)
- [Servicios, actualizaciones, registro y supervisión](operations.md)
