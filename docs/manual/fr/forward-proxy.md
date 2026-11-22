# Mandataire direct

Felucca peut exposer une entrée de mandataire SOCKS5 ou HTTP depuis l'arrière d'un NAT. Cette
page couvre les trois modes, l'authentification par comptes, les ACL de cible et le
comportement UDP.

## Trois modes

```toml
[[tunnel]]
mode = "socks5"
port = 9080
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
mode = "httpproxy"
port = 9081
bind = "127.0.0.1"

[[tunnel]]
mode = "mixedproxy"
port = 9082
bind = "127.0.0.1"
```

| `mode` | Prise en charge |
| --- | --- |
| `socks5` | CONNECT + UDP ASSOCIATE ; IPv4/IPv6 ; résolution DNS côté agent ; BIND renvoie "unsupported command" |
| `httpproxy` | Requêtes d'URL absolue + CONNECT |
| `mixedproxy` | Distingue SOCKS5 de HTTP au début de la requête |

Les exemples de mandataire direct se lient uniquement à `127.0.0.1`. Lorsque vous avez besoin
d'un accès distant, définissez **aussi** une liste d'autorisation de source ; sinon, quiconque
peut atteindre ce port peut utiliser votre sortie.

## Authentification par comptes

Ajoutez une table de comptes sous le tunnel concerné pour activer l'authentification :

```toml
[tunnel.options.auth.users]
alice = "replace-with-a-password"
bob = "another-password"
```

| Entrée | Authentification | Réponse en cas d'échec |
| --- | --- | --- |
| SOCKS5 | Méthode nom d'utilisateur et mot de passe, **sans repli** sur l'anonymat | Échec d'authentification |
| Mandataire direct HTTP / CONNECT | `Proxy-Authorization: Basic` | 407 |
| Site en mandataire inverse HTTP | `Authorization: Basic` | 401 |

Sans table de comptes, l'accès anonyme est autorisé. Les identifiants de mandataire et de
site vérifiés sont supprimés avant l'envoi de la requête au back-end. Les messages de
configuration et d'enregistrement contiennent les mots de passe des comptes : le pont devrait
donc utiliser TLS, WSS ou QUIC par défaut ; `tcp` et `ws` sont des transports en clair choisis
explicitement.

Les tunnels fichier utilisent `tunnel.file.auth`, qui ne peut pas être employé avec
`tunnel.options.auth`.

## Politique de cible

Les tunnels à back-end fixe utilisent `target_policy` pour limiter la cible réelle. Un
mandataire direct en a particulièrement besoin :

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

Un `allow` vide ne restreint pas les cibles autorisées, `deny` a toujours priorité, et un
`ports` vide autorise tout port non nul. Les règles prises en charge sont IP/CIDR, `domain:`
pour un nom de domaine exact, `suffix:` pour un suffixe (le domaine racine et les
sous-domaines à toute profondeur étant inclus), `regex:` pour une expression régulière, et
`*` pour correspondre à toutes les cibles. Les noms sont insensibles à la casse, et un point
final est normalisé.

Le DNS est d'abord résolu, puis le **nom d'origine et l'IP réelle** sont vérifiés l'un après
l'autre. Une connexion n'utilise que les adresses qui passent les vérifications et n'est pas
résolue une seconde fois : un domaine autorisé ne peut donc pas contourner une IP refusée.
GeoIP et GeoSite sont aussi pris en charge ; voir
[Contrôle d'accès, quotas et codage de flux](security.md).

## SOCKS5 UDP ASSOCIATE

- Seule l'IP de l'interlocuteur de la connexion de contrôle est acceptée : lorsque la requête
  nomme un port, ce port est fixé ; sinon, le port source du premier paquet UDP valide est
  lié.
- Fermer la connexion TCP de contrôle efface toutes les associations de cible.
- Chaque session autorise au plus 64 cibles, chacune mettant 8 paquets en file, et les entrées
  inactives sont réclamées après 60 secondes.
- Un FRAG non nul est abandonné, et le réassemblage de fragments SOCKS n'est pas implémenté.

## Débit et quota

- `tunnel.options.limits.rate_limit` est une limite en octets par seconde, partagée par tous
  les tunnels de l'agent et par les deux sens ; elle autorise une rafale initiale d'une seconde
  de trafic.
- `quota.flow_limit` compte les deux sens, et `quota.expires_at` utilise des secondes Unix.
- Les quotas sont vérifiés périodiquement et interrompent les connexions en cours de
  transfert : le trafic simultané en vol peut donc dépasser le quota. Les compteurs ne servent
  pas à une facturation stricte.

## Étapes suivantes

- [Tunnels et mappages de ports](tunnels.md)
- [Partages privés et visiteurs](private-shares.md) — visiteurs SOCKS sans port public
- [Connexion directe P2P et repli sur le relais](p2p.md)
