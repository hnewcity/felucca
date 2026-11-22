# トンネルとポートマッピング

トンネルは Felucca の基本単位です。1 つの公開入口とバックエンドの集合からなります。このページ
では共通のフィールド、8 つのモード、ポートの一括マッピング、PROXY プロトコルを取り上げます。

## トンネルの共通フィールド

```toml
[[tunnel]]
id = 1                 # Stable ID; pin it once traffic snapshots are enabled
mode = "tcp"           # Entry protocol
port = 8080            # Port opened on the server
bind = "0.0.0.0"       # IP only, no port attached
remark = "internal web app"
public = true          # false means a private share that does not take a public port
balance = "round_robin"   # Goes inside [[tunnel]], not after [[tunnel.backend]]

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`bind` はポートではなく IP アドレスを取ります。ポートは `tunnel.port` から来ます。`bind` を省略
するとすべてのインターフェースで待ち受けます。`id` を設定しない場合は、空いている正の整数が
割り当てられます。トラフィックのスナップショットを有効にしたら、配列の順序に頼らず `id` を明示的に
設定してください。

## モード

| `mode` | 入口の動作 | 対象の決まり方 |
| --- | --- | --- |
| `tcp` | 公開の TCP ポート。生のバイト列を転送します | `backend` |
| `udp` | 公開の UDP ポート。データグラムの境界を保ちます | `backend` |
| `http` | サーバーの HTTP、HTTPS、HTTP/3 のリスナーを再利用し、ドメインとパスで振り分けます | `backend` |
| `socks5` | SOCKS5 のフォワードプロキシ。CONNECT と UDP ASSOCIATE | クライアントが指定します |
| `httpproxy` | HTTP のフォワードプロキシ。絶対 URL と CONNECT | クライアントが指定します |
| `mixedproxy` | 混合の入口。リクエストの先頭で SOCKS5 と HTTP を判別します | クライアントが指定します |
| `file` | ファイルの配信と WebDAV | エージェントのホスト上のディレクトリ |
| `transparent`（別名 `tcptrans`） | カーネルから元の宛先アドレスを読み取ります | カーネル |

`http` トンネルは個別のリスナーを開きません。その `port` はサーバーの `http.bind`、
`http.tls_bind`、`http.http3_bind` のいずれかと一致する必要があり、複数の HTTP トンネルが 1 つの
ポートを共有できます。`tunnel.bind` は HTTP のルーティングに影響しません。HTTP のリスナーは
サーバー側で有効にするもので、エージェントの `allowed_ports` による制限を受けません。

## ポートの一括マッピング

`[[mapping]]` が対応するのは TCP と UDP だけで、ある範囲のポートを 1 対 1 で順に対応付けます。

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

両者の数は一致している必要があり、サーバー側のポートの重複は許されません。展開されたトンネルは
明示的な `[[tunnel]]` のエントリと一緒に検証され、サーバーのポートの認可とトンネル数上限に対して
引き続きカウントされます。`target_host` にはドメイン名、IPv4 アドレス、IPv6 アドレスを指定でき、
ポートは指定できません。`tls`/`trust`、`health`、`acl`、`options`、`target_policy` も設定
できます。

## UDP の動作

- 送信元の IP:port ごとに独立の関連づけを確立し、エージェント側で 1 つのバックエンドソケットに
  固定します。
- 空のパケットとデータグラムの境界は保たれます。データグラムの最大は 65507 バイトですが、実際の
  ネットワーク上で使える MTU はこれより小さいことがあります。
- 1 つのトンネルで許可される送信元の関連づけは最大 256 件で、それぞれ 8 パケットをキューイング
  します。キューがいっぱいのときはパケットを破棄し、関連づけは 60 秒のアイドル後に回収されます。
- データグラムは信頼できるトンネルのストリームで運ばれ、QUIC DATAGRAM では**ありません**。その
  ため損失の多い経路では再送を待ちます。
- UDP の統計はデータグラムのペイロードだけを対象とし、内部の長さフレームは含みません。

## PROXY プロトコル

元の送信元アドレスと入口の宛先アドレスをバックエンドに送ります。

```toml
[tunnel.options]
proxy_protocol = "v1"     # "off" | "v1" | "v2"
```

TCP、HTTP、TLS では、バックエンド接続ごとに 1 つのヘッダーを送ります。UDP は v2 だけに対応し、
データグラムごとに DGRAM ヘッダーを付加します。ヘッダーを含めて 65507 バイトを超えるパケットは
破棄されます。バックエンド側が PROXY プロトコルを解析できる必要があります。

受信側では、対象のリスナーの `proxy_accept` を設定します。

```toml
[http.proxy_accept]
mode = "required"                    # optional | required
trusted = ["192.0.2.10", "2001:db8:1::/64"]
bridge.proxy_accept = { ... }
[tunnel.options.proxy_accept]
mode = "optional"
trusted = ["10.0.0.0/8"]
```

`optional` はヘッダーなしの直接接続を許可し、`required` はヘッダーがない接続を拒否します。これを
有効にする場合、`trusted` を空にしてはいけません。送信元 ACL、送信元ハッシュ、HTTP の転送ヘッダーは
すべて検証済みの元アドレスを使います。同じポート上のブリッジと HTTP のリスナーには、同じポリシーを
設定する必要があります。受信は 5 秒に制限され、UDP のリスナーは PROXY ヘッダーを受け付けません。

## ストリームごとの符号化

```toml
[tunnel.options.payload]
compress = true     # Snappy, default false
encrypt = true      # ChaCha20-Poly1305, default false
```

これは TCP と UDP、ファイルと WebDAV の転送、HTTP のリバースプロキシとフォワードプロキシの
データストリームに適用されます。圧縮を有効にすると、データは暗号化される前に圧縮されます。
各ストリームは新しいノンスを交換し、そのエージェントの vkey から 2 つの独立した方向鍵を導出
します。両端で設定が一致しない場合はネゴシエーションが拒否され、平文への自動フォールバックは
ありません。詳細は [アクセス制御、クォータ、ストリーム符号化](security.md) を参照してください。

## 次の手順

- [バックエンド、負荷分散、ヘルスチェック](backends.md)
- [HTTP/HTTPS リバースプロキシ](http-services.md)
- [フォワードプロキシ](forward-proxy.md)
- [ファイルサービスと WebDAV](file-services.md)
- [サーバー上のローカルな対象と入れ子ルート](local-services.md)
