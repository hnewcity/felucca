# P2P 直连与中继回退 · P2P direct and relay fallback

两端都在 NAT 后时,Felucca 可以尝试 UDP 打洞建立 QUIC 直连,失败再回落到服务端中继。
应用数据不走服务端,主连接仍保留控制和计量。此功能使用当前桥接协议版本,
**server、提供端 agent 和访客 agent 必须一起升级**。

When both ends sit behind NAT, Felucca can try UDP hole punching for a QUIC direct link and
fall back to the server relay. Application data bypasses the server; the main connection
still carries control and accounting. Server, provider agent and visitor agent must be
upgraded together.

## 服务端:观察端口 · Server: the observation port

```toml
[bridge]
bind = "0.0.0.0:14028"

[p2p]
bind = "0.0.0.0:8025"        # 本机监听
advertise = "203.0.113.10:8025"   # 两端都能访问的外部 UDP 地址,不带 scheme
timeout = "5s"
max_pending = 128
```

| 字段 | 默认 | 约束 |
| --- | --- | --- |
| `bind` | — | 本机监听地址 |
| `advertise` | — | 外部可达 UDP 地址,**不含** URL scheme |
| `timeout` | `5s` | 500ms–10s |
| `max_pending` | `128` | 1–4096 |

`advertise` 不能与 KCP、QUIC 或 HTTP/3 的 UDP 端口重叠。配置里的 `127.0.0.1` 只适合
所有进程都在同一主机的演示;跨网络要改成服务器公网域名和端口。

## 提供端:允许直连 · Provider: allow direct

```toml
# provider agent
[p2p]
enabled = true
max_sessions = 64        # 默认 64,上限 1024
bind_ip = "192.0.2.10"   # 可选,固定本地 UDP IP

[[tunnel]]
id = 1
mode = "tcp"
public = false
backend = [{ addr = "127.0.0.1:3000" }]
```

```toml
# server: 共享条目必须显式开启直连
[share.office-web]
client = "office"
tunnel = 1
secret = "replace-this-shared-secret"
allow_clients = ["laptop"]
direct = true
```

服务端、本地 provider 和共享分别**显式开启**(server 的 `[p2p]`、provider 的
`[p2p] enabled = true`、共享的 `direct = true`)。两端主连接都必须使用已验证证书的
TLS、WSS 或 QUIC。`bind_ip` 省略时采用 agent 的出口 IP 配置;它不改变后端拨号出口。

## 访客:连接偏好 · Visitor: connection preference

```toml
# visitor agent
[[visitor]]
name = "office-web"
share = "office-web"
secret = "replace-this-shared-secret"
bind = "127.0.0.1:18080"
mode = "tcp"
connection = "prefer_direct"    # relay | prefer_direct | direct
```

| `connection` | 行为 |
| --- | --- |
| `relay` | 默认,使用现有中继 |
| `prefer_direct` | 尝试直连;建立失败且还没有发送应用数据时回退中继 |
| `direct` | 只接受直连;不可用时关闭本地连接 |

## 建连过程 · How the link is established

1. 主连接先完成节点和共享密钥认证;服务端检查访问者身份、来源 ACL 和双方连接配额。
2. 服务端签发**一次性票据**,绑定隧道、来源、目标、UDP 类型、编码选项和 PROXY 信息,
   不传递 provider 的节点密钥。
3. 两端用不同的票据探测密钥观察 UDP 地址;服务端用短期、绑定来源地址的 cookie 确认
   回包路径,限制有效探测次数、票据数量和生存时间;未知票据不回复。
4. provider 保持观察使用的同一个 UDP socket,向 visitor 观察到的端点发打洞包。
5. visitor 用 QUIC TLS 1.3 直连,对 provider 的临时证书固定指纹,再完成绑定票据和
   双随机数的双向 HMAC。
6. 后端选择、健康状态、目标 ACL、文件认证和目录能力句柄继续复用原服务引擎。

打洞失败先清理 provider 的尝试及双方名额,再允许中继连接。转发开始后的断开**不会重放
应用数据。主连接断开、节点停用、配额耗尽或服务停止会关闭相关直连。

## 计量与限制 · Accounting and limits

- 应用数据走两端之间的 QUIC,不经过服务端数据中继。
- provider 每方向最多保留一个等待记账的数据块(TCP 块 ≤ 16 KiB,UDP ≤ 65507 字节)。
- 发送前由服务端检查并调度双方流量及速率范围,发送后提交实际字节数;拒绝重复序号、
  重叠授权和超额提交。
- 计数使用**解码后的应用字节**,不含探测、TLS、压缩开销、UDP 帧头或 TCP PROXY 头。
- 直连计数依赖已认证 provider 的报告;服务端不能独立观察直连载荷。provider 崩溃或控制
  连接在最终报告前断开时,最后已发送但未提交的块可能没有记入持久计数。正常关闭通过
  确认消息排空。
- 逐块额度请求会增加对主连接 RTT 的依赖,**不应**把它描述为脱离服务端的无限期离线直连。

## 支持的服务类型 · Supported services

支持私有/公开 TCP、UDP 和受限文件服务,以及 dynamic share 上的 SOCKS/HTTP 正代目标。
站点 HTTP/SNI 入口、服务端 local/嵌套路由和需要单独代理账号的目标不绕过原入口:
`prefer_direct` 会走原有中继,`direct` 会拒绝。
UDP 后端启用 PROXY 头时使用中继;TCP PROXY 头由 provider 按服务端认证的来源写入后端。

## 部署边界 · Deployment boundaries

存在目的相关映射、UDP 被禁用等情况时使用中继。真实公网 NAT 类型和运营商网络仍需在对应
部署环境验收;不要把本机模拟当作公网结果。

## 下一步 · Next steps

- [私有共享与访客](private-shares.md)
- [felucca-server 模块](server.md)
- [访问控制、配额与数据流编码](security.md)
- 示例:[p2p-server.toml](../../../examples/p2p-server.toml) ·
[p2p-provider.toml](../../../examples/p2p-provider.toml) ·
[p2p-visitor.toml](../../../examples/p2p-visitor.toml)
