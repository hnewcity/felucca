# Services, mises à jour, journalisation et supervision

Cette page couvre l'enregistrement de Felucca auprès d'un gestionnaire de services, la mise à
jour sûre des binaires, la rotation des journaux, les métriques et le profilage CPU.

## Installation en tant que service système

Les deux programmes utilisent les mêmes sous-commandes, et `--name` prend en charge plusieurs
instances :

```sh
felucca-server service --name office render --config /srv/felucca/server.toml
felucca-server service --name office --dry-run install --config /srv/felucca/server.toml
felucca-server service --name office install --config /srv/felucca/server.toml
felucca-server service --name office start
felucca-server service --name office status
felucca-server service --name office restart
felucca-server service --name office stop
felucca-server service --name office uninstall
```

| Plateforme | Gestionnaire | Portée par défaut |
| --- | --- | --- |
| Linux | systemd | `user` ; `--scope system` est au niveau machine |
| macOS | launchd | `user` ; `--scope system` est au niveau machine |
| Windows | SCM | `system` uniquement |

`render --platform systemd|launchd|windows` affiche la définition de n'importe quel hôte pris
en charge, sans avoir à assembler des commandes dans un shell. `--workdir` vaut par défaut le
répertoire qui contient la configuration, et les chemins relatifs écrits dans la configuration
sont interprétés à partir de celui-ci. `--binary` choisit le programme qui est installé. Les
arguments supplémentaires du démon se placent après `--`, par exemple
`-- --log-file /srv/felucca/agent.log`.

L'installation **n'écrase pas** un service existant du même nom, et elle ne démarre pas le
service immédiatement. Sous Linux, elle active le service ; sous macOS, l'amorçage a lieu à la
prochaine connexion ou au prochain démarrage, ou lorsque vous lancez `start` explicitement. Un
service systemd utilisateur qui doit être présent au démarrage exige que la personne qui
déploie active le lingering, et un service utilisateur macOS dépend d'une session graphique
ouverte. Les services au niveau machine s'exécutent avec l'identité par défaut du gestionnaire
de services : modifiez l'utilisateur d'exécution pour l'adapter à votre déploiement. Sous
Windows, l'identité d'installation est `NT AUTHORITY\LocalService`, et la personne qui déploie
doit lui accorder les permissions correspondantes sur les répertoires de configuration, de
journaux, d'état et de racine de fichiers.

La désinstallation conserve l'exécutable, la configuration, les journaux, les certificats et
l'état d'exécution.

Périmètre de ce qui a été vérifié : le dépôt teste la génération des définitions, l'analyse des
plist macOS, les mises à jour par copie isolée et un profilage CPU réel. Une image serveur Linux
arm64 a aussi été construite et exécutée dans OrbStack, avec vérification de la connexion
d'administration, de la supervision, du TLS de pont et de la persistance après recréation du
conteneur ; voir le compte rendu de déploiement.
L'installation en tant que service système et les gestionnaires de services Linux/Windows
restent non vérifiés.

## Mise à jour des binaires

Récupérez le SHA-256 d'un exécutable autonome sur un canal de publication de confiance, puis
vérifiez ou remplacez le programme en cours d'exécution :

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --expect-version 0.1.0
felucca-agent  update --source https://releases.example/felucca-agent --sha256 <64-hex-digest>
```

Le condensé constitue la **base de confiance** et doit provenir d'un manifeste de publication de
confiance. L'entrée doit être un binaire autonome : déballez donc d'abord le tar.gz/zip. Les
téléchargements HTTPS vérifient les certificats et ne suivent pas les redirections ; pour les
téléchargements privés, utilisez `--token-env VARIABLE_NAME`, et les identifiants ne sont jamais
écrits dans les journaux. Les téléchargements sont limités à 512 MiB et 5 minutes. La sonde
`--version`, bornée à 10 secondes, ne s'exécute qu'une fois le condensé vérifié, et elle contrôle
le type server/agent ainsi que la version facultative. En cas d'échec, les fichiers temporaires
sont nettoyés, et un verrou de mise à jour empêche tout remplacement concurrent. Le remplacement
conserve les permissions d'origine du fichier.

Ensuite, vous devez lancer explicitement `service ... restart` ; les processus déjà en cours
d'exécution continuent d'utiliser l'ancienne version. La mise à jour à l'intérieur d'un conteneur
ne fonctionne pas sur un système de fichiers d'image en lecture seule — remplacez plutôt l'image.

Lorsque la version de protocole change, le serveur et l'agent doivent être mis à niveau ensemble.
Le script d'empaquetage exécute réellement `--build-info` et refuse de mélanger des binaires de
versions de protocole ou de cibles différentes.

## Journalisation

Côté serveur :

```toml
[log]
level = "info"
file = "/data/felucca.log"
json = true

[log.rotation]
max_bytes = 10485760
max_files = 5
interval = "24h"
```

La sortie va à la fois vers le fichier et vers stderr. La rotation a lieu entre des événements
complets : un seuil de taille peut donc laisser passer un événement de plus. Les archives sont
numérotées de `.1` à `.5`, `.1` étant la plus récente. `interval` est mesuré à partir du moment
où le fichier courant est ouvert, et un redémarrage relance la période. Un verrou exclusif
empêche plusieurs processus de faire tourner le même chemin, et les nouveaux fichiers de journal
et de verrou sont en 0600 sur Unix.

**Modifier la configuration de journalisation du serveur exige un redémarrage ; le plan
d'administration refuse les modifications en ligne de ces réglages.** L'agent utilise des
arguments en ligne de commande à la place ; voir [le module felucca-agent](agent.md).

## Supervision

`GET /metrics` et `GET /api/metrics` renvoient du texte Prometheus. Utilisez un jeton Bearer de
compte d'administration ou une session valide, car il n'existe **aucun point d'entrée de métriques
anonyme**, et les métriques respectent la visibilité de l'utilisateur du compte.

Elles couvrent la santé du service, la révision de la configuration, l'état en ligne/activé des
nœuds, les nombres de connexions, le trafic cumulé des nœuds, l'état de disponibilité des tunnels
et le trafic cumulé des tunnels. Les compteurs de trafic réutilisent le MeterStore persisté
existant : ils survivent donc à un redémarrage du serveur dès que vous activez `[state]`. La page
Supervision de l'interface d'administration affiche le CPU de l'hôte, la mémoire physique, la
capacité du système de fichiers et les débits réseau sous forme de courbes interactives, sans
texte Prometheus ni instructions de configuration. Les échantillons arrivent toutes les cinq
secondes et restent en mémoire une heure ; un redémarrage du processus efface l'historique.
Choisissez une fenêtre de 5 minutes, 15 minutes ou 1 heure. Survolez, cliquez pour figer, ou
utilisez les touches fléchées pour inspecter l'heure exacte des échantillons, le fuseau horaire
local et les valeurs. Faites glisser le curseur de plage ou utilisez les boutons de zoom et de
réinitialisation pour zoomer les quatre graphiques ensemble ; les mises à jour en direct
conservent la fenêtre temporelle absolue sélectionnée. Les cartes affichent le minimum, la moyenne
des échantillons et le maximum des observations visibles, et la légende du réseau bascule les
séries de réception et d'envoi. Les métriques de l'hôte sont réservées aux administrateurs ; les
autres rôles voient le trafic et les connexions des ressources qui leur sont autorisées. Le point
d'entrée structuré est `GET /api/monitor?window=900` (fenêtres prises en charge : 300, 900, 3600
secondes).

## Profilage CPU

Après avoir défini `[admin] profiling = true`, un administrateur peut appeler `POST /api/profile`
avec le corps JSON `{"seconds":5}`, ou télécharger un graphique en flammes CPU depuis la page
Supervision. Linux et macOS sont pris en charge, et les autres plateformes refusent de l'activer.
Chaque exécution dure de 1 à 30 secondes à 49 Hz, une seule tâche est autorisée à la fois dans le
processus, et la sortie est plafonnée à 8 MiB. Une requête HTTP abandonnée ne libère pas
prématurément le verrou d'échantillonnage, et l'arrêt du démon annule l'échantillonnage. Lorsqu'il
n'y a aucun échantillon pendant une période d'inactivité, un SVG explicatif est renvoyé. Les
versions de release conservent les symboles de fonctions pour le profilage mais abandonnent les
informations de ligne de débogage ; la recherche au niveau de la ligne exige une version que vous
compilez avec les informations de débogage.

## Docker

```sh
docker build --target server -t felucca-server:local .
docker build --target agent  -t felucca-agent:local .

docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

Les images utilisent par défaut UID/GID 10001. Montez le répertoire de configuration en lecture
seule, et laissez l'image écrire dans le répertoire de données. Ouvrez explicitement dans votre
configuration les ports TCP/UDP supplémentaires dont vous avez besoin, et liez-vous à des adresses
joignables depuis l'intérieur du conteneur. Des images multi-architectures peuvent être
construites avec Buildx en utilisant les cibles natives du Dockerfile. `STOPSIGNAL` est SIGTERM,
ce qui correspond au chemin d'arrêt du programme.

### OrbStack local

Démarrez OrbStack, puis lancez `sh scripts/run-orbstack.sh` depuis la racine du dépôt. Cela ne
construit et ne déploie que le serveur, interface d'administration incluse. L'hôte a besoin de
Docker Compose, de Python 3 et d'OpenSSL. Ouvrez `http://127.0.0.1:8088` et connectez-vous en tant
que `root` avec le mot de passe aléatoire de `conf/orbstack/credentials.json`. L'écouteur
d'administration se lie uniquement à la boucle locale.

Les identifiants et le certificat de pont persistant vivent dans le répertoire `conf/orbstack/`,
ignoré par Git. La configuration et les instantanés de trafic vivent dans le volume
`felucca_server-data`. Le fichier d'identifiants enregistre le mot de passe initial ; la
configuration d'administration enregistrée a préséance sur le TOML d'amorçage. Relancer le script
construit les sources actuelles et met à jour le conteneur tout en préservant les identifiants,
les certificats et les données existants.

Le conteneur redémarre avec le moteur Docker, sauf arrêt manuel. Aucun client n'est configuré au
départ. Le TCP et le QUIC du pont utilisent le port 14028. Compose utilise la mise en réseau hôte
d'OrbStack : les ports de tunnel supplémentaires n'ont donc besoin d'aucun mappage de port Docker
individuel. Évitez les ports déjà utilisés par d'autres programmes locaux.

```sh
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

`down` conserve le volume de données. Ajouter `-v` supprime la configuration persistée et les
données de trafic.

## Empaquetage et intégration continue

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

Les compilations depuis les sources exigent Node.js 22.12+ (24 recommandé). L'interface
d'administration React + Vite de `apps/admin/` est intégrée au binaire du serveur ; les
déploiements n'ont pas besoin de Node.js. Recompilez le frontend après l'avoir modifié : Cargo
rejette les paquets manquants ou périmés.

Les artefacts contiennent les deux programmes, les exemples, la documentation, un manifeste de
condensés par binaire et un condensé d'archive. Les binaires qui ne peuvent pas s'exécuter sur
l'hôte d'empaquetage sont rejetés par défaut, et l'intégration continue multiplateforme empaquette
sur chaque plateforme cible. Le workflow GitHub exécute les vérifications, les tests et les
archives pour Linux amd64/arm64, macOS arm64 et Windows amd64, et vérifie séparément la version
minimale de Rust 1.88 et la construction Docker. Le workflow ne téléverse que les artefacts de
tâche ; il ne crée pas de release GitHub et ne pousse pas d'images.

## Liste de vérification

```sh
felucca-server service --name office render --config conf/server.toml
felucca-server --config conf/server.toml --fingerprint
felucca-agent  --config conf/agent.toml  --check
```

`--fingerprint` exige que la configuration référence des fichiers PEM fixes. `--check` ne signifie
pas que les back-ends, le DNS, les fichiers de certificats ou les ports publics sont déjà
disponibles.

## Étapes suivantes

- [Le module felucca-server](server.md)
- [Interface d'administration et configuration centralisée](management.md)
- [Référence](reference.md)
