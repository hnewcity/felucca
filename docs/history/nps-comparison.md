> 历史记录：保留原始开发背景与当时的验证结果，不作为当前开发指令或版本状态。当前架构见 `docs/architecture.md`，验证状态见 `docs/validation.md`。

# nps 源码对照审计

> 这是补齐工作开始前的历史审计。用户随后授权实施全部缺项，包括原阶段之外的
> P2P、Web 管理和 KCP；只有 Go 线协议兼容仍排除。最新状态与验收依据见
> [全功能补齐实施记录](implementation-status.md)，下文保留原审计观察。

日期：2026-09-16。对照目录：本地 nps Go 源码检出。
对照提交：`259b7efafb7b0c20802261b7c9abbf5ffe29c148`（2026-06-11）。nps 工作树干净，本次没有修改它。

## 结论与更正

**felucca 没有补齐完整 nps 功能，也不能把首期涉及的每个模块称为完整实现。**
上轮“首期剩余功能已补齐”的表述过宽：它把模块的基础通路可用，等同于模块功能已完成。
此前的 356 项测试验证了已有路径，没有覆盖 nps 的全部特性，更不是功能对齐验收。

现在准确的状态是：核心隧道与若干代理路径已经可用；HTTP/HTTPS、SOCKS5、ACL、mux、
证书和运维仍有明确差距。本次还通过新增失败用例确认并修复了两个已有实现的缺陷。

原先明确排除的 **Go 线协议兼容、P2P 和 Web 管理界面** 仍是范围约定，不应混同为首期漏做。
KCP 也已在架构设计中列为本阶段不移植。其他模块的子功能没有逐条约定排除，不能仅凭
“采用了不同设计”就默认已经达到 nps 的功能覆盖。

## 核对方式

检查了 nps 的模式注册、配置模型、代理处理器、目标策略、HTTP/SNI/证书代码、mux、
持久化、客户端启动入口和管理路由，并与 felucca 的配置字段、运行时调用和测试交叉核对。
下面的来源均指本地上述提交；不是根据网上另一版本的功能宣传推断。
这是功能与主要实现路径审计，不是对全部 Go/Rust 行做过逐行安全审计的声明。

## 首期模块对照

“已有基础实现”不代表与 nps 的参数、错误处理、性能或全部边界语义相同。

| 项目 | nps 源码依据 | felucca 当前状态 | 判定 |
| --- | --- | --- | --- |
| TCP/TLS/WS/WSS/QUIC 桥接 | `lib/conn/{tls,websocket,quic}.go` | 五种传输均有真实转发测试 | 已有基础实现 |
| TCP、UDP 独立端口转发 | `server/proxy/{tunnel_server,udp_server}.go` | TCP 半关闭；UDP 边界、空包、来源关联 | 已有基础实现 |
| SOCKS5 CONNECT | `server/proxy/socks5_handler.go:105` | IPv4/IPv6/DNS 目标，agent 侧拨号 | 已有基础实现 |
| SOCKS5 账号密码、多账号 | `server/proxy/socks5_auth.go:18` | 仅方法 0，无配置账号、认证或多账号 | **缺失** |
| SOCKS5 UDP ASSOCIATE | `server/proxy/socks5_handler.go:259`、`socks5_udp_session.go` | 命令 3 返回代码 7；独立 UDP 隧道不等价于 SOCKS UDP | **缺失** |
| HTTP 正代 / CONNECT | `server/proxy/http_proxy_handler.go:99` | 绝对 HTTP URL、CONNECT、混合入口 | 已有基础实现 |
| HTTP 代理认证 | `server/proxy/http_proxy_handler.go:88` | 没有认证，移除 Proxy-Authorization 后转发 | **缺失** |
| 来源 ACL | `lib/file/access_policy.go` | 客户端/隧道 IP、CIDR；含 IPv4 映射地址处理 | 部分：缺全局/用户层级、GeoIP 等 |
| 目标 ACL | `lib/file/access_policy.go:77`、`:148`、`:250` | 仅检查 `peer.ip()`；动态目标直接由 agent 拨号 | **缺失：来源 ACL 不能代替目标 ACL** |
| 域名与 IP 策略 | `lib/policy/{domain,ip,geoip,geosite}.go` | 无目标域名、正则、GeoIP/GeoSite 策略 | **缺失** |
| HTTP 精确/通配域名 | `lib/file/db_host.go`、`lib/index/host.go` | 精确优先，单层通配 | 基础实现，规则语义有差别 |
| 同域名 URL 路径路由 | `lib/file/db_host.go:88`、`lib/index/host.go` | 仅域名索引，同域名不能交给不同路径隧道 | **缺失** |
| 路径重写 | `server/proxy/httpproxy/http.go:385` | URI 只转换为 origin-form，没有配置式重写 | **缺失** |
| 后端 Host、自定义请求/响应头 | `lib/file/obj.go:191`、`httpproxy/http.go:275` | 自动来源头和逐跳头处理；无任意头/Host 改写配置 | **部分** |
| 站点 Basic Auth | `server/proxy/httpproxy/http.go:396` | 没有站点认证模型 | **缺失** |
| 强制 HTTPS、自定义重定向、CORS | `httpproxy/http.go:242`、`:286`、`:374` | 没有这些开关和处理路径 | **缺失** |
| HTTPS 终止、SNI 证书、HTTPS 后端 | `server/proxy/httpproxy/https.go:187` | SNI resolver、固定 PEM、TLS 后端与指纹验证 | 已有基础实现 |
| 按 SNI TLS 直通 | `server/proxy/httpproxy/https.go:222`、`:315` | HTTPS 监听先 TLS 终止，再解析 HTTP | **缺失**；固定 TCP 端口透传不能代替按 SNI 分流 |
| 非 HTTP TLS offload | `server/proxy/httpproxy/https.go:248`、`:340` | 未提供域名入口上的任意 TCP TLS 终止模式 | **缺失** |
| HTTP/2、HTTP/3 | `server/proxy/httpproxy/https.go:161`、`:681` | ALPN 仅 http/1.1，HTTP driver 仅 http1 | **缺失**；QUIC 桥接不等于 HTTP/3 |
| HTTP 流式传输、升级 | `httpproxy/http.go`、`http_backend.go` | 2 MB 流式往返、keep-alive、101 缓冲字节测试 | 已有基础实现 |
| 后端 HTTP 连接复用池 | `server/proxy/httpproxy/http_backend.go` | 每个请求新建后端连接 | **缺失**；客户端 keep-alive 不等于后端连接池 |
| ACME 签发、续期、缓存 | `server/proxy/httpproxy/httpproxy.go:176` | HTTP-01、本地账户/证书缓存、定时续期和热替换 | 已有基础实现；公网签发未验收 |
| 证书扩展与刷新 | `httpproxy/https.go:97`、`:173`；`lib/cache/cert.go:104` | 静态 PEM 只启动时读取；HTTP-01 需 HTTP 入口 | 缺静态证书自动刷新、443-only/TLS 挑战路径 |
| CA 专用接入 | `httpproxy/httpproxy.go:178`、`:192` | 可指定 directory；无 ZeroSSL API/EAB 参数 | **部分**，不能泛称全部 CA 接入已对齐 |
| 多后端与健康检查 | `lib/file/obj_tunnel_host_runtime.go`、`client/health.go` | 五种选择策略，TCP/TLS/HTTP 探测、摘除恢复 | 已有基础实现；本次修正大权重游标回绕 |
| 多路复用数据流 | `lib/mux/conn*.go`、`mux_io.go` | credit、背压、半关闭、RTT 保活 | 已有基础实现 |
| mux 动态窗口与会话预算 | `lib/mux/conn_receive_window.go:79`、`budget.go` | 默认固定 256 KiB 窗口，无动态增大过程 | **部分**，详见下方待完成项 |
| 端口范围批量映射 | `lib/config/config.go`；`docs/reference/features-routing.md` | 单隧道一个 port；allowed_ports 范围只是授权范围 | **缺失**，手写多条配置可以替代部分使用场景 |
| Proxy Protocol | `lib/conn/util.go:247`、`server/proxy/base_transport_runtime.go:671` | 没有 PROXY v1/v2 头收发路径 | **缺失**；HTTP X-Forwarded-For 不是同一能力 |
| 桥接与 HTTP/HTTPS 同端口 | `lib/pmux/{pmux,classifier}.go` | 只合并了几种桥接协议；HTTP/HTTPS 独立 bind | **部分**，同端口配置已实测启动失败 |
| 服务端本地目标、嵌套转发 | `lib/file/runtime_route.go`、`base_transport_runtime.go` | 后端一律 agent 侧拨号；无 tunnel://、bridge://、file:// | **缺失** |
| 来源/目标限制以外的配额 | `lib/file/obj.go`、`server/proxy/base_limit_runtime.go` | agent 级总量/速率/连接/隧道数、周期式截止 | **部分**：缺用户/独立隧道/域名层级配额 |
| 配置、流量跨重启持久化 | `lib/file/{jsondb,local_store}.go` | TOML 配置和 ACME 持久化，流量只跨 agent 重连保留 | **部分**：服务端重启后计数清零 |

felucca 对应核心入口：

- [SOCKS5](../crates/felucca-server/src/socks.rs)、[HTTP](../crates/felucca-server/src/http.rs)、[反代](../crates/felucca-server/src/reverse.rs)
- [来源准入](../crates/felucca-server/src/relay.rs)、[agent 拨号](../crates/felucca-client/src/agent.rs)
- [mux 配置](../crates/felucca-mux/src/config.rs)、[mux 会话](../crates/felucca-mux/src/session.rs)、[credit](../crates/felucca-mux/src/credit.rs)
- [证书签发](../crates/felucca-server/src/acme.rs)、[注册与监听](../crates/felucca-server/src/runtime.rs)

## 其他 nps 功能

这些功能不应被已有 TCP/HTTP 模块的测试计数掩盖；也没有在本次核对中顺手扩大为实施范围。

| 项目 | nps 依据 | felucca |
| --- | --- | --- |
| 文件服务 / WebDAV / 只读控制 | `client/client_channel_runtime.go:500` | 没有 file 模式 |
| secret 访客隧道 | `server/proxy/secret_server.go`、`client/control.go` | 没有 |
| P2P 打洞及回落 | `client/p2p_*.go`、`server/proxy/p2p.go` | 没有；原定首期之外 |
| tcpTrans 透明代理 | `server/server.go:568`、`server/proxy/transparent_tcp_handler_nonwindows.go` | 没有 |
| KCP | `lib/conn/kcp.go` | 没有；架构已选择本阶段不移植 |
| Snappy 压缩 / crypt 额外加密 | `lib/conn/conn.go:1012`、`lib/conn/util.go:37` | 没有；依赖中出现 snap 不等于已经接入 |
| Web UI / 用户和权限管理 | `web/views`、`web/service`、`web/routers` | 没有；原定首期之外 |
| 管理 API、批量、导入导出、事件 | `web/routers/router.go:349` 等 | 没有 |
| 多节点平台、reverse WS、callback、重放 | `web/service/node_{control,platform}.go` | 没有 |
| 多服务器、多实例、远程启动描述 | `client/launch*.go`、`docs/reference/npc-launch*.md` | 一进程一份 TOML、一台服务器 |
| 上游代理、自定义 DNS、出口 IP | `docs/reference/npc-cli.md:20`、`:21`、`:48`、客户端连接入口 | 没有对应运行配置 |
| 环境变量模板、NTP | `lib/config/config.go`、客户端启动入口 | 只有 FELUCCA_CONFIG/RUST_LOG 等少量环境入口 |
| 安装系统服务、自动更新、打包 | `lib/{daemon,install}`、安装脚本、Dockerfile、构建脚本 | 无服务安装/更新命令；无 Docker/跨平台发布验收 |
| 日志轮转、运行监控与分析 | `lib/logs`、pprof 和管理快照 | stderr/文件追加日志；无轮转和管理监控接口 |

## 这次发现并修复的两个缺陷

### TCP RST 留住后端与连接名额

复现条件：客户端总连接上限 1；TCP 后端接收请求后保持空闲；外部访问者用
`SO_LINGER=0` 关闭连接，产生 RST。

修复前，实际 release 程序观察到：后端 2 秒后仍无 EOF；第二次连接被 reset，
无法到达后端。新增端到端回归测试也在旧代码上失败。

原因：`copy::splice` 使用 `tokio::join!`，一方向 I/O 报错后，仍等另一方向永久空闲的读取。
`copy_metered` 的错误提前返回没有给对端 half-close，转发任务及 ConnectionGuard 也就不释放。

修复：改为 `try_join!`，正常 EOF 仍允许另一方向排空，但错误会取消另一方向。
回归测试验证了后端 EOF、连接名额归零，以及后续连接再次到达后端。
源码：[copy.rs](../crates/felucca-server/src/copy.rs)；
测试：[features.rs](../crates/felucca-cli/tests/features.rs) 的
`a_public_reset_releases_an_idle_backend_and_the_connection_slot`。

### 大权重的轮询游标提前回绕

两个后端均配置 `weight = u32::MAX` 时，总权重为 64 位，但游标仍是 AtomicU32。
游标从 `u32::MAX` 再推进会归零，没有走完权重环，造成分配失真。
新增用例在旧代码上实际失败（预期第二后端，实际第一后端）。

修复：游标改为 AtomicU64，并在取模后再转下标。
源码与测试：[pool.rs](../crates/felucca-client/src/pool.rs) 的
`weighted_selection_does_not_wrap_before_the_end_of_a_large_ring`。

## mux 和控制配置仍待完成

以下为源码审阅发现，本次没有把它们标记为已经修好：

1. `max_stream_window` 只参与配置归一化，接收窗口没有增长逻辑；`open_timeout` 没有运行时使用点。
   乐观打开是已有设计，但保留一个不生效的超时参数仍会误导调用者。
2. 初始窗口没有通过 OPEN/设置帧协商，两端都按各自配置假定对端窗口；默认一致可以工作，
   不能据此保证不同自定义窗口也能互通。
3. 出站是 unbounded channel。正常数据受 credit 约束，但 Ping/Pong 等控制帧没有独立队列预算；
   对端 WindowUpdate 的累计信用也没有与已发送数据核对。因此不能宣称具有无条件的会话内存上限。
   本次没有做内存耗尽实验；后续需要预算/协议约束设计，不能靠丢弃 credit 更新解决。
4. 握手中的 `keepalive_secs` 在 agent 侧没有传入 mux 配置；客户端仍用默认配置。
   来源：`felucca-proto/src/session.rs` 的返回值与 `felucca-client/src/agent.rs` 的 `shake`/`mux`。

## 可重复验证的观察

探测脚本 使用临时配置和回环监听，不改用户配置：

```sh
cargo build --release --locked --offline
python3 docs/audit/probe-runtime.py
```

修复前结果 包括：

- 只提供用户名密码方法时 SOCKS5 返回 `05ff`，说明没有该认证方法。
- UDP ASSOCIATE 返回 `05070001000000000000`，说明命令不支持。
- RST 后名额泄漏（本次已修复，脚本可重复验证）。
- 桥接与 HTTP 配为同端口时退出 1，错误是 Address already in use。

修复后结果 中，RST 后后端收到 EOF，第二次请求能够到达后端。
其中 `open without reply` 是探测用后端刻意不发送响应，不是转发失败。
认证、UDP ASSOCIATE 和同端口配置的缺项依旧存在，没有被这两处修复掩盖。

SOCKS5 BIND **不列为对齐缺项**：nps 的 `dispatchSocks5Request` 本身也拒绝它。
HTTP 缓存 **不列为必补项**：nps 的功能文档已把它标为弃用。
没有证据说明 nps 在此配置下支持 DNS-01/通配自动证书，故也不按这个假设增加缺项。

## 本次验证结果

- 两个新增回归用例均先在旧实现上失败，修复后通过。
- `cargo test --workspace --locked --offline`：358 项通过，0 失败。
- Clippy（所有 target，warnings 视为错误）及格式检查通过。
- Release 重新构建通过，实际进程探测再次确认 RST 修复。
- 五份示例配置、指纹稳定性、日志与 SIGTERM 退出检查通过。
- nps 源码未修改；本次没有对 nps 启动外部服务，也没有公网 CA 验收或跨平台压力测试。

## 后续验收顺序

1. 先完成代理认证、目标 ACL、mux 配置/预算问题，保证访问控制与资源边界明确。
2. 再完成 HTTP 路径路由/改写、Host/Header、站点认证、HTTPS 跳转、SNI 直通、证书刷新。
3. 补 Proxy Protocol、压缩、端口范围、持久化及部署产物，完成高并发/弱网/跨平台验收。
4. HTTP/2/3、file/secret、管理控制面等按完整 nps 对齐目标另列里程碑；P2P/Web UI 遵循原先范围约定。

每项要以具体行为和测试验收，不能再以“有这个模块”或“总测试数增加”判断完成。
