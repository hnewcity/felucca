# Certificats, SNI et ACME

Cette page couvre les trois types de certificats : le certificat de pont, les certificats
statiques de site, et les certificats émis et renouvelés automatiquement par ACME.

## Certificat de pont

```toml
[bridge]
bind = "0.0.0.0:14028"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }
cert_reload_interval = "30s"
```

Si `bridge.tls` est omis et que le stockage de configuration est actif (notamment avec `--init` ou la console), le serveur conserve son identité dans `<snapshot>.identity.json`, à côté de l’instantané. Gardez ce fichier lors des redémarrages et sauvegardes. Sans stockage, l’identité est temporaire. `--fingerprint` lit les fichiers PEM explicites ; les empreintes automatiques figurent dans la console active et le journal de démarrage.

Les fichiers PEM sont rechargés selon `cert_reload_interval` (30 secondes par défaut). Les
nouvelles connexions TLS/WSS et QUIC adoptent le nouveau certificat, et les connexions
existantes ne sont pas interrompues. Si une mise à jour de certificat ou de clé est
incomplète, la valeur précédente est conservée. Un agent qui utilise la confiance `pinned`
doit mettre à jour son empreinte avant la reconnexion suivante ; un agent qui valide face à
l'autorité de certification correcte n'a pas besoin de figer un certificat particulier.

```sh
target/release/felucca-server --config conf/server.toml --fingerprint
```

Côté agent, vous avez deux possibilités :

```toml
[trust]
mode = "pinned"
fingerprint = "<from --fingerprint>"

[trust]
mode = "webpki"        # CA-issued bridge certificate
```

## Certificats de site

```toml
[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.certificates."*.example.com"]
cert = "conf/wildcard.pem"
key = "conf/wildcard.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"
```

Les correspondances exactes ont priorité sur les jokers, et les jokers sur `default_tls`. Si
rien ne correspond, la poignée de main est rejetée. Un conflit entre le SNI et le Host HTTP
renvoie 421. Les certificats statiques sont relus toutes les 30 secondes par défaut,
intervalle que vous pouvez ajuster avec `http.cert_reload_interval`. Un certificat n'est
remplacé qu'une fois qu'il correspond à sa clé privée, et un échec laisse l'ancien certificat
en place.

HTTP/3 et HTTP/HTTPS partagent le même résolveur SNI, y compris le renouvellement ACME et le
rechargement à chaud des fichiers PEM statiques.

## Émission ACME

```toml
[http.acme]
email = "admin@example.com"
accept_tos = true
challenge = "http-01"          # http-01 | tls-alpn-01
cache_dir = "state/acme"
staging = false
domains = ["app.example.com"]
```

| Champ | Description |
| --- | --- |
| `accept_tos` | Ne mettez `true` qu'après avoir accepté les conditions de l'autorité de certification |
| `challenge` | `http-01` exige un TCP 80 public pour atteindre `http.bind`. `tls-alpn-01` exige un TCP 443 public pour atteindre directement `http.tls_bind`, auquel cas `http.bind` peut être fermé |
| `staging` | Vérifiez d'abord la connectivité avec l'environnement de test ; cela épingle l'environnement Let's Encrypt staging |
| `directory` | Pointez vers une autre autorité de certification ; la valeur par défaut est la production Let's Encrypt |
| `cache_dir` | Répertoire du cache de compte et de certificats. **Conservez-le et protégez-le** |
| `domains` | Limite explicitement les noms émis. Lorsqu'elle est vide, les noms sont déduits chaque minute des domaines exacts enregistrés, et les jokers sont ignorés |

Le compte est stocké dans `account.json`, et la chaîne de certificats et la clé privée de
chaque domaine cohabitent dans un seul fichier JSON, remplacé de façon atomique. Sur Unix,
les fichiers utilisent le mode 0600 et les nouveaux répertoires de cache 0700. Les
certificats sont restaurés depuis le cache au démarrage. Le renouvellement est calculé à
partir de la validité réelle du certificat, avec une avance d'au plus un tiers de cette
validité. Les vérifications ont lieu chaque minute, les échecs sont réessayés au bout d'une
heure, et l'ancien certificat reste en service tant que le renouvellement échoue. Une
émission réussie est basculée à chaud et n'interrompt pas les connexions TLS existantes.

**ACME ne gère que les certificats du mandataire inverse HTTP/HTTPS. Il ne modifie ni le
certificat de pont ni l'empreinte figée d'un agent.**

### EAB et ZeroSSL

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

ZeroSSL peut utiliser un EAB manuel, ou basculer en mode clé d'API :

```toml
[http.acme]
directory = "https://acme.zerossl.com/v2/DV90"
staging = false
zerossl_api_key = "your-account-key"
```

Le mode clé d'API obtient un EAB lors du premier enregistrement du compte, puis émet les
certificats via ACME. **Vous ne pouvez pas configurer un `eab` manuel en même temps.** Aucun
nouvel EAB n'est demandé dès lors qu'un cache de compte existe. La clé d'API et les secrets
des réponses ne sont pas écrits dans les messages d'erreur.

## Notes de déploiement

- Aucun des deux défis ACME ne **prend en charge les certificats joker**. Pour les noms
  joker, utilisez des certificats statiques émis à l'extérieur.
- Lorsque votre application écoute sur 8080/8443, faites correspondre 80/443 à ces ports
  depuis l'extérieur.
- Tant que l'émission n'a pas abouti, la poignée de main TLS de ce domaine peut échouer ;
  HTTP-01 fournit tout de même la validation.
- Un mandataire qui termine TLS en amont doit transmettre le TLS du défi tel quel et ne
  renvoyer le certificat du défi que pour les valeurs ALPN contenant `acme-tls/1`.
- Utilisez un nouveau `cache_dir` lorsque vous changez d'autorité de certification ou passez
  de l'environnement de test à la production.
- Le dépôt pilote de véritables flux de CSR et de défi face à une autorité de certification
  simulée locale. L'émission réelle par une autorité publique doit encore être vérifiée dans
  votre propre environnement de domaine.

## Étapes suivantes

- [Mandataire inverse HTTP/HTTPS](http-services.md)
- [Le module felucca-server](server.md)
- [Services, mises à jour, journalisation et supervision](operations.md)
