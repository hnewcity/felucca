# Tunnels et mappages de ports

Un tunnel est l'unité de base de Felucca : une entrée publique plus un ensemble de
back-ends. Cette page couvre les champs partagés, les huit modes, les mappages de ports en
lot et le protocole PROXY.

## Champs partagés des tunnels

```toml
[[tunnel]]
id = 1                 # Stable ID; pin it once traffic snapshots are enabled
mode = "tcp"           # Entry protocol
port = 8080            # Port opened on the server
bind = "0.0.0.0"       # IP only, no port attached
remark = "internal web app"
public = true          # false means a private share that does not take a public port
balance = "round_robin"   # Goes inside [[tunnel]], not after [[tunnel.backend]]

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`bind` accepte une adresse IP, pas un port ; le port provient de `tunnel.port`. Omettez
`bind` pour écouter sur toutes les interfaces. Lorsque `id` n'est pas défini, un entier
positif disponible est attribué. Dès que les instantanés de trafic sont activés, définissez
`id` explicitement plutôt que de compter sur l'ordre du tableau.

## Modes

| `mode` | Comportement de l'entrée | Source de la cible |
| --- | --- | --- |
| `tcp` | Port TCP public, transfert d'octets brut | `backend` |
| `udp` | Port UDP public, frontières de datagrammes préservées | `backend` |
| `http` | Réutilise l'écouteur HTTP, HTTPS ou HTTP/3 du serveur et répartit par domaine et par chemin | `backend` |
| `socks5` | Mandataire direct SOCKS5, CONNECT + UDP ASSOCIATE | Définie par le client |
| `httpproxy` | Mandataire direct HTTP, URL absolue + CONNECT | Définie par le client |
| `mixedproxy` | Entrée mixte, distingue SOCKS5 de HTTP au début de la requête | Définie par le client |
| `file` | Service de fichiers et WebDAV | Un répertoire sur l'hôte de l'agent |
| `transparent` (alias `tcptrans`) | Lit l'adresse de destination d'origine auprès du noyau | Le noyau |

Un tunnel `http` n'ouvre **pas** d'écouteur distinct : son `port` doit correspondre à
`http.bind`, `http.tls_bind` ou `http.http3_bind` du serveur, et plusieurs tunnels HTTP
peuvent partager un même port. `tunnel.bind` est sans effet sur le routage HTTP. L'écouteur
HTTP est activé par le serveur et n'est pas limité par les `allowed_ports` de l'agent.

## Mappages de ports en lot

`[[mapping]]` ne prend en charge que TCP et UDP, et mappe une plage de ports un à un, dans
l'ordre :

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

Les deux nombres doivent être égaux, et les ports serveur en doublon ne sont pas autorisés.
Les tunnels dépliés sont validés en même temps que les entrées `[[tunnel]]` explicites, et
continuent de compter pour l'autorisation de ports du serveur et pour la limite de tunnels.
`target_host` peut être un nom de domaine, une adresse IPv4 ou une adresse IPv6, et ne peut
pas porter de port. Vous pouvez aussi définir `tls`/`trust`, `health`, `acl`, `options` et
`target_policy`.

## Comportement UDP

- Une association distincte est établie pour chaque IP:port source, et rattachée à une socket
  de back-end côté agent.
- Les paquets vides et les frontières de datagrammes sont préservés. La taille maximale d'un
  datagramme est de 65507 octets, et le MTU utilisable sur le réseau réel peut être inférieur.
- Chaque tunnel autorise au plus 256 associations source, chacune mettant 8 paquets en file.
  Les paquets sont abandonnés lorsqu'une file est pleine, et une association est réclamée
  après 60 secondes d'inactivité.
- Les datagrammes sont transportés sur un flux de tunnel fiable, **pas** par QUIC DATAGRAM :
  une liaison avec pertes attend donc la retransmission.
- Les statistiques UDP portent uniquement sur la charge utile des datagrammes, et non sur les
  trames de longueur internes.

## Protocole PROXY

Envoyez au back-end l'adresse source d'origine et l'adresse de destination de l'entrée :

```toml
[tunnel.options]
proxy_protocol = "v1"     # "off" | "v1" | "v2"
```

Pour TCP, HTTP et TLS, un en-tête est envoyé par connexion au back-end. L'UDP ne prend en
charge que la v2, en ajoutant un en-tête DGRAM par datagramme ; les paquets de plus de 65507
octets, en-tête compris, sont abandonnés. Le back-end doit être capable d'analyser le
protocole PROXY.

Côté réception, configurez `proxy_accept` pour l'écouteur concerné :

```toml
[http.proxy_accept]
mode = "required"                    # optional | required
trusted = ["192.0.2.10", "2001:db8:1::/64"]
bridge.proxy_accept = { ... }
[tunnel.options.proxy_accept]
mode = "optional"
trusted = ["10.0.0.0/8"]
```

`optional` autorise les connexions directes sans en-tête, et `required` refuse une requête
sans en-tête. Lorsque vous activez cette option, `trusted` ne doit pas être vide. Les ACL de
source, le hachage par source et les en-têtes de transfert HTTP utilisent tous l'adresse
d'origine vérifiée. Un pont et un écouteur HTTP sur un même port doivent être configurés avec
la même politique. La réception est limitée à 5 secondes, et un écouteur UDP n'accepte pas
d'en-têtes PROXY.

## Codage par flux

```toml
[tunnel.options.payload]
compress = true     # Snappy, default false
encrypt = true      # ChaCha20-Poly1305, default false
```

Cela s'applique à TCP et UDP, aux transferts de fichiers et WebDAV, ainsi qu'aux flux de
données des mandataires inverse et direct HTTP. Lorsque la compression est activée, les
données sont compressées avant d'être chiffrées. Chaque flux échange un nonce neuf, et deux
clés directionnelles indépendantes sont dérivées du vkey de cet agent. Lorsque les deux
extrémités ne correspondent pas, la négociation est refusée, sans repli automatique sur le
texte clair. Voir [Contrôle d'accès, quotas et codage de flux](security.md) pour le détail.

## Étapes suivantes

- [Back-ends, répartition et contrôles de santé](backends.md)
- [Mandataire inverse HTTP/HTTPS](http-services.md)
- [Mandataire direct](forward-proxy.md)
- [Services de fichiers et WebDAV](file-services.md)
- [Cibles locales au serveur et routes imbriquées](local-services.md)
