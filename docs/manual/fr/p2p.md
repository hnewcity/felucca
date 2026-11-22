# Connexion directe P2P et repli sur le relais

Lorsque les deux extrémités se trouvent derrière un NAT, Felucca peut tenter un percement de
trou UDP pour établir une liaison directe QUIC, et se replier sur le relais du serveur. Les
données applicatives contournent le serveur ; la connexion principale transporte toujours le
contrôle et la comptabilisation. Le serveur, l'agent fournisseur et l'agent visiteur doivent
être mis à niveau ensemble.

## Serveur : le port d'observation

```toml
[bridge]
bind = "0.0.0.0:14028"

[p2p]
bind = "0.0.0.0:8025"        # Listen on this host
advertise = "203.0.113.10:8025"   # External UDP address both ends can reach, without a scheme
timeout = "5s"
max_pending = 128
```

| Champ | Par défaut | Contrainte |
| --- | --- | --- |
| `bind` | — | Adresse d'écoute locale |
| `advertise` | — | Adresse UDP accessible de l'extérieur, **sans** schéma d'URL |
| `timeout` | `5s` | 500ms–10s |
| `max_pending` | `128` | 1–4096 |

`advertise` ne doit pas chevaucher les ports UDP utilisés par KCP, QUIC ou HTTP/3. Le
`127.0.0.1` de la configuration ne convient qu'aux démonstrations où tous les processus
tournent sur le même hôte ; pour un usage inter-réseaux, remplacez-le par le nom de domaine
public et le port du serveur.

## Fournisseur : autoriser la connexion directe

```toml
# provider agent
[p2p]
enabled = true
max_sessions = 64        # Default 64, maximum 1024
bind_ip = "192.0.2.10"   # Optional; fixes the local UDP IP

[[tunnel]]
id = 1
mode = "tcp"
public = false
backend = [{ addr = "127.0.0.1:3000" }]
```

```toml
# server: a share must enable direct explicitly
[share.office-web]
client = "office"
tunnel = 1
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]
direct = true
```

Le serveur, le fournisseur local et le partage doivent chacun être **activés explicitement**
(le `[p2p]` du serveur, le `[p2p] enabled = true` du fournisseur et le `direct = true` du
partage). La connexion principale des deux extrémités doit utiliser TLS, WSS ou QUIC avec un
certificat vérifié. Lorsque `bind_ip` est omis, la configuration d'IP de sortie de l'agent est
utilisée ; cela ne change pas la sortie employée pour les connexions aux back-ends.

## Visiteur : préférence de connexion

```toml
# visitor agent
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"
connection = "prefer_direct"    # relay | prefer_direct | direct
```

| `connection` | Comportement |
| --- | --- |
| `relay` | Le défaut ; utilise le relais existant |
| `prefer_direct` | Tente d'abord la liaison directe, et se replie sur le relais lorsque la liaison directe ne peut pas être établie et qu'aucune donnée applicative n'a été envoyée |
| `direct` | N'accepte que la liaison directe ; ferme la connexion locale lorsque celle-ci est indisponible |

## Comment la liaison est établie

1. La connexion principale termine d'abord l'authentification du nœud et du secret de
   partage ; le serveur vérifie l'identité du visiteur, les ACL de source et les quotas de
   connexions des deux côtés.
2. Le serveur émet un **billet à usage unique**, lié au tunnel, à la source, à la cible, au
   type UDP, aux options de codage et aux informations PROXY, et ne transmet pas la clé de
   nœud du fournisseur.
3. Les deux extrémités sondent les adresses UDP observées à l'aide de clés de billet
   différentes ; le serveur confirme le chemin de retour par un cookie de courte durée, lié à
   la source, et limite le nombre de sondes valides, le nombre de billets et leur durée de
   vie ; les billets inconnus ne reçoivent pas de réponse.
4. Le fournisseur conserve la même socket UDP que celle utilisée pour l'observation et envoie
   les paquets de percement vers le point terminal observé par le visiteur.
5. Le visiteur se connecte directement en QUIC TLS 1.3, en épinglant l'empreinte du certificat
   éphémère du fournisseur, puis effectue un HMAC bidirectionnel du billet lié et de deux
   nonces.
6. La sélection du back-end, l'état de santé, les ACL de cible, l'authentification fichier et
   les descripteurs de capacité de répertoire continuent de réutiliser le moteur de service
   d'origine.

Si le percement de trou échoue, la tentative du fournisseur et les quotas des deux côtés sont
d'abord nettoyés, et ce n'est qu'ensuite qu'une connexion par relais est autorisée. Une chute
après le début du transfert **ne rejoue pas les données applicatives**. Une chute de la
connexion principale, un nœud désactivé, des quotas épuisés ou un service arrêté ferment les
liaisons directes concernées.

## Comptabilisation et limites

- Les données applicatives circulent en QUIC entre les deux extrémités et ne passent pas par
  le relais de données du serveur.
- Le fournisseur conserve au plus un bloc de données en attente de comptabilisation par sens
  (blocs TCP ≤ 16 Kio, UDP ≤ 65507 octets).
- Le serveur vérifie et planifie le trafic et les plages de débit des deux côtés avant
  l'envoi, et valide les nombres d'octets réels après l'envoi ; les numéros de séquence en
  doublon, les autorisations qui se chevauchent et les validations au-delà du quota sont
  refusés.
- Le comptage repose sur les **octets applicatifs décodés**, à l'exclusion des sondes, de TLS,
  du surcoût de compression, des en-têtes de trame UDP et des en-têtes PROXY TCP.
- La comptabilisation directe repose sur les rapports du fournisseur authentifié ; le serveur
  ne peut pas observer lui-même les charges utiles directes. Si le fournisseur s'arrête
  brutalement ou si la connexion de contrôle tombe avant le rapport final, les derniers blocs
  envoyés mais non validés peuvent ne pas être enregistrés dans les compteurs persistants. Un
  arrêt normal se vide par des messages de confirmation.
- Les demandes de quota par bloc accroissent la dépendance au RTT de la connexion principale :
  cela ne doit donc **pas** être décrit comme une liaison directe hors ligne indéfinie,
  détachée du serveur.

## Services pris en charge

Les services TCP, UDP et de fichiers restreints, privés comme publics, sont pris en charge,
ainsi que les cibles de mandataire direct SOCKS et HTTP sur les partages dynamiques. Les
entrées HTTP/SNI de site, les routes locales au serveur et imbriquées, et les cibles exigeant
des comptes de mandataire distincts ne contournent pas l'entrée d'origine : `prefer_direct`
utilise le relais d'origine, et `direct` refuse. Les back-ends UDP avec en-têtes PROXY activés
utilisent le relais ; les en-têtes PROXY TCP sont écrits au back-end par le fournisseur, en
utilisant l'adresse source authentifiée par le serveur.

## Limites de déploiement

Lorsqu'un mappage dépendant de la destination est présent, lorsque l'UDP est désactivé, et
dans des cas similaires, le relais est utilisé. Les véritables types de NAT publics et les
réseaux d'opérateur doivent encore être validés dans l'environnement de déploiement où ils
fonctionnent ; ne considérez pas une simulation locale comme un résultat de réseau public.

## Étapes suivantes

- [Partages privés et visiteurs](private-shares.md)
- [Le module felucca-server](server.md)
- [Contrôle d'accès, quotas et codage de flux](security.md)
- Exemples : [p2p-server.toml](../../../examples/p2p-server.toml) ·
  [p2p-provider.toml](../../../examples/p2p-provider.toml) ·
  [p2p-visitor.toml](../../../examples/p2p-visitor.toml)
