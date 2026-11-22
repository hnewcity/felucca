# Référence

Commandes, champs, points d'entrée, un index des exemples et un glossaire. Les guides expliquent
*pourquoi* ; cette page énumère *quoi*.

## CLI

Arguments partagés par les deux programmes :

| Argument | Variable d'environnement | Description |
| --- | --- | --- |
| `--build-info` | — | Affiche le nom du programme, la version, le protocole et la cible de compilation |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | Fichier TOML/JSON, URL HTTP(S), JSON en ligne, base64 ou URI `felucca://` ; répétable pour l'agent |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | Expanse `${VAR}` / `${VAR:-default}` |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | La variable qui contient le jeton Bearer de la configuration distante |
| `--check` | — | Valide et quitte sans lier ni se connecter |
| sous-commande `service` | — | Gestion du service système |
| sous-commande `update` | — | Vérifie et installe un binaire autonome |

`felucca-server` ajoute :

| Argument | Variable d'environnement | Description |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | Chemin de l'instantané de configuration d'exécution |
| `--fingerprint` | — | Affiche le SHA-256 du certificat `bridge.tls` |
| `--hash-password` | — | Lit une ligne de mot de passe sur l'entrée standard et affiche un hachage PBKDF2 ; au moins 12 octets |
| `--generate-token` | — | Affiche un jeton Bearer et son `token_hash` |

`felucca-agent` ajoute :

| Argument | Défaut | Description |
| --- | --- | --- |
| `--launch` | — | Alias de `--config` |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | Ajoute à la fin et écrit aussi sur stderr ; le répertoire parent doit exister |
| `--log-json` | — | Événements JSON structurés |
| `--log-max-bytes <N>` | 10 MiB | Exige `--log-file` |
| `--log-backups <N>` | 5 | Exige `--log-file` |
| `--log-rotate-interval <D>` | — | Par exemple `24h` ; exige `--log-file` |

sous-commande `service` :

| Argument | Description |
| --- | --- |
| `--name <NAME>` | Nom de l'instance de service |
| `--scope <user\|system>` | Vaut `user` par défaut ; `system` uniquement sous Windows |
| `--dry-run` | N'affiche que les actions qui seraient exécutées |
| `render` | Génère la définition du service ; `--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | Cycle de vie |
| `--workdir <DIR>` | Vaut par défaut le répertoire qui contient la configuration |
| `--binary <PATH>` | Sélectionne le programme à installer |
| `-- <ARGS>` | Arguments supplémentaires du démon |

sous-commande `update` :

| Argument | Description |
| --- | --- |
| `--source <PATH_OR_URL>` | Un binaire autonome, ou une archive tar.gz/zip |
| `--sha256 <HEX>` | 64 chiffres hexadécimaux ; la base de confiance |
| `--check-only` | Vérifie sans remplacer |
| `--expect-version <V>` | Assertion de version facultative |
| `--token-env <NAME>` | La variable qui contient le jeton Bearer d'un téléchargement privé |

## Structure de la configuration

| Fichier | Contenu de premier niveau |
| --- | --- |
| server | `allowed_ports`, `max_tunnels`, `max_connections`, `[bridge]`, `[http]`, `[admin]`, `[state]`, `[geo]`, `[ntp]`, `[log]`, `[p2p]`, `[node]`, `[user.*]`, `[client.*]`, `[local.*]`, `[share.*]`, `[managed.*]` |
| agent | `server`, `key_id`, `vkey`, `transport`, `reconnect_min/max`, `dial_timeout`, `max_connections`, `[trust]`, `[network]`, `[mux]`, `[kcp]`, `[ntp]`, `[remote]`, `[geo]`, `[[tunnel]]`, `[[mapping]]`, `[[visitor]]`, `[p2p]`, `[log]` (l'agent utilise des arguments en ligne de commande) |

`allowed_ports` doit apparaître avant le premier en-tête de table TOML. Toutes les tables activent
`deny_unknown_fields`.

## Aide-mémoire des champs de tunnel

| Champ | Modes concernés | Description |
| --- | --- | --- |
| `id` | tous | ID stable ; fixez-le dès que les instantanés de trafic sont activés |
| `mode` | tous | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | tous sauf privé | Le port du serveur ; `http` doit correspondre à un écouteur HTTP |
| `bind` | tcp/udp/mandataire/fichier | Adresse IP uniquement |
| `public` | tous | `false` signifie un partage privé |
| `hosts` | http, file | Liste de domaines ; `*.example.com` est pris en charge |
| `balance` | back-ends fixes | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | back-ends fixes | `addr`, `weight`, `tls`, `trust` |
| `health` | back-ends fixes | `type`, `path`, `interval`, `timeout`, `failures_to_evict`, `successes_to_restore`, `expect_status` |
| `acl_mode`/`acl_rules` | tous | ACL de source : off/allow/deny |
| `target_policy` | back-ends fixes et mandataires directs | `allow`/`deny`/`ports` |
| `options.auth.users` | mandataires directs, HTTP | Table des comptes |
| `options.http.*` | http | Chemins, réécriture, en-têtes, redirections, CORS, pages d'erreur, tls_mode, backend_http2 |
| `options.payload.*` | tous | `compress`, `encrypt` |
| `options.limits.*` | tous | `max_connections`, `rate_limit`, `quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | entrées de mandataire | Accepte les en-têtes PROXY |
| `file` | file | `root`, `prefix`, `read_only`, `auto_index`, `auth` |

## Résumé de l'API d'administration

L'authentification se fait par cookie avec Origin et CSRF, ou par `Authorization: Bearer <token>`.
Les écritures exigent `If-Match: "<revision>"` et un `Idempotency-Key` unique. La limite du corps de
requête est de 2 MiB.

| Groupe | Chemins |
| --- | --- |
| Session | `/api/login`, `/api/session`, `/api/logout` |
| Ressources | `/api/users`, `/api/clients`, `/api/locals`, `/api/shares`, `/api/accounts`, chacun avec son `<id>` |
| Configuration centralisée | `/api/managed/<client>`, `/api/managed/<client>/tunnels/<id>` |
| Tunnels locaux au serveur | `/api/locals/<group>/tunnels/<id>` |
| Configuration complète | `/api/config`, `/api/config?secrets=1`, `/api/validate`, `/api/batch` |
| Temps réel | `/api/events` (SSE), `/metrics`, `/api/metrics`, `/api/profile` |
| Plateforme | `/api/node/*`, `/api/platforms/<name>`, `/api/webhooks/<name>`, `/api/ws`, `/api/realtime/subscriptions` |

Voir [Interface d'administration et configuration centralisée](management.md) et
[Intégration aux plateformes externes](platform.md) pour le détail des champs.

## Index des exemples de configuration

| Fichier | Contenu |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | Exemple de serveur minimal |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | Exemple d'agent minimal |
| [features-server.toml](../../../examples/features-server.toml) | Pont QUIC + HTTP/HTTPS + certificats |
| [features-agent.toml](../../../examples/features-agent.toml) | Tous les modes de mandataire, répartition de charge, contrôles de santé |
| [shared-server.toml](../../../examples/shared-server.toml) | Ports TCP/UDP partagés + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | Côté agent et mappage de ports en masse |
| [acme-server.toml](../../../examples/acme-server.toml) | Serveur ACME |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL et TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | ACL en couches, quotas, persistance |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | Services de fichiers |
| [network-agent.toml](../../../examples/network-agent.toml) | Mandataire amont, DNS personnalisé, NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy et chiffrement indépendant des flux |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | Pont KCP |
| [local-server.toml](../../../examples/local-server.toml) | Cibles locales au serveur et transfert imbriqué |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | Fournisseur privé et visiteur |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P et repli |
| [admin-server.toml](../../../examples/admin-server.toml) | Serveur d'administration |
| [managed-agent.toml](../../../examples/managed-agent.toml) | Agent géré de façon centralisée |
| [node-server.toml](../../../examples/node-server.toml) | Plateforme externe et gestion des nœuds |

Les chemins de configuration sont relatifs au répertoire de travail du processus. Les 26 exemples
passent tous la validation de configuration.

## Limites numériques

| Élément | Limite |
| --- | --- |
| Configuration unique | 4 MiB |
| Instantané de configuration d'exécution | 16 MiB |
| Instances d'agent | 32 |
| Sauts de routage imbriqués | 16 |
| Opérations par lot | 128 éléments |
| Import de trafic | 128 éléments par requête |
| Enregistrements d'idempotence conservés | 256 (256 chacun pour les actions de rappel et l'import de trafic) |
| File de rappels | `queue_max` par cible, 4096 à l'échelle du nœud |
| WebSockets d'administration directs | 16 |
| Concurrence RPC WS | 2 par connexion / 16 à l'échelle du nœud |
| Abonnements temps réel | 32 éléments par connexion, 32 KiB par élément |
| Pool de connexions des back-ends HTTP | 8 en veille par origine, 30 secondes d'inactivité, 256 entrées dans la table externe |
| HTTP/3 | 128 flux par connexion, 1 MiB par flux / fenêtre de connexion de 16 MiB, en-têtes de 32 KiB |
| UDP | 65507 octets par source ; 256 associations par tunnel |
| SOCKS5 UDP | 64 destinations par session, 8 paquets en file pour chacune |
| Pages d'erreur | 400..599, 64 KiB par page |
| Correspondance de domaines HTTP | Les domaines exacts ont priorité sur les jokers |

## Environnement et compilation

| Élément | Valeur |
| --- | --- |
| Rust minimal | 1.88 |
| Environnement vérifié | macOS Intel, Rust/Cargo 1.98.1 |
| Tests de l'espace de travail | 581 réussis / 0 échoués / 0 ignorés (2026-09-17) |
| Binaires | `target/release/felucca-server`, `target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

Ne lancez qu'une seule commande Cargo à la fois sur le même répertoire cible. Terminez les
compilations, les tests et rustdoc avant de modifier le code source et les pages intégrées. Les
tests ont besoin de la permission d'écouter sur des ports TCP/UDP locaux.

## Glossaire

| Terme | Signification |
| --- | --- |
| agent | Le processus `felucca-agent` qui s'exécute dans un réseau privé et se connecte vers l'extérieur |
| bridge | La connexion authentifiée de longue durée entre le serveur et l'agent |
| client / `[client.<id>]` | La table d'identité du serveur pour un agent ; `key_id` est le nom de la table |
| key_id | Le nom de l'agent sur le serveur |
| vkey | Le secret de pont partagé par l'agent et le serveur |
| transport | Le transport du pont : tcp/tls/ws/wss/quic/kcp |
| trust | La façon dont l'agent vérifie le certificat du serveur : webpki/pinned/insecure |
| tunnel | Un mappage d'un point d'entrée public vers un ensemble de back-ends |
| backend | La cible à laquelle l'agent se connecte réellement |
| mapping | Mappage de ports un à un en masse |
| local group | Un groupe des services propres du serveur qui n'a besoin d'aucun agent |
| share | Le serveur autorise un invité à utiliser un tunnel |
| visitor | Un agent qui détient un secret de partage et écoute localement |
| provider | La partie qui fournit un service dans un partage privé ou une configuration P2P |
| managed | La configuration que le serveur envoie à un agent |
| mux | Multiplexage borné sur des transports de type TCP |
| FEC | Correction d'erreurs sans retour de KCP, 10 données + 3 parité par défaut |
| quota / rate_limit | Quotas de trafic et limites de débit en octets par seconde |
| snapshot | Une persistance atomique du trafic ou de la configuration d'exécution |

## Documentation associée

- [Architecture](../../architecture.md) — frontières de responsabilité des crates
- [Configuration et déploiement](../../configuration.md) — détail champ par champ
- [P2P](../../p2p.md), [Tunnels privés et accès partagé](../../visitors.md),
  [Gestion et configuration centralisée](../../management.md),
  [Nœuds et plateformes externes](../../node.md), [Opérations et distribution](../../operations.md),
  [TCP transparent](../../transparent.md)
