# Services de fichiers et WebDAV

`mode = "file"` sert les fichiers directement depuis l'agent, sans serveur HTTP distinct. Il
peut utiliser un port public dédié ou emprunter l'entrée de domaine HTTP du serveur. WebDAV,
les requêtes Range, les listes de répertoires et l'authentification par compte sont pris en
charge.

## Port dédié

```toml
[[tunnel]]
id = 100
mode = "file"
port = 9001
bind = "127.0.0.1"

[tunnel.file]
root = "public"
prefix = "/files"
read_only = true
auto_index = true

[tunnel.file.auth.users]
reader = "replace-this-password"
```

| Champ | Description |
| --- | --- |
| `root` | Obligatoire. Un répertoire qui **existe déjà** sur l'agent, relatif au répertoire de travail du processus ; un descripteur de répertoire est ouvert au démarrage |
| `prefix` | Préfixe d'URL |
| `read_only` | Vaut `true` par défaut |
| `auto_index` | Listes de répertoires |
| `auth.users` | Table de comptes ; laissez-la vide pour servir anonymement |

Les requêtes ne peuvent pas sortir de `root` via `..`, des chemins absolus ou des liens
symboliques, et les cibles de COPY/MOVE sont soumises à la même contrainte. `root` et la
configuration des comptes de fichiers restent sur l'agent et ne sont **pas** placées dans le
message d'enregistrement du serveur.

## Entrée de domaine HTTP

Lorsque vous définissez `hosts`, les services de fichiers utilisent l'entrée de domaine
HTTP/HTTPS/HTTP3 du serveur, et `port` doit correspondre à cette entrée :

```toml
[[tunnel]]
mode = "file"
port = 8443
hosts = ["files.example.com"]

[tunnel.file]
root = "public"
read_only = false
```

Dans ce mode, vous pouvez aussi définir `tunnel.options.http.backend_http2 = true`. Les ACL de
source, les quotas et les limites de débit continuent de s'appliquer. Le mode fichier ne
configure **pas** de back-end réseau, de politique de réseau cible, de sortie PROXY ni de mode
TLS brut.

## Méthodes WebDAV

| Mode | Méthodes autorisées |
| --- | --- |
| Lecture seule (défaut) | `GET`, `HEAD`, `PROPFIND`, `OPTIONS` |
| `read_only = false` | Ajoute `PUT`, `DELETE`, `MKCOL`, `COPY`, `MOVE`, `LOCK`, `UNLOCK` |

Les verrous sont partagés entre les multiples connexions d'un même tunnel fichier et sont
libérés après le redémarrage de l'agent. Les requêtes Range, les requêtes conditionnelles, les
listes de répertoires et `index.html` sont pris en charge. Les gros fichiers sont transférés
par blocs et utilisent la contre-pression plutôt que d'être mis en cache intégralement.

## Fichiers sous configuration centralisée

Lorsque le serveur envoie un service de fichiers, il ne peut référencer qu'une capacité de
répertoire nommée, déclarée à l'avance sur l'agent :

```toml
# agent
[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

```toml
# server: inside [[managed.<client>.tunnel]]
[tunnel.file]
root = "documents/shared"
```

Le serveur ne peut pas envoyer un chemin absolu arbitraire. Les chemins et les liens
symboliques ne peuvent pas sortir d'un répertoire autorisé, et un répertoire en lecture seule
ne peut pas devenir inscriptible via la configuration distante.

## Liste de vérification

1. `felucca-agent --config agent.toml --check` aboutit.
2. `curl -i http://127.0.0.1:9001/files/` liste le répertoire ou renvoie `index.html`.
3. Une requête avec `Range: bytes=0-99` renvoie 206.
4. En mode lecture seule, `PUT` renvoie 405.
5. `curl --path-as-is 'http://127.0.0.1:9001/files/../../etc/passwd'` est rejetée.

## Étapes suivantes

- [Tunnels et mappages de ports](tunnels.md)
- [Mandataire inverse HTTP/HTTPS](http-services.md)
- [Interface d'administration et configuration centralisée](management.md)
- Exemples : [file-server.toml](../../../examples/file-server.toml) ·
  [file-agent.toml](../../../examples/file-agent.toml)
