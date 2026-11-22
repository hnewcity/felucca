# HTTP/HTTPS リバースプロキシ

このページでは、内部の HTTP サービスをドメインとパスで公開する方法を取り上げます。ルーティング、
ヘッダーと Host の書き換え、リダイレクト、CORS、エラーページ、SNI、HTTP/2、HTTP/3、ポートの共有、
そして転送ヘッダーです。

## HTTP ルートを登録する

```toml
# Server side
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"

# Agent side
[[tunnel]]
mode = "http"
port = 8080                       # Must match http.bind or http.tls_bind
hosts = ["app.example.test", "*.example.test"]
balance = "round_robin"

[tunnel.options.http]
path = "/api/"
scheme = "any"
rewrite_prefix = "/v2/"
host = "backend.internal"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`http` トンネルは個別のリスナーを開きません。`port` はどの HTTP リスナーに相乗りするかを決め、
HTTP と HTTPS の両方で到達できます。

## ルーティングのルール

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"        # any | http | https
```

一致の優先順位は**完全一致するドメイン、次に最長のパスのプレフィックス、最後に完全一致する
スキーム**です。ドメイン名の大文字と小文字は区別されず、末尾のドットは正規化されます。

| パターン | 一致するもの |
| --- | --- |
| 完全一致 `app.example.test` | そのドメインだけ |
| ワイルドカード `*.example.test` | `a.example.test` に一致しますが、`example.test` や `a.b.example.test` には一致しません |

同じドメイン、パス、プロトコルの重複登録は拒否されます。一致するルートがないリクエストは 404 を
返します。同じドメインに複数のパスを登録し、別々のトンネルに割り当てることもできます。

## リクエストとレスポンスのオプション

```toml
[tunnel.options.http]
rewrite_prefix = "/v2/"
host = "backend.internal"
cors = true
redirect = "https://new.example{path}"
force_https = true
request_headers = { x-source = "{remote_ip}", x-remove = "" }
response_headers = { x-proxy = "felucca" }
error_pages = { "502" = "<h1>Service unavailable</h1>" }
```

| オプション | 動作 |
| --- | --- |
| `rewrite_prefix` | プレフィックスを書き換え、残りのパスとクエリ文字列はそのまま保ちます |
| `host` | バックエンドに送る Host を書き換えます |
| `request_headers` | ヘッダーのテンプレート。空の値にするとそのヘッダーを削除します |
| `response_headers` | レスポンスヘッダーのテンプレート |
| `cors` | `Access-Control-Allow-Origin: *` を返し、有効なプリフライトにはその場で 204 を返します。プリフライトはサイトアカウントを要求しませんが、実際のリクエストは認証が必要です |
| `redirect` | 307 を返します |
| `force_https` | 301 を返します。転送先はサーバーの HTTPS リスナーのポートを使い、443 は省略されます。登録時点で HTTPS リスナーが存在している必要があります |
| `error_pages` | ステータスコードごとにレスポンスボディを置き換えます。400..599 に対応し、1 ページあたり最大 64 KiB です |

テンプレートは `{host}`、`{path}`（クエリ文字列を含む）、`{scheme}`、`{remote_ip}`、
`{remote_port}` を使えます。値は元のリクエストから取られ、置換結果がさらに展開されることは
ありません。Content-Length や Transfer-Encoding といったメッセージ境界のヘッダー、およびホップ
バイホップのヘッダーは書き換えられません。Host は専用のフィールドを持ちます。

サイト認証は `Authorization: Basic` を使い、失敗すると 401 を返します。検証済みの資格情報が
バックエンドへ転送されることはありません。

## TLS のモード

```toml
[tunnel.options.http]
tls_mode = "http"        # http | passthrough | offload
```

| モード | 動作 |
| --- | --- |
| `http` | 既定。一般的な HTTPS リバースプロキシ |
| `passthrough` | SNI に応じて生の TLS ストリームをバックエンドに通し、バックエンドの証明書がそのまま利用者に見えます。このモードでは `backend.tls` を無効にする必要があります |
| `offload` | サーバーの SNI 証明書で TLS を終端し、その後で任意の TCP コンテンツをバックエンドに送ります |

`passthrough` と `offload` は SNI 全体を占有するため、HTTP のパスやヘッダーのオプション、
サイト認証と組み合わせることはできません。

## ポートとプロトコル

| 機能 | 説明 |
| --- | --- |
| 入口 | HTTP/1.1、HTTP/2、HTTP/3 |
| バックエンド | HTTP/1.1、HTTP/2（`backend_http2 = true`） |
| ALPN | TLS 入口は `h2` と `http/1.1` に対応します。平文入口は HTTP/2 の prior knowledge に対応します |
| WebSocket | 3 種類の入口と 2 種類のバックエンドの間で自動的に変換されます。HTTP/1.1 入口は 101 を返し、HTTP/2 または HTTP/3 は `:protocol = websocket` の拡張 CONNECT を使います |
| HTTP/3 | `http.http3_bind` が必要です。1 つの QUIC 接続あたり双方向ストリームは最大 128 本で、ストリームごとの受信ウィンドウは 1 MiB、接続ごとは 16 MiB です。30 秒アイドルが続くと閉じられ、リクエストヘッダーは 32 KiB に制限されます |

HTTP/3 は公開側の入口プロトコルにすぎません。バックエンドは引き続き HTTP/1.1 または HTTP/2 を
使います。通常の HTTPS レスポンスには `Alt-Svc` ヘッダーが付きます。外側でマッピングするポートが
異なる場合は、`response_headers` で上書きできます。

## ポートの共有

`bridge.bind`、`http.bind`、`http.tls_bind` はすべて**同じ** IP:port にできます。1 つのソケットが
ハンドシェイクの特徴で多重化します。直接接続する TLS/WSS のエージェントは `felucca/9` ALPN を
使い、Web サイトは `h2`/`http/1.1` または ALPN なしを使います。通常の `GET /ws` は Web サイトへの
リクエストのままで、正しい WebSocket アップグレードだけがブリッジに入ります。QUIC ブリッジと
HTTP/3 も 1 つの UDP アドレスを共有でき、ALPN で多重化されます。ブリッジ証明書と Web サイト証明書は
別々に選択され、別々に更新されます。

## クライアントのアドレス

既定では、クライアントから送られた `Forwarded`、`X-Forwarded-*`、`X-Real-IP` ヘッダーは削除され、
その後に `X-Forwarded-For`、`X-Real-IP`、`X-Forwarded-Host`、`X-Forwarded-Proto` が、直接の送信元
アドレスまたは `http.proxy_accept` で検証済みの PROXY 送信元から設定されます。
`forwarded_headers = false` を設定すると、この書き換えを中止できます。プロキシの連鎖をすべて
自分で管理しているデプロイに向いています。ホップバイホップのヘッダー、および `Connection`
で名指しされたヘッダーは次のホップに渡されません。

## 次の手順

- [証明書、SNI、ACME](certificates.md)
- [バックエンド、負荷分散、ヘルスチェック](backends.md)
- [ファイルサービスと WebDAV](file-services.md) — ドメインによるファイル配信
- [アクセス制御、クォータ、ストリーム符号化](security.md) — ドメイン単位のクォータと ACL
