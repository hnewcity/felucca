# Mandataire inverse HTTP/HTTPS

Cette page couvre l'exposition de services HTTP internes par domaine et par chemin :
routage, réécriture des en-têtes et du Host, redirections, CORS, pages d'erreur, SNI, HTTP/2
et HTTP/3, ports partagés et en-têtes de transfert.

## Déclarer une route HTTP

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

Un tunnel `http` n'ouvre aucun écouteur distinct ; `port` détermine à quel écouteur HTTP il
se rattache, et il est accessible à la fois en HTTP et en HTTPS.

## Règles de routage

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"        # any | http | https
```

Priorité de correspondance : **domaine exact, puis préfixe de chemin le plus long, puis
schéma exact**. Les noms de domaine sont insensibles à la casse, et un point final est
normalisé.

| Motif | Correspondances |
| --- | --- |
| `app.example.test` exact | Ce domaine uniquement |
| Joker `*.example.test` | Correspond à `a.example.test`, mais **pas** à `example.test` ni à `a.b.example.test` |

Les déclarations en doublon avec le même domaine, le même chemin et le même protocole sont
rejetées. Une requête sans route correspondante renvoie 404. Vous pouvez déclarer plusieurs
chemins pour un même domaine et les rattacher à des tunnels différents.

## Options de requête et de réponse

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

| Option | Comportement |
| --- | --- |
| `rewrite_prefix` | Réécrit le préfixe et conserve le chemin restant ainsi que la chaîne de requête |
| `host` | Réécrit le Host envoyé au back-end |
| `request_headers` | Modèles d'en-têtes ; une valeur vide supprime cet en-tête |
| `response_headers` | Modèles d'en-têtes de réponse |
| `cors` | Renvoie `Access-Control-Allow-Origin: *`, et un préflight valide renvoie directement 204. Un préflight n'exige pas de compte de site, mais la requête réelle exige toujours une authentification |
| `redirect` | Renvoie 307 |
| `force_https` | Renvoie 301, la cible utilisant le port de l'écouteur HTTPS du serveur (443 est omis). Un écouteur HTTPS doit exister lorsque vous la déclarez |
| `error_pages` | Remplace le corps de la réponse selon le code de statut. Prend en charge 400..599, avec au plus 64 Kio par page |

Les modèles prennent en charge `{host}`, `{path}` (chaîne de requête incluse), `{scheme}`,
`{remote_ip}` et `{remote_port}`. Les valeurs sont tirées de la requête d'origine, et le
résultat d'une substitution n'est pas expansé une seconde fois. Les en-têtes de délimitation
de message tels que Content-Length et Transfer-Encoding, ainsi que les en-têtes hop-by-hop,
ne peuvent pas être réécrits ; Host dispose de son propre champ.

L'authentification du site utilise `Authorization: Basic`, et un échec renvoie 401. Les
identifiants vérifiés ne sont pas transmis au back-end.

## Modes TLS

```toml
[tunnel.options.http]
tls_mode = "http"        # http | passthrough | offload
```

| Mode | Comportement |
| --- | --- |
| `http` | Le défaut. Un mandataire inverse HTTPS ordinaire |
| `passthrough` | Transmet le flux TLS brut au back-end selon le SNI, et le certificat du back-end est présenté directement aux visiteurs. Dans ce mode, `backend.tls` doit être désactivé |
| `offload` | Termine TLS avec le certificat SNI du serveur, puis envoie un contenu TCP arbitraire au back-end |

`passthrough` et `offload` occupent tout le SNI, et ne peuvent pas être combinés avec les
options de chemin ou d'en-têtes HTTP, ni avec l'authentification du site.

## Ports et protocoles

| Capacité | Description |
| --- | --- |
| Entrée | HTTP/1.1, HTTP/2, HTTP/3 |
| Back-end | HTTP/1.1, HTTP/2 (`backend_http2 = true`) |
| ALPN | Les entrées TLS prennent en charge `h2` et `http/1.1` ; les entrées en clair prennent en charge la connaissance préalable de HTTP/2 |
| WebSocket | Converti automatiquement entre les trois types d'entrée et les deux types de back-end. Une entrée HTTP/1.1 renvoie 101, tandis que HTTP/2 ou HTTP/3 utilise le CONNECT étendu `:protocol = websocket` |
| HTTP/3 | Exige `http.http3_bind`. Chaque connexion QUIC autorise au plus 128 flux bidirectionnels, avec une fenêtre de réception de 1 Mio par flux et de 16 Mio par connexion. Elle se ferme après 30 secondes d'inactivité, et les en-têtes de requête sont limités à 32 Kio |

HTTP/3 n'est qu'un protocole d'entrée public ; les back-ends continuent d'utiliser HTTP/1.1
ou HTTP/2. Une réponse HTTPS normale porte un en-tête `Alt-Svc`, que vous pouvez remplacer
via `response_headers` lorsque le port exposé diffère.

## Partage des ports

`bridge.bind`, `http.bind` et `http.tls_bind` peuvent tous désigner la **même** paire
IP:port, une seule socket démultiplexant selon les caractéristiques de la poignée de main :
les agents TLS/WSS directement connectés utilisent l'ALPN `felucca/9`, tandis que les sites
web utilisent `h2`/`http/1.1` ou aucun ALPN. Un `GET /ws` ordinaire reste une requête de
site web, et seule une mise à niveau WebSocket correcte entre dans le pont. Le pont QUIC et
HTTP/3 peuvent aussi partager une même adresse UDP, démultiplexés par ALPN, les certificats
de pont et les certificats de site étant choisis et renouvelés séparément.

## Adresses des clients

Par défaut, les en-têtes `Forwarded`, `X-Forwarded-*` et `X-Real-IP` fournis par le client
sont supprimés, puis `X-Forwarded-For`, `X-Real-IP`, `X-Forwarded-Host` et
`X-Forwarded-Proto` sont définis à partir de l'adresse source directe ou de la source PROXY
vérifiée par `http.proxy_accept`. Définissez `forwarded_headers = false` pour désactiver
cette réécriture, ce qui convient aux déploiements où vous contrôlez vous-même toute la
chaîne de mandataires. Les en-têtes hop-by-hop, ainsi que les en-têtes nommés dans
`Connection`, ne sont pas transmis au saut suivant.

## Étapes suivantes

- [Certificats, SNI et ACME](certificates.md)
- [Back-ends, répartition et contrôles de santé](backends.md)
- [Services de fichiers et WebDAV](file-services.md) — entrée fichier par domaine
- [Contrôle d'accès, quotas et codage de flux](security.md) — quotas et ACL par domaine
