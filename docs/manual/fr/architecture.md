# Architecture et concepts

Cette page explique les composants de Felucca, la façon dont le trafic circule et le
vocabulaire utilisé par tous les guides suivants. La répartition des modules internes est
documentée à part dans [Architecture](../../architecture.md).

## Forme du déploiement

```text
                    public network
 ┌──────────────────────────────────────────────────────┐
 │  felucca-server                                      │
 │  bridge   14028 (TCP/TLS/WS/WSS share one socket)    │
 │  quic     14028 (UDP, native QUIC streams)           │
 │  kcp      8025 (UDP)                                 │
 │  http     8080 / 8443 (+ http3 UDP)                  │
 │  admin    8088 (admin UI, optional)                  │
 └───────────────▲──────────────────────────────────────┘
                 │ agent dials out
        ┌─────────────────────┐
        │  felucca-agent      │    private network
        │  local service 3000 │
        └─────────────────────┘
```

L'agent compose vers un `felucca-server` situé sur le réseau public : le réseau privé n'a
donc besoin d'aucune IP publique, d'aucun mappage de ports sur le routeur ni d'aucune
modification de pare-feu. Chaque point d'entrée public — ports, domaines, mandataires et
fichiers — appartient au serveur.

## Les deux programmes

| Programme | Responsabilités | Ports entrants nécessaires |
| --- | --- | --- |
| `felucca-server` | Accepte les agents, détient les écouteurs publics, répartit, administre, mesure | Oui |
| `felucca-agent` | Compose depuis le réseau privé, détient les écouteurs locaux, envoie les requêtes aux back-ends | Non |

Les deux doivent utiliser la même version de protocole. La version actuelle du protocole de
pont est 10, et **le serveur et l'agent doivent être mis à niveau ensemble**. Utilisez
`--build-info` pour afficher le nom du programme, la version, le protocole et la cible de
compilation :

```sh
target/release/felucca-server --build-info
target/release/felucca-agent --build-info
```

## Concepts clés

| Terme | Signification |
| --- | --- |
| **pont** | La connexion de longue durée entre le serveur et l'agent. `bridge.bind` est l'adresse partagée par TCP/TLS/WS/WSS et vaut par défaut `0.0.0.0:14028` ; QUIC et KCP utilisent chacun leur propre port UDP |
| **identité** | L'agent est identifié par `key_id` + `vkey` ; le serveur doit disposer d'une entrée de table `[client.<key_id>]` correspondante |
| **transport** | Côté agent, vous pouvez choisir `tcp`, `tls`, `ws`, `wss`, `quic`, `kcp`. Sur le réseau public, préférez TLS/WSS/QUIC |
| **confiance** | La manière dont l'agent vérifie le certificat du serveur : `webpki` (validation par une autorité de certification), `pinned` (empreinte SHA-256 figée), `insecure` (diagnostic uniquement) |
| **tunnel** | Un mappage d'un point d'entrée public vers un ensemble de back-ends. `mode` détermine le protocole d'entrée, `backend` détermine la cible |
| **back-end** | La cible à laquelle l'agent se connecte réellement, sous la forme `127.0.0.1:3000` |
| **écouteur** | La socket que le serveur ouvre pour un tunnel. Une reconnexion avec le même `key_id` remplace l'ancien écouteur |
| **partage** | Le serveur accorde un tunnel enregistré à un visiteur nommé sans utiliser de port public |
| **visiteur** | Un autre agent détenant la clé partagée, qui ouvre un écouteur sur sa propre machine pour atteindre ce partage |
| **local** | Un groupe de configuration dans lequel le serveur fournit lui-même le service, sans agent |
| **mux** | La multiplexation bornée au-dessus de TCP/TLS/WS/WSS/KCP, qui fournit la négociation de fenêtres, le budget et la contre-pression |
| **géré** | La configuration de tunnel que le serveur envoie à l'agent ; l'agent doit activer `[remote]` explicitement |

## Vie d'une requête

Prenons l'exemple d'un tunnel TCP :

1. Un visiteur se connecte au port public du serveur.
2. Le serveur vérifie l'ACL de source, le quota et l'allocation de connexions, puis attribue
   la connexion au flux de pont de l'agent correspondant.
3. L'agent reçoit la requête, choisit un back-end sain par répartition de charge, et se
   connecte.
4. Les données circulent dans les deux sens sur un flux mux (ou un flux QUIC natif) jusqu'à
   ce que l'une des deux extrémités se ferme.
5. Le serveur mesure le trafic et peut le conserver d'un redémarrage à l'autre lorsque
   `[state]` ou le côté administration est configuré.

Un tunnel HTTP ajoute deux étapes : le routage par domaine et par chemin, et
l'authentification du site. Un tunnel SOCKS5 en ajoute deux de plus : la résolution de
l'adresse cible et une ACL de cible.

## Modules internes

En tant qu'utilisateur, vous n'avez normalement pas à vous en soucier, mais elles expliquent
ce que fait chaque crate :

| crate | Rôle |
| --- | --- |
| `felucca-core` | Configuration, identité, règles d'accès, types de mesure ; ne dépend d'aucun moteur asynchrone |
| `felucca-mux` | Multiplexation bornée, fenêtres et contre-pression |
| `felucca-proto` | Poignée de main, messages de contrôle, protocoles de visiteur et de connexion directe |
| `felucca-transport` | Adaptation des connexions TCP/TLS/WS/QUIC/KCP et réseau de sortie |
| `felucca-service` | Connexion aux back-ends, répartition de charge, contrôles de santé et service de fichiers restreint, communs aux deux côtés |
| `felucca-server` | Acceptation des agents, routage d'entrée, administration, état d'exécution |
| `felucca-client` | Connexion de l'agent, reconnexions, réception de la configuration, visiteurs |
| `felucca-cli` | Les deux programmes, configuration de démarrage, journalisation, services système, mises à jour |

## Étapes suivantes

- [Prise en main](get-started.md) — revenez en arrière si vous n'avez pas encore de tunnel
  fonctionnel
- [Sources de configuration et CLI](configuration.md)
- [Le module felucca-server](server.md) / [Le module felucca-agent](agent.md)
