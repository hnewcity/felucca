# システムサービス、更新、ログ、監視

このページでは、Felucca をサービス管理下に登録する方法、バイナリを安全に更新する方法、ログの
ローテーション、メトリクス、そして CPU プロファイルを取り上げます。

## システムサービスとしてインストールする

どちらのプログラムも同じサブコマンドを使い、`--name` による複数インスタンスに対応しています。

```sh
felucca-server service --name office render --config /srv/felucca/server.toml
felucca-server service --name office --dry-run install --config /srv/felucca/server.toml
felucca-server service --name office install --config /srv/felucca/server.toml
felucca-server service --name office start
felucca-server service --name office status
felucca-server service --name office restart
felucca-server service --name office stop
felucca-server service --name office uninstall
```

| プラットフォーム | 管理機能 | 既定のスコープ |
| --- | --- | --- |
| Linux | systemd | `user`。`--scope system` はマシン全体 |
| macOS | launchd | `user`。`--scope system` はマシン全体 |
| Windows | SCM | `system` のみ |

`render --platform systemd|launchd|windows` を使うと、シェルでコマンドを組み立てることなく、
対応するすべてのホストの定義を確認できます。`--workdir` の既定値は設定ファイルがある
ディレクトリで、設定内の相対パスはそこを基準に解釈されます。`--binary` はどのプログラムを
インストールするかを選びます。デーモンへの追加引数は `--` の後に置きます。例えば
`-- --log-file /srv/felucca/agent.log` です。

インストールは同名の既存サービスを**上書きしません**。また、サービスを直ちに起動することも
ありません。Linux ではサービスを有効化します。macOS では次回のログイン時または起動時に
ブートストラップされ、明示的に `start` を実行しても起動できます。ブート時に存在する必要がある
ユーザーの systemd サービスでは、デプロイする担当者が lingering を有効にする必要があり、
macOS のユーザーサービスはグラフィカルなログインセッションに依存します。マシンレベルのサービスは
サービス管理機能の既定の識別情報で実行されるため、運用するユーザーはデプロイに合わせて編集して
ください。Windows でのインストール識別情報は `NT AUTHORITY\LocalService` で、デプロイする担当者が
設定、ログ、状態、ファイルルートの各ディレクトリに対する適切な権限を付与する必要があります。

アンインストールしても実行ファイル、設定、ログ、証明書、実行時の状態は残ります。

検証済みの範囲: このリポジトリでは定義の生成、macOS の plist の解析、隔離されたコピーによる更新、
実際の CPU プロファイルをテストしています。Linux arm64 のサーバーイメージを OrbStack で
ビルドして実行し、管理画面へのログイン、監視、ブリッジの TLS、コンテナの再作成をまたいだデータの
永続化も確認しました。デプロイの記録 を参照してください。
システムサービスとしてのインストールおよび Linux/Windows のサービス管理機能は未検証です。

## バイナリの更新

信頼できるリリースチャネルからスタンドアロンの実行ファイルの SHA-256 を取得し、実行中の
プログラムを検証または置き換えます。

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64-hex-digest> --expect-version 0.1.0
felucca-agent  update --source https://releases.example/felucca-agent --sha256 <64-hex-digest>
```

ダイジェストは**信頼の基点**であり、信頼できるリリースのマニフェストから取得する必要があります。
入力はスタンドアロンのバイナリである必要があるため、tar.gz/zip は先に展開してください。HTTPS の
ダウンロードは証明書を検証し、リダイレクトはたどりません。プライベートなダウンロードには
`--token-env VARIABLE_NAME` を使い、資格情報がログに書き出されることはありません。ダウンロードは
512 MiB と 5 分に制限されます。10 秒に制限された `--version` の確認はダイジェストが一致した後に
だけ実行され、サーバー/エージェントの種別と任意指定のバージョンを調べます。失敗時には一時ファイルを
削除し、更新ロックによって同時置き換えを防ぎます。置き換え後も元のファイルのパーミッションは
保たれます。

その後は `service ... restart` を明示的に実行する必要があります。すでに実行中のプロセスは古い
バージョンを使い続けます。読み取り専用のイメージファイルシステムでは、コンテナ内での更新は
機能しません。イメージを置き換えてください。

プロトコルバージョンが変わる場合は、サーバーとエージェントを同時にアップグレードする必要が
あります。パッケージ化スクリプトは実際に `--build-info` を実行し、プロトコルバージョンや
ビルド対象の異なるバイナリを混在させることを拒否します。

## ログ

サーバー側では以下のように設定します。

```toml
[log]
level = "info"
file = "/data/felucca.log"
json = true

[log.rotation]
max_bytes = 10485760
max_files = 5
interval = "24h"
```

出力はファイルと stderr の両方に行われます。ローテーションは完全なイベントの間で行われるため、
サイズのしきい値を超えてちょうど 1 イベント分多く書き込まれることがあります。アーカイブは
`.1` から `.5` まで番号が振られ、`.1` が最新です。`interval` は現在のファイルを開いた時点から
測られ、再起動すると期間がやり直しになります。排他的なロックによって複数のプロセスが同じパスを
ローテーションすることを防ぎ、Unix では新しいログファイルとロックファイルのモードは 0600 に
なります。

**サーバーのログ設定を変更するには再起動が必要で、管理プレーンはこれらの設定のオンライン変更を
拒否します。** エージェントは代わりにコマンドライン引数を使います。
[felucca-agent モジュール](agent.md) を参照してください。

## 監視

`GET /metrics` と `GET /api/metrics` は Prometheus のテキスト形式を返します。管理アカウントの
Bearer トークンまたは有効なセッションを使う必要があります。**匿名で使えるメトリクスの
エンドポイントはありません**。メトリクスはアカウントのユーザーに見える範囲に従います。

対象はサービスの健全性、設定のリビジョン、ノードのオンライン/有効状態、接続数、ノードの累計
トラフィック、トンネルの準備状況、トンネルの累計トラフィックです。トラフィックのカウントは
既存の永続化された MeterStore を再利用するため、`[state]` を有効にしていればサーバーを
再起動しても保持されます。管理 UI の監視ページでは、ホストの CPU、物理メモリ、ファイルシステムの
容量、ネットワークの速度を、Prometheus のテキスト形式やセットアップの説明なしで対話的な折れ線
グラフとして表示します。サンプルは 5 秒ごとに取得され、1 時間メモリ上に保持されます。プロセスを
再起動すると履歴は消えます。5 分、15 分、1 時間のいずれかの時間枠を選べます。ホバー、クリックで
固定、または矢印キーで、正確なサンプル時刻、ローカルのタイムゾーン、値を確認できます。範囲の
スライダーをドラッグするか、ズーム/リセットのボタンで 4 つのグラフをまとめてズームできます。
ライブ更新は選択した絶対時刻の範囲を保ちます。カードには表示されている観測値の最小値、平均、
ピークが表示され、ネットワークの凡例では受信/送信の系列の表示を切り替えられます。ホストの
メトリクスは管理者専用で、それ以外のロールは許可されたリソースのトラフィックと接続のみを
見られます。構造化されたエンドポイントは `GET /api/monitor?window=900` です
（指定できる時間枠は 300、900、3600 秒）。

## CPU プロファイル

`[admin] profiling = true` を設定すると、管理者が `POST /api/profile` を JSON のボディ
`{"seconds":5}` 付きで呼び出せるようになります。または監視ページから CPU のフレームグラフを
ダウンロードできます。Linux と macOS に対応し、それ以外のプラットフォームでは有効化を拒否
します。1 回の実行は 1 秒から 30 秒、49 Hz で行われ、プロセス内で同時に実行できるタスクは 1 つ
だけです。出力は最大 8 MiB です。HTTP リクエストがドロップされもサンプリングのロックは早期に
解放されず、デーモンを停止するとサンプリングはキャンセルされます。アイドル期間中にサンプルが
1 つもない場合は、説明を記した SVG を返します。リリースビルドではプロファイル用に関数シンボルは
保持しますが、デバッグの行情報は落とされます。行単位の参照には、デバッグ情報付きで自分で
ビルドしたものが必要です。

## Docker

```sh
docker build --target server -t felucca-server:local .
docker build --target agent  -t felucca-agent:local .

docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

イメージの UID/GID は既定で 10001 です。設定ディレクトリは読み取り専用でマウントし、データ
ディレクトリにはイメージから書き込めるようにしてください。設定で必要になる追加の TCP/UDP ポートは
明示的に開き、コンテナの内部から到達できるアドレスにバインドしてください。マルチアーキテクチャの
イメージは、Dockerfile のネイティブなターゲットを使って Buildx でビルドできます。
`STOPSIGNAL` は SIGTERM で、プログラムの終了処理と一致します。

### ローカルの OrbStack

OrbStack を起動し、リポジトリのルートから `sh scripts/run-orbstack.sh` を実行します。これは
管理 UI を含むサーバーのみをビルドしてデプロイします。ホストには Docker Compose、Python 3、
OpenSSL が必要です。`http://127.0.0.1:8088` を開き、`root` としてサインインします。パスワードは
`conf/orbstack/credentials.json` にあるランダムなものを使います。管理用のリスナーはループバックに
のみバインドされます。

資格情報と永続化されるブリッジ証明書は、Git の無視対象になっている `conf/orbstack/` ディレクトリ
に置かれます。設定とトラフィックのスナップショットは `felucca_server-data` ボリュームに置かれ
ます。資格情報のファイルには初期パスワードが記録され、保存された管理用の設定はブートストラップの
TOML より優先されます。スクリプトを再度実行すると、現在のソースからビルドし、既存の資格情報、
証明書、データを保持したままコンテナを更新します。

コンテナは手動で停止しないかぎり Docker エンジンとともに再起動します。最初はクライアントは
設定されていません。ブリッジの TCP と QUIC はポート 14028 を使います。Compose は OrbStack の
ホストネットワークを使うため、追加のトンネルポートに Docker のポートマッピングを個別に設定する
必要はありません。他のローカルプログラムがすでに使っているポートは避けてください。

```sh
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

`down` はデータのボリュームを保持します。`-v` を付けると、永続化された設定とトラフィックの
データを削除します。

## パッケージ化と CI

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

ソースからのビルドには Node.js 22.12 以降（24 を推奨）が必要です。`apps/admin/` の
React + Vite の管理 UI はサーバーのバイナリに埋め込まれるため、デプロイ先に Node.js は
不要です。フロントエンドを変更したら再ビルドしてください。バンドルが欠けているか古いと
Cargo は拒否します。

成果物には 2 つのプログラム、サンプル、ドキュメント、バイナリごとのダイジェストのマニフェスト、
アーカイブのダイジェストが含まれます。パッケージ化するホストで実行できないバイナリは既定で
拒否され、クロスプラットフォームの CI は各ターゲットプラットフォームでパッケージ化します。
GitHub のワークフローは、Linux amd64/arm64、macOS arm64、Windows amd64 についてチェック、
テスト、アーカイブを実行し、Rust 1.88 の最小バージョンと Docker ビルドを個別に検証します。
ワークフローがアップロードするのはジョブの成果物のみで、GitHub のリリースは作成せず、
イメージのプッシュも行いません。

## 確認チェックリスト

```sh
felucca-server service --name office render --config conf/server.toml
felucca-server --config conf/server.toml --fingerprint
felucca-agent  --config conf/agent.toml  --check
```

`--fingerprint` は、設定が固定の PEM ファイルを参照していることを要求します。`--check` は、
バックエンド、DNS、証明書ファイル、公開ポートがすでに利用可能であることを意味するものでは
ありません。

## 次の手順

- [felucca-server モジュール](server.md)
- [管理 UI と集中構成](management.md)
- [リファレンス](reference.md)
