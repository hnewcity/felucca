# 設定の取得元と CLI

このページでは、設定がどこから来るか、どう渡すか、どの環境変数が効くか、そして起動せずに検証する
方法を取り上げます。各フィールドの意味は各モジュールのガイドで説明しています。

## 設定ファイル

どちらのプログラムも TOML と JSON を受け付けます。相対パスはプロセスの作業ディレクトリを基準に
解決されるため、`--workdir` と systemd/launchd の起動ディレクトリの両方が影響します。

```sh
felucca-server --config /srv/felucca/server.toml
felucca-agent  --config /srv/felucca/agent.toml
```

`--config` がない場合、プログラムは作業ディレクトリの `felucca-server.toml` /
`felucca-agent.toml` を読み込みます。`-c` は `--config` の短縮形で、エージェントはエイリアスとして
`--launch` も受け付けます。

設定はすべて `deny_unknown_fields` を使います。未知のフィールドはエラーになり、黙って無視される
ことはありません。

## 指定できる形式

`--config` は 4 つの形式を受け付けます。

| 形式 | 例 | 詳細 |
| --- | --- | --- |
| ローカルファイル | `conf/server.toml` | TOML または JSON |
| HTTP(S) URL | `https://cfg.example/edge.json` | 最大 15 秒待ち、200 を要求します。リダイレクトはたどらず、圧縮されたレスポンスも受け付けません。URL には userinfo もフラグメントも指定できず、クエリパラメータで署名付きのダウンロードアドレスを渡せます |
| インライン JSON | `'{"bridge":{"bind":"0.0.0.0:14028"}}'` | JSON オブジェクトを直接渡します |
| base64 | `base64:eyJicmlkZ2Ui...` | パディングなしの標準 base64 または base64url |

1 つの設定は最大 4 MiB までです。`--config-token-env NAME` はプライベートなダウンロードアドレス用の
Bearer トークンを環境変数から読み込みます。この資格情報がログに書き出されることはありません。URL、
トークン、インラインの本文が設定の取得元名として表示されることもありません。

遠隔構成においても、証明書、Geo データ、ファイルルートのパスは**エージェントのマシン上**のパスを
指します。

## エージェントの起動 URI

`felucca-agent` は短い URI も受け付けます。そのままでは接続のみで、ローカルのトンネルには TOML/JSON が必要です。`remote=1` を付けるとトンネルとアクセス範囲をすべてサーバーが配布するため、コマンド 1 つで済みます。

```sh
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&transport=tls'
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&fingerprint=SHA256_HEX&remote=1'
```

| フィールド | 必須 | 詳細 |
| --- | --- | --- |
| ポート | 必須 | 0 以外である必要があります |
| `vkey` | 必須 | 共有鍵。値はパーセントエンコードする必要があります |
| `key_id` | 任意 | 省略した場合は既定の識別情報を使います |
| `transport` | 任意 | `tcp`/`tls`/`ws`/`wss`/`quic`/`kcp` |
| `fingerprint` | 任意 | 証明書の SHA-256 固定検証を有効にします |
| `ws_path` | 任意 | WebSocket のパス |
| `remote` | いいえ | `1` でサービスとアクセス範囲をすべてサーバーに任せます。`remote.enabled` と `remote.console_scope` を有効にし、サーバーが範囲を送るまではローカルアドレスのみ許可します |

未知のフィールドと繰り返されたフィールドはエラーになります。

## エージェントの複数インスタンス

`--config` は繰り返せます。1 プロセスで最大 32 インスタンスです。

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://cfg.example/edge.json' --config-token-env CONFIG_TOKEN
```

各インスタンスは接続、バックオフ、ルート、プローブ、接続数上限をそれぞれ独自に保持します。設定は
すべて先に検証と初期化が行われ、その接続が始まるのはその後です。`server` と `key_id` の組み合わせが
重複するとエラーになります。1 つのサーバーに到達できなくても他のインスタンスは動作し続け、終了の
シグナルを受けるとすべてのインスタンスを閉じます。

## 環境変数の展開

既定では無効です。`--expand-env`（または `FELUCCA_EXPAND_ENV=true`）を設定すると、解決された文字列の
値が展開されます。

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}` は変数の存在を要求します。`${VAR:-default}` は変数が存在しないか空のときに既定値を使います。
`$$` はリテラルの `$` です。展開は 1 回だけ実行され、フィールド名には触れず、文字列を数値に変換する
こともありません。展開結果が 4 MiB を超える場合は拒否されます。

## 実行時の設定スナップショット

```sh
felucca-server --config bootstrap.toml --runtime-config conf/runtime.json
```

パスは `FELUCCA_RUNTIME_CONFIG` でも設定できます。最初の起動に成功したあと、正規化された設定が
アトミックに保存され、以降の起動では先にスナップショットを読み込みます。`--check` と `--fingerprint`
も同じスナップショットを読み込みます。ファイルが壊れている場合、起動は失敗し起動ファイルに
フォールバックすることはありません。`[state].path` のトラフィックファイルと同じパスに設定しては
**いけません**。

スナップショットは最大 16 MiB で、実際のシークレットを含みます。Unix では新しいファイルの
パーミッションは 0600、新しいディレクトリは 0700 になり、ファイルロックによって別のデーモンが
同時に使うことを防ぎます。

管理 UI から設定を変更する場合も同じしくみを使います。
[管理 UI と集中構成](management.md) を参照してください。

## 検証とログ

```sh
felucca-server --config conf/server.toml --check
felucca-agent  --config conf/agent.toml  --check
```

`--check` は TOML/JSON の構造、フィールドの制約、ポート、ACL、Geo データを検証します。バックエンドへ
の接続、公開 DNS の検証、証明書ファイルの中身の検査は**行いません**。証明書の中身とバインドの衝突は
起動時に検査されます。

エージェントのログ関連フラグは以下のとおりです。

```sh
felucca-agent -c agent.toml --log-level info \
  --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

`RUST_LOG` は両側でログレベルを上書きします。サーバーのログは設定された `[log]` テーブルに書き込まれ、
変更するには再起動が必要です。

## 環境変数

| 変数 | 効果 |
| --- | --- |
| `FELUCCA_CONFIG` | `--config` と同じ（エージェントでは繰り返し指定可能） |
| `FELUCCA_RUNTIME_CONFIG` | サーバーの `--runtime-config` と同じ |
| `FELUCCA_EXPAND_ENV` | `--expand-env` と同じ |
| `FELUCCA_CONFIG_TOKEN_ENV` | 遠隔構成の Bearer トークンを保持する変数の名前 |
| `RUST_LOG` | ログのフィルタレベルを上書きします |

## 次の手順

- [felucca-server モジュール](server.md)
- [felucca-agent モジュール](agent.md)
- [システムサービス、更新、ログ、監視](operations.md)
