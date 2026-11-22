> 历史记录：保留原始开发背景与当时的验证结果，不作为当前开发指令或版本状态。当前架构见 `docs/architecture.md`，验证状态见 `docs/validation.md`。

# 已有实现验证记录

验证日期：2026-09-16。平台：macOS Intel。最新完整回归对应
v9 透明 TCP、日志监控与运维阶段，539 项测试通过；对应 v9 release 已完成实际进程、
Chrome 与打包验收。当前正在开发的 v10 P2P 尚未纳入这次完整回归。

测试通过不代表完整 nps 对齐，也不代表首期模块子功能全部完成。
最新工作见 [实施记录](implementation-status.md)，原始差距见 [源码对照审计](nps-parity-audit.md)。

## 工具链

```text
rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)
binary: rustc
commit-hash: 48a229ceaefd4985c50990b14116b6d856af0985
commit-date: 2026-09-01
host: x86_64-apple-darwin
release: 1.98.1
LLVM version: 22.1.8
```

不需要 DYLD_LIBRARY_PATH 临时兼容设置。构建输出位于外置盘的项目 target 目录，
TMPDIR 设为 `target/tmp`，Cargo 缓存也已放到外置盘。
可用 `sh scripts/cargo-external.sh <cargo 参数>` 复用该环境。

## 自动检查

- `cargo test --workspace --locked`：**539 passed, 0 failed**。
- `cargo clippy --workspace --all-targets --locked -- -D warnings`：通过。
- `cargo fmt --all --check`：通过。
- `cargo build --release --locked`：通过，协议 v9。

| 测试位置 | 通过数 |
| --- | ---: |
| core | 136 |
| proto | 45 |
| mux 单元 / 预算集成 / 会话集成 | 70 / 6 / 16 |
| transport | 92 |
| server | 37 |
| service / client | 24 / 0 |
| CLI 库 / 核心端到端 / 功能端到端 / 运维进程 | 7 / 6 / 98 / 2 |

端到端测试包含真实 TCP/TLS/WS/WSS/QUIC/KCP 转发、并发与大文件、半关闭、重连恢复监听、
UDP 空包/50 KB 包/来源隔离、SOCKS5 IP 与 DNS、HTTP CONNECT、域名路由、来源头防伪造、
SNI/Host 冲突、固定证书 TLS 后端、HTTP 2 MB 流式往返/连接复用、101 升级中的缓冲数据。
其他回归测试覆盖 IPv4 映射地址 ACL、跨重连计数、并发限速债务、配置未知字段和信任模式拼写。
新增回归包括 HTTP/2/3、真实 IPv6 SOCKS UDP、共用端口、PROXY v1/v2、桥接证书刷新、
Geo 数据及四层来源 ACL、分层连接上限、域名 Host 改写隔离、原子快照和重启后配额拒绝。
后续回归覆盖文件/WebDAV 的权限和目录边界、六种 WebSocket 前后端组合、
QUIC/HTTP3 共用 UDP 及独立证书刷新、远程配置和多实例进程、
自定义 DNS 和上游代理、NTP 校准与过期/取消、Snappy 与独立加密的完整性和背压。
KCP/FEC 阶段日志在 `target/tmp/kcp-fec-workspace.log`、
`target/tmp/kcp-fec-clippy.log`。22 份示例通过 release 程序检查。

ACME 用本地模拟 CA 驱动真实 HTTP-01 请求、CSR 签名、凭据序列化恢复及证书/私钥匹配，
同时验证 TLS-ALPN-01、EAB HMAC、ZeroSSL API 返回解析、失败清理、缓存权限和续期计算。
**没有向公网 CA 申请证书**；
真实域名的 DNS、80/443 连通性与公网签发须在部署环境验证。

## v9 Release 程序验证

二十二份 examples TOML 全部通过 `--check`。通过 `python3 docs/audit/probe-state.py` 验证：

- 配置证书的 `--fingerprint` 等于 OpenSSL 导出 DER 的 SHA-256。
- TLS 桥接与真实 TCP 往返；agent 和 server 收到 SIGTERM 都以 0 退出。
- 用户、客户端、隧道分别记录 5 字节入站和 5 字节出站；正常退出保存最终数据。
- 启动第二组进程读取同一快照，已耗尽配额的用户仍被拒绝，计数保持不变。

保存结果见 实际进程验收。
本轮之前验证过的错误配置、临时证书指纹限制和日志行为仍有相应历史审计记录。

构建产物在 `target/release/`：

- `felucca-server`: 30,108,200 bytes
- `felucca-agent`: 14,671,208 bytes

`python3 docs/audit/probe-kcp.py` 另外验证真实 KCP 进程：默认 10+3 FEC，
压缩和加密开启，2 MiB TCP 半关闭往返、0/1/50000/13 字节 UDP 报文、准确原始字节计数、
双方 SIGTERM 退出码为 0。代理模拟丢包 219 次、重复 115 次、乱序 177 次；
观察到 858 个 FEC 校验包。记录和二进制 SHA-256 见 KCP 实测。

`python3 docs/audit/probe-local.py` 验证无 agent 的两跳本地转发、运行配置覆盖启动配置、
损坏快照拒绝、两次立即 SIGTERM 退出码 0，以及两个隧道和客户端分别正确计量。
这项实测发现 CLI 过晚注册退出信号的问题；现在在启动监听前注册 SIGINT/SIGTERM。
日志与结果见 本地进程实测。

## 管理端实际浏览器验证

`node docs/audit/probe-admin.mjs` 使用 release 服务端/agent 和隔离的 Chrome profile，
执行登录、隧道创建/编辑/停用/恢复、读取真实后端、脱敏导出和服务进程重启。
配置恢复、会话撤销均通过，JavaScript 异常数为 0，三个进程 SIGTERM 退出码均为 0。
结果与 SHA-256 见 `audit/admin-runtime.json`，桌面/手机截图在同目录。

## 访客实际进程验证

`python3 docs/audit/probe-visitors.py` 使用 v9 release，三条私有服务没有公开端口，
2 MiB TCP 半关闭、两来源 UDP 0/1/50000/13 字节包、本地 SOCKS 认证/CONNECT/UDP
和错误共享密钥拒绝通过。server 重启后保留访客监听并恢复访问；两端每方向业务字节
均为 2,247,206，错误密钥节点计数为 0。5 个进程退出码均为 0。
结果、计数及二进制散列见 `audit/visitor-runtime.json`。
Chrome 另外验证公开/私有切换、共享创建/编辑及密钥脱敏；`audit/admin-shares.png`
保存共享管理截图，JSON 记录当前验收的实际协议和 profile。

## 功能边界

详见 README 与 docs/configuration.md。当前开发源码使用协议 v10，两端一起升级，无 Go nps 兼容。
HTTP 入口支持 1.1/2/3，后端支持 1.1/2；WebSocket 的扩展 CONNECT 已通过互转测试。
SOCKS5 支持 CONNECT 和 UDP ASSOCIATE。管理 UI 已通过专项与真实 Chrome 验证，P2P 和其他审计项仍有待办。
启用 `[state]` 后流量跨服务端重启恢复；逐块/报文检查配额，允许并发在途超额。
这些验证不包含跨平台构建、长时间压力测试或公网 CA 部署验证。

## v9 运维回归

539 项完整回归日志：`target/tmp/maintenance-workspace-final.log`；clippy 日志：
`target/tmp/maintenance-clippy-final.log`。其中包括真实 CPU 采样、权限与并发拒绝、
停机取消，以及独立副本更新失败保护、并发锁、原子替换与重启执行。

首次完整回归的 KCP 重排测试超时，单独复现耗时约 17 秒。检查发现模拟器把一个包扣留到
下一个包到来，可能无限延迟窗口末尾的 ACK。已给重排设置 10ms 最大延迟；丢包和重复规则
保留，20 秒测试期限未放宽。专项用时约 4 秒，之后完整回归通过。


v9 实际进程验证另覆盖监控页面、真实 CPU SVG 下载和日志 JSON 轮转。
`audit/logging-runtime.json` 记录 23 个完整 JSON 事件、两个归档、独占写入、0600 权限和退出码 0。
发布归档为 `dist/felucca-0.1.0-x86_64-apple-darwin-release.tar.gz`；解包后实际执行两程序
`--build-info`，并核对内置 manifest 的 SHA-256，结果见 `audit/package-runtime.json`。
服务端 SHA-256：`f363281c730eab6d1b2b50cb94762f0813f8d5b1155b8409c7f78c943719e04f`；
agent SHA-256：`485cca6b250052aea307a600b55fab263ab8cfe67c3300fe39ee08c44f0181f9`。
Linux/Windows 系统服务、Docker 和远端 CI 仍没有实际运行验收。

## v10 P2P 回归

完整 workspace 558 项测试通过，0 失败，所有 rustdoc 完成；全目标 clippy 与 fmt 通过。
日志 `target/tmp/p2p-workspace-final.log`、`target/tmp/p2p-clippy-final.log`。
P2P 的 11 项实际网络专项包含两个端口过滤 NAT 映射、双端限速/配额、RST/断线回收和前后阶段回退；
详细能力和计量边界见 [P2P](p2p.md)。v10 release 已重新构建并通过以下实际进程验收；后续 node 平台改动不包含在该二进制中。


v10 实际 release：server 30,374,928 字节，SHA-256
`f9064feb7d0854e95510eba180f480e4e7c58a08ee18cd3184be7919f767abd1`；
agent 15,873,392 字节，SHA-256
`72218533436b043cf4b48c98999b7cc26e53a1f5112ed55732f31b7b6038becb`。
P2P 的独立进程强制直连验收见 `audit/p2p-runtime.json`；与普通中继分别执行，均验证 TCP、
UDP、SOCKS 和重启，双端每方向 2,247,206 字节。Chrome、25 份示例、状态/本地/KCP/日志回归通过。
