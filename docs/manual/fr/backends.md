# Back-ends, répartition et contrôles de santé

Cette page couvre la configuration des back-ends, les cinq stratégies de répartition, et la
façon dont les contrôles de santé évince et rétablissent les back-ends.

## Back-ends

```toml
[[tunnel]]
mode = "tcp"
port = 9000
bind = "127.0.0.1"
balance = "round_robin"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
weight = 2

[[tunnel.backend]]
addr = "backend.internal:3001"
weight = 1
tls = true
[tunnel.backend.trust]
mode = "pinned"
fingerprint = "<backend SHA-256>"
```

| Champ | Description |
| --- | --- |
| `addr` | Obligatoire. Un nom d'hôte ou une adresse IP accompagnés d'un port |
| `weight` | Un poids entier positif utilisé par la stratégie `weighted` |
| `tls` | Active TLS vers le back-end. La validation WebPKI est utilisée par défaut, et le SNI provient du nom d'hôte de l'adresse |
| `trust` | Pour des back-ends auto-signés, utilisez `mode = "pinned"` accompagné de `fingerprint` |

`balance` appartient à `[[tunnel]]` et ne doit **pas** être écrit après `[[tunnel.backend]]`.
Les contrôles de santé et les connexions de données utilisent la même configuration TLS.

## Stratégies de répartition

| Stratégie | Comment un back-end est choisi |
| --- | --- |
| `round_robin` | Alterne entre les back-ends sains |
| `weighted` | Répartit les connexions selon le `weight`, un entier positif |
| `least_connections` | Choisit le back-end ayant actuellement le moins de connexions ouvertes |
| `source_hash` | Choisit un back-end de façon stable à partir de l'adresse IP d'origine de l'utilisateur |
| `failover` | Choisit le premier back-end sain dans l'ordre de la configuration |

Le choix a lieu sur l'**agent**. Le compteur est libéré lorsqu'une connexion à un back-end se
ferme ou est annulée. Les poids répartissent des nombres de connexions, pas des nombres
d'octets.

Détails de comportement :

- Lorsque tous les back-ends sont marqués non sains, l'un d'eux est tout de même tenté, afin
  qu'une sonde mal configurée ne puisse pas rendre le tunnel entier indisponible.
- Un échec de connexion isolé signale l'erreur et ne réessaie **pas** un autre back-end dans
  la même requête.
- `source_hash` utilise l'adresse d'origine vérifiée par l'ACL de source ou le protocole
  PROXY.

## Contrôles de santé

```toml
[tunnel.health]
type = "http"                # "tcp" | "http"
path = "/healthz"
interval = "10s"
timeout = "2s"
failures_to_evict = 3
successes_to_restore = 2
expect_status = [200, 204]
```

| Champ | Description |
| --- | --- |
| `type = "tcp"` | Vérifie uniquement qu'une connexion peut être établie |
| `type = "http"` | Demande le `path` indiqué et accepte par défaut les codes 2xx |
| `expect_status` | Énumère explicitement les codes de statut acceptables |
| `interval` | L'intervalle entre deux sondes |
| `timeout` | Le délai d'expiration d'une seule sonde |
| `failures_to_evict` | Le nombre d'échecs consécutifs qui évince un back-end |
| `successes_to_restore` | Le nombre de succès consécutifs qui rétablissent un back-end |

Une sonde HTTP utilise l'adresse du back-end comme Host. Des échecs consécutifs évincant un
back-end et des succès consécutifs le rétablissent ; les deux cas sont journalisés.

Les tunnels UDP et les mandataires à cible dynamique (socks5/httpproxy/mixedproxy) ne
devraient **pas** recevoir de sondes de back-end TCP ou HTTP, car ils n'ont pas de back-end
fixe. Il en va de même pour le mode fichier, où la cible est déterminée par le descripteur de
capacité de répertoire.

## Mise en pool des connexions de back-end HTTP

Le pool de connexions aux back-ends des tunnels HTTP est isolé par session client, tunnel, IP
source, cible et origine HTTP : chaque origine autorise au plus 8 connexions inactives, avec
un délai d'inactivité de 30 secondes, et la table externe contient au plus 256 entrées.
Lorsque l'envoi d'un en-tête PROXY est activé, le pool est aussi isolé par port source.
Plusieurs requêtes HTTP/2 peuvent partager une même connexion.

`response_timeout` (`[http]` du serveur) limite le temps entre l'envoi d'une requête et la
réception des en-têtes de réponse du back-end ; augmentez-le en conséquence pour les envois
lents de gros fichiers. Les corps de réponse utilisent la contre-pression et ne sont pas mis
en cache intégralement.

## Étapes suivantes

- [Tunnels et mappages de ports](tunnels.md)
- [Mandataire inverse HTTP/HTTPS](http-services.md)
- [Contrôle d'accès, quotas et codage de flux](security.md)
