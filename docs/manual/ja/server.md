# felucca-server モジュール

このページでは `felucca-server` のデプロイと設定を取り上げます。ブリッジのリスナー、クライアント
テーブル、HTTP の受信、ポートの認可、管理用のリスナー、サーバー上のローカルサービス、そして
外部プラットフォームの設定項目です。

## 最小構成

```toml
[bridge]
bind = "0.0.0.0:14028"
ws_path = "/ws"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }

[client.agent-1]
vkey = "change-me-to-a-long-random-string"

[log]
level = "info"
```

`bridge.tls` を省略し、実行時設定の保存を有効にすると（`--init` や管理画面を含む）、サーバーはスナップショットの隣の `<snapshot>.identity.json` に識別情報を保存します。再起動やバックアップ時も保持してください。保存を使わない場合は一時的な識別情報になります。`--fingerprint` は指定された PEM を読みます。自動生成された指紋は管理画面と起動ログで確認できます。

```sh
target/release/felucca-server --config conf/server.toml --check
target/release/felucca-server --config conf/server.toml
```

## ブリッジのリスナー

| フィールド | デフォルト | 説明 |
| --- | --- | --- |
| `bridge.bind` | `0.0.0.0:14028` | TCP、TLS、WS、WSS が 1 つのソケットを共有し、ハンドシェイクの特徴で区別されます |
| `bridge.quic_bind` | なし | 独立した UDP アドレス。TCP と同じポート番号を使えます |
| `bridge.kcp_bind` | なし | KCP の UDP リスナー。QUIC/HTTP3 と UDP ポートを共有できません |
| `bridge.ws_path` | `/ws` | WS/WSS のパス。両端で一致させる必要があり、他のパスでのアップグレードは拒否されます |
| `bridge.tls` | なし | `{ cert, key }`。省略すると一時的な証明書を生成します |
| `bridge.cert_reload_interval` | `30s` | PEM の再読み込み間隔。再読み込みは既存の接続を中断しません |
| `bridge.proxy_accept` | off | 信頼できるロードバランサーからの PROXY ヘッダーを受け付けます |
| `[bridge.mux]` | 以下を参照 | mux のウィンドウと予算 |
| `[bridge.kcp]` | 以下を参照 | KCP のパラメータ |

`[bridge.mux]` は `initial_stream_window`、`max_stream_window`、`max_session_window`、
`max_send_queue`、`max_control_queue`、`max_streams`、`accept_backlog`、`open_timeout` を
公開します。エージェント側にも同じフィールドを持つ `[mux]` セクションがあります。ネイティブな
QUIC ストリームはこれらの設定を使いません。

`[bridge.kcp]` は `mtu`、`send_window`、`receive_window`、`interval_ms`、`nodelay`、
`fast_resend`、`congestion_control`、`session_timeout`、`data_shards`、`parity_shards`、
`max_sessions` を受け付けます。既定の FEC 設定はデータ 10 に対してパリティ 3 で、グループ内の
13 個のパケットのうち 3 個まで失っても復元できます。両端は同じ MTU とシャード数を使う必要が
あります。

## クライアントテーブル

各エージェントは 1 つの `[client.<key_id>]` テーブルを持ちます。`key_id` はサーバー上での
エージェントの名前であり、エージェント自身の `key_id` はそのテーブル名と一致する必要があります。

| フィールド | 説明 |
| --- | --- |
| `vkey` | 必須。相互 HMAC 認証に使う共有シークレット。その長さとランダム性は利用者の責任です |
| `remark` | メモ。管理画面に表示されます |
| `user` | `[user.<name>]` テーブルに関連づけます。階層的なクォータ、ACL、テナントの権限に使います |
| `enabled` | `false` にすると、このクライアントを永続的に無効化して切断します |
| `allow_config_push` | このエージェントがトンネルを登録してよいかどうか。`true` にすると `[[managed.<id>.tunnel]]` の設定をプッシュできます |
| `max_tunnels` | このエージェントのトンネル数上限 |
| `max_connections` | このエージェントの接続数上限 |
| `rate_limit` | すべてのトンネルと双方向で共有される 1 秒あたりのバイト数の上限 |
| `acl_mode` / `acl_rules` | このクライアントの公開入口へのアクセスに対する送信元 ACL |
| `quota` | `{ flow_limit, expires_at }`。単位はバイトと Unix 秒です |

同じ `key_id` での新しい接続は古い接続を置き換えます。古いリスナーは停止して解放されたあとで
ポートが再度バインドされます。トラフィックのカウンタと帯域のリミッタは、同じプロセス内では
再接続をまたいで保持されます。

## ポートの認可

`allowed_ports` はトップレベルのフィールドで、**最初の TOML のテーブルヘッダーより前に記述する
必要があります**。

```toml
allowed_ports = [8080, { from = 9000, to = 9100 }]
```

空のリストにすると、1024 より上のすべての TCP/UDP ポートを許可します。`max_tunnels` と
`max_connections` はトップレベルの全体上限で、エージェントごとにカウントされます。

## HTTP の受信

```toml
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"      # set to "" to disable HTTPS
http3_bind = "0.0.0.0:8443"    # optional, HTTP/3 UDP ingress
forwarded_headers = true
response_timeout = "100s"
cert_reload_interval = "30s"

[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"

[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

詳しい使い方は [HTTP/HTTPS リバースプロキシ](http-services.md) と
[証明書、SNI、ACME](certificates.md) を参照してください。

## その他のトップレベルのテーブル

| テーブル | 用途 | 参照先 |
| --- | --- | --- |
| `[admin]` / `[admin.account.<name>]` | 管理用のリスナー、アカウント、ロール | [管理 UI と集中構成](management.md) |
| `[state]` | トラフィックのスナップショットの永続化 | [アクセス制御、クォータ、ストリーム符号化](security.md) |
| `[user.<name>]` / `[user.<name>.limits]` | ユーザーごとのクォータと集約 | [アクセス制御、クォータ、ストリーム符号化](security.md) |
| `[local.<name>]` | サーバー上のローカルサービスのグループ | [サーバー上のローカルな対象と入れ子ルート](local-services.md) |
| `[share.<name>]` | プライベート共有の認可 | [プライベート共有とビジター](private-shares.md) |
| `[managed.<client>]` | 集中構成のプッシュ | [管理 UI と集中構成](management.md) |
| `[node]` / `[node.platform.<name>]` / `[node.webhook.<name>]` | 外部プラットフォーム | [外部プラットフォーム連携](platform.md) |
| `[p2p]` | P2P の観測用ポート | [P2P 直接接続とリレーへのフォールバック](p2p.md) |
| `[geo]` | GeoIP/GeoSite のデータファイル | [アクセス制御、クォータ、ストリーム符号化](security.md) |
| `[ntp]` | ブリッジ認証の時刻を較正します | [felucca-agent モジュール](agent.md) |
| `[log]` / `[log.rotation]` | ログとローテーション | [システムサービス、更新、ログ、監視](operations.md) |
| `allowed_ports` | ポートの認可 | このページ |

## 便利なコマンド

| コマンド | 用途 |
| --- | --- |
| `--check` | 設定を検証し、ポートをバインドせずに終了します |
| `--fingerprint` | `bridge.tls` の証明書の SHA-256 を表示します。固定の PEM が設定されている必要があります |
| `--hash-password` | 標準入力からパスワードを 1 行読み取り、管理者のパスワードのハッシュを表示します。パスワードは 12 バイト以上必要です |
| `--generate-token` | 管理者の Bearer トークンとその保存用のハッシュを表示します |
| `--runtime-config PATH` | 実行時の設定スナップショットを有効にします |
| `--build-info` | プログラム名、バージョン、プロトコル、ビルド対象を表示します |

## ライフサイクル

- `SIGINT` / `SIGTERM` はリスナーと現在の接続を閉じてから終了します。
- 起動時のファイルへの変更にはプロセスの再起動が必要です。管理用のリスナーを通じて行った変更は
  実行時の設定スナップショットのトランザクションを通り、転送中の接続は中断されますが、エージェント
  はその後で再接続します。
- 管理用のバインド、証明書ファイルのパス、公開オリジン、状態のパスの変更には**プロセスの再起動が
  必要です**。
- トラフィックのカウンタはエージェントの再接続をまたいで保持されます。サーバーを再起動しても
  保持されるのは、`[state]` が設定されている場合だけです。

## 次の手順

- [felucca-agent モジュール](agent.md) — 内部のサービスを公開する
- [トンネルとポートマッピング](tunnels.md)
- [HTTP/HTTPS リバースプロキシ](http-services.md)
- [システムサービス、更新、ログ、監視](operations.md) — systemd / launchd / Windows サービス
