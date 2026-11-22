# 服务端本地目标与嵌套路由 · Server-local targets and nested routes

`[local.<name>]` 让 server 自己提供服务,不需要 agent,也不需要桥接凭据。
本地隧道的后端还可以写成别名,进入另一条已注册隧道(包括 agent 上的隧道),
从而组成嵌套路由。

`[local.<name>]` lets the server provide services itself, with no agent and no bridge
credentials. Local tunnel backends can also use aliases that enter another registered
tunnel, including tunnels on an agent, which composes into nested routes.

## 本地服务组 · A local service group

```toml
[user.office]
[user.office.limits]
max_connections = 100

[local.office]
user = "office"
dial_timeout = "10s"
[local.office.network]
local_ip = "127.0.0.1"
forward_local_ip = true

[[local.office.tunnel]]
id = 1
mode = "tcp"
port = 9001
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
[local.office.tunnel.health]
type = "tcp"
interval = "10s"
timeout = "2s"
```

每组可配 `user`、`acl_mode`/`acl_rules`、`limits`、`network`、`dial_timeout`,
并使用与 agent 相同的 TCP/UDP、HTTP/SNI、代理、文件、负载均衡、探针和 ACL 配置。
只有本地服务时可以省略 client 表。

本地组在注册表和流量快照中记作 `local.<name>`,**这个命名空间不能用作外部 agent 的
key_id**。本地组没有桥接,上游代理配置会被拒绝;`forward_local_ip = true` 才把出口 IP
用于后端。文件根目录、证书和后端配置在启动时准备,监听冲突会使该次服务启动失败。

## 后端别名 · Backend aliases

| 地址 | 行为 |
| --- | --- |
| `tunnel://local.office/7` | 进入另一个已注册本地隧道的服务入口 |
| `tunnel://agent-1/7` | 进入在线 agent 的已注册隧道 |
| `file://agent-1/7` | 同上,但要求注册目标确实为 file 模式;保留目录边界、只读和文件鉴权 |
| `bridge://tcp`、`tls`、`ws`、`wss` | 进入相应桥接入口,仍须 agent 双向认证;TLS 使用桥接证书 |
| `bridge://http` | 进入服务端 HTTP 入口,继续执行 Host、站点认证和权限检查 |
| `bridge://web` | 连接经过同样鉴权的管理端 |

隧道号必须显式稳定。指定 `user` 的本地组只能引用同一用户的目标;未指定 `user` 的组由
服务端操作者管理,可以跨用户引用。`bridge` 别名只允许操作者组。
远端 agent 和动态代理请求不能把别名当作网络目标使用。

别名不支持额外 `backend.tls`、网络健康探针或入口目标 ACL:权限由每一跳目标执行。
HTTPS/SNI 别名可直接传入 TLS 数据,由目标站点终止或直通。UDP 只能嵌套至 UDP,其他模式
使用流入口;SOCKS5/HTTP 代理鉴权不会被跳过。

## 环路与计费 · Loop control and accounting

- 每跳保留原始来源 IP/端口及路由历史。
- 重复 `(client, id)` 或超过 **16 跳**立即拒绝。
- HTTP 连接池同时按历史隔离,防止复用连接绕过环路检查。
- 每个实际经过的隧道分别取得连接名额并记录流量;同一用户/组经过多跳时,汇总额度也按
  每一跳计费。
- UDP 统计报文内容,不含内部长度帧。

## 典型用法 · Common patterns

| 场景 | 做法 |
| --- | --- |
| server 本机也要暴露一个服务 | `[local.ops]` + `[[local.ops.tunnel]]` |
| 给内网服务套一层 TLS 卸载 | 本地组 HTTP 隧道 + `tls_mode = "offload"` |
| 把内网服务再套一层入口(如加站点认证) | 外层本地组,backend 写 `tunnel://agent-1/7` |
| 从内网服务复用管理端鉴权 | backend 写 `bridge://web` |

## 下一步 · Next steps

- [隧道与端口映射](tunnels.md)
- [felucca-server 模块](server.md)
- [访问控制、配额与数据流编码](security.md)
- 示例:[local-server.toml](../../../examples/local-server.toml)
