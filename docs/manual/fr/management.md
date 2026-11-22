# Interface d'administration et configuration centralisée

`felucca-server` fournit la gestion des appareils, services, accès privés et configurations. Les changements courants sont synchronisés en ligne et les services inchangés continuent de fonctionner. Les changements d’infrastructure, notamment les écouteurs globaux, rechargent le transfert ; l’API de gestion reste disponible.

## Se connecter sans éditer la configuration

1. Dans **Appareils → Connecter un appareil**, saisissez le nom, confirmez le serveur et choisissez le système et le processeur. Décompressez le client téléchargé puis ouvrez `Connect.command` (macOS) ou `Connect.cmd` (Windows). Sous Linux, lancez `sh connect.sh` dans le dossier extrait. Le programme et la connexion sont inclus, sans fichier à modifier.
2. Dans **Services → Ajouter un service**, choisissez l’appareil, le type et l’adresse de l’application sur cet appareil. L’accès public utilise un port ou domaine du serveur ; l’accès privé est destiné aux appareils autorisés. Les identifiants et secrets sont générés automatiquement.
3. Chaque appareil destinataire reçoit aussi son propre client et utilise l’adresse locale affichée. Pour démarrer à l’ouverture de session, fermez la fenêtre de connexion puis lancez `Install.command`, `Install.cmd` ou `sh install.sh`. Le client est enregistré dans votre dossier personnel.

La navigation principale comprend Vue d’ensemble, Appareils, Services, Accès privé, Surveillance et Paramètres. Comptes, quotas, diagnostics, écouteurs et intégrations se trouvent dans Paramètres. Les opérations courantes et avancées utilisent des contrôles, sans éditeur JSON.

Activez les connexions directes dans Paramètres pour les privilégier, avec repli sur le relais serveur. Sinon, les fichiers générés utilisent le relais. Il s’agit d’un accès par service, pas d’un VPN système. Le partage de fichiers exige l’autorisation d’un dossier existant sur l’appareil. Gardez les fichiers de connexion privés : ils contiennent des secrets. Pour un destinataire, conservez le fichier initial ou créez une nouvelle connexion privée. Les modifications des appareils, services et accès ne redémarrent pas tout le transfert. Les services modifiés ou désactivés ferment leurs connexions. Les services inchangés et les changements de nom conservent leurs connexions. Un ancien agent ne reconnecte que l’appareil concerné. Les écouteurs bridge, HTTP et P2P nécessitent encore un rechargement.

Seules les versions du client présentes sur le serveur sont proposées. Les versions macOS non notariées par Apple sont signalées et peuvent être bloquées après un téléchargement normal. Demandez une version de distribution à l’administrateur sans désactiver la protection du système.

## Activation

```sh
target/release/felucca-server --hash-password      # read one password line from stdin; >=12 bytes
target/release/felucca-server --generate-token     # Bearer token and its stored hash
```

```toml
[admin]
bind = "127.0.0.1:8088"
state_path = "state/runtime-config.json"
profiling = true
# required for non-loopback binds: the origin the browser uses; use https://
# unless you accept plaintext management traffic (then http://host:port)
# origin = "https://admin.example.com"

[admin.account.root]
role = "admin"
password_hash = "pbkdf2-sha256$600000$..."
```

Les fichiers de configuration peuvent être en TOML ou en JSON. L'exemple utilise l'adresse
d'administration `http://127.0.0.1:8088` et le compte `root`. Le mot de passe de l'exemple ne
sert qu'à une démonstration locale ; remplacez-le par votre propre hachage dans de vrais
déploiements.

## Comptes et rôles

| `role` | Permissions |
| --- | --- |
| `admin` | Gère toutes les ressources et tous les comptes ; aucun champ `user` n'est défini |
| `operator` | Gère les nœuds et les tunnels centralisés de l'utilisateur `user` attribué |
| `viewer` | Accès en lecture seule aux ressources de l'utilisateur `user` attribué |

Les comptes opérateur et lecteur doivent référencer un utilisateur existant. Les services
locaux au serveur peuvent atteindre des répertoires locaux : seul un administrateur peut donc
les modifier. Les quotas utilisateur, les comptes d'administration et la configuration
complète sont également réservés aux administrateurs. Conservez au moins un compte
administrateur actif. `disabled = true` désactive un compte.

Les mots de passe utilisent PBKDF2-SHA256 avec un sel aléatoire et un nombre fixe de 600000
itérations. Les sessions sont conservées en mémoire. Les cookies sont HttpOnly et
SameSite=Strict, et portent aussi l'attribut Secure lorsque vous configurez HTTPS. Les
écritures depuis un navigateur exigent une Origin de même origine et un jeton CSRF. Changer un
mot de passe, désactiver un compte ou supprimer un compte invalide les anciennes sessions, et
un redémarrage du processus invalide aussi les sessions par cookie.

Pour l'automatisation, utilisez un jeton Bearer : copiez le `token_hash` affiché par
`--generate-token` dans le `token_hash` du compte, et envoyez le jeton brut à chaque appel. Les
requêtes Bearer n'ont pas besoin du jeton CSRF du cookie, mais les mêmes vérifications de
permissions du compte s'appliquent. **N'écrivez jamais le jeton brut dans la configuration du
serveur.**

## Transactions d'écriture

Chaque écriture exige deux en-têtes :

```text
If-Match: "current configuration revision"
Idempotency-Key: unique identifier for this operation
```

- Une révision périmée renvoie 409.
- Le même ID avec la même requête renvoie le résultat précédent et ne redémarre **pas** le plan
  de données une seconde fois ; le même ID avec une requête différente renvoie 400.
- Les 256 opérations réussies les plus récentes sont stockées avec la configuration : les
  nouvelles tentatives d'opérations telles que DELETE sont donc encore reconnues après un
  redémarrage.
- `durable = false` signifie que la synchronisation du répertoire n'a pas encore été confirmée.
- Les modifications des appareils, services et accès ne redémarrent pas tout le transfert. Les services modifiés ou désactivés ferment leurs connexions. Les services inchangés et les changements de nom conservent leurs connexions. Un ancien agent ne reconnecte que l’appareil concerné. Les écouteurs bridge, HTTP et P2P nécessitent encore un rechargement.

Modifier l'adresse de liaison d'administration, les chemins des fichiers de certificats,
l'origine publique ou le chemin d'état exige un redémarrage du processus. Vous ne pouvez pas
les modifier pendant que le serveur fonctionne.

## Pages de l'interface

| Page | Contenu |
| --- | --- |
| Vue d'ensemble | État des appareils, services et connexions visibles par ce compte. |
| Appareils | Choisissez un ordinateur ou un serveur connecté pour gérer ses services. |
| Services | Choisissez l’appareil et l’application. Felucca les relie à l’adresse choisie. |
| Accès privé | Choisissez un service privé et créez le fichier d’un autre appareil. Aucun port public du service nécessaire. |
| Supervision | Consultez les métriques de l’hôte, les connexions et le trafic cumulé. |
| Paramètres | Connexions du serveur, accès et intégrations. Les tâches quotidiennes restent dans la navigation principale. |

## API

L'API parle JSON, accepte des corps de requête jusqu'à 2 Mio et rejette les requêtes de
navigateur inter-origines. Les lectures renvoient normalement `{revision, config}`, et les
lectures de configuration caviardent les secrets par défaut. Lors d'une modification, un espace
réservé `<redacted>` existant conserve la valeur actuelle de ce champ ; les identifiants des
nouvelles ressources ne peuvent pas utiliser cet espace réservé.

| Chemin | Méthode et rôle |
| --- | --- |
| `/api/login` | POST `{account, password}` ; Origin est obligatoire |
| `/api/session` | GET l'identité actuelle et le jeton CSRF |
| `/api/logout` | POST pour révoquer la session par cookie |
| `/api/overview` | GET les nœuds en ligne, les connexions, le trafic et les tunnels enregistrés |
| `/api/users`, `/api/users/<id>` | GET la liste ou une entrée unique ; PUT et DELETE sur une entrée unique |
| `/api/clients`, `/api/clients/<id>` | GET la liste ou une entrée unique ; PUT et DELETE sur une entrée unique |
| `/api/managed/<client>` | GET, PUT la configuration centralisée complète ; DELETE restaure la configuration locale |
| `/api/managed/<client>/tunnels/<id>` | GET, PUT, DELETE un tunnel unique |
| `/api/locals`, `/api/locals/<id>` | GET la liste ou une entrée unique ; PUT et DELETE sur une entrée unique |
| `/api/locals/<group>/tunnels/<id>` | PUT, DELETE un tunnel local au serveur |
| `/api/shares`, `/api/shares/<name>` | GET, PUT, DELETE un partage privé |
| `/api/accounts`, `/api/accounts/<id>` | GET la liste ; PUT et DELETE sur une entrée unique |
| `/api/config` | GET la configuration complète caviardée ; PUT pour importer une configuration complète |
| `/api/config?secrets=1` | Exportation explicite réservée aux administrateurs, identifiants inclus |
| `/api/validate` | POST pour valider une configuration complète sans démarrer les écouteurs |
| `/api/batch` | POST un lot atomique de modifications, au plus 128 opérations |
| `/api/events` | GET un flux SSE des changements de révision de configuration, avec battement de cœur de 10 secondes et revérification de session |
| `/metrics`, `/api/metrics` | GET les métriques Prometheus, filtrées selon les permissions du compte |
| `/api/profile` | POST pour télécharger un graphique en flammes CPU à durée limitée |

Un lot utilise une seule révision et un seul identifiant d'opération. Toutes les opérations
sont validées avant que quoi que ce soit ne soit appliqué, et si une opération sort du périmètre
ou si la configuration est invalide, aucune des opérations précédentes n'est validée :

```json
{
  "operations": [
    {
      "method": "PUT",
      "path": "/api/managed/office/tunnels/1",
      "value": {
        "id": 1,
        "enabled": false,
        "mode": "tcp",
        "port": 18080,
        "backend": [{"addr": "127.0.0.1:3000"}]
      }
    }
  ]
}
```

Lorsque vous faites un `PUT` sur un compte, vous pouvez envoyer un `password`, et le serveur
génère le hachage. Si vous l'omettez, le `password_hash` existant est conservé.

## Configuration centralisée

Le serveur stocke sous `[managed.<client-id>]` la configuration que les agents doivent
exécuter :

```toml
[[managed.office.tunnel]]
id = 1
mode = "tcp"
port = 18080
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
```

Un agent doit activer explicitement `[remote] enabled = true`, et la connexion principale doit
utiliser TLS/WSS/QUIC authentifié. Le serveur vérifie aussi que le pont réel est chiffré : des
connexions en clair ne peuvent donc pas demander de configuration. Les envois utilisent du JSON
canonique accompagné d'une empreinte, et les identifiants de la configuration n'apparaissent pas
dans les journaux de débogage des messages de contrôle.

Le plan d'administration ne peut modifier que le contenu des tunnels. Il ne peut pas envoyer de
nouvelles identités d'agent, de mandataires de sortie ni de frontières d'autorisation locales au
serveur. Les services de fichiers doivent voir leurs capacités de répertoire nommées configurées
à l'avance ; voir [Services de fichiers et WebDAV](file-services.md).

Exception : lorsqu'un agent définit `remote.console_scope = true`, le `target_policy` de `[managed.<client-id>]` remplace sa portée de destination.

| Opération | Effet |
| --- | --- |
| `tunnel.enabled = false` | Conserve la configuration et l'ID stable, mais n'enregistre pas l'écouteur et ne démarre pas la sonde de back-end |
| Conserver une entrée `managed` vide | Arrête tous les tunnels centralisés de ce nœud |
| Supprimer l'entrée `managed` entière | Restaure la configuration de tunnels locale de l'agent au démarrage |

Les pages d'administration distinguent « publié », « en attente d'application par le nœud » et
« désactivé ».

## Liste de vérification

1. `felucca-server --config server.json --check` aboutit.
2. Ouvrez l'adresse d'administration dans un navigateur, connectez-vous, et vérifiez qu'il n'y a
   aucune erreur JS.
3. Créez un tunnel TCP et confirmez que le back-end renvoie réellement du contenu.
4. `GET /api/config` n'affiche aucun secret en clair.
5. Après un redémarrage du serveur, la configuration et les tunnels sont toujours là, et une
   nouvelle tentative avec la même `Idempotency-Key` n'applique pas la modification deux fois.

## Étapes suivantes

- [Intégration aux plateformes externes](platform.md) — laisser un autre système gérer ce nœud
- La section `[remote]` du [module felucca-agent](agent.md)
- [Services, mises à jour, journalisation et supervision](operations.md)
- Exemples : [admin-server.toml](../../../examples/admin-server.toml) ·
  [managed-agent.toml](../../../examples/managed-agent.toml)

La console fournit des formulaires structurés pour les poids et TLS des backends, les sondes, le routage HTTP, les comptes de service, les ACL et quotas, les services de fichiers, les groupes de tunnels locaux et les mappages de ports.  La page des comptes permet de créer, remplacer et révoquer les jetons de gestion ; un nouveau jeton est téléchargé après son enregistrement.

La validation indique les paramètres nécessitant un redémarrage. L’export fournit une configuration initiale, un instantané d’exécution avec les identifiants et des instructions de remplacement hors ligne. Arrêtez le processus et sauvegardez les fichiers existants. L’instantané prime sur le fichier initial, y compris le chemin sélectionné avec `--runtime-config`. Conservez les journaux de trafic et de plateforme ; ne remplacez pas l’instantané d’un processus actif.

Le diagnostic présente le trafic par tunnel, les résultats existants des sondes, les sessions P2P actives et les événements de relais récents. Les administrateurs peuvent examiner les certificats actifs, relancer les requêtes ACME échouées, importer les certificats et clés PEM dans un brouillon et lire une fin de journal limitée et expurgée des identifiants. Les anciens agents sans flux de diagnostic optionnel indiquent une indisponibilité, sans interrompre le transfert. La lecture ne lance aucune nouvelle sonde. La génération de configurations agent et visiteur préserve les autorisations locales des répertoires et destinations : vérifiez et exécutez le fichier sur l’agent, en y utilisant `--check` avant le démarrage. Les commandes de service et de mise à jour sont fournies pour vérification et non exécutées par la console.
