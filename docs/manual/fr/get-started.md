# Prise en main

Utilisez votre Server public ou demandez une commande de connexion à son opérateur. Vous n’avez pas à préparer de certificat, modifier un fichier de configuration ni installer l’agent au préalable.

## Connecter un appareil en une commande

1. Dans **Appareils → Connecter un appareil**, saisissez un nom. Par défaut, cet appareil, le réseau local et Internet sont accessibles. Pour limiter l’accès, ouvrez **Paramètres avancés : autorisations d’accès** et choisissez cet appareil, le réseau local ou une plage personnalisée.
2. Copiez la commande dans Terminal (macOS/Linux) ou PowerShell (Windows) sur l’appareil. Elle détecte le processeur, télécharge le client adapté et vérifie sa somme de contrôle.
3. Gardez la fenêtre ouverte. Une fois en ligne, choisissez **Ajouter un service**, saisissez son adresse locale et utilisez l’adresse publiée. La reconnexion est automatique.
4. Pour un accès privé, créez un appareil destinataire dans **Accès privé** et exécutez sa commande sur l’autre ordinateur. Le destinataire n’a pas besoin de connexion à la console.

La commande contient les identifiants de l’appareil : partagez-la uniquement avec la personne prévue. Pour vous reconnecter, fermez l’ancienne fenêtre puis relancez la commande. Le serveur doit fournir un paquet adapté au système de l’appareil. ZIP, permissions de dossiers et réglages avancés restent facultatifs.

## Lancer votre Server

Avec les programmes de la distribution, initialisez une seule fois dans un dossier accessible en écriture :

```sh
./felucca-server --init
```

La commande crée la configuration, affiche un mot de passe root aléatoire et démarre le serveur. Ouvrez l’adresse affichée. Conservez `felucca-server.toml` et tout le dossier `felucca-server.data/` lors des mises à jour ou sauvegardes. Redémarrez sans `--init` :

```sh
./felucca-server
```

Pour un déploiement public, utilisez `--init --admin-url https://console.example.com` derrière un proxy HTTPS transmettant vers `127.0.0.1:8990` et conservant Host. Une adresse HTTP directe ouvre le port correspondant publiquement ; utilisez HTTPS pour l’accès public. Les appareils doivent atteindre le pont TCP 9001 et les visiteurs le port du service publié. Voir [déploiement et distribution des clients](../../management.md).

## Prérequis

| Exigence | Détails |
| --- | --- |
| Rust 1.88 ou ultérieur | Pour la compilation ; ce dépôt est vérifié sur Rust 1.98.1 |
| Python 3 + OpenSSL | Nécessaire uniquement pour le script de démonstration locale `scripts/demo-local.py` |
| Un hôte disposant d'une IP publique | Exécute `felucca-server` |
| Un hôte sur un réseau privé | Exécute `felucca-agent`, avec le service que vous souhaitez exposer |
| Autorisation de lier des ports TCP/UDP locaux | Nécessaire pour les tests ; les règles de pare-feu sont gérées par la personne qui déploie |

Felucca n'installe aucun service système et ne modifie aucune règle de pare-feu.

## Option 1 — démonstration locale

Si vous voulez simplement le voir fonctionner sur votre propre machine (page d'administration
et transfert de ports), sans réseau public :

```sh
python3 scripts/demo-local.py
```

Le script compile les binaires de débogage de façon incrémentale, génère un certificat et des
identifiants aléatoires, démarre un site web de test, le serveur et l'agent, vérifie le
transfert réel, puis ouvre la page de démonstration lui-même. Le terminal affiche l'adresse
d'administration, le compte et le mot de passe. Appuyez sur `Ctrl+C` pour arrêter tous les
services de démonstration.

Chaque écouteur se lie uniquement à `127.0.0.1`, et un port libre est choisi automatiquement
lorsqu'un port par défaut est déjà occupé. Chaque exécution conserve sa configuration, ses
informations de connexion et ses journaux dans son propre répertoire
`target/local-demo/run-*`.

| Indicateur | Effet |
| --- | --- |
| `--no-open` | Ne pas ouvrir le navigateur automatiquement |
| `--no-build` | Ignorer la compilation lorsque les binaires sont déjà construits |

## Option 2 — compilation

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
```

Le résultat est `target/release/felucca-server` et `target/release/felucca-agent`.
`scripts/cargo-external.sh` place le cache Cargo sur le disque où se trouve l'espace de
travail. Lorsque vous disposez de beaucoup d'espace disque, vous pouvez aussi appeler `cargo`
directement.

## Option 3 — premier tunnel établi à la main

### 1. Générer le certificat de pont

Vous générez le certificat de pont une seule fois, et vous conservez la clé privée :

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

### 2. Définir la clé partagée

La `vkey` des deux fichiers doit être la même chaîne aléatoire :

```sh
openssl rand -hex 32
```

Inscrivez-la dans `[client.agent-1].vkey` de `conf/server.local.toml` et dans `vkey` de
`conf/agent.local.toml`. Le `key_id` de l'agent doit correspondre à l'entrée de table
`[client.<id>]` du serveur. Remplacez le `server` de l'agent par une adresse accessible de
l'hôte serveur.

### 3. Lire l'empreinte du certificat

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
```

Recopiez l'empreinte SHA-256 affichée dans `[trust].fingerprint` de l'agent. Vérifiez ensuite
les deux fichiers :

```sh
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

### 4. Démarrer les deux programmes

Exécutez une commande dans chacun des deux terminaux :

```sh
target/release/felucca-server --config conf/server.local.toml
```

```sh
target/release/felucca-agent --config conf/agent.local.toml
```

### 5. Vérifier

La configuration d'exemple relaie le TCP `8080` du serveur vers `127.0.0.1:3000` sur la
machine de l'agent. Démarrez sur la machine de l'agent un service qui écoute sur le port
3000, puis ouvrez le port 8080 sur le serveur.

## Étapes suivantes

- [Architecture et concepts](architecture.md) — ce que signifient pont, transport et tunnel
- [Le module felucca-server](server.md) — la table des clients, les écouteurs HTTP,
  l'autorisation de ports
- [Le module felucca-agent](agent.md) — les reconnexions, le réseau de sortie, les instances
  multiples
- [Tunnels et mappages de ports](tunnels.md) — les modes TCP/UDP/HTTP/fichier/mandataire
- [Référence](reference.md) — toutes les commandes et tous les champs
