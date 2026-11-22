# 正向代理 · Forward proxy

Felucca 可以把内网机器的 SOCKS5 / HTTP 代理入口通过桥接暴露出来,给经过授权的客户端使用。
本页说明三种模式、账号认证、目标 ACL 和 UDP 行为。

Felucca can expose a SOCKS5 or HTTP proxy entry from behind NAT. This page covers the three
modes, account authentication, target ACLs and UDP behaviour.

## 三种模式 · Three modes

```toml
[[tunnel]]
mode = "socks5"
port = 9080
bind = "127.0.0.1"
acl_mode = "allow"
acl_rules = "127.0.0.0/8, ::1/128"

[[tunnel]]
mode = "httpproxy"
port = 9081
bind = "127.0.0.1"

[[tunnel]]
mode = "mixedproxy"
port = 9082
bind = "127.0.0.1"
```

| `mode` | 支持 |
| --- | --- |
| `socks5` | CONNECT + UDP ASSOCIATE;IPv4/IPv6;agent 侧 DNS 解析;BIND 返回"不支持命令" |
| `httpproxy` | 绝对 URL 请求 + CONNECT |
| `mixedproxy` | 按请求开头区分 SOCKS5 与 HTTP |

正代示例默认只绑 `127.0.0.1`。需要远程使用时**同时**设置来源白名单,否则任何能连到该
端口的人都能用你的出口。

## 账号认证 · Account authentication

在当前隧道下加账号表即可启用认证:

```toml
[tunnel.options.auth.users]
alice = "replace-with-a-password"
bob = "another-password"
```

| 入口 | 认证方式 | 失败响应 |
| --- | --- | --- |
| SOCKS5 | 用户名密码方法,**不会降级**到匿名 | 认证失败 |
| HTTP 正代 / CONNECT | `Proxy-Authorization: Basic` | 407 |
| HTTP 反代站点 | `Authorization: Basic` | 401 |

不配置账号表时允许匿名。已验证的代理/站点凭据会在发往后端之前移除。
配置和注册信息含账号密码,所以桥接应使用默认 TLS、WSS 或 QUIC;`tcp`/`ws` 是明确选择的
明文传输。

文件隧道使用 `tunnel.file.auth`,不能与 `tunnel.options.auth` 同时使用。

## 目标 ACL · Target policy

固定后端的隧道用 `target_policy` 限制实际目标。正代尤其需要它:

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

`allow` 为空表示不限制允许目标;`deny` 始终优先;`ports` 为空允许任意非零端口。
支持 IP/CIDR、`domain:` 精确域名、`suffix:` 后缀(含根域名和任意层级子域名)、`regex:`
正则,`*` 匹配所有目标。名称不区分大小写,末尾点会规范化。

先解析 DNS,再逐一检查**原名称与实际 IP**,连接只使用通过检查的地址,不二次解析;
允许域名不能绕过被拒绝的 IP。GeoIP/GeoSite 也支持,见
[访问控制、配额与数据流编码](security.md)。

## SOCKS5 UDP ASSOCIATE · UDP associate

- 只接受控制连接对端 IP:请求指定端口时固定该端口,否则绑定首个有效 UDP 包的源端口。
- 控制 TCP 关闭会清理全部目标关联。
- 每会话最多 64 个目标,各排队 8 个包,闲置 60 秒回收。
- 非零 FRAG 会被丢弃,不实现 SOCKS 分片重组。

## 带宽与配额 · Rate and quota

- `tunnel.options.limits.rate_limit` 是 agent 全部隧道、两个方向共享的字节/秒限制,
  允许一秒流量的初始突发。
- `quota.flow_limit` 累加两个方向;`quota.expires_at` 用 Unix 秒。
- 配额按周期检查并中止正在传输的连接,存在并发在途流量超额,计数不用于严格计费。

## 下一步 · Next steps

- [隧道与端口映射](tunnels.md)
- [私有共享与访客](private-shares.md) — 不带公开端口的 SOCKS 访客
- [P2P 直连与中继回退](p2p.md)
