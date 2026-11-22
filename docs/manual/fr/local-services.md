# Cibles locales au serveur et routes imbriquées

`[local.<name>]` permet au serveur de fournir lui-même des services, sans agent et sans
identifiants de pont. Les back-ends des tunnels locaux peuvent aussi utiliser des alias qui
entrent dans un autre tunnel enregistré, y compris un tunnel sur un agent, ce qui compose des
routes imbriquées.

## Un groupe de services locaux

```toml
[user.office]
[user.office.limits]
max_connections = 100

[local.office]
user = "office"
dial_timeout = "10s"
[local.office.network]
local_ip = "127.0.0.1"
forward_local_ip = true

[[local.office.tunnel]]
id = 1
mode = "tcp"
port = 9001
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
[local.office.tunnel.health]
type = "tcp"
interval = "10s"
timeout = "2s"
```

Chaque groupe peut définir `user`, `acl_mode`/`acl_rules`, `limits`, `network` et
`dial_timeout`, et peut utiliser la même configuration TCP/UDP, HTTP/SNI, mandataire,
fichier, répartition de charge, sonde et ACL qu'un agent. Lorsque vous n'exploitez que des
services locaux, vous pouvez omettre la table des clients.

Un groupe local est enregistré sous le nom `local.<name>` dans le registre et dans les
instantanés de trafic, et **cet espace de noms ne peut pas servir de key_id à un agent
externe**. Un groupe local n'a pas de pont : la configuration d'un mandataire amont est donc
rejetée. Seul `forward_local_ip = true` utilise l'IP de sortie pour le back-end. Les racines
de fichiers, les certificats et la configuration des back-ends sont préparés au démarrage, et
un conflit d'écoute empêche ce service de démarrer.

## Alias de back-end

| Adresse | Comportement |
| --- | --- |
| `tunnel://local.office/7` | Entre dans l'entrée de service d'un autre tunnel local enregistré |
| `tunnel://agent-1/7` | Entre dans un tunnel enregistré sur un agent en ligne |
| `file://agent-1/7` | Identique ci-dessus, mais exige que la cible enregistrée soit en mode `file` ; les frontières de répertoire, l'état de lecture seule et l'authentification fichier sont préservés |
| `bridge://tcp`, `tls`, `ws`, `wss` | Entre dans l'entrée de pont correspondante ; l'agent a toujours besoin d'une authentification mutuelle, et TLS utilise le certificat de pont |
| `bridge://http` | Entre dans l'entrée HTTP du serveur, où le Host, l'authentification du site et les vérifications de permissions continuent de s'appliquer |
| `bridge://web` | Se connecte au point d'accès d'administration, qui s'authentifie de la même manière |

Les numéros de tunnel doivent être explicites et stables. Un groupe local doté d'un `user` ne
peut référencer que des cibles de ce même utilisateur ; un groupe sans `user` est géré par
l'opérateur du serveur et peut référencer des cibles d'autres utilisateurs. Les alias `bridge`
ne sont autorisés que pour les groupes d'opérateur. Un agent distant et les requêtes de
mandataire dynamique ne peuvent pas utiliser un alias comme cible réseau.

Les alias ne prennent ni `backend.tls` supplémentaire, ni sonde de santé réseau, ni ACL de
cible d'entrée : les permissions sont appliquées par la cible de chaque saut. Un alias
HTTPS/SNI peut transmettre directement les données TLS, et le site cible y met fin ou les
relaie. L'UDP ne peut s'imbriquer que dans de l'UDP, et les autres modes utilisent l'entrée de
flux. L'authentification du mandataire SOCKS5/HTTP n'est pas ignorée.

## Contrôle de boucle et comptabilisation

- Chaque saut conserve l'IP et le port source d'origine ainsi que l'historique de routage.
- Un `(client, id)` répété ou plus de **16 sauts** est immédiatement rejeté.
- Le pool de connexions HTTP est aussi isolé par historique : une connexion réutilisée ne peut
  donc pas contourner le contrôle de boucle.
- Chaque tunnel réellement traversé reçoit sa propre allocation de connexions et enregistre le
  trafic ; lorsque le même utilisateur ou groupe traverse plusieurs sauts, le quota agrégé est
  aussi imputé à chaque saut.
- La comptabilisation UDP porte sur la charge utile du message et exclut les trames de longueur
  internes.

## Schémas courants

| Scénario | Comment |
| --- | --- |
| Exposer aussi un service de l'hôte serveur | `[local.ops]` + `[[local.ops.tunnel]]` |
| Ajouter une terminaison TLS devant un service interne | Un tunnel HTTP dans un groupe local avec `tls_mode = "offload"` |
| Ajouter une autre entrée par-dessus un service interne, par exemple une authentification de site | Un groupe local externe dont le back-end est écrit `tunnel://agent-1/7` |
| Réutiliser l'authentification du point d'accès d'administration depuis un service interne | Écrivez le back-end sous la forme `bridge://web` |

## Étapes suivantes

- [Tunnels et mappages de ports](tunnels.md)
- [Le module felucca-server](server.md)
- [Contrôle d'accès, quotas et codage de flux](security.md)
- Exemples : [local-server.toml](../../../examples/local-server.toml)
