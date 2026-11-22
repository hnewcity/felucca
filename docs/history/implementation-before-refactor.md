> 历史记录：保留原始开发背景与当时的验证结果，不作为当前开发指令或版本状态。当前架构见 `docs/architecture.md`，验证状态见 `docs/validation.md`。

# 全功能补齐实施记录

用户已授权补齐 `nps-parity-audit.md` 的全部缺项，包括 Web 管理、P2P、KCP。
仍使用 felucca 自有协议，不实现 Go nps/npc 线协议兼容。

此文件记录实施和验收结果；创建字段、空处理器、依赖或文档不能算完成。
历史审计保留当时结果，以下状态代表本轮进度。

| 工作 | 状态 | 验收依据 |
| --- | --- | --- |
| SOCKS5、HTTP 正代、站点多账号认证 | 已接入并测试 | `mixed_proxy_authenticates_each_account_and_never_downgrades`；站点认证由 HTTP 路由测试覆盖 |
| SOCKS5 UDP ASSOCIATE | 已接入并测试 | 空包/25 KB 报文、来源端口隔离、FRAG 丢弃、控制连接结束清理；真实 IPv6 控制连接和 UDP 往返通过 |
| 目标 IP、域名、正则、GeoIP/GeoSite；分层来源 ACL | 已接入并测试 | protobuf 数据集、属性筛选、内置 private、错误数据拒绝；全局/用户/客户端/隧道 ACL 交集；域名允许不能绕过解析 IP 拒绝，真实 TLS/QUIC 转发验证 |
| HTTP 路径/协议路由、重写、头部模板、重定向、CORS、错误页 | 已接入并测试 | 同域名最长路径、HTTP/HTTPS scheme 区分、Host/头改写、站点 Basic、307、CORS、自定义 502 端到端；301 端口/IPv6/查询串单测 |
| SNI 直通、非 HTTP TLS offload | 已接入并测试 | 同端口两张独立证书、原始 TCP 往返，验证直通/终止的证书归属 |
| HTTP/2、HTTP/3、后端连接池 | 已接入并测试 | HTTP/2 前后端、TLS h2、连接复用；HTTP/3 8 并发/2 MiB；WebSocket 三种入口 × 两种后端、2 MiB 帧、Ping/Pong、关闭、握手拒绝；修复 h3-quinn 待读取取消 panic |
| 静态证书刷新、ACME TLS-ALPN-01、EAB、ZeroSSL | 已接入并测试 | HTTP/桥接 PEM 先 key 后 cert 更新；TLS/QUIC 新连接使用新证书，旧隧道继续；真实 TLS 挑战、critical 扩展、EAB HMAC 与成功/失败清理；ZeroSSL API 解析、边界及凭据脱敏通过 |
| mux 窗口协商/增长、预算、credit 校验、超时与保活 | 已接入并测试 | 不对称窗口、增长/回收、伪造 credit、ACK 超时、发送阻塞取消、ping 洪泛、流 ID 重用；握手参数防篡改 |
| 批量端口、PROXY v1/v2、公共入口端口复用 | 已接入并测试 | 端口列表有序展开；可信入口头、来源 ACL、TLS 后端头、UDP v2；共用 TCP 端口四种桥接传输及 HTTP/HTTPS；QUIC 桥接与 HTTP3 共用 UDP，证书分别刷新且旧连接继续 |
| 本地目标、嵌套路由及环路控制 | 已接入并测试 | 无 agent 的 TCP/UDP、10 项嵌套端到端、PROXY 来源、文件鉴权、SNI 范围、四种桥接与取消清理、UDP 原始计量、跨用户及循环拒绝 |
| 用户/隧道/域名配额、流量与运行配置持久化 | 已接入并测试 | 四层计数、速率和并发名额；名额回滚、TCP RST 回收、Host 改写隔离；原子快照和文件锁；真实服务端重启后仍拒绝已用完额度的用户；显式稳定隧道 ID |
| 文件服务、WebDAV、只读控制 | 已接入并测试 | 独立端口和 HTTP/2 域名路径；多账号、Range、2 MiB 上传/下载、COPY/MOVE、LOCK/UNLOCK、只读；目录能力句柄、符号链接及并发替换目录回归通过 |
| secret 访客、私有隧道 | 已接入并测试 | 10 项访客端到端、共享管理租户权限；TCP/UDP/SOCKS、本地 UDP ASSOCIATE、文件权限、密钥重放拒绝、双端计量、停用/重连/RST/取消回收 |
| 透明 TCP | 本机测试通过，内核部署待验收 | Linux 原目标/TPROXY、macOS/FreeBSD PF；本机适配器专项与 agent 目标 ACL 验证；尚无实际防火墙重定向验收 |
| P2P 与回落 | 已接入并测试，真实公网 NAT 待部署验收 | 一次性票据、端点观察、固定指纹 QUIC；11 项真实网络专项，包括端口过滤 NAT 模拟、回退、权限/配额/速率/清理；完整 558 项通过 |
| KCP、Snappy、额外加密 | 已接入并测试 | 六种桥接 × 三种编码组合、UDP、PROXY 与原始计量；编码完整性、背压；KCP 丢包/乱序/重复、会话上限、重启、写后关闭；FEC 任意三包丢失、缓存上限及异常元数据专项通过 |
| Web UI、身份权限、多租户、管理 API、导入导出及事件 | 已接入并测试 | 5 项管理端到端、账号哈希专项；真实 Chrome 登录/增改启停、后端响应、脱敏、重启恢复、会话撤销，桌面和手机截图 |
| 多节点、反向 WS、callback、重试与重放 | 已接入并测试 | 8 项节点端到端：平台权限/WSS、心跳、回调签名与竞争重放、队列分页/丢弃、流量导入/kick/sync；574 项完整回归及真实进程/Chrome 验证通过 |
| 多服务器/实例、远程启动、上游代理、DNS/出口、环境模板/NTP | 已接入并测试 | 配置源、环境展开、多实例；HTTP/SOCKS5 上游鉴权与四种桥接；DNS TTL/TCP 回退、源地址和 ACL/探针；两端 NTP 校准、缓存失效、坏包及取消等待 |
| 日志轮转、Prometheus、CPU 采样 | 已接入并测试 | 并发日志完整性、归档保留、租户指标、真实 CPU 样本、权限/并发拒绝、停机取消 |
| 系统服务、更新、Docker、跨平台发布 | 已接入，部署验收待完成 | 三平台定义生成、macOS plist 解析、独立程序副本校验与原子更新；SCM/systemd 实机与 Docker/CI 尚未运行 |

最近完成的验证：

- 本地/嵌套转发与 Daemon 运行配置阶段：499 项测试，0 失败；clippy 与格式检查通过。
- 对应 v6 release 已重新构建。实际进程测试验证 17 份示例、流量与运行配置重启恢复，以及启动后立即 SIGTERM 的退出修复。
- v6 KCP/FEC 实测：2 MiB TCP、0/1/50000/13 字节 UDP、215 次丢包、113 次重复、171 次乱序，双方退出码 0。二进制大小/散列与流量数据见 `docs/audit/*-runtime.json`。
- 远端配置 v7：501 项完整回归通过；随后新增 6 项端到端专项全部通过，workspace clippy 通过。TLS/WSS/QUIC 下发、替换/停用/删除/恢复、持久化与重启、目标 ACL 和预授权文件目录边界已验证。
- 管理端加入后的完整回归：513 项，0 失败，日志 `target/tmp/admin-workspace.log`。Origin 规范化与编辑状态更新后，5 项管理专项再次通过。
- v7 release 已构建并通过真实 Chrome 端到端，结果 `docs/audit/admin-runtime.json` 记录 release profile 与散列；状态持久化、本地两跳、KCP 弱网真实进程测试再次通过。KCP 198 次丢包、104 次重复、164 次乱序、777 个 FEC 校验包，TCP/UDP 原始计数正确，进程正常退出。

- v8 访客阶段：526 项完整回归通过，clippy 与 fmt 通过；release 真实进程与 Chrome 专项通过，22 份示例、状态/本地/KCP 进程回归通过。进程计量各方向 2,247,206 字节，服务重启后本地访客端口恢复，5 次 SIGTERM 退出码均为 0。

共享后端位于 felucca-service，client 的 24 项测试随模块移动，没有重复计数。
开发源码注册协议现为 v10，server 和 agent 必须同时更新。磁盘 release 为已经实测的 v10 P2P 版本，
不能把当前源码的新功能算入该二进制验收。公网 CA、跨操作系统和真实 NAT 验收另行记录。

- v9 运维阶段：539 项完整回归、clippy 与 fmt 通过；release 已构建并通过实际进程、Chrome、日志轮转与归档解包执行。P2P 的 v10 改动未纳入该阶段验收。

- v10 P2P 阶段：558 项完整回归、全目标 clippy 和 fmt 通过；release 实际 P2P/访客/浏览器/状态/本地/KCP/日志回归通过，25 份示例有效。

- 节点控制阶段：574 项完整回归通过（含 rustdoc），全目标 clippy 与 fmt 通过。debug 实际 server/agent、WSS 平台、回调与 Chrome 验收通过；发布版正在重建，尚未记入本阶段发布验收。
