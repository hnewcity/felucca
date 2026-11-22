# felucca-agent 模块 · The felucca-agent module

本页说明怎么部署和配置 `felucca-agent`:连接 server、证书信任、重连、出口网络、
上游代理、日志和多实例。

This page covers deploying and configuring `felucca-agent`: connecting to the server,
certificate trust, reconnects, egress networking, upstream proxies, logging and multiple
instances.

## 最小可用配置 · A minimal configuration

```toml
server = "bridge.example.com:14028"
key_id = "agent-1"
vkey = "change-me-to-a-long-random-string"
transport = "tls"          # tcp | tls | ws | wss | quic | kcp

[trust]
mode = "pinned"
fingerprint = "0000...0000"   # 来自 server 的 --fingerprint

[[tunnel]]
mode = "tcp"
port = 8080
[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`server` 写主机名或 IP 加端口。`key_id` 必须对应服务端的 `[client.<key_id>]` 表名。

## 传输与信任 · Transport and trust

`transport` 可选 `tcp`、`tls`、`ws`、`wss`、`quic`、`kcp`。公网桥接优先用
TLS/WSS/QUIC;`tcp` 和 `ws` 有身份校验但不加密传输内容,是明确选择的明文传输。

`[trust]` 决定怎么验证 server 证书:

| `mode` | 行为 |
| --- | --- |
| `webpki` | 正常的公开 CA 验证,不填 `fingerprint` |
| `pinned` | 固定整张证书的 SHA-256;验证握手签名,不验证域名和有效期;换证时必须同步更新指纹 |
| `insecure` | 仅用于临时诊断 |

配了 `bridge.tls` 固定 PEM 后,用 `felucca-server --fingerprint` 取得指纹。

## 连接与重连 · Connection and reconnection

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `reconnect_min` | — | 退避下限 |
| `reconnect_max` | — | 退避上限 |
| `dial_timeout` | — | 单次拨号超时 |
| `max_connections` | — | 该 agent 的连接上限 |

断开后自动重连,监听器和会话状态由 server 重新注册;流量计数跨重连保留。

## 出口网络 · Egress networking

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

| 字段 | 说明 |
| --- | --- |
| `local_ip` | 绑定桥接出口 IP;必须是本机实际拥有的地址,不能填接口名 |
| `forward_local_ip` | 后端连接和健康探针也使用该地址;默认只绑桥接 |
| `dns` | 字面量 `IP:port` 列表;为空用系统解析器。按 TTL 缓存,支持 A/AAAA 和 UDP 截断后 TCP 回退 |
| `network.proxy` | 上游代理,只承载桥接 TCP/TLS/WS/WSS,不转发后端请求和 NTP UDP |

target ACL 对**实际解析到的 IP** 执行检查,再连接这些已检查地址,不会二次解析引入绕过。
`local_ip` 与 DNS 地址须同族。QUIC/KCP 桥接不能同时使用 TCP 上游代理。

## 时间校准 · Time synchronization

server 和 agent 都可以配 `[ntp]` 校准桥接认证时间:

```toml
[ntp]
server = "time.example.com:123"
interval = "5m"
timeout = "5s"
max_offset = "24h"
```

默认不查询时间服务。`interval` 范围 1 秒到 24 小时,`timeout` 为正且不超过 30 秒并小于
`interval`,`max_offset` 为正且不超过 7 天。首次失败使用系统时间;校准不修改系统时钟,
TLS 证书有效期和配额到期仍用系统时间。NTP 源应由部署方信任,普通 SNTP 不提供时间源
身份认证。

agent 的 NTP 使用自己的 DNS/出口配置;server 用系统 DNS 和默认出口。

## 集中配置接收 · Receiving centralized configuration

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

`remote.enabled` 必须显式开启,且主连接必须使用已验证证书的 TLS/WSS/QUIC;明文连接不能
索取配置。`remote.target_policy` 是 agent 本机的硬边界,与每条下发隧道取交集。
`remote.file_root` 预先声明命名目录能力,server 只能下发能力名以内的路径,且只读目录不能
通过远端配置升级为可写。详见 [管理界面与集中配置](management.md)。

设置 `remote.console_scope = true` 后，服务端 `[managed.<client-id>]` 中的 `target_policy` 会替换本机的 `remote.target_policy`，可在管理页面的设备编辑中修改；服务端不提供时仍使用本机范围。未开启时，带 `target_policy` 的下发配置会被拒绝。文件目录能力始终只由本机配置决定。控制台生成的连接包默认开启此项。

## 日志 · Logging

```sh
felucca-agent -c agent.toml --log-level debug --log-file /data/agent.log --log-json
```

设置了 `--log-file` 就默认启用 10 MiB、5 份归档。日志文件追加写,同时输出 stderr;
父目录必须存在。`--log-max-bytes`、`--log-backups`、`--log-rotate-interval` 需要
`--log-file`。原始密钥不会进入日志。

## 多实例 · Multiple instances

```sh
felucca-agent --config office.toml --config home.json --check
felucca-agent --config office.toml --config home.json
```

一个进程最多 32 个实例,详见 [配置来源与启动参数](configuration.md)。

## P2P 与访客开关 · P2P and visitor switches

| 表 | 作用 |
| --- | --- |
| `[p2p] enabled = true` | 允许该 agent 作为提供端接受直连 |
| `[[visitor]]` | 在自己机器上监听,访问别人的私有共享 |

见 [P2P 直连](p2p.md) 和 [私有共享与访客](private-shares.md)。

## 下一步 · Next steps

- [隧道与端口映射](tunnels.md)
- [后端、负载均衡与健康检查](backends.md)
- [运维](operations.md) — 安装成系统服务
