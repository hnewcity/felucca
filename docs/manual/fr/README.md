# Index de la documentation Felucca · Français

[中文](../README.md) · [English](en/README.md) · [日本語](ja/README.md) · [한국어](ko/README.md) · Français · [Español](es/README.md) · [Русский](ru/README.md)

Voici la documentation utilisateur de Felucca : du premier tunnel fonctionnel à la
configuration et à l'utilisation de chaque module, jusqu'aux références de commandes, de
champs, de points d'entrée et de terminologie. Elle décrit **comment utiliser Felucca**,
pas comment il est implémenté.

## Prise en main

| Page | Contenu |
| --- | --- |
| [Prise en main](get-started.md) | Installation, démonstration locale et premier tunnel établi à la main |
| [Architecture et concepts](architecture.md) | Les composants, le chemin suivi par le trafic et un glossaire |
| [Sources de configuration et CLI](configuration.md) | Sources de configuration, variables d'environnement, instances multiples, `--check` |

## Guides

| Page | Contenu |
| --- | --- |
| [Le module felucca-server](server.md) | Pont, table des clients, entrée HTTP, autorisation de ports |
| [Le module felucca-agent](agent.md) | Connexion, confiance, reconnexions, réseau de sortie, mandataire amont |
| [Tunnels et mappages de ports](tunnels.md) | Les huit modes, `[[mapping]]`, UDP, protocole PROXY |
| [Back-ends, répartition et contrôles de santé](backends.md) | Champs des back-ends, les cinq stratégies, éviction et rétablissement |
| [Mandataire inverse HTTP/HTTPS](http-services.md) | Routage par domaine et par chemin, réécriture, CORS, SNI, HTTP/2, HTTP/3 |
| [Certificats, SNI et ACME](certificates.md) | Certificats de pont, certificats statiques, émission et renouvellement |
| [Mandataire direct](forward-proxy.md) | Entrées SOCKS5 / HTTP / mixtes, comptes, ACL de cible |
| [Services de fichiers et WebDAV](file-services.md) | Capacités des répertoires, contrôle en lecture seule, Range, méthodes WebDAV |
| [Cibles locales au serveur et routes imbriquées](local-services.md) | `[local.*]`, alias de back-end, contrôle de boucle à 16 sauts |
| [Partages privés et visiteurs](private-shares.md) | Accès TCP/UDP/fichier/mandataire sans port public |
| [Connexion directe P2P et repli sur le relais](p2p.md) | Billets à usage unique, percement de trou, direct QUIC, repli |
| [Contrôle d'accès, quotas et codage de flux](security.md) | ACL de source et de cible, GeoIP, quotas en couches, compression et chiffrement |
| [Interface d'administration et configuration centralisée](management.md) | Interface d'administration, rôles des comptes, API, envoi de configuration |
| [Intégration aux plateformes externes](platform.md) | Jetons de plateforme, RPC WSS inverse, rappels signés, import de trafic |
| [Services, mises à jour, journalisation et supervision](operations.md) | systemd/launchd/SCM, mises à jour, rotation des journaux, métriques |

## Référence

| Page | Contenu |
| --- | --- |
| [Référence](reference.md) | CLI, aide-mémoire des champs, API d'administration, index des exemples, limites numériques, glossaire |

## Documentation associée

- [Architecture](../../architecture.md) — frontières de responsabilité des crates
- [Configuration et déploiement](../../configuration.md) — détail champ par champ
- [État d'avancement](../../implementation-status.md) · [Validation](../../validation.md)

Les documents de référence un niveau plus haut restent en chinois et en anglais.
