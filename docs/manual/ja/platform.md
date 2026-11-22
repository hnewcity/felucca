# 外部プラットフォーム連携

`[node]` を使うと、外部のコントロールプレーンがこのノードを管理できます。外部からは専用の
トークンで管理 API を呼び出せるほか、ノード側からダイヤルアウトして逆方向の WebSocket で
リクエストを受け取ることもできます。設定の変更は署名付きのコールバックとしてプッシュでき、
外部のトラフィックを取り込むこともできます。これは Felucca 独自の制御プロトコルであり、
nps とはワイヤー互換ではありません。

## 有効化とスコープ

```toml
[node]
id = "edge-a"
state_path = "state/node-events.json"
changes_window = 1024
usage_interval = "10s"

[node.platform.panel]
enabled = true
token = "replace-with-a-random-platform-token"
scope = "account"
user = "office"
mode = "direct"
```

| フィールド | 値と意味 |
| --- | --- |
| `scope` | `full`（完全な管理。`user` は同時に設定できません）または `account`（存在する `user` を設定する必要があり、そのユーザーのクライアントとトンネルだけを管理でき、このテナントのコールバック状態を読めます） |
| `mode` | `direct`（このトークンは公開の管理 HTTP API にアクセスできます）、`reverse`（逆方向の WS RPC のみ。このトークンを管理 HTTP API に直接使うと 401 を返します）、`dual`（双方向） |
| `enabled = false` | 両方の入口が動作を停止し、既存の逆方向接続は閉じられます |

トークンは 16 バイト以上である必要があり、プラットフォームごとに別のものを使う必要があります。
HTTP では `Authorization: Bearer <token>` を送ります。プラットフォームの識別情報はローカルの
管理者アカウントとは別のものです。トークン、そのスコープ、接続の設定を変更すると、逆方向の接続は
置き換えられます。古い接続上のリクエストは、現在の設定で引き続き確認されます。

公開アドレスは HTTPS/WSS を使う必要があります。平文の HTTP/WS が許されるのは、リテラルな
ループバックアドレスのみです。WSS は既定で公開 CA を検証します。代わりに
`trust = { mode = "pinned", fingerprint = "..." }` を使うこともできます。信頼しない設定は
許可されません。HTTP のコールバックは公開 CA を検証し、リダイレクトはたどりません。

## 直接の管理 API

書き込みは JSON、`If-Match: "<configuration revision>"`、一意な `Idempotency-Key` を使います。
ブラウザの Cookie 認証では、さらに Origin と CSRF トークンが必要です。

| エンドポイント | 動作 |
| --- | --- |
| `GET /api/node/status`、`/api/node/registration` | ノード ID、起動 ID、永続化されたエポック、バージョン、イベントのウィンドウ、参照可能なプラットフォームの接続状態 |
| `GET /api/node/usage` | 現在のスコープ内でのクライアントのオンライン状態、接続数、双方向の累計バイト数 |
| `GET /api/node/changes?after=<epoch>:<sequence>&limit=100` | 永続化された変更のウィンドウ。`limit` は 1–512 です |
| `GET /api/node/callbacks?target=...&offset=0&limit=100` | 参照可能なコールバックのキュー、配送の統計、ページ付けされたエントリ |
| `POST /api/node/callbacks/replay` | `{"ids":["delivery-uuid"]}`。失敗状態を解除して再度配送します |
| `POST /api/node/callbacks/delete` | 指定された参照可能なエントリだけを削除します |
| `POST /api/node/kick` | `{"ids":["client-name"]}`。クライアントを永続的に無効化して切断し、すぐの再接続を阻止します |
| `POST /api/node/sync` | 管理者がローカルに永続化された設定を再有効化し、古い実行時の状態、キャッシュ、接続を消去します |
| `POST /api/node/traffic` | 外部のトラフィックの差分を取り込み、クライアントと所有ユーザーのカウンタをアトミックに累積します |
| `GET/PUT/DELETE /api/platforms/<name>` | プラットフォームの管理 |
| `GET/PUT/DELETE /api/webhooks/<name>` | 永続化された Webhook の作成、変更、削除 |

設定の一括変更、完全なエクスポート、リストアは `/api/batch` と `GET/PUT /api/config` を
再利用します。同期と蹴出しは同じ設定トランザクションと冪等性の履歴を再利用し、設定のリビジョンを
増やします。コールバックの操作とトラフィックの取り込みは、設定のリビジョンを変更しません。

### トラフィックの取り込み

`state.path` を設定する必要があります。

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

1 つのリクエストが運べるのは最大 128 件です。バイト数は符号なし整数である必要があり、同じ
クライアントの複数の項目は先にマージされます。すべての権限確認を通過するまで何も書き込まれず、
いずれかのエントリが範囲外または無効な場合はバッチ全体が拒否されます。
**このノードのデータプレーンですでに計測されていない**トラフィックを取り込んでください。
ノードがすでに計測したトラフィックを再取り込みすると 2 重にカウントされます。最初の取り込みで
スナップショットはバージョン 2 に更新され、それ以前のビルドでは読めなくなります。

## 逆方向の WSS プロトコル v1

`[node]` を有効にすると、`GET /api/ws` は HTTP/1.1 の WebSocket アップグレードを受け付けます。
ネイティブのクライアントは Bearer トークンを使います。ブラウザは同一オリジンのログイン Cookie
を使えるほか、サブプロトコル
`["felucca-node-v1", "felucca-token.<unpadded base64url token>"]` でトークンを渡せます。
サーバーが選ぶのは `felucca-node-v1` だけで、URL パラメータによる認証は受け付けません。同時に
保持できる直接の管理用 WebSocket は最大 16 本です。

ノードは `reverse_url` にダイヤルし、ハンドシェイクではプラットフォームの Bearer トークンと
`felucca-node-v1` サブプロトコルを運びます。プラットフォームは応答でそのサブプロトコルを
選ぶ必要があります。アプリケーションのメッセージはすべて JSON テキストです。

ノードは最初に `hello`（node、boot、epoch、cursor、max_message、max_inflight、heartbeat_ms）を
送り、その後でプラットフォームがリクエストを送ります。

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

レスポンスは
`{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}` のようになり
ます。RPC は通常の管理ハンドラの認証、テナントによる絞り込み、CAS、冪等性のふるまいを
再利用します。If-Match、Idempotency-Key、Accept、X-Felucca-CSRF は渡せますが、Host と
Authorization は上書きできません。ログイン、ログアウト、SSE、CPU のサンプリング、WebSocket
のアップグレード自体は RPC を通りません。

| 制限 | 値 |
| --- | --- |
| メッセージの最大サイズ | 2 MiB |
| 1 接続あたりの同時 RPC | 2 |
| ノード全体の同時 RPC | 16。超過したリクエストは 429 を返します |
| 進行中のリクエスト ID の重複 | 接続を閉じます |
| リクエストのタイムアウト | 504。書き込みは同じ冪等性キーで再試行してください |

どちらの側も `{"type":"ping","nonce":1}` を送れます。ノードは未応答の ping を上書きせず、
2 ハートビート分を超えても対応する pong が届かない場合は切断して再接続し、1–30 秒でバックオフ
します。設定が変更されるとイベントが送られ、サブスクリプションが遅れると再同期が送られます。
`usage_interval` を設定すると、ノードは定期的にテナントで絞り込んだ使用状況のメッセージを
送ります。

## 署名付きコールバック

```toml
[node.webhook.events]
user = "office"

[node.webhook.events.delivery]
enabled = true
url = "https://panel.example/felucca/events"
signing_key = "replace-with-a-random-callback-signing-key"
timeout = "5s"
retry_max = 2
retry_backoff = "2s"
queue_max = 100
```

リクエストのヘッダーは以下のとおりです。

| ヘッダー | 内容 |
| --- | --- |
| `X-Felucca-Event-Id` | 配送の UUID。自動の再試行と手動の再配送を通じて変わりません |
| `X-Felucca-Timestamp` | このリクエストの Unix 時刻（ミリ秒） |
| `X-Felucca-Signature` | `v1=` に小文字の 16 進数の HMAC-SHA256 を続けたもの |

署名の入力は、UTF-8 のプレフィックスに**生のリクエストボディのバイト列**を直接つなげたものです。

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

受け取る側は署名を検証し、時間のウィンドウを確認し、イベント ID で重複を除去する必要があります。
成功と見なされるのは 2xx だけです。`retry_max` は最初の配送に加えた追加の試行回数で、
`retry_backoff` は固定の間隔です。配送は**少なくとも 1 回**行われます。成功の確認応答が
ディスクに書き込まれる前にノードが終了すると、再起動後に同じ ID が再度配送されることがあり
ます。コールバックが運ぶのは変更のメタデータだけであり、設定のシークレットは含みません。
イベントを受け取ったら、必要なリソースは現在認可されている API で取得してください。

`queue_max` は対象ごとの保留中と失敗したエントリの永続化された件数を制限します。上限を超えると
古いエントリから破棄され、破棄としてカウントされます。`0` にすると最終的な失敗エントリは
保持しません。ノード全体の上限は 4096 件です。サブスクリプションを無効化しても既存のキューは
残ります。サブスクリプションを削除する、または受信 URL、署名鍵、フィルタ、テンプレートを
変更すると、古いキューは消去されます。

イベントのウィンドウには `node.started`、`config.changed`、`config.resync`、およびクライアント、
ユーザー、集中管理トンネル、ドメインの作成、更新、削除のメタデータが記録されます。クエリは
テナントで絞り込まれます。期限切れ、将来すぎる、エポックが一致しないカーソルには
410/reset_required を返します。その場合はまず現在の設定を取得し、次に返された位置から
サブスクリプションを再開してください。この上限付きのウィンドウは設定の差分全体を保持するもの
ではなく、長期の監査ログでもありません。

## リアルタイムのサブスクリプションとテンプレート

直接と逆方向のどちらの WebSocket 接続でも、**WS RPC 専用**の以下のパスを使えます。
`GET /api/realtime/subscriptions`、および
`GET/PUT/DELETE /api/realtime/subscriptions/<name>`。サブスクリプションは永続化された設定を
変更せず、If-Match も要求しません。1 接続あたり最大 32 個、それぞれ最大 32 KiB の
サブスクリプションを作成でき、接続が切れるとすべて解放されます。

```json
{
  "enabled": true,
  "selector": {"event_names":["client.updated"],"clients":["office"]},
  "content": {
    "body_template": {"message":"node {{event.client}} updated","event":"{{event}}"},
    "header_templates": {"x-event-name":"{{event.kind}}"}
  }
}
```

テンプレートは JSON の値であり、スクリプトは実行しません。`{{node}}`、`{{id}}`、`{{epoch}}`、
`{{event}}`、`{{event.kind}}`、リソース、アクション、ユーザー、クライアント、トンネル、
ホスト、対象、シーケンス、リビジョン、timestamp_ms、トラフィックに対応します。文字列全体が
1 つのプレースホルダだけの場合は、元の JSON の型が保たれます。テンプレートは認証、Cookie、
HTTP の長さと接続に関するヘッダー、`X-Felucca-*` の署名ヘッダーを上書きできません。

## トラフィックの通知

`traffic_report_interval`（100ms–1h）と `traffic_report_step_bytes` は、取り込んだトラフィックの
通知を有効にします。`client.traffic.reported` の通知は、トラフィックのクォータを持つクライアント
への最初の取り込み時と、その後は時間またはバイトのしき値に達したときに送られます。この通知は
`ephemeral: true` を運ぶため、変更の履歴、失敗のキュー、再試行には入りません。受信が遅れると
取りこぼすことがあります。完全なデータが必要な場合は `/api/node/usage` を読んでください。

## 確認チェックリスト

1. `GET /api/node/status` がノード ID、エポック、プラットフォームの接続状態を返します。
2. 逆方向の WSS 接続で `hello` → `request` → `response` の往復が完了します。
3. コールバックの受け手が署名の検証に成功し、同じイベント ID を再配送しても業務上の重複した
   効果が発生しません。
4. 蹴出し（`kick`）の後、クライアントが切断され、すぐには再接続できません。
5. トラフィックを取り込んだ後、`/api/node/usage` の累計バイト数が直ちに変化します。

## 次の手順

- [管理 UI と集中構成](management.md)
- [felucca-server モジュール](server.md)
- サンプル: [node-server.toml](../../../examples/node-server.toml)
