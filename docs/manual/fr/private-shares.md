# Partages privés et visiteurs

Un tunnel privé s'enregistre auprès du serveur mais n'occupe **aucun port public**. Un autre
agent, le visiteur, ouvre un écouteur sur sa propre machine et l'atteint par le pont
authentifié et chiffré, à l'aide d'un secret de partage distinct. Utilisez ce mécanisme pour
exposer un service interne à des nœuds précis plutôt qu'à l'internet public.

## Les trois configurations

L'agent fournisseur, l'agent visiteur et le serveur changent chacun en un seul endroit.

### Fournisseur : rendre le tunnel privé

```toml
[[tunnel]]
id = 1
mode = "tcp"
public = false                 # Omit port and bind
backend = [{ addr = "127.0.0.1:3000" }]

[[tunnel]]
id = 3
mode = "socks5"
public = false
[tunnel.target_policy]
allow = ["127.0.0.0/8", "::1", "10.0.0.0/8"]
```

Les entrées TCP, UDP, fichier et mandataire sont prises en charge. **Le routage HTTP par
domaine repose sur l'écouteur HTTP et ne peut pas être rendu privé** ; un tunnel TCP privé
peut toutefois relayer n'importe quel back-end HTTP ou HTTPS. Définir explicitement
`port = 0` équivaut à un tunnel privé.

### Serveur : déclarer le partage

```toml
[share.office-web]
client = "office"              # Node name; for a server-local group use client = "local.<group-name>"
tunnel = 1                     # Stable tunnel ID
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]     # An empty list allows every authenticated node that holds the share secret
direct = true                  # Allow P2P direct connection attempts
```

Un `secret` mesure de 16 à 4096 octets, et chaque partage devrait en utiliser un différent.
`acl_mode`/`acl_rules` vérifient l'adresse source des connexions de pont des visiteurs. Un
secret de partage seul ne peut **pas** servir à se connecter en client anonyme : un visiteur a
toujours besoin d'une identité de nœud normale et d'une vkey. Le serveur peut désactiver un
nœud de façon permanente avec `client.NAME.enabled = false`.

### Visiteur : écouter localement

```toml
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"                   # tcp | udp | socks5
max_connections = 64
udp_idle = "60s"
```

| Champ | Description |
| --- | --- |
| `name` | Unique au sein de cet agent |
| `share` | Le nom du partage sur le serveur |
| `mode` | `tcp`, `udp` ou `socks5` |
| `bind` | Exige un port fixe ; la liaison a lieu au démarrage, et un port déjà pris fait échouer l'opération immédiatement |
| `max_connections` | 1–4096, 64 par défaut ; connexions TCP ou sessions source UDP |
| `udp_idle` | de 1 seconde à 1 heure, 60 secondes par défaut ; une activité dans l'un ou l'autre sens prolonge la session |
| `acl_mode` / `acl_rules` | Vérifient l'adresse source des connexions vers l'écouteur du visiteur |
| `target` | Partages dynamiques uniquement, voir ci-dessous |

Tous les visiteurs partagent aussi la limite de connexions de l'agent et un budget de mise en
file UDP de 8 Mio. Un visiteur n'accepte que TLS, WSS ou QUIC avec un certificat vérifié.

Le serveur applique l'ACL de partage, l'ACL du visiteur et l'ACL de routage du fournisseur sur
l'**adresse source réelle du pont du visiteur**. Une adresse brute qui atteint le port du
visiteur sur le réseau local n'est pas transmise au serveur comme source de confiance.

## Ce que vous obtenez

Avec les configurations ci-dessus, atteindre `127.0.0.1:18080` sur la machine du visiteur
arrive sur `127.0.0.1:3000` de l'agent office, et l'UDP `127.0.0.1:15353` correspond à
`127.0.0.1:5353` sur office.

Les connexions et le trafic brut sont comptés à la fois pour le visiteur et pour le
fournisseur, ainsi que pour leurs ressources utilisateur et tunnel ; lorsqu'un même
utilisateur détient les deux extrémités, les consommations des deux sont additionnées. Les
en-têtes de longueur UDP et les messages d'authentification de partage ne sont pas comptés
comme trafic métier.

## Cibles dynamiques

Lorsqu'un partage serveur définit `dynamic = true`, le visiteur peut désigner une cible :

```toml
[[visitor]]
name = "office-proxy"
share = "office-proxy"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:11080"
mode = "socks5"
target = "internal.example:443"
```

- Le fournisseur doit être un tunnel SOCKS5, mandataire direct HTTP ou mandataire mixte ; les
  cibles UDP exigent SOCKS5 ou un mandataire mixte.
- L'IP réellement résolue et la politique de cible du fournisseur sont toujours vérifiées.
- Un partage dynamique exige que l'entrée de mandataire n'ait **aucun compte de mandataire
  distinct** ; si des comptes sont configurés, la demande est refusée, afin qu'un secret de
  partage ne puisse pas contourner l'authentification propre du mandataire.
- Sans `target`, le visiteur se connecte à l'entrée de service complète ; les comptes de
  fichiers et de mandataire s'authentifient normalement.
- Un visiteur TCP peut faire transiter une session SOCKS CONNECT ou de mandataire HTTP ;
  l'adresse UDP de serveur renvoyée par SOCKS UDP ASSOCIATE n'est pas automatiquement projetée
  sur une adresse locale du visiteur.

Lorsqu'il vous faut un accès SOCKS5 complet, réglez le visiteur sur `mode = "socks5"` et
référencez un partage dynamique, sans `target` : SOCKS CONNECT et UDP ASSOCIATE sont résolus
côté visiteur, et chaque destination passe par l'authentification de partage et la politique
de cible du fournisseur. `[visitor.auth.users]` configure les comptes SOCKS locaux, et lorsque
des comptes existent, le visiteur ne se replie pas sur l'authentification anonyme.
`max_connections` compte ensemble les connexions de contrôle SOCKS et les associations de cible
UDP : mettez-le à au moins 2 si vous utilisez l'UDP.

## Administration et exécution

- La page « Accès partagé » de l'interface d'administration crée, modifie et supprime des
  partages ; les secrets sont masqués par défaut, et laisser le champ vide pendant une
  modification conserve la valeur actuelle.
- Les opérateurs ne peuvent gérer que les partages côté fournisseur des utilisateurs qu'ils
  détiennent ; les comptes en lecture seule ne peuvent apporter aucune modification.
- L'API est `/api/shares` et `/api/shares/NAME` ; les écritures conservent la version de
  configuration et la clé d'idempotence de la requête.
- Après une chute du pont, l'écouteur local est conservé, les sessions existantes se
  terminent, et les nouvelles requêtes reprennent après reconnexion.
- Les modifications d'un partage prennent effet par un rechargement normal du démon, et les
  connexions existantes sont fermées.
- Ce chemin passe par le relais du serveur ; pour une liaison directe, voir
  [Connexion directe P2P et repli sur le relais](p2p.md).

## Liste de vérification

1. Le `--check` du serveur et les `--check` des deux agents aboutissent.
2. Le `127.0.0.1:18080` du visiteur renvoie le contenu du `127.0.0.1:3000` du fournisseur.
3. Une connexion de visiteur avec un `secret` erroné est refusée.
4. Les nœuds absents de `allow_clients` sont refusés.
5. Le serveur n'a gagné aucun nouvel écouteur public (comparez avec `ss -ltnp`).

## Étapes suivantes

- [Connexion directe P2P et repli sur le relais](p2p.md)
- [Le module felucca-agent](agent.md)
- [Interface d'administration et configuration centralisée](management.md)
- Exemples : [visitor-server.toml](../../../examples/visitor-server.toml) ·
  [private-agent.toml](../../../examples/private-agent.toml) ·
  [visitor-agent.toml](../../../examples/visitor-agent.toml)
