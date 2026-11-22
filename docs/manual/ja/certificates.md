# 証明書、SNI、ACME

このページでは 3 種類の証明書を取り上げます。ブリッジ証明書、Web サイト用の固定証明書、そして
ACME によって自動的に発行・更新される証明書です。

## ブリッジ証明書

```toml
[bridge]
bind = "0.0.0.0:14028"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }
cert_reload_interval = "30s"
```

`bridge.tls` を省略し、実行時設定の保存を有効にすると（`--init` や管理画面を含む）、サーバーはスナップショットの隣の `<snapshot>.identity.json` に識別情報を保存します。再起動やバックアップ時も保持してください。保存を使わない場合は一時的な識別情報になります。`--fingerprint` は指定された PEM を読みます。自動生成された指紋は管理画面と起動ログで確認できます。

PEM ファイルは `cert_reload_interval`（既定では 30 秒）で再読み込みされます。新しい TLS/WSS 接続と
QUIC 接続は新しい証明書を使い、既存の接続は切断されません。証明書または鍵の更新が不完全だった場合は
直前の値が保持されます。`pinned` の信頼方式を使うエージェントは、次の再接続までにフィンガープリントを
更新する必要があります。正しい CA に対して検証するエージェントは、特定の証明書を固定する必要は
ありません。

```sh
target/release/felucca-server --config conf/server.toml --fingerprint
```

エージェント側では 2 つの方法から選びます。

```toml
[trust]
mode = "pinned"
fingerprint = "<from --fingerprint>"

[trust]
mode = "webpki"        # CA-issued bridge certificate
```

## Web サイトの証明書

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

完全一致はワイルドカードより優先され、ワイルドカードは `default_tls` より優先されます。どれにも
一致しない場合、ハンドシェイクは拒否されます。SNI と HTTP の Host が矛盾する場合は 421 を返します。
固定証明書は既定で 30 秒ごとに再読み込みされます。この間隔は `http.cert_reload_interval` で調整
できます。証明書は秘密鍵と対になっていることを確認できた時点でのみ置き換えられ、失敗した場合は
古い証明書がそのまま使われます。

HTTP/3 と HTTP/HTTPS は同じ SNI リゾルバを共有します。これには ACME の更新と、固定 PEM ファイルの
ホットリロードも含まれます。

## ACME による発行

```toml
[http.acme]
email = "admin@example.com"
accept_tos = true
challenge = "http-01"          # http-01 | tls-alpn-01
cache_dir = "state/acme"
staging = false
domains = ["app.example.com"]
```

| フィールド | 説明 |
| --- | --- |
| `accept_tos` | CA の利用規約に同意した後にのみ `true` にしてください |
| `challenge` | `http-01` は `http.bind` に到達するために公開された TCP 80 が必要です。`tls-alpn-01` は `http.tls_bind` に直接到達するために公開された TCP 443 が必要で、その場合は `http.bind` を閉じていても構いません |
| `staging` | まずステージングで接続性を検証します。これは Let's Encrypt のステージングに固定されます |
| `directory` | 別の CA を指定します。既定は Let's Encrypt の本番環境です |
| `cache_dir` | アカウントと証明書のキャッシュを置くディレクトリ。**必ず保持し、保護してください** |
| `domains` | 発行対象の名前を明示的に限定します。空の場合は、登録済みの完全一致ドメインから毎分名前が導かれ、ワイルドカードは除外されます |

アカウントは `account.json` に保存され、各ドメインの証明書チェーンと秘密鍵は 1 つの JSON ファイルに
まとめて置かれ、アトミックに置き換えられます。Unix ではファイルのモードは 0600、新しいキャッシュ
ディレクトリは 0700 になります。証明書は起動時にキャッシュから復元されます。更新は証明書の実際の
有効期間から計算され、その期間の 3 分の 1 を超えない余裕を持って行われます。確認は 1 分ごとに
実行され、失敗した場合は 1 時間後に再試行し、更新が失敗している間も古い証明書が使われ続けます。
発行に成功するとホットで切り替わり、既存の TLS 接続は切断されません。

**ACME が管理するのは HTTP/HTTPS リバースプロキシの証明書だけです。ブリッジ証明書やエージェントの
フィンガープリントの固定は変更しません。**

### EAB と ZeroSSL

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

ZeroSSL では手動の EAB を使うか、API キーモードに切り替えられます。

```toml
[http.acme]
directory = "https://acme.zerossl.com/v2/DV90"
staging = false
zerossl_api_key = "your-account-key"
```

API キーモードでは、アカウントを初めて登録するときに EAB を取得し、その後は ACME を通じて証明書を
発行します。**手動の `eab` と同時に設定することはできません。** アカウントのキャッシュが存在すると、
新たな EAB の要求は行われません。API キーとレスポンス内のシークレットがエラーメッセージに
書き出されることはありません。

## デプロイ時の注意点

- どちらの ACME チャレンジも**ワイルドカード証明書には対応していません**。ワイルドカードの名前には
  外部で発行した固定証明書を使ってください。
- アプリケーションが 8080/8443 で待ち受けている場合は、外側から 80/443 をそれらのポートに
  マッピングしてください。
- 発行が成功するまでは、そのドメインの TLS ハンドシェイクが失敗する可能性があります。HTTP-01 による
  検証は引き続き機能します。
- 前面に TLS を終端するプロキシを置く場合、チャレンジの TLS はそのまま転送し、チャレンジの証明書は
  `acme-tls/1` を含む ALPN 値のときだけ返してください。
- CA を切り替えるとき、ステージングと本番を切り替えるときは新しい `cache_dir` を使ってください。
- このリポジトリではローカルの模擬 CA に対して実際の CSR とチャレンジの流れを実行しています。公開 CA
  からの実際の発行は、ご自身のドメイン環境で検証する必要があります。

## 次の手順

- [HTTP/HTTPS リバースプロキシ](http-services.md)
- [felucca-server モジュール](server.md)
- [システムサービス、更新、ログ、監視](operations.md)
