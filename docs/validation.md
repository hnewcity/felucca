# 验证记录

> 原始验收产物（日志、截图、JSON 结果）只保存在维护者本地的 `docs/audit/`，未随开源仓库发布；下文提到的台账与记录均指这些本地产物。

本次可靠性问题验收已完成（2026-11-02），见完成验收台账。
最终工作区 634 项通过、0 失败、1 个原有文档示例忽略；Clippy 与格式检查通过。
协议矩阵 45/45、负载均衡/健康检查 9/9。macOS 两轮、Linux 一轮 UDP 六档矩阵均
完整；8 KiB/s 的双向 1 MiB 测试中 161/161 次小请求满足原 5 s 截止时间。
公网六协议的双向 4 MiB 均完整，KCP 上传 4.166 s，
下载 6.487 s。监听恢复、采样关闭和测试清理均通过。
失败基线、探针纠正、平台源码边界与实际发送速率保留在台账中。
以下 C 阶段与更早记录属于历史结果，其中遗留问题的最终处理以本次完成验收为准。

The completion audit records the resolved reliability failures.
The final workspace passes 634 tests, with no failures and one existing ignored doc example.
Static checks, the 45-case protocol matrix and nine load-balancing/health checks pass.
Two macOS and one Linux six-case UDP matrices receive every sent packet. All 161 short requests
finish within five seconds during real 8 KiB/s bulk transfers. All six public transports complete
4 MiB in each direction within the original 120-second deadline. Listener/profile shutdown recovery
and cleanup pass. Prior failures, fixture corrections and platform-specific provenance remain documented.
The C-stage and older records below are historical; this completion audit supersedes their outstanding issues.

C 阶段可靠性修复（2026-11-02）见拥塞误断线跟进：
修复发送排队与存活判断引起的误断线；macOS Rust 618 项、协议矩阵 45/45 通过。
8 KiB/s 限速下 1 MiB 文件完整传输且负载期间不重连。新 Linux release 公网复测中，
TCP/TLS/WS/WSS/QUIC 双向 4 MiB 均通过；KCP 下载通过，上传仍超过 120 s 超时。
UDP 突发与弱网请求延迟限制仍在，不放行大规模生产就绪。

此前 A/B 阶段见可靠性审计：修复监听恢复、采样关闭竞争
和最少连接并发选择，完成 1,000 设备与隧道、故障恢复及持续负载测试。
该阶段的规模与 Linux 单测计数保留为当时源码证据，不算 C 阶段重新运行。

最新默认范围验证（2026-10-28）见默认允许所有网络：
普通接入和简短启动 URI 默认允许本机、局域网及互联网，高级设置保留限制。
实际验证默认公网 HTTPS 代理、仅本机限制的 403 拒绝、取消限制后恢复及 Chrome 操作。

最新跨系统网络验证（2026-10-27）见Lima 实测台账：
重新构建的 Mac debug / Linux x64 release 完成 128 项实际检查，包含跨 VM 网站、TCP/UDP、
公网 HTTPS 代理、Chrome 视频播放与跳转、持久证书和重启恢复。
Linux 扩展探针修复了两项被端口冲突跳过的问题，完整结果为 56/56。

最新产品接入验证（2026-10-27）见一行命令接入台账：
涵盖默认身份持久化、命令下载安装、公开/私有转发和重启恢复。下列内容保留为 2026-09-17
结构重构基线；不把旧版结果视为当前发行版本的公网或跨平台验收。

验证日期：2026-09-17。环境：macOS Intel，Rust/Cargo 1.98.1。
本轮对应 [结构重构后的架构](architecture.md)，通信协议仍为 10。
实际进程验证使用本轮重新构建的 **debug** server/agent；已有 release 产物不计入本轮验收。

## 本轮结果

| 检查 | 结果 | 依据 |
| --- | --- | --- |
| 全工作区测试，含 rustdoc | **581 通过，0 失败，0 忽略** | 完整日志 |
| 全目标 Clippy，警告视为错误 | 通过 | 日志 |
| `cargo fmt --all --check` | 通过 | 汇总记录 |
| server/agent 构建 | 通过 | 构建日志 |
| 前端模块解析、导入及依赖图 | 15 个模块全部解析；导入可解析；无循环依赖 | 汇总记录 |
| 管理端真实 Chrome | 登录、隧道增改启停、公私切换、共享脱敏、监控/CPU 下载、重启恢复通过；无 JS 异常 | 浏览器结果 |
| 外部平台真实进程及 Chrome | WSS/RPC、租户隔离、签名回调、队列恢复、幂等重放、流量导入、停用与 WebSocket 会话通过；无 JS 异常 | 平台结果 |
| 配置与进程持久化 | 26 份示例通过；证书指纹与 OpenSSL 一致；SIGTERM、重启后的计量和配额恢复通过 | 进程结果 |
| 协议矩阵：六种 bridge 传输与载荷协议 | **45 通过，0 失败**：TCP/UDP、HTTP/WS/WebSocket/h2/h3、SOCKS5、HTTP 代理、mixed、文件、压缩加密载荷、私享与访客 | 运行时结果 |
| 协议功能矩阵：入口、后端与网络特性 | **54 通过，0 失败**：WebSocket over h2/h3、HTTP/2 后端、TLS 后端（pinned/webpki/错指纹）、CONNECT 内 TLS、代理认证、PROXY v1/v2 进出、上游链路与 remote_dns、自定义 DNS、NTP 校时、IPv6、五种负载均衡、健康检查摘除与恢复、半关闭、配额与限速 | 运行时结果 |

机器可读汇总记录本轮范围。
三组实际进程结果记录的二进制 SHA-256 一致。
管理概览、手机页面
和平台页面截图保存在同一目录。

## 重构覆盖

原有 579 个测试声明全部保留。新增两项不经过 HTTP 的管理业务测试，分别验证：

- 类型化命令的租户权限、批量原子性、只读限制、停用和同步权限。
- 删除成功后跨进程重启的幂等重放，以及变更摘要和过期版本的拒绝。

协议源文件、Cargo 依赖清单及锁文件未改变。新构建脚本只负责嵌入浏览器静态模块，
未增加产品运行时依赖。Python 平台探针的可选依赖单独记录在
[requirements.txt](../scripts/probes/requirements.txt)。

重构前首次并发基线运行曾在 `failed_config_commit_rolls_back_and_prepared_files_do_not_leak`
中发生回滚监听绑定失败；独立复跑通过。未放宽断言或增加超时，最终完整工作区回归通过。
这一记录用于区分基线现象与重构结果，不将独立复跑视为对并发原因的确认。

## 复现

```sh
sh scripts/cargo-external.sh test --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
sh scripts/cargo-external.sh build -p felucca-cli --bins --locked

FELUCCA_PROBE_PROFILE=debug \
FELUCCA_PROBE_OUTPUT=target/tmp/admin-audit \
node scripts/probes/probe-admin.mjs

FELUCCA_PROBE_PROFILE=debug \
FELUCCA_PROBE_OUTPUT=target/tmp/node-audit \
target/probe-venv/bin/python scripts/probes/probe-node.py

FELUCCA_PROBE_PROFILE=debug \
python3 scripts/probes/probe-state.py

python3 scripts/probes/probe-protocol-matrix.py

python3 scripts/probes/probe-protocol-features.py
```

两个协议探针各自使用真实的 server/agent 进程，只绑定 127.0.0.1。矩阵探针
覆盖六种 bridge 传输与各载荷协议；功能探针覆盖 WebSocket over h2/h3、
TLS 后端、PROXY 协议、上游链路、自定义 DNS、NTP、IPv6、负载均衡、健康检查、
半关闭与配额限额。产物默认落在 `target/tmp/protocol-*-audit/`，
可用 `FELUCCA_PROBE_OUTPUT` 覆盖；HTTP/3 与 h2 客户端需要虚拟环境里的
aioquic 与 h2。

Python 探针环境初始化见 [贡献指南](../CONTRIBUTING.md)。一次运行一个 Cargo 命令，
构建与 rustdoc 完成前不要修改 Rust 或嵌入的页面资源。

## 部署范围

本轮验证限于上述本机环境，未进行 release 重建、Linux/Windows 实机服务安装、
Docker 运行、系统防火墙变更、公网 NAT 或公共 CA 签发验收。
这些部署验证与本轮结构重构的本机回归分开记录。
旧阶段结果保留在 [历史验证记录](history/validation-before-refactor.md)。
