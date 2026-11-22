# Le module felucca-server

Cette page couvre le déploiement et la configuration de `felucca-server` : écouteurs de pont,
table des clients, entrée HTTP, autorisation de ports, écouteur d'administration, services
locaux au serveur et commutateurs de plateforme externe.

## Une configuration minimale

```toml
[bridge]
bind = "0.0.0.0:14028"
ws_path = "/ws"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }

[client.agent-1]
vkey = "change-me-to-a-long-random-string"

[log]
level = "info"
```

Si `bridge.tls` est omis et que le stockage de configuration est actif (notamment avec `--init` ou la console), le serveur conserve son identité dans `<snapshot>.identity.json`, à côté de l’instantané. Gardez ce fichier lors des redémarrages et sauvegardes. Sans stockage, l’identité est temporaire. `--fingerprint` lit les fichiers PEM explicites ; les empreintes automatiques figurent dans la console active et le journal de démarrage.

```sh
target/release/felucca-server --config conf/server.toml --check
target/release/felucca-server --config conf/server.toml
```

## Écouteurs de pont

| Champ | Par défaut | Remarques |
| --- | --- | --- |
| `bridge.bind` | `0.0.0.0:14028` | TCP, TLS, WS et WSS partagent une seule socket et sont distingués par les caractéristiques de la poignée de main |
| `bridge.quic_bind` | aucun | Adresse UDP indépendante ; peut utiliser le même numéro de port que TCP |
| `bridge.kcp_bind` | aucun | Écouteur UDP KCP ; ne peut pas partager un port UDP avec QUIC/HTTP3 |
| `bridge.ws_path` | `/ws` | Chemin WS/WSS. Les deux extrémités doivent correspondre ; les mises à niveau sur d'autres chemins sont refusées |
| `bridge.tls` | aucun | `{ cert, key }`. Un certificat temporaire est généré en cas d'omission |
| `bridge.cert_reload_interval` | `30s` | Intervalle de rechargement du PEM. Le rechargement n'interrompt pas les connexions existantes |
| `bridge.proxy_accept` | désactivé | Accepte les en-têtes PROXY provenant de répartiteurs de charge de confiance |
| `[bridge.mux]` | voir ci-dessous | fenêtres et budgets mux |
| `[bridge.kcp]` | voir ci-dessous | paramètres KCP |

`[bridge.mux]` expose `initial_stream_window`, `max_stream_window`, `max_session_window`,
`max_send_queue`, `max_control_queue`, `max_streams`, `accept_backlog` et `open_timeout`.
Côté agent, une section `[mux]` correspondante reprend les mêmes champs. Les flux QUIC
natifs n'utilisent pas ces réglages.

`[bridge.kcp]` accepte `mtu`, `send_window`, `receive_window`, `interval_ms`, `nodelay`,
`fast_resend`, `congestion_control`, `session_timeout`, `data_shards`, `parity_shards` et
`max_sessions`. Le réglage FEC par défaut est de 10 fragments de données plus 3 fragments de
parité : 3 des 13 paquets d'un groupe peuvent donc être perdus et malgré tout récupérés. Les
deux extrémités doivent utiliser le même MTU et les mêmes nombres de fragments.

## La table des clients

Chaque agent possède une table `[client.<key_id>]`. Le `key_id` est le nom de l'agent sur le
serveur, et le `key_id` de l'agent doit correspondre au nom de la table.

| Champ | Remarques |
| --- | --- |
| `vkey` | Obligatoire. Le secret partagé utilisé pour l'authentification HMAC mutuelle. Sa longueur et son caractère aléatoire relèvent de votre responsabilité |
| `remark` | Une note, affichée sur la page d'administration |
| `user` | Associe une table `[user.<name>]`, utilisée pour les quotas en couches, les ACL et les permissions de locataire |
| `enabled` | `false` désactive et déconnecte ce client de façon persistante |
| `allow_config_push` | Si cet agent peut enregistrer des tunnels. Mettez `true` pour envoyer la configuration `[[managed.<id>.tunnel]]` |
| `max_tunnels` | La limite de tunnels pour cet agent |
| `max_connections` | La limite de connexions pour cet agent |
| `rate_limit` | Une limite en octets par seconde, partagée par tous les tunnels et les deux sens |
| `acl_mode` / `acl_rules` | ACL de source pour l'accès au point d'entrée public de ce client |
| `quota` | `{ flow_limit, expires_at }`, en octets et en secondes Unix |

Une nouvelle connexion avec le même `key_id` remplace l'ancienne ; les anciens écouteurs
s'arrêtent et libèrent les ports avant que ceux-ci ne soient liés à nouveau. Les compteurs de
trafic et les limiteurs de bande passante survivent aux reconnexions au sein d'un même
processus.

## Autorisation de ports

`allowed_ports` est un champ de premier niveau et **doit apparaître avant le premier en-tête
de table TOML** :

```toml
allowed_ports = [8080, { from = 9000, to = 9100 }]
```

Une liste vide autorise tous les ports TCP/UDP supérieurs à 1024. `max_tunnels` et
`max_connections` sont des limites globales de premier niveau, comptées par agent.

## Entrée HTTP

```toml
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"      # set to "" to disable HTTPS
http3_bind = "0.0.0.0:8443"    # optional, HTTP/3 UDP ingress
forwarded_headers = true
response_timeout = "100s"
cert_reload_interval = "30s"

[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"

[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

Pour l'utilisation complète, voir [Mandataire inverse HTTP/HTTPS](http-services.md) et
[Certificats, SNI et ACME](certificates.md).

## Autres tables de premier niveau

| Table | Rôle | Voir aussi |
| --- | --- | --- |
| `[admin]` / `[admin.account.<name>]` | Écouteur d'administration, comptes et rôles | [Interface d'administration et configuration centralisée](management.md) |
| `[state]` | Persistance de l'instantané de trafic | [Contrôle d'accès, quotas et codage de flux](security.md) |
| `[user.<name>]` / `[user.<name>.limits]` | Quotas et agrégation par utilisateur | [Contrôle d'accès, quotas et codage de flux](security.md) |
| `[local.<name>]` | Groupes de services locaux au serveur | [Cibles locales au serveur et routes imbriquées](local-services.md) |
| `[share.<name>]` | Autorisation de partage privé | [Partages privés et visiteurs](private-shares.md) |
| `[managed.<client>]` | Envoi de configuration centralisée | [Interface d'administration et configuration centralisée](management.md) |
| `[node]` / `[node.platform.<name>]` / `[node.webhook.<name>]` | Plateformes externes | [Intégration aux plateformes externes](platform.md) |
| `[p2p]` | Port d'observation P2P | [Connexion directe P2P et repli sur le relais](p2p.md) |
| `[geo]` | Fichiers de données GeoIP/GeoSite | [Contrôle d'accès, quotas et codage de flux](security.md) |
| `[ntp]` | Calibrage de l'heure d'authentification du pont | [Le module felucca-agent](agent.md) |
| `[log]` / `[log.rotation]` | Journalisation et rotation | [Services, mises à jour, journalisation et supervision](operations.md) |
| `allowed_ports` | Autorisation de ports | Cette page |

## Commandes utiles

| Commande | Rôle |
| --- | --- |
| `--check` | Valide la configuration et quitte sans lier de ports |
| `--fingerprint` | Affiche le SHA-256 du certificat `bridge.tls`. Nécessite qu'un PEM fixe soit configuré |
| `--hash-password` | Lit une ligne de mot de passe sur l'entrée standard et affiche un hachage de mot de passe d'administration. Le mot de passe doit comporter au moins 12 octets |
| `--generate-token` | Affiche un jeton Bearer d'administration et son hachage de stockage |
| `--runtime-config PATH` | Active l'instantané de configuration d'exécution |
| `--build-info` | Affiche le nom du programme, la version, le protocole et la cible de compilation |

## Cycle de vie

- `SIGINT` / `SIGTERM` ferment les écouteurs et les connexions en cours, puis quittent.
- Les modifications apportées aux fichiers de démarrage exigent un redémarrage du processus.
  Les modifications effectuées via l'écouteur d'administration passent par une transaction
  d'instantané de configuration d'exécution, qui interrompt les connexions relayées, mais
  l'agent se reconnecte ensuite.
- Modifier la liaison d'administration, les chemins des fichiers de certificats, l'origine
  publique ou le chemin d'état **exige un redémarrage du processus**.
- Les compteurs de trafic survivent aux reconnexions des agents. Ils ne survivent à un
  redémarrage du serveur que si `[state]` est configuré.

## Étapes suivantes

- [Le module felucca-agent](agent.md) — mettre en place les services internes
- [Tunnels et mappages de ports](tunnels.md)
- [Mandataire inverse HTTP/HTTPS](http-services.md)
- [Services, mises à jour, journalisation et supervision](operations.md) — systemd / launchd
  / service Windows
