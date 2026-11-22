# P2P 直连与中继回退

此功能使用 v10 协议，需要同时更新服务端、提供服务的 agent 和 visitor agent。
当前开发代码已通过 11 项端到端专项及 558 项工作区回归；v10 release 的实际进程验收也已通过。

## 配置

配套配置见 `examples/p2p-server.toml`、`p2p-provider.toml` 和 `p2p-visitor.toml`。
替换节点密钥、共享密钥和服务端证书指纹，随后分别启动 server、provider agent、visitor agent。

服务端 `[p2p]` 提供独立 UDP 观察端口：`bind` 是本机监听地址，`advertise` 是两端都能访问的
外部 UDP 地址，不包含 URL scheme。它不能与 KCP、QUIC 或 HTTP/3 的 UDP 端口重叠。
`timeout` 为 500ms 至 10s，`max_pending` 为 1 至 4096，默认 5s / 128。
配置中的 127.0.0.1 仅适合所有进程都在同一主机的演示，跨网络时改成服务器公网域名和端口。

提供服务的 agent 通过 `[p2p] enabled=true` 允许直连，`max_sessions` 默认 64，上限 1024。
`bind_ip` 可固定本地 UDP IP，省略时采用 agent 的出口 IP 配置；它不改变后端拨号出口。
共享条目还必须设置 `direct=true`。服务端、本地 provider 和共享分别显式开启。
两端主连接都必须使用验证证书的 TLS、WSS 或 QUIC。

visitor 的 `connection` 可选：

- `relay`：默认，使用现有中继。
- `prefer_direct`：尝试直连，建立失败且还没有发送应用数据时回退中继。
- `direct`：只接受直连；不可用时关闭本地连接。

支持私有/公开 TCP、UDP 和受限文件服务，以及 dynamic share 上的 SOCKS/HTTP 正代目标。
visitor 本地仍可选择 TCP、UDP、SOCKS5。站点 HTTP/SNI 入口、服务端 local/嵌套路由和需要
单独代理账号的目标不绕过原入口；prefer_direct 会走原有中继，direct 会拒绝。
UDP 后端启用 PROXY 头时使用中继。TCP PROXY 头由 provider 按服务端认证的来源写入后端。

## 建连与访问控制

主连接先完成节点和共享密钥认证；服务端检查访问者身份、来源 ACL 和双方连接配额。
一次性票据绑定隧道、来源、目标、UDP 类型、编码选项和 PROXY 信息，不传递 provider 的节点密钥。
两端用不同的票据探测密钥观察 UDP 地址。服务端用短期、绑定来源地址的 cookie 确认回包路径，
限制有效探测次数、票据数量和生存时间；未知票据不回复。

provider 保持观察使用的同一个 UDP socket，并向 visitor 观察到的端点发送打洞包。
visitor 用 QUIC TLS 1.3 直连，对 provider 的临时证书固定指纹，再完成绑定票据和双随机数的双向 HMAC。
后端选择、健康状态、实际解析 IP 的目标 ACL、文件认证及目录能力句柄继续复用原服务引擎。

打洞失败先清理 provider 的尝试及双方名额，再允许中继连接。转发开始后的断开不会重放应用数据。
主连接断开、节点停用、配额耗尽或服务停止会关闭相关直连。

## 流量与资源

应用数据走两端之间的 QUIC，不经过服务端数据中继；主连接保留控制和计量消息。
provider 每方向最多保留一个等待记账的数据块，TCP 块不超过 16 KiB，UDP 不超过 65507 字节。
发送前由服务端检查并调度 provider/visitor 对应的流量及速率范围，发送后提交实际字节数。
服务端拒绝重复序号、重叠授权和超额提交。计数使用解码后的应用字节，不含探测、TLS、
压缩开销、UDP 帧头或 TCP PROXY 头。并发在途配额语义与中继一致。

直连计数依赖已认证 provider 的报告；服务端不能独立观察直连载荷。如果 provider 崩溃或控制连接
在最终报告前断开，最后已发送但未提交的块可能没有记入持久计数。正常关闭通过确认消息排空。
逐块额度请求还会增加对主连接 RTT 的依赖，因此不应把此实现描述为脱离服务端的无限期离线直连。

## 验证范围

已通过实际回环 TCP 2 MiB 半关闭、原始/压缩加密编码、UDP 空包和 50 KB 包、单连接名额复用、
双端原始计量、目标 IP 拒绝、独立代理账号拒绝、文件认证/只读/目录边界，以及配额耗尽不重放。
还验证了 provider 未开启、观察端口无回应，以及观察成功但 QUIC 被丢弃后的回退。
两组独立端口过滤 NAT 映射模拟已通过 2 MiB 往返，双方单独限速、RST/断线及 UDP 监听释放也通过。
真实公网 NAT 类型和运营商网络仍需部署验证；存在目的相关映射、UDP 禁用等情况时使用中继。

`python3 scripts/probes/probe-visitors.py --direct` 强制全部业务使用直连；release 实测
TCP 2 MiB、两来源 UDP、本地 SOCKS5 CONNECT/UDP、错误密钥和重启恢复，双端每方向
2,247,206 字节、五次 SIGTERM 退出 0，见 `audit/p2p-runtime.json`。
