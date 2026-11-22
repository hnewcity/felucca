# Le module felucca-agent

Cette page couvre le déploiement et la configuration de `felucca-agent` : connexion au
serveur, confiance dans les certificats, reconnexions, réseau de sortie, mandataires amont,
journalisation et instances multiples.

## Une configuration minimale

```toml
server = "bridge.example.com:14028"
key_id = "agent-1"
vkey = "change-me-to-a-long-random-string"
transport = "tls"          # tcp | tls | ws | wss | quic | kcp

[trust]
mode = "pinned"
fingerprint = "0000...0000"   # from the server's --fingerprint

[[tunnel]]
mode = "tcp"
port = 8080
[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`server` accepte un nom d'hôte ou une adresse IP accompagnés d'un port. `key_id` doit
correspondre à un nom de table `[client.<key_id>]` sur le serveur.

## Transport et confiance

`transport` accepte `tcp`, `tls`, `ws`, `wss`, `quic` et `kcp`. Pour des ponts publics,
préférez TLS/WSS/QUIC : `tcp` et `ws` authentifient l'interlocuteur mais ne chiffrent pas le
trafic ; ce sont donc des transports en clair que vous choisissez délibérément.

`[trust]` contrôle la façon dont le certificat du serveur est vérifié :

| `mode` | Comportement |
| --- | --- |
| `webpki` | Vérification normale face aux autorités de certification publiques. Ne renseignez pas `fingerprint` |
| `pinned` | Épingle le SHA-256 de l'intégralité du certificat. La signature de la poignée de main est vérifiée, mais ni le nom de domaine ni la période de validité. Mettez à jour l'empreinte à chaque changement de certificat |
| `insecure` | Pour un diagnostic temporaire uniquement |

Après avoir configuré un PEM fixe dans `bridge.tls`, récupérez l'empreinte avec
`felucca-server --fingerprint`.

## Connexion et reconnexion

| Champ | Par défaut | Remarques |
| --- | --- | --- |
| `reconnect_min` | — | Borne inférieure du repli exponentiel |
| `reconnect_max` | — | Borne supérieure du repli exponentiel |
| `dial_timeout` | — | Délai d'expiration d'une seule tentative de connexion |
| `max_connections` | — | La limite de connexions pour cet agent |

L'agent se reconnecte automatiquement après une déconnexion ; le serveur réenregistre les
écouteurs et l'état de session. Les compteurs de trafic survivent aux reconnexions.

## Réseau de sortie

```toml
[network]
local_ip = "192.0.2.10"
forward_local_ip = true
dns = ["192.0.2.53:53"]

[network.proxy]
kind = "socks5"           # http (CONNECT) | socks5
server = "192.0.2.20:1080"
remote_dns = true
[network.proxy.auth]
username = "edge"
password = "replace-with-proxy-password"
```

| Champ | Remarques |
| --- | --- |
| `local_ip` | Lie l'IP de sortie du pont. Ce doit être une adresse réellement détenue par cette machine ; les noms d'interface ne sont pas acceptés |
| `forward_local_ip` | Les connexions aux back-ends et les sondes de santé utilisent aussi cette adresse. Par défaut, seul le pont la lie |
| `dns` | Une liste littérale de `IP:port`. Lorsqu'elle est vide, le résolveur système est utilisé. Les réponses sont mises en cache selon leur TTL, avec prise en charge de A/AAAA et repli sur TCP après troncature UDP |
| `network.proxy` | Mandataire amont. Il ne porte que le TCP/TLS/WS/WSS du pont et ne relaie ni les requêtes aux back-ends ni l'UDP NTP |

Les ACL de cible sont vérifiées par rapport à l'**adresse IP réellement renvoyée par la
résolution**, et l'agent se connecte ensuite à ces adresses vérifiées : aucune seconde
résolution ne peut donc introduire un contournement. `local_ip` et les adresses DNS doivent
appartenir à la même famille d'adresses. Un pont QUIC/KCP ne peut pas utiliser en même temps
un mandataire amont TCP.

## Synchronisation horaire

Le serveur et l'agent acceptent tous deux une section `[ntp]` pour calibrer l'heure
d'authentification du pont :

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

Par défaut, aucun service horaire n'est interrogé. `interval` va de 1 seconde à 24 heures.
`timeout` doit être positif, ne pas dépasser 30 secondes et être inférieur à `interval`.
`max_offset` doit être positif et ne pas dépasser 7 jours. Si la première requête échoue,
l'heure système est utilisée. Le calibrage ne modifie pas l'horloge système : la validité des
certificats TLS et l'expiration des quotas suivent donc toujours l'heure système. Vous devez
faire confiance à la source NTP : le SNTP simple n'authentifie pas la source horaire.

Les requêtes NTP de l'agent utilisent sa propre configuration DNS et de sortie ; le serveur
utilise le DNS système et la sortie par défaut.

## Réception de la configuration centralisée

```toml
[remote]
enabled = true

[remote.target_policy]
allow = ["10.0.0.0/8"]
ports = [80, 443]

[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

`remote.enabled` doit être défini explicitement. La connexion principale doit utiliser
TLS/WSS/QUIC avec un certificat vérifié ; une connexion en clair ne peut pas demander de
configuration. `remote.target_policy` est une frontière stricte sur la machine de l'agent,
intersectée avec chaque tunnel envoyé. `remote.file_root` déclare à l'avance des capacités de
répertoire nommées, et le serveur ne peut envoyer que des chemins correspondant à ces noms de
capacité. Un répertoire en lecture seule ne peut pas être transformé en répertoire
inscriptible via la configuration distante. Voir
[Interface d'administration et configuration centralisée](management.md).

Avec `remote.console_scope = true`, un `target_policy` placé dans `[managed.<client-id>]` sur le serveur remplace le `remote.target_policy` local : la portée se modifie alors depuis l'éditeur d'appareil de la console. Sans valeur côté serveur, la portée locale reste appliquée. Sans cette option, une configuration envoyée contenant `target_policy` est refusée. Les capacités de répertoire restent toujours locales. Les paquets de connexion générés par la console activent cette option.

## Journalisation

```sh
felucca-agent -c agent.toml --log-level debug --log-file /data/agent.log --log-json
```

Définir `--log-file` active la rotation, avec par défaut des fichiers de 10 MiB et 5 copies
archivées. Les fichiers de journal sont ouverts en ajout et stderr continue de recevoir la
sortie. Le répertoire parent doit déjà exister. `--log-max-bytes`, `--log-backups` et
`--log-rotate-interval` exigent `--log-file`. Les secrets bruts n'atteignent jamais le
journal.

## Instances multiples

```sh
felucca-agent --config office.toml --config home.json --check
felucca-agent --config office.toml --config home.json
```

Un seul processus prend en charge jusqu'à 32 instances. Voir
[Sources de configuration et CLI](configuration.md).

## Commutateurs P2P et visiteur

| Table | Effet |
| --- | --- |
| `[p2p] enabled = true` | Permet à cet agent d'accepter des connexions directes en tant que fournisseur |
| `[[visitor]]` | Écoute sur votre propre machine pour atteindre le partage privé d'un tiers |

Voir [Connexion directe P2P et repli sur le relais](p2p.md) et
[Partages privés et visiteurs](private-shares.md).

## Étapes suivantes

- [Tunnels et mappages de ports](tunnels.md)
- [Back-ends, répartition et contrôles de santé](backends.md)
- [Services, mises à jour, journalisation et supervision](operations.md) — installer en tant
  que service système
