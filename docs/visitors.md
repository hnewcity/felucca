# 私有隧道和共享访问

私有隧道注册到服务端，但不占用服务端公开端口。访客 agent 在自己的机器监听
TCP 或 UDP 端口，经过已认证的加密桥接和独立共享密钥访问服务。

服务端使用 `examples/visitor-server.toml`，提供服务的 agent 使用
`examples/private-agent.toml`，访客使用 `examples/visitor-agent.toml`。
替换节点 vkey、共享 secret 和两端证书指纹后，访问访客机器的
`127.0.0.1:18080` 会到达 office agent 的 `127.0.0.1:3000`。
UDP `127.0.0.1:15353` 对应 office 的 `127.0.0.1:5353`。

这组功能引入于协议 v8，当前源码为 v9，两端同时更新；访客仅接受验证证书的 TLS、WSS 或 QUIC。
每个访客拥有普通节点身份和 vkey，服务端可用 `client.NAME.enabled = false`
停用它。只有共享密钥无法作为匿名客户端接入。

## 配置

- 隧道设置 `public = false`，省略 `port` 和 `bind`。显式 `port = 0` 也可。
  支持 TCP、UDP、文件和代理入口。HTTP 域名路由依赖 HTTP 监听，不能设为私有；
  私有 TCP 可转发任意 HTTP/HTTPS 后端。
- 服务端 `[share.NAME]` 指向一个节点名和稳定隧道 ID。服务端本机组使用
  `client = "local.组名"`。`secret` 长度为 16–4096 字节。
- `allow_clients` 列出允许访问的访客节点；空列表允许持有该共享密钥的已认证节点。
  `acl_mode` / `acl_rules` 检查访客桥接连接的来源地址。
- 访客 `[[visitor]]` 的 `name` 在本 agent 唯一；`share` 指向服务端共享名；
  `mode` 为 `tcp`、`udp` 或 `socks5`，`bind` 要求固定端口。
  本地 `acl_mode` / `acl_rules` 检查连接访客监听器的来源地址。
- `max_connections` 限制 TCP 连接数或 UDP 来源会话数，范围 1–4096，默认 64。
  全部访客还共享 agent 的连接上限，以及 8 MiB UDP 排队字节预算。
  `udp_idle` 范围 1 秒–1 小时，默认 60 秒，双向活动都会延长会话。

服务端以访客桥接的实际来源地址执行共享 ACL、访客 ACL 和提供端的路由 ACL；
局域网内访问访客端口的原始地址不会被当作可信来源转交服务端。
连接与原始流量同时计入访客和提供端及其用户/隧道资源；同一用户拥有两端时，
也会累计两端用量。UDP 长度头和共享认证报文不计入业务流量。

## 动态目标

服务端共享设置 `dynamic = true` 时，访客还可以配置
`target = "internal.example:443"`。提供端须为 SOCKS5、HTTP 正代或混合代理隧道；
UDP 目标要求 SOCKS5 或混合代理。提供端的实际解析 IP 和目标策略仍会检查。

动态共享要求该代理入口没有另配代理账号；有账号时会拒绝，避免共享凭据绕过
代理自己的认证。未配置 `target` 时接入完整服务入口，文件或代理账号仍须正常认证。
TCP 访客可透传 SOCKS CONNECT 或 HTTP 代理会话；SOCKS UDP ASSOCIATE 返回的服务端
UDP 地址不会自动映射为访客本地地址。

需要完整 SOCKS5 访问时，访客设置 `mode = "socks5"` 并引用动态共享，无需配置 `target`。
SOCKS CONNECT 和 UDP ASSOCIATE 在访客侧解析，每个目的地经过共享认证和提供端目标策略。
UDP 监听和返回地址都在访客机器上，TCP 控制连接关闭后释放全部关联；
绑定来源 IP/端口、拒绝分片，单控制连接最多 64 个 UDP 目标。
`[visitor.auth.users]` 配置本地 SOCKS 账号，存在账号时不降级为匿名认证。
`max_connections` 同时计入 SOCKS 控制连接和 UDP 目标关联，因此 UDP 使用时至少设为 2。
SOCKS 解析和认证与公网入口共用实现。固定 UDP 和指定 `target` 的动态 UDP 仍可使用独立 UDP 访客。

## 运行和管理

管理端的“共享访问”页面支持创建、编辑和删除共享，密钥默认脱敏，编辑留空保留原值。
操作员只能管理所属用户的提供端共享，只读账号不能更改。API 是
`/api/shares` 与 `/api/shares/NAME`，写入沿用配置版本和幂等请求键。

agent 在启动时绑定访客监听器，端口占用直接报错。桥接断开后保留本地监听，
现有会话结束，重连后新请求恢复；退出或取消 agent 任务会释放监听和会话。
共享配置变更通过正常 Daemon 重载生效，已有连接会中断。

当前这条路径经过服务端中继；P2P 打洞与自动回落仍在后续实施范围内。
