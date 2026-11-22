# ファイルサービスと WebDAV

`mode = "file"` を使うと、別途 HTTP サーバーを立てずにエージェントから直接ファイルを配信できます。
専用の公開ポートを使うことも、サーバーの HTTP ドメイン入口に相乗りすることもできます。WebDAV、
Range リクエスト、ディレクトリの一覧表示、アカウントごとの認証に対応しています。

## 専用ポート

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

| フィールド | 説明 |
| --- | --- |
| `root` | 必須。エージェント上に**すでに存在する**ディレクトリで、プロセスの作業ディレクトリからの相対パス。起動時にディレクトリハンドルを開きます |
| `prefix` | URL のプレフィックス |
| `read_only` | 既定値は `true` |
| `auto_index` | ディレクトリの一覧表示 |
| `auth.users` | アカウントのテーブル。空のままにすると匿名で配信します |

リクエストは `root` から `..`、絶対パス、シンボリックリンクを使って外へ出ることはできず、
COPY/MOVE の対象にも同じ制約が課されます。`root` とファイルアカウントの設定はエージェント側に残り、
サーバーへの登録メッセージには**含められません**。

## HTTP ドメイン入口

`hosts` を設定すると、ファイルサービスはサーバーの HTTP/HTTPS/HTTP3 のドメイン入口を使い、`port` は
その入口と一致させる必要があります。

```toml
[[tunnel]]
mode = "file"
port = 8443
hosts = ["files.example.com"]

[tunnel.file]
root = "public"
read_only = false
```

このモードでは `tunnel.options.http.backend_http2 = true` も設定できます。送信元 ACL、クォータ、
レート制限は引き続き適用されます。file モードではネットワークバックエンド、対象ネットワークポリシー、
PROXY 出力、生の TLS モードは設定**できません**。

## WebDAV のメソッド

| モード | 許可されるメソッド |
| --- | --- |
| 読み取り専用（既定） | `GET`、`HEAD`、`PROPFIND`、`OPTIONS` |
| `read_only = false` | `PUT`、`DELETE`、`MKCOL`、`COPY`、`MOVE`、`LOCK`、`UNLOCK` が追加されます |

ロックは同じファイルトンネルの複数の接続で共有され、エージェントの再起動後に解放されます。Range
リクエスト、条件付きリクエスト、ディレクトリの一覧表示、`index.html` に対応しています。大きなファイルは
チャンク単位で転送され、全体をキャッシュするのではなく背圧を使って扱われます。

## 集中構成でのファイル

サーバーがファイルサービスをプッシュする場合、参照できるのはエージェント側であらかじめ宣言された
名前付きのディレクトリ機能だけです。

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

サーバーが任意の絶対パスをプッシュすることはできません。パスとシンボリックリンクは許可された
ディレクトリの外に出ることができず、読み取り専用のディレクトリを遠隔構成で書き込み可能に昇格させる
ことはできません。

## 確認チェックリスト

1. `felucca-agent --config agent.toml --check` が成功します。
2. `curl -i http://127.0.0.1:9001/files/` がディレクトリの一覧を返すか `index.html` を返します。
3. `Range: bytes=0-99` を付けたリクエストが 206 を返します。
4. 読み取り専用モードで `PUT` が 405 を返します。
5. `curl --path-as-is 'http://127.0.0.1:9001/files/../../etc/passwd'` が拒否されます。

## 次の手順

- [トンネルとポートマッピング](tunnels.md)
- [HTTP/HTTPS リバースプロキシ](http-services.md)
- [管理 UI と集中構成](management.md)
- サンプル: [file-server.toml](../../../examples/file-server.toml) ·
  [file-agent.toml](../../../examples/file-agent.toml)
