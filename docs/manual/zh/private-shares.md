# 私有共享与访客 · Private shares and visitors

私有隧道注册到 server,但**不占用任何公开端口**。另一个 agent 作为"访客"在自己机器上
开监听,通过已认证的加密桥接和独立共享密钥访问它。适合把内网服务只暴露给特定节点,
而不是整个公网。

A private tunnel registers with the server but occupies **no public port**. Another agent,
the visitor, opens a listener on its own machine and reaches it over the authenticated,
encrypted bridge using a separate share secret. Useful for exposing an internal service to
specific nodes rather than the public internet.

## 三方配置 · The three configs

提供端 agent、访客 agent 和 server 各改一处。

### 提供端:设为私有 · Provider: make the tunnel private

```toml
[[tunnel]]
id = 1
mode = "tcp"
public = false                 # 省略 port 和 bind
backend = [{ addr = "127.0.0.1:3000" }]

[[tunnel]]
id = 3
mode = "socks5"
public = false
[tunnel.target_policy]
allow = ["127.0.0.0/8", "::1", "10.0.0.0/8"]
```

支持 TCP、UDP、文件和代理入口。**HTTP 域名路由依赖 HTTP 监听,不能设为私有**;私有 TCP
可以转发任意 HTTP/HTTPS 后端。显式 `port = 0` 等价于私有。

### 服务端:声明共享 · Server: declare the share

```toml
[share.office-web]
client = "office"              # 节点名;服务端本机组用 client = "local.组名"
tunnel = 1                     # 稳定隧道 ID
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]     # 空列表表示允许持有该共享密钥的已认证节点
direct = true                  # 允许 P2P 直连尝试
```

`secret` 长度 16–4096 字节,各共享应不同。`acl_mode`/`acl_rules` 检查访客桥接连接的来源
地址。只有共享密钥**不能**作为匿名客户端接入,访客仍需普通节点身份和 vkey。
服务端可以用 `client.NAME.enabled = false` 持久停用一个节点。

### 访客:在自己机器监听 · Visitor: listen locally

```toml
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"                   # tcp | udp | socks5
max_connections = 64
udp_idle = "60s"
```

| 字段 | 说明 |
| --- | --- |
| `name` | 本 agent 内唯一 |
| `share` | 服务端共享名 |
| `mode` | `tcp`、`udp` 或 `socks5` |
| `bind` | 要求固定端口,启动时绑定,端口占用直接报错 |
| `max_connections` | 1–4096,默认 64;TCP 连接数或 UDP 来源会话数 |
| `udp_idle` | 1 秒–1 小时,默认 60 秒,双向活动都会延长会话 |
| `acl_mode` / `acl_rules` | 检查连接访客监听器的来源地址 |
| `target` | 仅动态共享可用,见下 |

全部访客还共享 agent 的连接上限,以及 8 MiB UDP 排队字节预算。访客只接受已验证证书的
TLS、WSS 或 QUIC。

服务端以访客桥接的**实际来源地址**执行共享 ACL、访客 ACL 和提供端的路由 ACL;
局域网内访问访客端口的原始地址不会被当作可信来源转交服务端。

## 访问效果 · What you get

按上面的例子:访问访客机器的 `127.0.0.1:18080` 会到达 office agent 的 `127.0.0.1:3000`;
UDP `127.0.0.1:15353` 对应 office 的 `127.0.0.1:5353`。

连接与原始流量同时计入访客和提供端及其用户/隧道资源;同一用户拥有两端时也会累计两端
用量。UDP 长度头和共享认证报文不计入业务流量。

## 动态目标 · Dynamic targets

服务端共享设 `dynamic = true` 时,访客可以指定目标:

```toml
[[visitor]]
name = "office-proxy"
share = "office-proxy"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:11080"
mode = "socks5"
target = "internal.example:443"
```

- 提供端须为 SOCKS5、HTTP 正代或混合代理隧道;UDP 目标要求 SOCKS5 或混合代理。
- 提供端的实际解析 IP 和目标策略仍会检查。
- 动态共享要求该代理入口**没有另配代理账号**,有账号时拒绝,避免共享凭据绕过代理自己的
  认证。
- 未配置 `target` 时接入完整服务入口,文件或代理账号仍须正常认证。
- TCP 访客可透传 SOCKS CONNECT 或 HTTP 代理会话;SOCKS UDP ASSOCIATE 返回的服务端 UDP
  地址不会自动映射为访客本地地址。

需要完整 SOCKS5 访问时,访客设 `mode = "socks5"` 并引用动态共享,无需 `target`:
SOCKS CONNECT 和 UDP ASSOCIATE 在访客侧解析,每个目的地经过共享认证和提供端目标策略。
`[visitor.auth.users]` 配置本地 SOCKS 账号,存在账号时不降级为匿名认证。
`max_connections` 同时计入 SOCKS 控制连接和 UDP 目标关联,因此 UDP 使用时至少设为 2。

## 管理与运行 · Management and runtime

- 管理端"共享访问"页面支持创建、编辑和删除共享;密钥默认脱敏,编辑留空保留原值。
- 操作员只能管理所属用户的提供端共享;只读账号不能更改。
- API 是 `/api/shares` 与 `/api/shares/NAME`,写入沿用配置版本和幂等请求键。
- 桥接断开后保留本地监听,现有会话结束,重连后新请求恢复。
- 共享配置变更通过正常 Daemon 重载生效,已有连接会中断。
- 当前这条路径经过服务端中继;要直连见 [P2P 直连与中继回退](p2p.md)。

## 验证清单 · Verification checklist

1. server `--check` 与两个 agent `--check` 都通过。
2. 访客 `127.0.0.1:18080` 返回提供端 `127.0.0.1:3000` 的内容。
3. 换错 `secret` 的访客连接被拒绝。
4. `allow_clients` 之外的节点被拒绝。
5. server 没有任何新增公开监听(`ss -ltnp` 对比)。

## 下一步 · Next steps

- [P2P 直连与中继回退](p2p.md)
- [felucca-agent 模块](agent.md)
- [管理界面与集中配置](management.md)
- 示例:[visitor-server.toml](../../../examples/visitor-server.toml) ·
[private-agent.toml](../../../examples/private-agent.toml) ·
[visitor-agent.toml](../../../examples/visitor-agent.toml)
