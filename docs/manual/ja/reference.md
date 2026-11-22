# リファレンス

コマンド、フィールド、エンドポイント、サンプルの一覧、用語集。各ガイドは*理由*を説明し、
このページは*対象*を列挙します。

## CLI

両方のプログラムで共有される引数です。

| 引数 | 環境変数 | 説明 |
| --- | --- | --- |
| `--build-info` | — | プログラム名、バージョン、プロトコル、ビルド対象を表示します |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | TOML/JSON ファイル、HTTP(S) URL、インライン JSON、base64、または `felucca://` URI。エージェントでは繰り返し指定できます |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | `${VAR}` / `${VAR:-default}` を展開します |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | 遠隔構成の Bearer トークンを保持する変数 |
| `--check` | — | 検証のみを行い、バインドも接続もせずに終了します |
| `service` サブコマンド | — | システムサービスの管理 |
| `update` サブコマンド | — | スタンドアロンのバイナリを検証してインストールします |

`felucca-server` が追加する引数です。

| 引数 | 環境変数 | 説明 |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | 実行時の設定スナップショットのパス |
| `--fingerprint` | — | `bridge.tls` の証明書の SHA-256 を表示します |
| `--hash-password` | — | 標準入力からパスワードを 1 行読み取り、PBKDF2 のハッシュを表示します。12 バイト以上必要です |
| `--generate-token` | — | Bearer トークンとその `token_hash` を表示します |

`felucca-agent` が追加する引数です。

| 引数 | デフォルト | 説明 |
| --- | --- | --- |
| `--launch` | — | `--config` のエイリアス |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | 追記し、stderr にも書き出します。親ディレクトリが存在している必要があります |
| `--log-json` | — | 構造化された JSON のイベント |
| `--log-max-bytes <N>` | 10 MiB | `--log-file` が必要です |
| `--log-backups <N>` | 5 | `--log-file` が必要です |
| `--log-rotate-interval <D>` | — | 例えば `24h`。`--log-file` が必要です |

`service` サブコマンドです。

| 引数 | 説明 |
| --- | --- |
| `--name <NAME>` | サービスのインスタンス名 |
| `--scope <user\|system>` | 既定は `user`。`system` は Windows でのみ指定できます |
| `--dry-run` | 実行される操作の表示だけを行います |
| `render` | サービスの定義を生成します。`--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | ライフサイクルの各操作 |
| `--workdir <DIR>` | 既定は設定ファイルがあるディレクトリ |
| `--binary <PATH>` | インストールするプログラムを選びます |
| `-- <ARGS>` | デーモンへの追加引数 |

`update` サブコマンドです。

| 引数 | 説明 |
| --- | --- |
| `--source <PATH_OR_URL>` | スタンドアロンのバイナリ、または tar.gz/zip のアーカイブ |
| `--sha256 <HEX>` | 16 進数 64 桁。信頼の基点です |
| `--check-only` | 置き換えずに検証だけ行います |
| `--expect-version <V>` | 任意指定のバージョンの表明 |
| `--token-env <NAME>` | プライベートなダウンロードの Bearer トークンを保持する変数 |

## 設定の構成

| ファイル | トップレベルの内容 |
| --- | --- |
| server | `allowed_ports`、`max_tunnels`、`max_connections`、`[bridge]`、`[http]`、`[admin]`、`[state]`、`[geo]`、`[ntp]`、`[log]`、`[p2p]`、`[node]`、`[user.*]`、`[client.*]`、`[local.*]`、`[share.*]`、`[managed.*]` |
| agent | `server`、`key_id`、`vkey`、`transport`、`reconnect_min/max`、`dial_timeout`、`max_connections`、`[trust]`、`[network]`、`[mux]`、`[kcp]`、`[ntp]`、`[remote]`、`[geo]`、`[[tunnel]]`、`[[mapping]]`、`[[visitor]]`、`[p2p]`、`[log]`（エージェントはコマンドライン引数を使います） |

`allowed_ports` は最初の TOML のテーブルヘッダーより前に記述する必要があります。すべての
テーブルで `deny_unknown_fields` が有効です。

## トンネルのフィールド早見表

| フィールド | 対象となるモード | 説明 |
| --- | --- | --- |
| `id` | すべて | 安定した ID。トラフィックのスナップショットを有効にしたら一度決めた値を固定します |
| `mode` | すべて | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | プライベート以外のすべて | サーバー側のポート。`http` は HTTP のリスナーと一致させる必要があります |
| `bind` | tcp/udp/proxy/file | IP アドレスのみ |
| `public` | すべて | `false` はプライベート共有を意味します |
| `hosts` | http、file | ドメインのリスト。`*.example.com` に対応します |
| `balance` | 固定のバックエンド | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | 固定のバックエンド | `addr`、`weight`、`tls`、`trust` |
| `health` | 固定のバックエンド | `type`、`path`、`interval`、`timeout`、`failures_to_evict`、`successes_to_restore`、`expect_status` |
| `acl_mode`/`acl_rules` | すべて | 送信元 ACL。off/allow/deny |
| `target_policy` | 固定のバックエンドとフォワードプロキシ | `allow`/`deny`/`ports` |
| `options.auth.users` | フォワードプロキシ、HTTP | アカウントのテーブル |
| `options.http.*` | http | パス、書き換え、ヘッダー、リダイレクト、CORS、エラーページ、tls_mode、backend_http2 |
| `options.payload.*` | すべて | `compress`、`encrypt` |
| `options.limits.*` | すべて | `max_connections`、`rate_limit`、`quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | プロキシの入口 | PROXY ヘッダーを受け付けます |
| `file` | file | `root`、`prefix`、`read_only`、`auto_index`、`auth` |

## 管理 API の概要

認証は Origin と CSRF を伴う Cookie、または `Authorization: Bearer <token>` です。書き込みには
`If-Match: "<revision>"` と一意な `Idempotency-Key` が必要です。リクエストボディの上限は
2 MiB です。

| グループ | パス |
| --- | --- |
| セッション | `/api/login`、`/api/session`、`/api/logout` |
| リソース | `/api/users`、`/api/clients`、`/api/locals`、`/api/shares`、`/api/accounts`。それぞれ `<id>` 付きもあります |
| 集中構成 | `/api/managed/<client>`、`/api/managed/<client>/tunnels/<id>` |
| サーバー上のローカルトンネル | `/api/locals/<group>/tunnels/<id>` |
| 設定全体 | `/api/config`、`/api/config?secrets=1`、`/api/validate`、`/api/batch` |
| リアルタイム | `/api/events`（SSE）、`/metrics`、`/api/metrics`、`/api/profile` |
| プラットフォーム | `/api/node/*`、`/api/platforms/<name>`、`/api/webhooks/<name>`、`/api/ws`、`/api/realtime/subscriptions` |

フィールドの詳細は [管理 UI と集中構成](management.md) と
[外部プラットフォーム連携](platform.md) を参照してください。

## サンプル設定の一覧

| ファイル | 内容 |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | サーバーの最小構成 |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | エージェントの最小構成 |
| [features-server.toml](../../../examples/features-server.toml) | QUIC ブリッジ + HTTP/HTTPS + 証明書 |
| [features-agent.toml](../../../examples/features-agent.toml) | すべてのプロキシモード、負荷分散、ヘルスチェック |
| [shared-server.toml](../../../examples/shared-server.toml) | TCP/UDP のポート共有 + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | エージェント側とポートの一括マッピング |
| [acme-server.toml](../../../examples/acme-server.toml) | ACME のサーバー |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL と TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | 階層的な ACL、クォータ、永続化 |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | ファイルサービス |
| [network-agent.toml](../../../examples/network-agent.toml) | 上流のプロキシ、独自の DNS、NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy と独立したストリームの暗号化 |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | KCP のブリッジ |
| [local-server.toml](../../../examples/local-server.toml) | サーバー上のローカルな対象と入れ子の転送 |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | プライベートな提供側とビジター |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P とフォールバック |
| [admin-server.toml](../../../examples/admin-server.toml) | 管理用サーバー |
| [managed-agent.toml](../../../examples/managed-agent.toml) | 集中管理されるエージェント |
| [node-server.toml](../../../examples/node-server.toml) | 外部プラットフォームとノードの管理 |

設定のパスはプロセスの作業ディレクトリからの相対パスです。26 個のサンプルはすべて設定の検証を
通ります。

## 数値の上限

| 項目 | 上限 |
| --- | --- |
| 1 つの設定 | 4 MiB |
| 実行時の設定スナップショット | 16 MiB |
| エージェントのインスタンス数 | 32 |
| 入れ子ルーティングのホップ数 | 16 |
| バッチの操作数 | 128 件 |
| トラフィックの取り込み | 1 リクエストあたり 128 件 |
| 保持される冪等性の記録 | 256 件（コールバックの操作とトラフィックの取り込みはそれぞれ 256 件） |
| コールバックのキュー | 対象ごとに `queue_max`、ノード全体では 4096 件 |
| 直接の管理用 WebSocket | 16 本 |
| WS RPC の同時実行数 | 1 接続あたり 2 / ノード全体では 16 |
| リアルタイムのサブスクリプション | 1 接続あたり 32 件、1 件あたり 32 KiB |
| HTTP のバックエンド接続プール | オリジンごとにアイドル 8 本、アイドル 30 秒、外側のマップは 256 エントリ |
| HTTP/3 | 1 接続あたり 128 ストリーム、ストリームごとに 1 MiB / 接続ウィンドウ 16 MiB、ヘッダー 32 KiB |
| UDP | 送信元ごとに 65507 バイト。トンネルごとに 256 の関連づけ |
| SOCKS5 の UDP | セッションごとに 64 宛先、それぞれ 8 パケットをキューイング |
| エラーページ | 400..599、1 ページあたり 64 KiB |
| HTTP のドメイン一致 | 完全一致するドメインがワイルドカードより優先されます |

## 環境とビルド

| 項目 | 値 |
| --- | --- |
| 最小の Rust | 1.88 |
| 検証環境 | macOS Intel、Rust/Cargo 1.98.1 |
| ワークスペースのテスト | 581 成功 / 0 失敗 / 0 無視（2026-09-17） |
| バイナリ | `target/release/felucca-server`、`target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

同じターゲットディレクトリに対して Cargo のコマンドは一度に 1 つだけ実行してください。ソース
コードと埋め込みページを編集する前に、ビルド、テスト、rustdoc を終わらせてください。テストには
ローカルの TCP/UDP ポートを待ち受ける権限が必要です。

## 用語集

| 用語 | 意味 |
| --- | --- |
| エージェント（agent） | プライベートネットワーク内で動き、外向きに接続する `felucca-agent` のプロセス |
| ブリッジ（bridge） | サーバーとエージェントの間の、認証された長期間の接続 |
| クライアント / `[client.<id>]` | 1 つのエージェントに対するサーバー側の識別情報のテーブル。`key_id` がテーブル名です |
| key_id | サーバー上でのエージェントの名前 |
| vkey | エージェントとサーバーが共有するブリッジのシークレット |
| transport | ブリッジの通信方式。tcp/tls/ws/wss/quic/kcp |
| trust | エージェントがサーバー証明書をどう検証するか。webpki/pinned/insecure |
| トンネル（tunnel） | 公開入口からバックエンドの集合へのマッピング |
| バックエンド（backend） | エージェントが実際に接続する対象 |
| マッピング（mapping） | ポートを一対一で一括に対応付けること |
| ローカルグループ（local group） | エージェントを必要としない、サーバー自身のサービスのグループ |
| プライベート共有（share） | サーバーがあるゲストに 1 つのトンネルの利用を認可すること |
| ビジター（visitor） | 共有シークレットを保持し、ローカルで待ち受けるエージェント |
| 提供側（provider） | プライベート共有または P2P の構成でサービスを提供する側 |
| 集中管理（managed） | サーバーがエージェントへプッシュする設定 |
| mux | TCP 類似の通信方式の上で動く上限付きの多重化 |
| FEC | KCP の前方誤り訂正。既定ではデータ 10 に対してパリティ 3 |
| quota / rate_limit | トラフィックのクォータと 1 秒あたりのバイト数のレート上限 |
| スナップショット（snapshot） | トラフィックまたは実行時の設定の 1 回のアトミックな永続化 |

## 関連ドキュメント

- [アーキテクチャ](../../architecture.md) — crate の責務の境界
- [設定とデプロイ](../../configuration.md) — フィールド単位の詳細
- [P2P](../../p2p.md)、[プライベートトンネルと共有アクセス](../../visitors.md),
  [管理と集中構成](../../management.md),
  [ノードと外部プラットフォーム](../../node.md)、[運用と配布](../../operations.md),
  [透過的な TCP](../../transparent.md)
