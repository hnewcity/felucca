# はじめに

自分の公開 Server を使うか、運営者から接続コマンドを受け取ります。証明書の準備、設定ファイルの編集、agent の事前インストールは不要です。

## 1 行のコマンドでデバイスを接続

1. **デバイス → デバイスを接続** で名前を入力します。既定では、このデバイス、LAN、インターネットにアクセスできます。制限する場合は **詳細設定：アクセス権限** を開き、このデバイス、LAN、またはカスタム範囲を選択します。
2. コマンドをコピーして、対象デバイスのターミナル（macOS/Linux）または PowerShell（Windows）で実行します。プロセッサーを検出し、対応するクライアントをダウンロードしてチェックサムを検証します。
3. ウィンドウを開いたままにします。オンラインになったら **サービスを追加** を選び、ローカルアドレスを入力して公開アドレスにアクセスします。切断時は自動的に再接続します。
4. プライベートアクセスでは受信デバイスを作成し、そのコマンドを別のコンピューターで実行します。受信者は管理画面へのログイン不要です。

コマンドにはデバイスの認証情報が含まれます。意図した利用者だけに共有してください。再接続には同じコマンドを使い、接続中のウィンドウを先に閉じます。サーバーには対象 OS のパッケージが必要です。ZIP ダウンロード、フォルダー権限、詳細設定は任意です。

## 自分の Server を起動

配布パッケージのプログラムを使い、書き込み可能なディレクトリで初回のみ実行します：

```sh
./felucca-server --init
```

設定を生成し、ランダムな root パスワードを表示して起動します。表示された管理アドレスを開きます。更新やバックアップでは `felucca-server.toml` と `felucca-server.data/` 全体を保持してください。以後は `--init` なしで起動します：

```sh
./felucca-server
```

公開環境では `--init --admin-url https://console.example.com` を使い、HTTPS リバースプロキシから Host を保持して `127.0.0.1:8990` へ転送します。HTTP アドレスを指定すると対応するポートを公開します。公開アクセスには HTTPS を使ってください。デバイスはブリッジ TCP 9001、利用者は公開サービスポートに到達できる必要があります。[配備とクライアント配布](../../management.md)を参照してください。

## 前提条件

| 要件 | 詳細 |
| --- | --- |
| Rust 1.88 以降 | ビルド用。このリポジトリは Rust 1.98.1 で検証しています |
| Python 3 + OpenSSL | ローカルデモ用スクリプト `scripts/demo-local.py` のみが必要とします |
| 公開 IP を持つホスト | `felucca-server` を実行します |
| プライベートネットワーク上のホスト | `felucca-agent` を実行し、公開したいサービスを置きます |
| ローカルの TCP/UDP をバインドする権限 | テストに必要です。ファイアウォールのルールはデプロイする担当者が管理します |

Felucca はシステムサービスをインストールせず、ファイアウォールのルールも変更しません。

## 選択肢 1 — ローカルデモ

公開網を使わず、自分のマシンですべてが動くところ（管理ページとポートフォワーディング）だけを
見たい場合:

```sh
python3 scripts/demo-local.py
```

このスクリプトはデバッグバイナリを差分ビルドし、証明書とランダムな資格情報を生成し、テスト用
Web サイト、サーバー、エージェントを起動して実際の転送を検証し、その後で自分でデモページを開きます。
ターミナルには管理アドレス、アカウント、パスワードが表示されます。`Ctrl+C` を押すとデモの
サービスをすべて停止します。

すべてのリスナーは `127.0.0.1` にのみバインドされ、既定のポートがすでに使われている場合は空き
ポートが自動的に選ばれます。実行ごとに、設定、ログイン情報、ログはそれぞれ独自の
`target/local-demo/run-*` ディレクトリに保存されます。

| フラグ | 効果 |
| --- | --- |
| `--no-open` | ブラウザを自動的に開きません |
| `--no-build` | バイナリがすでにビルド済みの場合にビルドを省きます |

## 選択肢 2 — ビルド

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
```

結果として `target/release/felucca-server` と `target/release/felucca-agent` ができます。
`scripts/cargo-external.sh` は Cargo のキャッシュを作業スペースのあるディスクに置きます。
ディスク容量に余裕がある場合は `cargo` を直接呼んでも構いません。

## 選択肢 3 — 手動で最初のトンネルを動かす

### 1. ブリッジ証明書を生成する

ブリッジ証明書は 1 度だけ生成し、秘密鍵は保管しておきます。

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

### 2. 共有鍵を設定する

両方のファイルの `vkey` は同じランダム文字列である必要があります。

```sh
openssl rand -hex 32
```

`[client.agent-1].vkey`（`conf/server.local.toml`）と `vkey`（`conf/agent.local.toml`）に
書き込みます。エージェントの `key_id` はサーバーの `[client.<id>]` テーブルのエントリと一致
している必要があります。エージェントの `server` は、サーバーホストの到達可能なアドレスに
変更してください。

### 3. 証明書のフィンガープリントを読み取る

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
```

表示された SHA-256 のフィンガープリントを、エージェントの `[trust].fingerprint` に入れます。
次に両方のファイルを検証します。

```sh
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

### 4. 両方のプログラムを起動する

2 つのターミナルでそれぞれ 1 つずつコマンドを実行します。

```sh
target/release/felucca-server --config conf/server.local.toml
```

```sh
target/release/felucca-agent --config conf/agent.local.toml
```

### 5. 動作を確認する

サンプルの設定では、サーバー側の TCP `8080` がエージェントのマシン上の `127.0.0.1:3000` に
転送されます。エージェントのマシンで 3000 番で待ち受けるサービスを起動し、サーバーの 8080 番を
開いてください。

## 次の手順

- [アーキテクチャと概念](architecture.md) — bridge、transport、tunnel の意味
- [felucca-server モジュール](server.md) — クライアントテーブル、HTTP リスナー、ポートの認可
- [felucca-agent モジュール](agent.md) — 再接続、エグレスネットワーク、複数インスタンス
- [トンネルとポートマッピング](tunnels.md) — TCP/UDP/HTTP/file/proxy の各モード
- [リファレンス](reference.md) — すべてのコマンドとフィールド
