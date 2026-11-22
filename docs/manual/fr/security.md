# Contrôle d'accès, quotas et codage de flux

Cette page rassemble la configuration liée à la sécurité : ACL de source, ACL de cible,
GeoIP/GeoSite, quotas et limites de débit en couches, instantanés de trafic, ainsi que la
compression et le chiffrement facultatifs par tunnel.

## ACL de source

```toml
[client.agent-1]
acl_mode = "allow"                 # off | allow | deny
acl_rules = "203.0.113.0/24, 2001:db8::/32"
```

Les règles sont des entrées IP/CIDR séparées par des virgules ou des retours à la ligne. Les
`acl_mode`/`acl_rules` de premier niveau du serveur, `[user.<name>]`, `[client.<id>]` et les
règles du tunnel sont **intersectés** ; un client s'associe à un utilisateur dans sa propre
configuration via `user = "<name>"`, et l'agent ne peut pas modifier cette association.

Ces règles s'appliquent aux clients qui atteignent une entrée publique et ne servent **pas à
refuser l'adresse de pont d'un agent** — l'identité du pont est vérifiée par la vkey. Les
règles portent sur l'IP source des clients atteignant le mandataire, et non sur l'adresse
cible.

Les ACL de source prennent en charge GeoIP. `acl_rules` peut aussi être écrit sur
`[[tunnel]]`, `[local.<name>]` et `[[visitor]]`.

## Politique de cible

Appliquée côté agent, elle concerne les back-ends fixes, les sondes de santé et les cibles de
mandataire direct :

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

| Élément | Description |
| --- | --- |
| `allow` vide | Aucune restriction sur les cibles autorisées |
| `deny` | A toujours priorité |
| `ports` vide | Tout port non nul est autorisé |
| Syntaxe | IP/CIDR, `domain:`, `suffix:`, `regex:`, `*` |
| Geo | `geoip:CN`, `geosite:category-ads-all`, `geosite:custom:LOCAL@fast@!ads` |

`@attribute` exige que l'attribut soit présent, `@!attribute` exige qu'il soit absent, et
plusieurs conditions sont intersectées. `geoip:private` couvre les plages IPv4/IPv6 privées,
de boucle locale, de lien local et CGNAT intégrées, et n'a besoin d'aucun fichier de données.
Le nom et l'IP résolue sont vérifiés ensemble ; les noms sont insensibles à la casse, et un
point final est normalisé.

Un fichier manquant, un groupe manquant, une expression régulière invalide ou un protobuf
corrompu **constituent une erreur**, et ne se transforment pas en règle vide qui autoriserait
tout. Les données sont chargées au démarrage : redémarrez le processus concerné après avoir
mis à jour un fichier de données. Chaque extrémité utilise ses propres fichiers de données
déclarés localement, et le côté distant ne peut que référencer des groupes et des alias.

## Données Geo

```toml
[geo]
geoip = "data/geoip.dat"
geosite = "data/geosite.dat"

[geo.extra]
mycorp = "data/mycorp.dat"
```

Faites pointer ces champs vers des fichiers `.dat` V2Ray au format protobuf, avec des chemins
relatifs au répertoire de travail du processus. Les préfixes IP utilisent des recherches de
plages compilées ; les domaines prennent en charge les correspondances exacte, par suffixe, par
mot-clé et par expression régulière.

## Quotas en couches

`[user.<name>.limits]`, `[tunnel.options.limits]` et
`[http.host_limit."<domain pattern>"]` acceptent les mêmes champs :

```toml
max_connections = 20
rate_limit = 1048576
quota = { flow_limit = 1073741824, expires_at = 1893456000 }
```

Les unités de trafic et de débit sont l'octet, et `expires_at` est exprimé en secondes Unix.
Omettre un champ signifie l'absence de limite ; un débit de 0 signifie l'absence de limite,
tandis qu'un nombre de connexions de 0 bloque explicitement les nouvelles connexions. Les
plusieurs clients d'un utilisateur, les plusieurs connexions d'un tunnel et les plusieurs
chemins d'un domaine partagent leurs propres compteurs et limites.

Plusieurs couches de débit sont imputées en même temps, et le temps d'attente retenu est celui
de la **couche la plus stricte** ; une demande de créneau de connexion refusée rend les
créneaux accordés par les couches précédentes. Les limites par domaine HTTP sont choisies
d'après le Host/SNI d'origine, un domaine exact a priorité sur un joker, et elles ne sont pas
affectées par la réécriture du Host du back-end. Le comptage des connexions HTTP porte sur les
connexions réelles aux back-ends ; plusieurs requêtes HTTP/2 peuvent réutiliser une même
connexion.

TCP, HTTP, SNI et UDP mettent tous à jour les compteurs de chaque couche. TCP est vérifié et
limité par blocs de 16 Kio, et l'UDP est traité par datagramme. Le trafic en vol lors de
transferts concurrents peut légèrement dépasser le quota total : ce n'est donc **pas** une
coupure exacte octet par octet. HTTP compte les octets de protocole du back-end, en-têtes et
surcoût de protocole inclus ; l'UDP ne compte que la charge utile. Un quota interrompt les
connexions en cours de transfert, et les compteurs ne servent pas à une facturation stricte.

## Instantanés de trafic

```toml
[state]
path = "data/traffic.json"
save_interval = "5s"
```

Une fois activée, l'instantané est remplacé de façon atomique à chaque intervalle, et les
compteurs finaux sont enregistrés lors d'une sortie normale et après la fin des connexions.
L'instantané inclut les ressources hors ligne : celles-ci continuent donc de compter pour les
quotas après un redémarrage du serveur ; un plantage peut faire perdre les données du dernier
intervalle de sauvegarde. Un instantané donné ne peut être écrit que par un seul serveur. Un
instantané corrompu ou d'une version inconnue empêche le démarrage, et le fichier d'origine est
conservé.

Il s'agit d'un instantané de trafic ; la configuration d'exécution de l'interface
d'administration est stockée à part dans `admin.state_path` / `--runtime-config`, et les deux
ne doivent pas utiliser le même chemin. Dès que les instantanés sont activés, figez l'`id` de
vos tunnels.

## Codage par flux

```toml
[tunnel.options.payload]
compress = true
encrypt = true
```

| Option | Implémentation | Comportement |
| --- | --- | --- |
| `compress` | Snappy | Fonctionne par blocs d'au plus 16 Kio de données brutes, et n'utilise un bloc compressé que lorsqu'il est plus petit ; la taille déclarée est vérifiée avant décompression |
| `encrypt` | ChaCha20-Poly1305 | Chaque flux échange un nonce neuf, et la vkey de l'agent vérifie l'identité des deux côtés ainsi que l'intégralité de la requête de routage ; HKDF dérive deux clés directionnelles indépendantes |

Lorsque la compression est activée, les données sont **compressées avant d'être chiffrées**.
Les en-têtes de longueur, les numéros de séquence et l'enregistrement de fin sont couverts par
des contrôles d'intégrité, et une altération, une relecture ou un enregistrement de fin
manquant ferment le flux de données. Lorsque les deux extrémités ne correspondent pas, la
négociation est refusée, sans repli automatique sur le texte clair. Chaque sens ne met en
tampon qu'un seul enregistrement borné.

Cette couche de codage vit à l'intérieur du flux mux/QUIC et n'ajoute pas sa propre couche de
multiplexation ; les en-têtes PROXY, le contenu HTTP/WebDAV et les datagrammes UDP passent tous
par le même codage. Les quotas de trafic et les limites de débit sont toujours calculés sur les
**octets bruts relayés**.

Frontière de sécurité :

- Le chiffrement indépendant protège la partie données du routage ; l'enregistrement de la
  configuration et les messages de contrôle du pont restent protégés par le transport de pont
  choisi : TLS, WSS ou QUIC reste donc le choix de déploiement par défaut.
- Cette couche PSK **n'assure pas à elle seule la confidentialité persistante**.
- La compression modifie la taille observable des enregistrements ; conservez
  `compress = false` lorsque vous devez masquer les caractéristiques de compression.

## Traitement des secrets

- Les secrets bruts n'atteignent jamais les journaux ; les affichages Debug et Display sont
  masqués.
- La persistance doit conserver le secret, et le masquage n'a lieu qu'à l'affichage ; lorsque
  vous ajoutez un champ d'identifiants, vérifiez du même coup le chemin de masquage.
- Les instantanés de configuration contiennent les véritables secrets, et sur Unix les nouveaux
  fichiers sont en 0600 et les nouveaux répertoires en 0700.
- N'écrivez pas le jeton Bearer d'administration brut dans la configuration du serveur ;
  n'écrivez que `token_hash`.

## Étapes suivantes

- [Tunnels et mappages de ports](tunnels.md)
- [Mandataire inverse HTTP/HTTPS](http-services.md)
- [Interface d'administration et configuration centralisée](management.md)
