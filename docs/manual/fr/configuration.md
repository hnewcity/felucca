# Sources de configuration et CLI

Cette page couvre l'origine de la configuration, la façon dont elle est transmise, les
variables d'environnement qui s'appliquent, et la manière de valider sans démarrer. La
signification des champs se trouve dans les guides des modules.

## Fichiers de configuration

Les deux programmes acceptent TOML et JSON. Les chemins relatifs sont résolus par rapport au
répertoire de travail du processus : `--workdir` et le répertoire de démarrage
systemd/launchd comptent donc tous les deux.

```sh
felucca-server --config /srv/felucca/server.toml
felucca-agent  --config /srv/felucca/agent.toml
```

Sans `--config`, les programmes lisent `felucca-server.toml` / `felucca-agent.toml` dans le
répertoire de travail. `-c` est la forme courte de `--config` ; l'agent accepte aussi l'alias
`--launch`.

Toute la configuration utilise `deny_unknown_fields` : un champ inconnu est une erreur et
n'est jamais ignoré silencieusement.

## Sources acceptées

`--config` accepte quatre formes :

| Forme | Exemple | Détails |
| --- | --- | --- |
| Fichier local | `conf/server.toml` | TOML ou JSON |
| URL HTTP(S) | `https://cfg.example/edge.json` | Attend au plus 15 secondes et exige un code 200 ; ne suit pas les redirections et n'accepte pas les réponses compressées ; l'URL n'accepte ni userinfo ni fragment, et les paramètres de requête peuvent porter une adresse de téléchargement signée |
| JSON inline | `'{"bridge":{"bind":"0.0.0.0:14028"}}'` | Transmet directement un objet JSON |
| base64 | `base64:eyJicmlkZ2Ui...` | base64 ou base64url standard, sans remplissage |

Une configuration donnée peut peser au plus 4 Mio. `--config-token-env NAME` lit un jeton
Bearer dans une variable d'environnement pour les adresses de téléchargement privées ; les
identifiants ne sont jamais écrits dans le journal. L'URL, le jeton et le corps inline ne
sont jamais affichés comme nom de source de configuration.

Dans une configuration distante, les chemins des certificats, des données Geo et des racines
de fichiers pointent toujours vers des chemins de la **machine de l'agent**.

## URI de lancement de l'agent

`felucca-agent` accepte aussi une URI courte. Seule, elle ne fait que connecter et les tunnels locaux exigent toujours du TOML/JSON ; avec `remote=1`, le serveur fournit les tunnels et la portée d'accès, une seule commande suffit :

```sh
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&transport=tls'
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&fingerprint=SHA256_HEX&remote=1'
```

| Champ | Obligatoire | Détails |
| --- | --- | --- |
| Port | Oui | Doit être non nul |
| `vkey` | Oui | La clé partagée ; la valeur doit être codée en pourcentage |
| `key_id` | Non | Utilise l'identité par défaut en cas d'omission |
| `transport` | Non | `tcp`/`tls`/`ws`/`wss`/`quic`/`kcp` |
| `fingerprint` | Non | Active la validation du certificat par empreinte SHA-256 figée |
| `ws_path` | Non | Le chemin WebSocket |
| `remote` | Non | `1` confie services et portée d'accès au serveur : active `remote.enabled` et `remote.console_scope`, et n'autorise que les adresses locales tant que le serveur n'a pas envoyé de portée |

Les champs inconnus et les champs répétés sont des erreurs.

## Instances multiples de l'agent

`--config` peut être répété, jusqu'à 32 instances dans un même processus :

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://cfg.example/edge.json' --config-token-env CONFIG_TOKEN
```

Chaque instance conserve ses propres connexions, son repli exponentiel, ses routes, ses
sondes et ses limites de connexions. Toute la configuration est validée et initialisée
d'abord, et ce n'est qu'ensuite que la connexion commence ; une combinaison `server` +
`key_id` répétée est une erreur. Lorsqu'un serveur est injoignable, les autres instances
continuent de fonctionner, et un signal de sortie ferme toutes les instances.

## Expansion des variables d'environnement

Désactivée par défaut. Définissez `--expand-env` (ou `FELUCCA_EXPAND_ENV=true`) et les
valeurs de chaîne résolues sont expansées :

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}` exige que la variable existe. `${VAR:-default}` utilise la valeur par défaut
lorsque la variable est absente ou vide. `$$` représente un `$` littéral. L'expansion n'a
lieu qu'une seule fois, ne touche pas aux noms de champs et ne convertit pas les chaînes en
nombres. Un résultat de plus de 4 Mio est rejeté.

## Instantané de configuration d'exécution

```sh
felucca-server --config bootstrap.toml --runtime-config conf/runtime.json
```

Vous pouvez aussi définir ce chemin avec `FELUCCA_RUNTIME_CONFIG`. Après le premier démarrage
réussi, la configuration normalisée est enregistrée de façon atomique, et les démarrages
suivants lisent d'abord l'instantané. `--check` et `--fingerprint` lisent le même instantané.
Si le fichier est corrompu, le démarrage échoue et ne revient pas au fichier de démarrage.
**Ne** lui donnez **pas** le même chemin que le fichier de trafic de `[state].path`.

L'instantané pèse au plus 16 Mio et contient de véritables secrets ; sur Unix, un nouveau
fichier reçoit la permission 0600 et un nouveau répertoire 0700, et un verrou de fichier
empêche un autre démon de l'utiliser en même temps.

Modifier la configuration depuis l'interface d'administration passe par le même mécanisme —
voir [Interface d'administration et configuration centralisée](management.md).

## Validation et journalisation

```sh
felucca-server --config conf/server.toml --check
felucca-agent  --config conf/agent.toml  --check
```

`--check` valide la structure TOML/JSON, les contraintes de champs, les ports, les ACL et les
données Geo. Il ne se connecte **pas** aux back-ends, ne vérifie **pas** le DNS public et
n'inspecte **pas** le contenu des fichiers de certificats. Le contenu des certificats et les
conflits de liaison sont vérifiés au démarrage.

Indicateurs de journalisation pour l'agent :

```sh
felucca-agent -c agent.toml --log-level info \
  --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

`RUST_LOG` outrepasse le niveau de journalisation des deux côtés. La journalisation du
serveur est définie dans la table `[log]` configurée, et la modifier exige un redémarrage.

## Variables d'environnement

| Variable | Effet |
| --- | --- |
| `FELUCCA_CONFIG` | Identique à `--config` (répétable sur l'agent) |
| `FELUCCA_RUNTIME_CONFIG` | Identique au `--runtime-config` du serveur |
| `FELUCCA_EXPAND_ENV` | Identique à `--expand-env` |
| `FELUCCA_CONFIG_TOKEN_ENV` | Le nom de la variable contenant le jeton Bearer de la configuration distante |
| `RUST_LOG` | Outrepasse le niveau du filtre de journalisation |

## Étapes suivantes

- [Le module felucca-server](server.md)
- [Le module felucca-agent](agent.md)
- [Services, mises à jour, journalisation et supervision](operations.md)
