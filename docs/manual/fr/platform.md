# Intégration aux plateformes externes

`[node]` permet à un plan de contrôle externe de gérer ce nœud : il peut appeler l'API
d'administration avec un jeton distinct, ou bien le nœud peut composer vers l'extérieur et
accepter des requêtes par un WebSocket inverse. Les changements de configuration peuvent aussi
être poussés sous forme de rappels signés, et du trafic externe peut être importé. Il s'agit du
protocole de contrôle propre à Felucca ; il n'est pas compatible au niveau du fil avec nps.

## Activation et portées

```toml
[node]
id = "edge-a"
state_path = "state/node-events.json"
changes_window = 1024
usage_interval = "10s"

[node.platform.panel]
enabled = true
token = "replace-with-a-random-platform-token"
scope = "account"
user = "office"
mode = "direct"
```

| Champ | Valeurs et signification |
| --- | --- |
| `scope` | `full` (gestion complète ; vous ne pouvez pas définir aussi `user`) ou `account` (vous devez définir un `user` existant, et vous ne pouvez gérer que les clients et tunnels de cet utilisateur, et lire l'état des rappels de ce locataire) |
| `mode` | `direct` (le jeton peut accéder à l'API HTTP d'administration publique), `reverse` (RPC WS inverse uniquement ; l'utilisation directe de ce jeton contre l'API HTTP d'administration renvoie 401), ou `dual` (les deux sens) |
| `enabled = false` | Les deux points d'entrée cessent de fonctionner et les connexions inverses existantes sont fermées |

Les jetons doivent compter au moins 16 octets, et chaque plateforme doit en utiliser un
différent. Par HTTP, envoyez `Authorization: Bearer <token>`. Les identités de plateforme sont
distinctes des comptes d'administration locaux. Changer un jeton, sa portée ou la configuration
de connexion remplace la connexion inverse ; les requêtes d'une ancienne connexion sont toujours
vérifiées par rapport à la configuration actuelle.

Les adresses publiques doivent utiliser HTTPS/WSS ; le HTTP/WS en clair n'est autorisé que pour
des adresses de boucle locale littérales. WSS vérifie par défaut les autorités de certification
publiques, et vous pouvez à la place utiliser
`trust = { mode = "pinned", fingerprint = "..." }`. La confiance non sécurisée n'est pas
autorisée. Les rappels HTTP vérifient les autorités de certification publiques et ne suivent
pas les redirections.

## API de gestion directe

Les écritures utilisent du JSON, `If-Match: "<configuration revision>"` et une
`Idempotency-Key` unique. L'authentification par cookie de navigateur exige en plus une Origin
et un jeton CSRF.

| Point d'accès | Comportement |
| --- | --- |
| `GET /api/node/status`, `/api/node/registration` | ID de nœud, ID de démarrage, époque persistée, version, fenêtre d'événements et état visible des connexions de plateforme |
| `GET /api/node/usage` | État en ligne des clients, nombres de connexions et octets bidirectionnels cumulés dans la portée actuelle |
| `GET /api/node/changes?after=<epoch>:<sequence>&limit=100` | La fenêtre de changements persistée ; `limit` va de 1 à 512 |
| `GET /api/node/callbacks?target=...&offset=0&limit=100` | Files de rappels visibles, statistiques de livraison et entrées paginées |
| `POST /api/node/callbacks/replay` | `{"ids":["delivery-uuid"]}` ; efface l'état d'échec et livre à nouveau |
| `POST /api/node/callbacks/delete` | Ne supprime que les entrées visibles spécifiées |
| `POST /api/node/kick` | `{"ids":["client-name"]}` ; désactive et déconnecte les clients de façon persistante, en bloquant les reconnexions immédiates |
| `POST /api/node/sync` | L'administrateur réactive la configuration locale persistée et efface les anciens états d'exécution, caches et connexions |
| `POST /api/node/traffic` | Importe des deltas de trafic externe, en accumulant de façon atomique les compteurs du client et de l'utilisateur propriétaire |
| `GET/PUT/DELETE /api/platforms/<name>` | Gestion administrative des plateformes |
| `GET/PUT/DELETE /api/webhooks/<name>` | Création, modification et suppression administratives des webhooks persistés |

Les changements de configuration par lot, l'exportation complète et la restauration réutilisent
`/api/batch` et `GET/PUT /api/config`. Sync et kick réutilisent la même transaction de
configuration et le même historique d'idempotence, et incrémentent la révision de
configuration. Les actions de rappel et l'import de trafic ne modifient pas la révision de
configuration.

### Import de trafic

Vous devez configurer `state.path` :

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

Une requête porte au plus 128 éléments. Les nombres d'octets doivent être des entiers non
signés, et plusieurs éléments pour un même client sont d'abord fusionnés. Rien n'est écrit avant
que toutes les vérifications de permissions ne soient passées, et si une entrée sort du périmètre
ou est invalide, tout le lot est rejeté. Importez du trafic qui n'est **pas déjà mesuré par le
plan de données de ce nœud** ; réimporter du trafic déjà mesuré par le nœud le compte deux fois.
Le premier import fait passer l'instantané à la version 2, que les versions antérieures ne
peuvent pas lire.

## Protocole WSS inverse v1

Dès que vous activez `[node]`, `GET /api/ws` accepte une mise à niveau WebSocket HTTP/1.1. Les
clients natifs utilisent un jeton Bearer. Un navigateur peut utiliser son cookie de connexion de
même origine, ou transmettre le jeton dans le sous-protocole
`["felucca-node-v1", "felucca-token.<unpadded base64url token>"]`. Le serveur ne sélectionne que
`felucca-node-v1` et n'accepte pas l'authentification par paramètres d'URL. Il conserve au plus
16 WebSocket d'administration directes en même temps.

Le nœud compose `reverse_url`, et la poignée de main porte le jeton Bearer de la plateforme et
le sous-protocole `felucca-node-v1`. La plateforme doit sélectionner ce sous-protocole dans sa
réponse. Tous les messages applicatifs sont du texte JSON.

Le nœud envoie d'abord `hello` (node, boot, epoch, cursor, max_message, max_inflight,
heartbeat_ms), puis la plateforme envoie des requêtes :

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

Les réponses ressemblent à
`{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}`. Le RPC réutilise
l'authentification, le filtrage par locataire, le CAS et le comportement d'idempotence des
gestionnaires d'administration normaux. Vous pouvez transmettre If-Match, Idempotency-Key,
Accept et X-Felucca-CSRF, mais vous ne pouvez pas outrepasser Host ni Authorization. La
connexion, la déconnexion, le SSE, l'échantillonnage CPU et la mise à niveau WebSocket elle-même
ne passent pas par le RPC.

| Limite | Valeur |
| --- | --- |
| Message maximal | 2 Mio |
| RPC concurrents par connexion | 2 |
| RPC concurrents à l'échelle du nœud | 16 ; les requêtes excédentaires renvoient 429 |
| ID de requête en vol répété | La connexion est fermée |
| Délai d'expiration de requête | 504 ; réessayez les écritures avec la même clé d'idempotence |

Chaque côté peut envoyer `{"type":"ping","nonce":1}`. Le nœud n'écrase pas un ping resté sans
réponse, et il ferme puis se reconnecte si aucun pong correspondant n'arrive au bout de plus de
deux périodes de battement, avec un repli de 1 à 30 secondes. Les changements de configuration
envoient un événement, et une resynchronisation est envoyée lorsqu'un abonnement prend du
retard. Une fois `usage_interval` défini, le nœud envoie périodiquement des messages
d'utilisation filtrés par locataire.

## Rappels signés

```toml
[node.webhook.events]
user = "office"

[node.webhook.events.delivery]
enabled = true
url = "https://panel.example/felucca/events"
signing_key = "replace-with-a-random-callback-signing-key"
timeout = "5s"
retry_max = 2
retry_backoff = "2s"
queue_max = 100
```

En-têtes de requête :

| En-tête | Contenu |
| --- | --- |
| `X-Felucca-Event-Id` | L'UUID de livraison ; il reste identique d'une nouvelle tentative automatique ou d'une relecture manuelle à l'autre |
| `X-Felucca-Timestamp` | L'heure Unix en millisecondes de cette requête |
| `X-Felucca-Signature` | `v1=` suivi du HMAC-SHA256 en hexadécimal minuscules |

L'entrée de signature est le préfixe UTF-8 concaténé directement aux **octets bruts du corps de
requête** :

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

Le récepteur devrait vérifier la signature, contrôler la fenêtre temporelle et dédupliquer par
ID d'événement. Seul un code 2xx compte comme un succès. `retry_max` est le nombre de tentatives
supplémentaires au-delà de la première livraison, et `retry_backoff` est un intervalle fixe. La
livraison est **au moins une fois** : si le nœud s'arrête avant qu'un accusé de réception réussi
ne soit écrit sur disque, un redémarrage peut livrer à nouveau le même ID. Les rappels ne portent
que des métadonnées de changement, pas de secrets de configuration ; après avoir reçu un
événement, récupérez les ressources dont vous avez besoin avec l'API actuellement autorisée.

`queue_max` limite les entrées en attente et en échec persistées par cible. Lorsque la limite est
dépassée, les entrées les plus anciennes sont abandonnées et comptées comme abandonnées ; `0` ne
conserve aucune entrée en échec définitif. Le maximum à l'échelle du nœud est de 4096 entrées.
Désactiver un abonnement conserve sa file existante. Supprimer l'abonnement, ou changer l'URL de
réception, la clé de signature, les filtres ou les modèles, efface l'ancienne file.

La fenêtre d'événements enregistre `node.started`, `config.changed`, `config.resync`, ainsi que
les métadonnées de création, de modification et de suppression des clients, utilisateurs,
tunnels centralisés et domaines. Les requêtes sont filtrées par locataire. Un curseur expiré,
trop avancé ou dont l'époque ne correspond pas renvoie 410/reset_required ; dans ce cas,
récupérez d'abord la configuration actuelle, puis reprenez l'abonnement à partir de la position
renvoyée. Cette fenêtre bornée ne conserve pas les diffs complets de configuration et n'est pas
un journal d'audit de long terme.

## Abonnements temps réel et modèles

Les connexions WebSocket directes et inverses peuvent utiliser ces chemins réservés au
**WS RPC** : `GET /api/realtime/subscriptions`, et
`GET/PUT/DELETE /api/realtime/subscriptions/<name>`. Les abonnements ne modifient pas la
configuration persistée et n'exigent pas If-Match. Chaque connexion autorise au plus 32
abonnements d'au plus 32 Kio chacun, et tous sont libérés lorsque la connexion tombe.

```json
{
  "enabled": true,
  "selector": {"event_names":["client.updated"],"clients":["office"]},
  "content": {
    "body_template": {"message":"node {{event.client}} updated","event":"{{event}}"},
    "header_templates": {"x-event-name":"{{event.kind}}"}
  }
}
```

Les modèles sont des valeurs JSON et n'exécutent aucun script. Ils prennent en charge
`{{node}}`, `{{id}}`, `{{epoch}}`, `{{event}}` et `{{event.kind}}`, ainsi que ressource, action,
utilisateur, client, tunnel, hôte, cible, séquence, révision, timestamp_ms et trafic. Lorsqu'une
chaîne entière est un unique espace réservé, le type JSON d'origine est préservé. Les modèles ne
peuvent pas outrepasser l'authentification, les cookies, les en-têtes HTTP de longueur/connexion
ni les en-têtes de signature `X-Felucca-*`.

## Notifications de trafic

`traffic_report_interval` (100ms–1h) et `traffic_report_step_bytes` activent les notifications
pour le trafic importé. Une notification `client.traffic.reported` est envoyée lors du premier
import pour un client disposant d'un quota de trafic, puis lorsque le seuil de temps ou d'octets
est atteint. La notification porte `ephemeral: true` : elle n'entre donc pas dans l'historique
des changements, la file d'échecs ni les nouvelles tentatives. Les récepteurs lents peuvent la
manquer ; lisez `/api/node/usage` lorsque vous avez besoin des données complètes.

## Liste de vérification

1. `GET /api/node/status` renvoie l'ID de nœud, l'époque et l'état des connexions de plateforme.
2. Une connexion WSS inverse effectue un aller-retour `hello` → `request` → `response`.
3. Le récepteur de rappels vérifie les signatures avec succès, et rejouer le même ID d'événement
   ne produit aucun effet métier en double.
4. Après un `kick`, le client est déconnecté et ne peut pas se reconnecter immédiatement.
5. Après un import de trafic, les octets cumulés de `/api/node/usage` changent immédiatement.

## Étapes suivantes

- [Interface d'administration et configuration centralisée](management.md)
- [Le module felucca-server](server.md)
- Exemples : [node-server.toml](../../../examples/node-server.toml)
