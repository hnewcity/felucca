# felucca-agent モジュール

このページでは `felucca-agent` のデプロイと設定を取り上げます。サーバーへの接続、証明書の信頼、
再接続、エグレスネットワーク、上流のプロキシ、ログ、そして複数インスタンスの運用です。

## 最小構成

```toml
server = "bridge.example.com:14028"
key_id = "agent-1"
vkey = "change-me-to-a-long-random-string"
transport = "tls"          # tcp | tls | ws | wss | quic | kcp

[trust]
mode = "pinned"
fingerprint = "0000...0000"   # from the server's --fingerprint

[[tunnel]]
mode = "tcp"
port = 8080
[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`server` にはホスト名または IP アドレスとポートを指定します。`key_id` はサーバー側の
`[client.<key_id>]` というテーブル名と一致している必要があります。

## 通信方式と信頼

`transport` に指定できるのは `tcp`、`tls`、`ws`、`wss`、`quic`、`kcp` です。公開網のブリッジでは
TLS/WSS/QUIC を推奨します。`tcp` と `ws` は相手の認証は行いますが通信内容を暗号化しないため、
平文の通信方式であり、それを理解したうえで選ぶ必要があります。

`[trust]` はサーバー証明書の検証方法を制御します。

| `mode` | 動作 |
| --- | --- |
| `webpki` | 公開 CA に対する通常の検証。`fingerprint` は設定しません |
| `pinned` | 証明書全体の SHA-256 を固定します。ハンドシェイクの署名は検証しますが、ドメイン名と有効期間は検証しません。証明書が入れ替わったらフィンガープリントを更新してください |
| `insecure` | 一時的な診断専用です |

`bridge.tls` に PEM を固定設定したあとは、`felucca-server --fingerprint` でフィンガープリントを
取得します。

## 接続と再接続

| フィールド | デフォルト | 説明 |
| --- | --- | --- |
| `reconnect_min` | — | バックオフの下限 |
| `reconnect_max` | — | バックオフの上限 |
| `dial_timeout` | — | 1 回のダイヤルあたりのタイムアウト |
| `max_connections` | — | このエージェントの接続数上限 |

切断後、エージェントは自動的に再接続し、サーバーがリスナーとセッションの状態を再登録します。
トラフィックのカウンタは再接続をまたいで保持されます。

## エグレスネットワーク

```toml
[network]
local_ip = "192.0.2.10"
forward_local_ip = true
dns = ["192.0.2.53:53"]

[network.proxy]
kind = "socks5"           # http (CONNECT) | socks5
server = "192.0.2.20:1080"
remote_dns = true
[network.proxy.auth]
username = "edge"
password = "replace-with-proxy-password"
```

| フィールド | 説明 |
| --- | --- |
| `local_ip` | ブリッジのエグレス IP をバインドします。このマシンが実際に保有しているアドレスである必要があり、インターフェース名は指定できません |
| `forward_local_ip` | バックエンドへの接続とヘルスプローブもこのアドレスを使います。デフォルトではブリッジだけがこのアドレスをバインドします |
| `dns` | `IP:port` のリテラルなリストです。空の場合はシステムのリゾルバを使います。TTL でキャッシュし、A/AAAA に対応し、UDP で打ち切られた後は TCP にフォールバックします |
| `network.proxy` | 上流のプロキシです。ブリッジの TCP/TLS/WS/WSS だけを運び、バックエンドへのリクエストや NTP の UDP は転送しません |

対象 ACL の検査は**名前解決で実際に返された IP アドレス**に対して行われ、エージェントはその検査済み
アドレスに接続します。そのため、2 回目の名前解決によって検査が回避されることはありません。`local_ip` と
DNS のアドレスは同じアドレスファミリーである必要があります。QUIC/KCP のブリッジは TCP の上流プロキシと
併用できません。

## 時刻の同期

サーバーとエージェントはどちらも `[ntp]` セクションを受け付け、ブリッジ認証の時刻を較正できます。

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

デフォルトでは時刻サービスへの問い合わせを行いません。`interval` の範囲は 1 秒から 24 時間です。
`timeout` は正の値で 30 秒以下、かつ `interval` より小さくなければなりません。`max_offset` は正の値で
7 日以下である必要があります。最初の問い合わせが失敗した場合はシステム時刻を使います。較正はシステム
クロックを変更しないため、TLS 証明書の有効期間とクォータの期限はシステム時刻に従います。NTP の送信元は
利用者が信頼する必要があります。平文の SNTP は時刻の送信元を認証しません。

エージェントの NTP 問い合わせは独自の DNS とエグレス設定を使います。サーバーはシステムの DNS と
デフォルトのエグレスを使います。

## 集中構成の受信

```toml
[remote]
enabled = true

[remote.target_policy]
allow = ["10.0.0.0/8"]
ports = [80, 443]

[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

`remote.enabled` は明示的に有効にする必要があります。主接続は検証済み証明書による TLS/WSS/QUIC である
必要があり、平文の接続では構成を要求できません。`remote.target_policy` はエージェント自身のマシン上の
硬い境界であり、プッシュされた各トンネルと交わられます。`remote.file_root` は名前付きのディレクトリ
機能を事前に宣言するもので、サーバーがプッシュできるのはそれらの機能名の範囲内のパスだけです。
読み取り専用のディレクトリを遠隔構成で書き込み可能に昇格させることはできません。
[管理 UI と集中構成](management.md) も参照してください。

`remote.console_scope = true` を設定すると、サーバーの `[managed.<client-id>]` にある `target_policy` がローカルの `remote.target_policy` を置き換え、コンソールのデバイス編集から範囲を変更できます。サーバー側に指定がなければローカルの範囲を使います。有効にしていない場合、`target_policy` を含む配布構成は拒否されます。ディレクトリ機能は常にローカルの設定だけで決まります。コンソールが生成する接続パッケージではこのオプションが有効です。

## ログ

```sh
felucca-agent -c agent.toml --log-level debug --log-file /data/agent.log --log-json
```

`--log-file` を設定すると、既定で 10 MiB のファイルを 5 件まで保管するローテーションが有効になります。
ログファイルには追記され、stderr にも引き続き出力されます。親ディレクトリはあらかじめ存在している
必要があります。`--log-max-bytes`、`--log-backups`、`--log-rotate-interval` には `--log-file` が
必要です。生のシークレットがログに出力されることはありません。

## 複数インスタンス

```sh
felucca-agent --config office.toml --config home.json --check
felucca-agent --config office.toml --config home.json
```

1 つのプロセスは最大 32 インスタンスまで対応します。[設定の取得元と CLI](configuration.md) を
参照してください。

## P2P とビジターの設定項目

| テーブル | 効果 |
| --- | --- |
| `[p2p] enabled = true` | このエージェントが提供側として直接接続を受け付けられるようになります |
| `[[visitor]]` | 自分のマシン上で待ち受け、他人のプライベート共有へアクセスします |

[P2P 直接接続とリレーへのフォールバック](p2p.md) と
[プライベート共有とビジター](private-shares.md) も参照してください。

## 次の手順

- [トンネルとポートマッピング](tunnels.md)
- [バックエンド、負荷分散、ヘルスチェック](backends.md)
- [システムサービス、更新、ログ、監視](operations.md) — システムサービスとしてインストールする
