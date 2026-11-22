# 隧道与端口映射 · Tunnels and port mappings

隧道是 Felucca 的基本单位:一个公开入口加一组后端。本页说明公共字段、八种模式、
批量端口映射和 PROXY 协议。

A tunnel is the basic unit of Felucca: one public entry plus a set of backends. This page
covers the shared fields, the eight modes, batch port mappings and the PROXY protocol.

## 隧道公共字段 · Shared tunnel fields

```toml
[[tunnel]]
id = 1                 # 稳定 ID,启用流量快照后应固定
mode = "tcp"           # 入口协议
port = 8080            # server 上开放的端口
bind = "0.0.0.0"       # 只填 IP,不附带端口
remark = "internal web app"
public = true          # false 表示私有共享,不占公开端口
balance = "round_robin"   # 放在 [[tunnel]] 内,不能放到 [[tunnel.backend]] 之后

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`bind` 填 IP 不填端口;端口来自 `tunnel.port`。省略 `bind` 时监听所有接口。
`id` 未指定时分配一个可用的正整数;启用了流量快照就应显式固定 ID,不要只靠数组顺序。

## 模式总览 · Modes

| `mode` | 入口行为 | 目标来源 |
| --- | --- | --- |
| `tcp` | 公开 TCP 端口,原始字节转发 | `backend` |
| `udp` | 公开 UDP 端口,保留报文边界 | `backend` |
| `http` | 复用 server 的 HTTP/HTTPS/HTTP3 监听,按域名/路径路由 | `backend` |
| `socks5` | SOCKS5 正代,CONNECT + UDP ASSOCIATE | 客户端指定 |
| `httpproxy` | HTTP 正代,绝对 URL + CONNECT | 客户端指定 |
| `mixedproxy` | 混合入口,按请求开头区分 SOCKS5 与 HTTP | 客户端指定 |
| `file` | 文件服务与 WebDAV | agent 本机目录 |
| `transparent`(别名 `tcptrans`) | 从内核读取原始目的地址 | 内核 |

`http` 隧道**不**创建独立监听:它的 `port` 必须匹配 server 的 `http.bind`、`http.tls_bind`
或 `http.http3_bind`,不同 HTTP 隧道可以共用同一端口。`tunnel.bind` 对 HTTP 路由无效。
HTTP 监听由服务器决定启用与否,不受 agent 的 `allowed_ports` 限制。

## 端口映射 · Batch port mappings

`[[mapping]]` 只支持 TCP/UDP,把一段端口按顺序一一映射:

```toml
[[mapping]]
mode = "tcp"
server_ports = "9001-9003,10000"
target_ports = "8001-8003,8080"
target_host = "127.0.0.1"
bind = "0.0.0.0"
```

数量必须相等,禁止重复服务端端口;展开后的隧道与显式 `[[tunnel]]` 一起校验,仍受服务端
端口授权和隧道上限约束。`target_host` 可为域名、IPv4 或 IPv6,不能附带端口。
可设置 `tls`/`trust`、`health`、`acl`、`options`、`target_policy`。

## UDP 行为 · UDP behaviour

- 每个来源 IP:port 单独建立关联,并在 agent 侧固定一个后端 socket。
- 保留空包和报文边界,最大报文 65507 字节;实际网络可用 MTU 可能更低。
- 每个隧道最多 256 个来源关联,各排队 8 个包;队列满丢包,闲置 60 秒回收。
- 报文经可靠隧道流承载,**不是** QUIC DATAGRAM,所以丢包链路上会等待重传。
- UDP 统计只含报文内容,不含内部长度帧。

## PROXY 协议 · PROXY protocol

向后端发送原始来源和入口目的地址:

```toml
[tunnel.options]
proxy_protocol = "v1"     # "off" | "v1" | "v2"
```

TCP/HTTP/TLS 每条后端连接发一次;UDP 仅支持 v2,逐报文附加 DGRAM 头,含头超过 65507 字节
的包会被丢弃。后端必须能解析 PROXY 协议。

接收侧要为所属监听配置 `proxy_accept`:

```toml
[http.proxy_accept]
mode = "required"                    # optional | required
trusted = ["192.0.2.10", "2001:db8:1::/64"]
bridge.proxy_accept = { ... }
[tunnel.options.proxy_accept]
mode = "optional"
trusted = ["10.0.0.0/8"]
```

`optional` 允许无头直连,`required` 拒绝缺失头;启用时 `trusted` 不得为空。
来源 ACL、来源哈希和 HTTP 来源头都使用验证后的原始地址。同端口 bridge/HTTP 必须配置
一致策略。接收限 5 秒;UDP listener 不接收 PROXY 头。

## 每流压缩与加密 · Per-stream coding

```toml
[tunnel.options.payload]
compress = true     # Snappy,默认 false
encrypt = true      # ChaCha20-Poly1305,默认 false
```

适用 TCP/UDP、文件/WebDAV、HTTP 反代和正代数据流。启用压缩时先压缩后加密;每条流交换
新随机数,用该 agent 的 vkey 派生两个方向独立的密钥。两端不匹配时拒绝协商,不自动退回
明文。详情见 [访问控制、配额与数据流编码](security.md)。

## 下一步 · Next steps

- [后端、负载均衡与健康检查](backends.md)
- [HTTP/HTTPS 反向代理](http-services.md)
- [正向代理](forward-proxy.md)
- [文件服务与 WebDAV](file-services.md)
- [服务端本地目标与嵌套路由](local-services.md)
