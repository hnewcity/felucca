# 后端、负载均衡与健康检查 · Backends, balancing and health checks

本页说明怎么配后端、五种负载均衡策略怎么选,以及健康检查怎么摘除和恢复后端。

This page covers backend configuration, the five balancing strategies, and how health
checks evict and restore backends.

## 后端 · Backends

```toml
[[tunnel]]
mode = "tcp"
port = 9000
bind = "127.0.0.1"
balance = "round_robin"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
weight = 2

[[tunnel.backend]]
addr = "backend.internal:3001"
weight = 1
tls = true
[tunnel.backend.trust]
mode = "pinned"
fingerprint = "<backend SHA-256>"
```

| 字段 | 说明 |
| --- | --- |
| `addr` | 必填。主机名或 IP 加端口 |
| `weight` | `weighted` 策略下的正整数权重 |
| `tls` | 对后端启用 TLS,默认 WebPKI 验证,SNI 取地址里的主机名 |
| `trust` | 自签名后端用 `mode = "pinned"` + `fingerprint` |

`balance` 属于 `[[tunnel]]`,**不能**误写到 `[[tunnel.backend]]` 之后。
健康检查与数据连接使用相同的 TLS 配置。

## 负载均衡策略 · Balancing strategies

| 策略 | 选择方式 |
| --- | --- |
| `round_robin` | 在健康后端之间轮询 |
| `weighted` | 按正整数 `weight` 分配连接 |
| `least_connections` | 选择当前占用连接最少的后端 |
| `source_hash` | 按原始用户 IP 稳定选择后端 |
| `failover` | 按配置顺序选择第一个健康后端 |

选择发生在 **agent**。后端连接关闭或取消时释放计数;权重分配的是连接数,不是字节数。

行为细节:

- 所有后端都被标为不健康时仍会尝试一个,防止错误探针配置导致全面不可用。
- 单次连接失败会回报错误,**不在同一次请求内**重试其他后端。
- `source_hash` 使用来源 ACL / PROXY 协议验证后的原始地址。

## 健康检查 · Health checks

```toml
[tunnel.health]
type = "http"                # "tcp" | "http"
path = "/healthz"
interval = "10s"
timeout = "2s"
failures_to_evict = 3
successes_to_restore = 2
expect_status = [200, 204]
```

| 字段 | 说明 |
| --- | --- |
| `type = "tcp"` | 只检查能否建连 |
| `type = "http"` | 请求指定 `path`,默认接受 2xx |
| `expect_status` | 显式列出可接受的状态码 |
| `interval` | 探测间隔 |
| `timeout` | 单次探测超时 |
| `failures_to_evict` | 连续失败多少次后摘除 |
| `successes_to_restore` | 连续成功多少次后恢复 |

HTTP 探针的 Host 使用后端地址。连续失败摘除、连续成功恢复,并输出日志。

UDP 隧道和动态目标代理(socks5/httpproxy/mixedproxy)**不应**配置 TCP/HTTP 后端探针——
它们没有固定后端。文件模式同理,目标由目录能力句柄决定。

## 后端连接池(HTTP) · HTTP backend pooling

HTTP 隧道的后端连接池按客户端会话、隧道、来源 IP、目标和 HTTP origin 隔离:
每个 origin 最多 8 个闲置连接、30 秒闲置超时,外层最多 256 项。
启用发送 PROXY 头时还按来源端口隔离。HTTP/2 多请求可以复用一个连接。

`response_timeout`(server `[http]`)限制从发送请求到收到后端响应头的时间;慢速大文件
上传需要相应调大。响应体使用背压,不整体缓存。

## 下一步 · Next steps

- [隧道与端口映射](tunnels.md)
- [HTTP/HTTPS 反向代理](http-services.md)
- [访问控制、配额与数据流编码](security.md)
