# felucca-server 模块 · The felucca-server module

本页说明怎么部署和配置 `felucca-server`:桥接入口、客户端表、HTTP 监听、端口授权、
管理端、本地服务和外部平台开关。

This page covers deploying and configuring `felucca-server`: bridge listeners, the client
table, HTTP ingress, port authorization, the admin listener, server-local services and
external platform switches.

## 最小可用配置 · A minimal configuration

```toml
[bridge]
bind = "0.0.0.0:14028"
ws_path = "/ws"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }

[client.agent-1]
vkey = "change-me-to-a-long-random-string"

[log]
level = "info"
```

省略 `bridge.tls` 且启用运行配置存储时（包括 `--init` 或管理后台），server 会在运行快照旁保存 `<snapshot>.identity.json`。重启和备份时请保留该文件。未启用运行配置存储时身份仍是临时的。`--fingerprint` 读取显式 PEM；自动身份的指纹可在运行中的管理页面和启动日志查看。

```sh
target/release/felucca-server --config conf/server.toml --check
target/release/felucca-server --config conf/server.toml
```

## 桥接入口 · Bridge listeners

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `bridge.bind` | `0.0.0.0:14028` | TCP/TLS/WS/WSS 共用一个 socket,按握手特征分流 |
| `bridge.quic_bind` | 无 | 独立 UDP 地址,可用与 TCP 相同的端口号 |
| `bridge.kcp_bind` | 无 | KCP UDP 监听,不能与 QUIC/HTTP3 共用 UDP 端口 |
| `bridge.ws_path` | `/ws` | WS/WSS 路径,两端必须一致;其他路径拒绝升级 |
| `bridge.tls` | 无 | `{ cert, key }`,省略时生成临时证书 |
| `bridge.cert_reload_interval` | `30s` | PEM 重载间隔,更新不打断旧连接 |
| `bridge.proxy_accept` | off | 接收可信负载均衡器的 PROXY 头 |
| `[bridge.mux]` | 见下 | mux 窗口与预算 |
| `[bridge.kcp]` | 见下 | KCP 参数 |

`[bridge.mux]` 可调 `initial_stream_window`、`max_stream_window`、`max_session_window`、
`max_send_queue`、`max_control_queue`、`max_streams`、`accept_backlog`、`open_timeout`。
agent 侧对应 `[mux]`,字段相同。QUIC 原生流不使用这些参数。

`[bridge.kcp]` 接受 `mtu`、`send_window`、`receive_window`、`interval_ms`、`nodelay`、
`fast_resend`、`congestion_control`、`session_timeout`、`data_shards`、`parity_shards`、
`max_sessions`。默认 FEC 为 10 数据 + 3 校验,一组 13 个包可恢复其中任意 3 个丢失。
两端必须使用相同的 MTU 和分片数。

## 客户端表 · The client table

每个 agent 一个 `[client.<key_id>]`。`key_id` 是 agent 在 server 上的名字,agent 的
`key_id` 必须与表名一致。

| 字段 | 说明 |
| --- | --- |
| `vkey` | 必填,双向 HMAC 认证用的共享密钥,长度与随机性由部署方保证 |
| `remark` | 备注,显示在管理页面 |
| `user` | 关联 `[user.<name>]`,用于分层配额、ACL 和租户权限 |
| `enabled` | `false` 持久停用并断开该客户端 |
| `allow_config_push` | 是否允许该 agent 注册隧道;`true` 才能用 `[[managed.<id>.tunnel]]` 下发 |
| `max_tunnels` | 该 agent 的隧道上限 |
| `max_connections` | 该 agent 的连接上限 |
| `rate_limit` | 全部隧道、双向共享的字节/秒限速 |
| `acl_mode` / `acl_rules` | 访问该客户端公开入口的来源 ACL |
| `quota` | `{ flow_limit, expires_at }`,单位字节与 Unix 秒 |

相同 `key_id` 的新连接会替换旧连接,旧监听停止并释放后才能重新绑定。同一进程内的流量
计数与带宽限速器跨重连保留。

## 端口授权与监听器 · Port authorization

`allowed_ports` 是顶层字段,**必须写在第一个 TOML 表头之前**:

```toml
allowed_ports = [8080, { from = 9000, to = 9100 }]
```

空列表表示允许所有 >1024 的 TCP/UDP 端口。`max_tunnels` 与 `max_connections` 是顶层
按 agent 计算的全局上限。

## HTTP/HTTPS 监听 · HTTP ingress

```toml
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"      # 设成 "" 关闭 HTTPS
http3_bind = "0.0.0.0:8443"    # 可选,HTTP/3 UDP 入口
forwarded_headers = true
response_timeout = "100s"
cert_reload_interval = "30s"

[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"

[http.proxy_accept]
mode = "required"
trusted = ["192.0.2.10", "2001:db8:1::/64"]
```

完整用法见 [HTTP/HTTPS 反向代理](http-services.md) 和
[证书、SNI 与 ACME](certificates.md)。

## 其他顶层表 · Other top-level tables

| 表 | 用途 | 详见 |
| --- | --- | --- |
| `[admin]` / `[admin.account.<name>]` | 管理监听、账号与角色 | [管理界面](management.md) |
| `[state]` | 流量快照持久化 | [访问控制与配额](security.md) |
| `[user.<name>]` / `[user.<name>.limits]` | 用户维度配额与聚合 | [访问控制与配额](security.md) |
| `[local.<name>]` | 服务端本地服务组 | [本地目标与嵌套路由](local-services.md) |
| `[share.<name>]` | 私有共享授权 | [私有共享与访客](private-shares.md) |
| `[managed.<client>]` | 集中配置下发 | [管理界面](management.md) |
| `[node]` / `[node.platform.<name>]` / `[node.webhook.<name>]` | 外部平台 | [外部平台接入](platform.md) |
| `[p2p]` | P2P 观察端口 | [P2P 直连](p2p.md) |
| `[geo]` | GeoIP/GeoSite 数据文件 | [访问控制与配额](security.md) |
| `[ntp]` | 校准桥接认证时间 | [felucca-agent 模块](agent.md) |
| `[log]` / `[log.rotation]` | 日志与轮转 | [运维](operations.md) |
| `allowed_ports` | 端口授权 | 本页 |

## 常用命令 · Useful commands

| 命令 | 作用 |
| --- | --- |
| `--check` | 校验配置后退出,不绑定端口 |
| `--fingerprint` | 打印 `bridge.tls` 证书 SHA-256,需已配置固定 PEM |
| `--hash-password` | 从 stdin 读一行密码,输出管理密码哈希;密码至少 12 字节 |
| `--generate-token` | 输出管理 Bearer token 及其存储哈希 |
| `--runtime-config PATH` | 启用运行配置快照 |
| `--build-info` | 打印程序名、版本、协议与构建 target |

## 生命周期 · Lifecycle

- `SIGINT` / `SIGTERM` 关闭监听与当前连接后退出。
- 改启动文件需要重启进程;改管理端配置会走运行配置快照事务,会中断转发连接,但 agent 随后重连。
- 切换管理绑定、证书文件路径、公共 origin 或状态路径**必须重启进程**。
- 流量计数跨 agent 重连保留;配了 `[state]` 才能跨 server 重启保留。

## 下一步 · Next steps

- [felucca-agent 模块](agent.md) — 把内网服务接上来
- [隧道与端口映射](tunnels.md)
- [HTTP/HTTPS 反向代理](http-services.md)
- [运维](operations.md) — systemd / launchd / Windows 服务
