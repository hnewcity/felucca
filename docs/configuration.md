# 配置与部署

## 配置来源与多实例启动

两个程序的 `--config` 支持本地 TOML/JSON 文件、HTTP(S) URL、直接传入的 JSON 对象，
以及 `base64:` 加标准 base64 或无 padding 的 base64url 内容。单份配置上限 4 MiB。
远程获取最多等待 15 秒，必须返回 200；不跟随重定向、不接受压缩响应。
HTTPS 验证公开 CA 证书。URL 不接受 userinfo 或 fragment，查询参数可用于签名下载地址。
`--config-token-env NAME` 从环境变量读取 Bearer token；URL、token 和内联配置正文不会
作为配置来源名称打印。远程配置中的证书、Geo 数据和文件根目录仍指本机路径，
相对路径以进程工作目录为基准。

agent 可重复 `--config` 或其别名 `--launch`，在同一进程运行最多 32 个实例：

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://config.example.com/edge.json' --config-token-env CONFIG_TOKEN
```

每个实例分别维护连接、退避、路由、探针和连接上限。全部配置先校验并初始化，
之后才开始连接；重复的 server/key_id 组合会报错，防止互相替换会话。
某个服务器不可达时其他实例继续工作，退出信号关闭所有实例。
`--check` 同样校验每份配置，但不建立桥接连接。

agent 还支持简短启动 URI：

```text
felucca://server.example.com:14028?key_id=edge&vkey=YOUR_PERCENT_ENCODED_KEY&transport=tls
```

URI 需要非零端口；可选字段为 `key_id`、`transport`、`fingerprint`、`ws_path` 和 `remote`，
`vkey` 必填。值使用 URL 百分号编码；`fingerprint` 启用固定证书 SHA-256 验证，
省略时使用正常 CA 验证。未知字段和重复字段会报错。URI 用于连接，
需要定义隧道时使用 TOML/JSON 配置。`remote=1` 开启 `remote.enabled` 与 `remote.console_scope`，
隧道和访问范围由服务端 `[managed.<client-id>]` 下发；默认允许所有网络，可在高级设置中限制
地址、域名和端口。已有的显式访问限制继续生效。
`remote=1` enables managed configuration and console-supplied scope. All networks are allowed by
default; advanced settings can restrict addresses, domains and ports. Existing explicit policies remain in effect.

设置 `--expand-env`（或 `FELUCCA_EXPAND_ENV=true`）可展开已解析的字符串值：

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}` 要求变量存在；`${VAR:-default}` 在变量不存在或为空时使用默认值。
`$$` 表示字面量 `$`。展开只执行一次，不修改字段名，也不把字符串转成数值；
变量中的引号、换行或 TOML/JSON 片段保持原来的字符串值。默认不开启展开。
JSON 重复字段（含嵌套账号表）、未知配置字段及展开后超过 4 MiB 的内容均被拒绝。

## 服务端与 agent

`bridge.bind` 是 TCP/TLS/WS/WSS 共用的桥接地址，默认 `0.0.0.0:14028`。
`bridge.quic_bind` 是独立 UDP 地址，可以使用相同端口号。WS/WSS 路径两端必须一致，
默认 `/ws`，其他路径会拒绝升级。为每个 agent 配置独立的 `key_id` 和随机 `vkey`；
服务端 `allow_config_push = true` 才允许注册隧道。

相同 `key_id` 的新连接会替换旧连接，旧监听停止并释放后才能重新绑定。
同一服务端进程中的流量计数与带宽限速器跨重连保留；配置 `[state]` 后，
流量计数也跨服务端重启保留，未配置时重启清零。
启动文件的修改通过重启进程生效。也可启用下述运行配置快照。

### 运行配置快照

`felucca-server --config bootstrap.toml --runtime-config conf/runtime.json` 启用
独立的运行配置文件，也可用 `FELUCCA_RUNTIME_CONFIG` 指定路径。
第一次启动成功后原子保存规范化配置；以后启动优先读取快照中的配置。
`--check` 和 `--fingerprint` 也读取同一快照。文件损坏时启动报错，不回退到旧启动文件。
不要把它与 `[state].path` 的流量文件设为同一个路径。

运行配置最大 16 MiB，包含实际密钥，Unix 上新文件权限为 0600、新目录为 0700。
文件锁拒绝另一个 daemon 同时使用它；准备文件采用独占创建、同步写入和同目录重命名。
提交后目录同步失败会记录错误，并明确标记持久性状态，不能声称已回滚。

库的 `Daemon::apply(config, expected_revision)` 提供带版本检查的更新事务。
先校验配置并准备新实例、回滚实例和快照，再关闭旧监听并启动新实例；
监听启动或文件提交失败时恢复旧配置。更新会中断当前转发连接，agent 随后重连。
进程内更新保留流量计数及临时桥接证书身份；改变流量文件路径需要重启 daemon。
调用方停止等待不会取消已经开始的事务。HTTP 管理界面与配置编辑 API 已接入，见 [管理说明](management.md)。

桥接推荐 TLS/WSS/QUIC。证书验证可以使用 `webpki` 或固定 SHA-256 的 `pinned`。
`pinned` 固定整张证书，换证时须同步更新指纹；它不验证域名和有效期，但验证握手签名。
`insecure` 仅用于临时诊断。省略 `bridge.tls` 时，有运行配置存储的服务端会保存
`<运行快照路径>.identity.json`，重启保持身份；未配置存储时才使用临时证书。备份应包含身份文件。
桥接 PEM 按 `bridge.cert_reload_interval`（默认 30 秒）重载，同时更新 TLS/WSS 和 QUIC
新连接的证书；不打断旧连接。证书或密钥更新不完整时保留旧值。pinned agent 在下一次重连前
需要更新指纹；使用正确 CA 验证的 agent 无需固定某张证书。

## 服务端本地目标与嵌套路由

`[local.<name>]` 定义服务端本地服务组，`[[local.<name>.tunnel]]` 使用与 agent 相同的
TCP/UDP、HTTP/SNI、代理、文件、负载均衡、探针和 ACL 配置。无需另起 agent；
只有本地服务时可以省略 client 表。本地组在注册表和流量快照中记作 `local.<name>`，
此命名空间不能用作外部 agent 的 key_id。示例见 `examples/local-server.toml`。

每组可配置 `user`、`acl_mode`/`acl_rules`、`limits`、`network` 和 `dial_timeout`。
来源权限取全局、用户、组、隧道交集；网络目标按实际解析 IP 检查目标 ACL。
本地组没有桥接，上游代理配置会被拒绝；`forward_local_ip=true` 才把出口 IP 用于后端。
文件根目录、证书和后端配置在启动时准备，监听冲突会使该次服务启动失败。

本地隧道的 backend.addr 也可以使用以下别名：

| 地址 | 行为 |
| --- | --- |
| `tunnel://local.office/7` | 进入另一个已注册本地隧道的服务入口 |
| `tunnel://agent-1/7` | 进入在线 agent 的已注册隧道 |
| `file://agent-1/7` | 同上，但要求注册目标确实为 file 模式；保留目录边界、只读和文件鉴权 |
| `bridge://tcp`、`tls`、`ws`、`wss` | 进入相应桥接入口，仍须 agent 的双向认证；TLS 使用桥接证书 |
| `bridge://http` | 进入服务端 HTTP 入口，继续执行 Host、站点认证和权限检查 |

隧道号必须显式稳定。指定 user 的本地组只能引用同一用户的目标；未指定 user
的组由服务端操作者管理，可以跨用户引用。bridge 别名只允许操作者组。
远端 agent 和动态代理请求不能把别名当作网络目标使用。
别名不支持额外 backend.tls、网络健康探针或入口目标 ACL：权限由每一跳目标执行；
HTTPS/SNI 别名可直接传入 TLS 数据，由目标站点终止或直通。
UDP 只能嵌套至 UDP，其他模式使用流入口；SOCKS5/HTTP 代理鉴权不会被跳过。

每跳保留原始来源 IP/端口及路由历史，重复 `(client,id)` 或超过 16 跳立即拒绝。
HTTP 连接池同时按历史隔离，防止复用连接绕过环路检查。
每个实际经过的隧道分别取得连接名额并记录流量；同一用户/组经过多跳时，
其汇总额度也按每一跳计费。UDP 统计报文内容，不包括内部长度帧。

## DNS、出口地址、上游代理和时间同步

agent 的 `[network]` 支持 `local_ip` 绑定桥接出口 IP，
`forward_local_ip = true` 也对后端连接和健康探针使用该地址。默认只绑定桥接；
地址必须实际属于本机，不能把 IP 配置成接口名称。DNS 服务器填字面量 IP:port：

```toml
[network]
local_ip = "192.0.2.10"
forward_local_ip = true
dns = ["192.0.2.53:53"]

[network.proxy]
kind = "socks5"
server = "192.0.2.20:1080"
remote_dns = true
[network.proxy.auth]
username = "edge"
password = "replace-with-proxy-password"
```

`dns` 为空时使用系统解析器。自定义解析器按 TTL 缓存，支持 A/AAAA、
UDP 截断后 TCP 回退；代理地址、桥接、后端、探针和 NTP 共用该解析器。
目标 ACL 对实际解析到的 IP 执行检查，再连接这些已检查地址。
`local_ip` 与 DNS 地址须同族；后端解析到的其他地址族在绑定时会被跳过。

上游代理 `kind` 为 `http`（CONNECT）或 `socks5`，仅承载桥接 TCP/TLS/WS/WSS。
`remote_dns = true` 把目标主机名交给代理解析，false 则先在本机解析；
代理本身的地址始终在本机解析。账号表可省略以使用匿名代理，
配置 SOCKS5 账号后不降级到匿名。QUIC/KCP 桥接不能同时使用 TCP 上游代理。
此设置不把 agent 的后端请求或 NTP UDP 转发到上游代理。

server 和 agent 都可配置 `[ntp]` 校准桥接认证时间：

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

默认不查询时间服务；配置后在启动认证前同步一次，随后按 interval 刷新。
只采用匹配本次请求、有效 stratum、正常往返时间且偏移未超过 max_offset 的应答。
interval 范围 1 秒到 24 小时，timeout 为正且不超过 30 秒、低于 interval；
max_offset 为正且不超过 7 天。首次失败使用系统时间，刷新失败保留最近有效样本，
样本超过三个 interval 后失效并回退系统时间；退出会取消等待中的请求。
NTP 源应由部署方信任，普通 SNTP 不提供时间源身份认证。
校准不修改系统时钟；TLS 证书有效期和配额到期检查仍使用系统时间。
agent 的 NTP 使用其 DNS/出口配置，server 使用系统 DNS 和默认出口。

## KCP 桥接

服务端配置 `bridge.kcp_bind = "0.0.0.0:8025"`，agent 配置 `transport = "kcp"`，
`server` 指向这个 UDP 端口。可与 TCP 桥接使用同一端口号，但与 QUIC/HTTP3
必须使用不同 UDP 监听。KCP 承载 felucca-mux，公网 TCP 的半关闭和并发流由 mux 管理。
它仍执行相同的双向 HMAC 身份认证、隧道注册、ACL 和配额检查。
KCP 本身是明文传输，可用 `tunnel.options.payload.encrypt` 加密路由数据；
控制配置的传输机密性仍应通过 TLS/WSS/QUIC 提供。

server 的 `[bridge.kcp]` 与 agent 的 `[kcp]` 接受：

| 参数 | 默认值 | 约束 |
| --- | --- | --- |
| mtu | 1200 | 576..1472，包含 FEC 头与校验分片开销 |
| send_window | 256 | 16..4096 个 KCP 段 |
| receive_window | 256 | 128..4096 个 KCP 段 |
| interval_ms | 20 | 10..1000 |
| nodelay | true | 启用较短的重传定时 |
| fast_resend | 2 | 0..3 |
| congestion_control | true | 保留拥塞控制 |
| session_timeout | 90s | 1s..24h |
| data_shards / parity_shards | 10 / 3 | 数据 1..32、校验 1..16；同时为 0 关闭 FEC |
| max_sessions | 128 | 1..4096，仅服务端监听使用，包含尚未完成认证的会话 |

两端须采用相同 MTU 和 FEC 分片数。数据包立即发送，每满 data_shards 个数据包后
附加 parity_shards 个 Reed-Solomon 校验包；默认一组 13 个包可恢复其中任意 3 个丢失包。
组未满也不会阻塞数据发送，剩余丢包仍由 KCP 重传处理。
每个会话最多缓存 32 个未完成 FEC 组，2 秒过期，已完成组去重历史最多 128 项。
声明超限、分片参数不符、损坏的 KCP 段和错误会话标识均在分配大缓冲区前拒绝。

驱动使用有界的 UDP 输入队列与发送窗口；socket 暂时不可写时按丢包处理并由 KCP
恢复，不积累无限待发队列。关闭应用流后最多等待 5 秒排空已接受的发送数据；
关闭监听会释放其所有会话。agent 使用自定义 DNS 和 local_ip，并验证 UDP 应答来源。

## 监听与注册

服务端 `allowed_ports` 是顶层字段，必须写在第一个 TOML 表头之前。空列表允许 >1024 的
TCP/UDP 隧道端口。`max_tunnels` 和 `max_connections` 按 agent 计算。

agent 的 `tunnel.bind` 填 IP，不附带端口；端口来自 `tunnel.port`。不填默认所有接口。
SOCKS5/HTTP 正向代理示例只绑定 `127.0.0.1`，需要远程使用时同时设置来源白名单。

`http` 隧道不创建独立监听。它的 `port` 必须匹配服务端 `http.bind` 或 `http.tls_bind`，
注册后的域名可由这两个监听访问。不同 HTTP 隧道可以使用同一端口。HTTP 监听由服务器
配置控制，因此不受 agent 的 `allowed_ports` 限制；`tunnel.bind` 对 HTTP 路由没有效果。
HTTP 与 HTTPS 是否启用由服务端决定：把相应地址设为 `""` 可关闭其中一个。
`http.http3_bind` 可启用独立 HTTP/3 UDP 入口，它的端口同样可用于 HTTP 隧道注册。

## HTTP/HTTPS 路由

域名大小写不敏感，末尾的点会归一化。精确匹配优先于通配匹配。
`*.example.com` 匹配 `a.example.com`，不匹配 `example.com` 或 `a.b.example.com`。
同域名可按路径和 HTTP/HTTPS 协议分配到不同隧道。相同域名、路径、协议的重复注册会拒绝。
没有匹配路由时返回 404。

HTTPS 使用 `http.certificates."域名"` 中的证书，精确匹配优先；没有匹配证书时使用
`http.default_tls`，也没有默认值则拒绝握手。SNI 与 HTTP Host 冲突返回 421。
使用 `http.default_tls = { cert = "...", key = "..." }` 时应放在 `[http]` 下。
静态证书须覆盖对应域名，默认每 30 秒重新读取，`http.cert_reload_interval` 可调整。
证书与私钥必须匹配后才替换；文件缺失、格式错误或更新期间不匹配会保留旧证书。

默认把客户端传入的 Forwarded / X-Forwarded-* / X-Real-IP 清除，然后设置
X-Forwarded-For、X-Real-IP、X-Forwarded-Host、X-Forwarded-Proto。
它们反映直连来源或经 `http.proxy_accept` 验证的 PROXY 协议来源。
设置 `forwarded_headers = false` 不执行这个重写，适合自行控制完整代理链的部署。
逐跳头和 `Connection` 指定的头不会透传到下一跳。

支持 HTTP/1.1 和 HTTP/2 流式请求/响应；TLS ALPN 支持 h2 和 http/1.1，
明文入口也支持 HTTP/2 prior knowledge。HTTP/1.1 后端支持 WebSocket 等 101 升级。
后端连接池按客户端会话、隧道、来源 IP、目标和 HTTP origin 隔离，每个 origin 最多
8 个闲置连接、30 秒闲置超时；外层最多 256 项。启用发送 PROXY 头时还按来源端口隔离。
`response_timeout` 限制从发送请求到
收到后端响应头的时间；慢速大文件上传需要相应调大。响应体使用背压，不整体缓存。

## 后端和健康检查

`balance` 放在 `[[tunnel]]` 内，不能误放到 `[[tunnel.backend]]` 后面。

| 策略 | 选择方式 |
| --- | --- |
| `round_robin` | 在健康后端之间轮询 |
| `weighted` | 按正整数 weight 分配连接 |
| `least_connections` | 选择当前占用连接最少的后端 |
| `source_hash` | 按原始用户 IP 稳定选择后端 |
| `failover` | 按配置顺序选择第一个健康后端 |

选择发生在 agent。后端连接关闭或取消时释放计数；权重分配的是连接数，不是字节数。
所有后端都被标为不健康时仍会尝试一个，防止错误探针配置导致全面不可用。
当前单次连接失败会回报错误，不在同一次请求内重试其他后端。

后端 `tls = true` 可用于 TCP/HTTP，默认用 WebPKI 验证，SNI 取后端地址里的主机名。
自签名后端可设置 `[tunnel.backend.trust] mode = "pinned"` 和 `fingerprint`。
健康检查与数据连接使用相同的 TLS 配置。

`tunnel.options.http.backend_http2 = true` 使用 HTTP/2 后端：
明文使用 prior knowledge，TLS 必须协商到 h2；健康探针也使用 HTTP/2。
WebSocket 自动转换 HTTP/1.1 Upgrade 与 HTTP/2 扩展 CONNECT。入口为 HTTP/1.1 时
返回 101；入口为 HTTP/2 或 HTTP/3 时使用 `:protocol = websocket`、版本 13 和 200 响应。
HTTP/2 后端必须声明支持扩展 CONNECT。Sec-WebSocket-Key/Accept 只用于 HTTP/1.1，
转换时重新生成并校验；Origin、子协议和扩展协商头仍透传。路径、认证、ACL 和 Host
改写照常执行。其他协议的 HTTP/1.1 Upgrade 仍要求 HTTP/1.1 后端。

HTTP/3 配置 `http3_bind = "0.0.0.0:8443"`，证书与 HTTP/HTTPS 共用 SNI resolver，
包括 ACME 续期及静态 PEM 热重载。UDP 可以与 TCP HTTPS 使用相同端口号，
`http.http3_bind` 与 `bridge.quic_bind` 可填同一个 IP:port，使用一个 UDP socket 按
ALPN 分流；桥接证书与网站证书分别选择、分别刷新。正常 HTTPS 响应会附加 Alt-Svc；
若外部映射端口不同，可用 response_headers 覆盖 Alt-Svc。
HTTP/3 保留流式背压和 trailers，使用相同的路径/认证/ACL/目标限制及后端池。
每个 QUIC 连接最多 128 个双向流，接收窗口每流 1 MiB、连接 16 MiB，闲置 30 秒关闭；
请求头限 32 KiB，拒绝重复 Content-Length、连接专用头及非法 framing trailers。
目前 HTTP/3 仅为公网入口协议，后端仍选 HTTP/1.1 或 HTTP/2。

`tunnel.options.http.tls_mode = "passthrough"` 按 SNI 转发原始 TLS 到后端，后端证书直接
展示给访问者；此时 backend.tls 必须关闭。`"offload"` 使用服务端的 SNI 证书终止 TLS，
然后把任意 TCP 内容发往后端。这两种模式占用完整 SNI，不能同时配置 HTTP 路径/头部选项
或站点认证；普通 HTTPS 反代使用默认 `"http"`。

`[tunnel.health] type = "tcp"` 检查建连；`type = "http"` 请求指定 path，默认接受 2xx，
可用 `expect_status` 指定。HTTP 探针 Host 使用后端地址。探针有超时，连续失败/成功
触发摘除/恢复，并输出日志。UDP 和动态目标代理不应配置 TCP/HTTP 后端探针。

## 正向代理、ACL 和 UDP

`mode = "socks5"` 提供 CONNECT 和 UDP ASSOCIATE，支持 IPv4、IPv6 与 agent 侧 DNS 解析。
BIND 返回“不支持命令”。`httpproxy` 支持绝对 HTTP URL 及 CONNECT；
`mixedproxy` 按请求开头区分 SOCKS5 和 HTTP。

在当前隧道下加入账号表启用认证；不配置账号表时允许匿名：

```toml
[tunnel.options.auth.users]
alice = "replace-with-a-password"
bob = "another-password"
```

SOCKS5 使用用户名密码方法，不会降级到匿名；HTTP 正代/CONNECT 使用 Proxy-Authorization
Basic，失败返回 407。HTTP 反代使用 Authorization Basic，失败返回 401。
已验证的代理/站点凭据会在发往后端之前移除。配置和注册信息含账号密码，
桥接应使用默认 TLS、WSS 或 QUIC；`tcp`/`ws` 是明确选择的明文传输。
当前注册协议为 v5，server 与 agent 必须一起更新。

### 隧道压缩与独立数据流加密

每个 `[[tunnel]]` 可配置以下选项，默认均为 false：

```toml
[tunnel.options.payload]
compress = true
encrypt = true
```

适用于 TCP/UDP、文件/WebDAV、HTTP 反代和正代的数据流。Snappy 以最多 16 KiB
原始数据为一块，仅在压缩后更小时使用压缩块。解压前检查声明大小，不接受超限块。
加密使用 ChaCha20-Poly1305，启用压缩时先压缩后加密；每条流交换新随机数，
用该 agent 的 vkey 验证双方身份及完整路由请求，HKDF 派生两个方向独立的密钥。
长度头、序号和结束记录受到完整性验证，篡改、重放和缺失结束记录会关闭该数据流。
两端不匹配时拒绝协商，不自动退回明文。每个方向只缓存一个有界记录。

这层编码位于 mux/QUIC 流内，不增加一套多路复用；PROXY 头、HTTP/WebDAV 内容和
UDP 报文都经过同一编码。流量配额和限速仍按原始转发字节计算。
独立加密保护路由的数据部分，配置注册及桥接控制消息仍由所选桥接传输保护，
因此 TLS/WSS/QUIC 仍是默认部署选择。该 PSK 层不单独提供前向保密；
压缩会改变可观察的记录长度，需要隐藏压缩特征时保持 compress=false。

### Geo 数据和分层来源 ACL

server 和 agent 顶层都支持 `[geo]`，`geoip`/`geosite` 指向 V2Ray protobuf
`.dat` 文件，路径相对于进程工作目录。额外文件由 `[geo.extra]` 声明别名。
例如 `geoip:CN`、`geosite:category-ads-all`、`geosite:custom:LOCAL@fast@!ads`。
`@属性` 要求存在，`@!属性` 要求不存在，多个条件取交集。
`geoip:private` 内置 IPv4/IPv6 私有、回环、链路本地和 CGNAT 网段，不需要文件。
IP 前缀采用已编译区间查询；域名支持精确、后缀、关键词和正则。
缺失文件、分组、错误正则和损坏 protobuf 会报错；不会退化成空规则放行。
数据在启动时加载，更新数据文件后需重启相应进程。

目标 `target_policy.allow/deny` 支持 GeoIP、GeoSite。名称和解析后的 IP 一起检查，
`deny` 优先，然后直接连接已检查的地址，避免重新解析引入绕过。
来源 `acl_rules` 支持 GeoIP；服务端的顶层 `acl_mode/acl_rules`、
`[user.<name>]`、`[client.<id>]` 与隧道规则取交集。
客户端通过 `user = "<name>"` 关联本机配置中的用户，agent 无权修改关联。
这些规则控制公开入口的访问者，不用于拒绝 agent 的桥接地址。
两端各自使用本机声明的数据文件；远端只能引用分组及别名。

### 分层配额与流量快照

`[user.<name>.limits]`、`[tunnel.options.limits]`、
`[http.host_limit."<域名模式>"]` 均接受：

```toml
max_connections = 20
rate_limit = 1048576
quota = { flow_limit = 1073741824, expires_at = 1893456000 }
```

流量和速率单位为字节，`expires_at` 为 Unix 秒。未填写表示不限；
速率 0 表示不限，连接数 0 明确禁止新连接。客户端已有的同名字段继续可用。
同用户多客户端、同隧道多连接、同域名多路径共用各自计数与限制。
多层速率同时计费，等待时间取严格的一层，连接名额申请失败会归还此前层的名额。
HTTP 域名限制按原始 Host/SNI 选择，精确域名优先于通配，不受后端 Host 改写影响。
HTTP 连接数统计实际后端连接；HTTP/2 多请求可以复用一个连接。

TCP、HTTP、SNI 和 UDP 都更新各层计数。TCP 以 16 KiB 块检查并限速，
UDP 以整个报文处理；并发中正在转发的数据可能使总量略超配额，
不是精确逐字节截断。HTTP 的计数是后端协议字节，含头部及协议开销。

```toml
[state]
path = "data/traffic.json"
save_interval = "5s"
```

启用后按间隔原子替换快照，并在正常退出、连接结束后保存最终计数。
快照包括离线资源，服务端重启后仍计入配额；崩溃可能丢失最后一个保存间隔内的数据。
同一快照只能由一个服务端写入。损坏或未知版本快照会拒绝启动，保留原文件。
这是流量快照；管理界面的运行配置另存于 admin.state_path / --runtime-config。
隧道可以指定稳定 `id`；启用快照后应固定这些 ID，避免仅靠数组顺序识别隧道。
没有显式 ID 时分配可用的正整数。

完整可校验配置见 `examples/policy-server.toml` 和 `examples/policy-agent.toml`。

### 文件服务与 WebDAV

`mode = "file"` 在 agent 上直接提供文件服务，无需另起 HTTP 服务。
未配置 `hosts` 时使用独立公开 TCP 端口；配置 `hosts` 时走 server 的 HTTP/HTTPS/HTTP3
域名入口，端口需匹配该入口。文件内容通过同一隧道传输，来源 ACL、配额和限速继续适用。

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

`root` 必须是 agent 本机已有目录，相对于进程工作目录；启动时打开目录句柄。
请求不能通过 `..`、绝对路径或符号链接越出该目录，COPY/MOVE 目标也受此约束。
root 和文件账号配置保留在 agent，不放进服务端注册消息。
账号允许留空以提供匿名服务；配置多个账号时，每次 HTTP 请求都验证对应密码。

默认只读，允许 GET、HEAD、PROPFIND 和 OPTIONS；设置 `read_only = false`
可使用 PUT、DELETE、MKCOL、COPY、MOVE、LOCK、UNLOCK。
锁在同一文件隧道的多条连接间共享，agent 重启后释放。
支持 Range、条件请求、目录列表和 `index.html`；大文件按块传输。
文件认证配置在 `tunnel.file.auth`，不同时使用 `tunnel.options.auth`。
文件模式不配置网络 backend、目标网络策略、PROXY 输出或 raw TLS 模式。
通过 HTTP 域名入口时可设置 `tunnel.options.http.backend_http2 = true`。

示例见 `examples/file-server.toml` 和 `examples/file-agent.toml`；
示例只读共享仓库的 `examples` 目录，并绑定回环地址。

ACL 支持 `off`、`allow`、`deny`，规则是逗号或换行分隔的 IP/CIDR。
客户端级 ACL 与隧道级 ACL 都必须允许来源。规则作用于访问代理的源 IP，不是目标地址；
桥接身份由 vkey 校验，不用来源 ACL。

目标限制单独配置，在 agent 侧执行，也适用于固定后端和健康探针：

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

`allow` 为空表示不限制允许目标；`deny` 始终优先；`ports` 为空允许任意非零端口。
支持 IP/CIDR、`domain:` 精确域名、`suffix:` 后缀（含根域名和任意层级子域名）、
`regex:` 正则；`*` 匹配所有目标。名称不区分大小写，末尾点会规范化。
先解析 DNS，再逐一检查原名称与实际 IP，连接只使用通过检查的地址，不二次解析。
允许域名不能绕过拒绝的 IP。GeoIP/GeoSite 数据库尚未接入。

SOCKS5 UDP 会话只接受控制连接对端 IP：请求指定端口时固定该端口，否则绑定首个有效
UDP 包的源端口。控制 TCP 关闭会清理全部目标关联；每会话最多 64 个目标、各排队
8 个包、闲置 60 秒回收。非零 FRAG 会丢弃，不实现 SOCKS 分片重组。

UDP 转发每个来源 IP:port 单独建立关联，并在 agent 侧固定一个后端 socket。
保留空包及报文边界，最大报文 65507 字节；实际网络可能有更低的可用 MTU。
每个隧道最多 256 个来源关联，各排队 8 个包；满队列丢包，闲置 60 秒回收。
报文通过可靠隧道流承载，不是 QUIC DATAGRAM，所以丢包链路上仍会等待重传。

`rate_limit` 是 agent 全部隧道、两个方向共享的字节/秒限制，允许一秒流量的初始突发。
`quota.flow_limit` 累加两个方向，`quota.expires_at` 使用 Unix 秒。
配额会中止正在传输的连接，但检查有周期，存在在途超额，计数不用于严格计费。
HTTP 的计数包含发往后端的协议头；UDP 计数仅含载荷，空包不会增加字节计数。

## HTTP 路径与请求选项

同一域名可注册多个路径。匹配优先级是精确域名、最长路径前缀、精确 scheme；
`scheme` 可为 `any`（默认）、`http`、`https`。相同域名/路径/scheme 的重复注册会拒绝。

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"
rewrite_prefix = "/v2/"
host = "backend.internal"
cors = true
request_headers = { x-source = "{remote_ip}", x-remove = "" }
response_headers = { x-proxy = "felucca" }
error_pages = { "502" = "<h1>Service unavailable</h1>" }
```

前缀改写保留剩余路径与查询串；头值为空表示删除。模板支持 `{host}`、`{path}`（含查询串）、
`{scheme}`、`{remote_ip}`、`{remote_port}`，取值为原请求，替换结果不会再次展开。
不可改写 Content-Length/Transfer-Encoding 等报文边界和逐跳头，Host 使用专用字段。
`redirect = "https://new.example{path}"` 返回 307；`force_https = true` 返回 301，
目标使用服务端 HTTPS 监听端口（443 省略），注册时必须有 HTTPS 监听。
`cors` 返回 `Access-Control-Allow-Origin: *`，合法预检直接返回 204，不要求预检携带
站点账号；实际请求仍要求认证。错误页按状态码替换响应体，支持 400..599、单页最多 64 KiB。

## ACME

参考 `examples/acme-server.toml`，替换 email、domains，确认 CA 条款后设置 accept_tos。
默认 `challenge = "http-01"`，公网域名的 A/AAAA 记录和 TCP 80 必须到达 `http.bind`。
也可设置 `challenge = "tls-alpn-01"`，只开放公网 TCP 443 并配置 `http.tls_bind`，
`http.bind` 可关闭。TLS-ALPN-01 临时证书仅响应只包含 acme-tls/1 的 ALPN，
正常站点握手不会拿到挑战证书。前置 TLS 终止代理必须将挑战 TLS 原样转发。
如果应用监听 8080/8443，需要外部将 80/443 映射到对应端口。
未成功签发前，该域名的 TLS 握手可能失败，HTTP-01 仍可提供验证。

先使用 staging 验证连通性。`staging = true` 固定选择 Let's Encrypt staging，
否则使用 `directory` 指定的 CA（默认 Let's Encrypt production）。切换 CA/环境时
使用新的 cache_dir。staging 证书不会被浏览器信任。

明确列出 `domains` 可以约束签发范围；为空时每分钟从已注册的精确域名派生，跳过通配符。
这两种挑战都不支持通配证书，需要通配域名时使用外部签发的静态证书。

要求 External Account Binding 的 CA 可设置：

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

EAB 密钥接受有/无 padding 的 base64url，日志显示脱敏；账户恢复使用已缓存凭据。

ZeroSSL 可用手动 EAB，或在 `[http.acme]` 设置
`directory = "https://acme.zerossl.com/v2/DV90"`、`staging = false` 和
`zerossl_api_key = "your-account-key"`。API key 模式在首次注册账户时获取 EAB，
随后通过 ACME 签发；不能同时配置手动 eab。请求不跟随重定向，响应限 16 KiB，
API key 和响应中的秘密不写入错误信息。账户缓存存在时不会重新申请 EAB。

账户保存在 account.json，每个域名的证书链与私钥保存在同一个 JSON 文件中，原子替换；
Unix 文件权限为 0600，新缓存目录为 0700。请保存并保护这个目录。
启动时从缓存恢复证书；续期以证书实际有效期计算，提前量不超过有效期的三分之一。
每分钟检查，失败一小时后重试；旧证书在续期失败时继续使用。
成功签发后热替换证书，不打断现有 TLS 连接。ACME 只管理 HTTP/HTTPS 反代证书，
不修改桥接证书或 agent 的固定指纹。

仓库用本地模拟 CA 驱动真实 CSR、HTTP-01 请求及 TLS-ALPN-01 握手，
检查 EAB HMAC、挑战证书 critical 扩展、凭据恢复、证书匹配和失败清理。
公网 CA 实际签发仍需用真实域名部署验证。

## 共用端口、批量映射与 PROXY 协议

`bridge.bind`、`http.bind`、`http.tls_bind` 可填相同 IP:port；一个监听按握手特征分流。
直接连接的 TLS/WSS agent 使用 felucca/9 ALPN；网站使用 h2/http/1.1 或无 ALPN。
普通 GET `/ws` 仍是站点请求，只有正确 WebSocket Upgrade 才进入桥接。
经 HTTP 反向代理接入的 WSS 可使用标准 http/1.1 ALPN，保留配置的 WebSocket 路径及升级头即可。
HTTP 与 HTTPS 也可以共用独立于 bridge 的端口。不同地址上相互覆盖的通配监听仍应避免。
QUIC 桥接与 HTTP/3 也可共享完全相同的 UDP 地址；桥接使用自己的 QUIC ALPN，
网站使用 h3。两者可以与上述 TCP 监听使用同一个端口号。

在 agent 配置中使用端口列表和包含两端的范围，按顺序一一映射：

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

mapping 仅支持 TCP/UDP，数量必须相等，禁止重复服务端端口，展开后与显式 tunnel 一起校验。
target_host 可为域名、IPv4 或 IPv6，不能附带端口。可设置 tls/trust、health、acl、
options、target_policy；每个展开后的隧道仍受服务端端口授权和隧道上限约束。

`tunnel.options.proxy_protocol = "v1"` 或 `"v2"` 向后端发送原始来源和入口目的地址，
默认 `"off"`。TCP/HTTP/TLS 每条后端连接发送一次；UDP 仅支持 v2，逐报文附加 DGRAM 头，
包含头部后超过 65507 字节的包会丢弃。后端必须能解析 PROXY 协议。

接收可信负载均衡器的 PROXY 头，配置所属监听的 `proxy_accept`：

```toml
[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

bridge 使用 `bridge.proxy_accept`，独立 TCP/SOCKS/HTTP 正代使用 `tunnel.options.proxy_accept`。
`optional` 也允许无头直连，`required` 拒绝缺失头；默认 off 不解析。
启用时 trusted 不得为空。来源 ACL、来源哈希和 HTTP 来源头使用验证后的原始地址，
不信任任意客户端自报的 IP。同端口 bridge/HTTP 必须配置一致策略。
接收限 5 秒，v1 最长 108 字节、v2 附加数据最多 4096 字节；不接收 UDP listener 的 PROXY 头。

## mux 窗口与预算

agent 使用 `[mux]`，server 使用 `[bridge.mux]`，QUIC 原生流不使用这些 mux 参数。

| 字段 | 默认值 |
| --- | ---: |
| initial_stream_window | 262144 |
| max_stream_window | 8388608 |
| max_session_window | 268435456 |
| max_send_queue | 8388608 |
| max_control_queue | 8192 |
| max_streams | 1024 |
| accept_backlog | 256 |
| open_timeout | `"30s"` |

OPEN/ACK 协商各自窗口，可先乐观发送一个 16 KiB 段；未收到 ACK 的流按 open_timeout 关闭。
消费速度满足条件时窗口逐步增长，所有广告窗口共享会话预算。DATA 队列按字节持有许可，
控制帧按数量限额；控制队列溢出终止会话，不能丢弃窗口更新。返回 credit 不得超过已发字节。
桥接保活间隔由服务器握手下发，client_id 和 keepalive 值都参与服务端 HMAC 证明。

## 日志与检查

`[log] level = "info"`，可选 `json = true`、`file = "felucca.log"`；文件追加写，同时写 stderr。
父目录须存在，日志不自动轮转。agent 用 `--log-level`，两端都可用 `RUST_LOG` 覆盖等级。
秘密密钥在 Debug/Display 中脱敏。

`--check` 验证 TOML、模式、端口、ACL 和字段关系；未知字段直接报错。
检查不代表后端、DNS、证书文件或公网端口已经可用。启动日志中的配置拒绝会列出原因，
部分注册失败时其他被接受的隧道仍可工作，agent 的完整就绪状态保持 false。

日志轮转、服务管理、更新、指标和 CPU 采样见 [运维与分发](operations.md)。

P2P 直连、打洞与中继回退配置见 [P2P 说明](p2p.md)。

外部平台权限、反向 WSS、签名回调和持久重放见 [节点管理](node.md)。
