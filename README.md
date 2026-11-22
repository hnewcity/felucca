# felucca

用 Rust 实现的 NAT 穿透与代理服务。内网运行 `felucca-agent`，由它主动连接公网的
`felucca-server`，服务器再把外部流量转发到内网服务。

Felucca 使用自己的通信协议和模块架构。当前协议版本为 10，服务端和 agent 需要一起升级。

面向使用者的技术文档见 [docs/manual](docs/manual/README.md)，从 Get Started 开始，
逐个模块说明怎么配置和使用。同一批页面有七种语言：中文、English、日本語、한국어、
Français、Español、Русский；代码块、字段名和命令在各种语言里完全一致。

项目结构见 [架构说明](docs/architecture.md)，功能范围见 [功能状态](docs/implementation-status.md)，
测试证据和部署验收边界见 [验证记录](docs/validation.md)。

## 支持的功能

| 功能 | 行为 |
| --- | --- |
| 隧道传输 | TCP、TLS、WebSocket、WSS、KCP（默认 10+3 FEC）；QUIC 使用原生多路流 |
| 多路复用 | 窗口协商/增长、会话与队列预算、背压、半关闭、RTT 自适应保活、断线重连 |
| 端口转发 | TCP/UDP、批量端口映射、PROXY v1/v2；UDP 保留报文边界、空包及来源隔离 |
| HTTP/HTTPS 反向代理 | 域名/路径/协议路由、HTTP/1.1/2/3、后端连接池、认证与重写、SNI 证书/直通/卸载、流式传输与升级 |
| 正向代理 | SOCKS5 CONNECT/UDP ASSOCIATE、HTTP 绝对 URL/CONNECT；混合入口、多账号认证和目标 ACL |
| 后端 | 多后端、TLS 后端、轮询、加权、最少连接、来源哈希、主备切换 |
| 健康检查 | TCP/TLS 或 HTTP 探测，连续失败摘除、连续成功恢复 |
| 本地与嵌套目标 | 服务端本地服务、tunnel/file/bridge 别名、逐跳权限和最多 16 跳环路控制 |
| 文件服务 | server 或 agent 本地目录、WebDAV、只读控制、多账号认证、Range 下载、目录边界保护 |
| ACME | HTTP-01 / TLS-ALPN-01、EAB/ZeroSSL API、账户和证书持久化、到期前续期、静态 PEM 与续期证书热替换 |
| 私有共享 | 无公开端口的 TCP/UDP/文件服务、双向共享密钥认证、本地 SOCKS CONNECT/UDP 访客 |
| P2P | 一次性票据、UDP 地址观察与打洞、固定证书 QUIC 直连、失败回落中继、双端配额与计量 |
| 外部平台管理 | 多平台权限、反向 WSS RPC、签名回调、持久队列与重放、流量导入、节点同步 |
| 运维 | 系统服务定义与命令、校验更新、日志轮转、Prometheus、CPU 采样、Docker/CI 构建配置 |
| 管理与集中配置 | 浏览器管理、租户权限、API/批量操作、持久化与回滚、远端下发、SSE 更新通知 |
| 启动配置 | TOML/JSON、远程配置与 Bearer 鉴权、base64/启动 URI、环境变量、多 agent 实例 |
| 网络出口 | 自定义 DNS、TTL 缓存、出口 IP、HTTP CONNECT/SOCKS5 上游代理、认证时钟 NTP 校准 |
| 数据流编码 | 每条隧道可选 Snappy 压缩、独立 ChaCha20-Poly1305 加密；保留半关闭和 UDP 边界 |
| 访问与限制 | GeoIP/GeoSite、四层来源 ACL、用户/客户端/隧道/域名配额、限速和连接数、流量快照、日志文件 |

## 编译与验证

需要 Rust 1.88 或更新版本及 Node.js 22.12+（推荐 24）。React + Vite 管理端位于
`apps/admin/`，构建产物嵌入服务端二进制，部署时不需要 Node.js。

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked
sh scripts/cargo-external.sh test --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

二进制位于 `target/release/felucca-server` 和 `target/release/felucca-agent`。
本轮进度和验收范围见 [实施记录](docs/implementation-status.md)。
测试覆盖现有实现；部署环境的验收边界见验证记录。
测试需要允许本机 TCP/UDP 端口绑定。无需连接公网 CA 即可运行 ACME 测试。

## 快速启动

连接已有公网 Server 时，在管理页面选择 **设备 → 接入设备**，填写名称并复制一行连接命令，
在本机终端（macOS/Linux）或 PowerShell（Windows）运行。命令自动选择客户端、下载并校验，
设备在线后选择 **添加服务**，填写目标地址即可。默认允许本机、局域网和互联网目标；需要限制时展开 **高级设置：访问权限**。不需要手写证书或配置。
保持终端窗口打开，断线自动重连。私有访问同样生成接收端命令，接收者无需管理后台账号。

自己部署时，在发行程序所在的可写目录首次运行：

```sh
./felucca-server --init
```

打开输出的管理地址，用生成的 root 密码登录。以后运行 `./felucca-server` 即可，保留
`felucca-server.toml` 及 `felucca-server.data/`。公网地址可用 `--admin-url` 指定；
HTTPS 反向代理、需要开放的端口和多平台客户端分发见[管理与部署说明](docs/management.md)。
命令包含设备凭据，只交给预期的使用者。高级配置和手动 ZIP 在次级入口按需使用。

### 开发者本地演示与手动配置

只想在本机体验管理页面和端口转发，可以直接运行（需要 Rust、Node.js/npm、Python 3 和 OpenSSL）：

```sh
python3 scripts/demo-local.py
```

脚本会构建 React 管理端并增量编译 debug 程序，生成证书和随机凭据，启动测试网站、server 和 agent，
验证真实转发后自动打开演示页面。终端会显示管理地址、账号和密码；按 Ctrl+C 停止全部
演示服务。所有监听仅绑定 `127.0.0.1`，默认端口被占用时自动选择空闲端口。
管理页面默认地址为 `http://127.0.0.1:8990`，实际地址以终端输出为准。
每次运行的配置、登录信息和日志保存在独立的 `target/local-demo/run-*` 目录中。
加 `--no-open` 可关闭自动打开浏览器；已编译后加 `--no-build` 可跳过构建。

以下命令在项目根目录执行。先生成一个固定的桥接证书；只做一次，并保留私钥文件：

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

修改两份配置的 `vkey` 为相同的随机密钥，例如用 `openssl rand -hex 32` 生成；
agent 的 `server` 改成服务器的可达地址。`key_id` 必须对应服务端的 `[client.<id>]`。
然后取得证书指纹，填入 agent 的 `[trust].fingerprint`：

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

分别运行：

```sh
target/release/felucca-server --config conf/server.local.toml
target/release/felucca-agent --config conf/agent.local.toml
```

示例把服务器的 TCP 8080 转到 agent 所在机器的 `127.0.0.1:3000`。启动对应后端服务后，
即可通过服务器的 8080 端口访问。

如果未配置 `bridge.tls`，启用运行配置存储（管理后台或 `--runtime-config`）的服务端会保存
`<运行快照路径>.identity.json`，重启后身份保持不变；未启用存储时才使用临时证书。
`--fingerprint` 读取显式 PEM；自动身份指纹可在运行中的管理页面和启动日志查看。
CA 签发的桥接证书可配合 `[trust] mode = "webpki"` 使用。公网桥接优先用 TLS/WSS/QUIC；
TCP/WS 有身份校验，但不加密传输内容。

## 其他配置

- [全部代理模式、负载均衡和健康检查](examples/features-agent.toml)
- [HTTP/HTTPS、SNI 和 QUIC 服务端](examples/features-server.toml)
- [ACME 服务端](examples/acme-server.toml)
- [共用端口及 HTTP/3 服务端](examples/shared-server.toml) / [agent 和批量端口映射](examples/shared-agent.toml)
- [ZeroSSL 与 TLS-ALPN-01](examples/zerossl-server.toml)
- [分层 ACL、配额与持久化服务端](examples/policy-server.toml) / [agent](examples/policy-agent.toml)
- [文件服务服务端](examples/file-server.toml) / [agent](examples/file-agent.toml)
- [上游代理、自定义 DNS 和 NTP](examples/network-agent.toml)
- [Snappy 与独立数据流加密](examples/payload-agent.toml)
- [KCP 服务端](examples/kcp-server.toml) / [agent](examples/kcp-agent.toml)
- [服务端本地目标和嵌套转发](examples/local-server.toml)
- [私有提供端](examples/private-agent.toml) / [访客端](examples/visitor-agent.toml) / [共享服务端](examples/visitor-server.toml) / [使用说明](docs/visitors.md)
- [管理服务端](examples/admin-server.toml) / [集中配置 agent](examples/managed-agent.toml) / [管理与 API 说明](docs/management.md)
- [外部平台与节点管理](docs/node.md) / [示例](examples/node-server.toml)
- [部署与配置说明](docs/configuration.md)
- [架构说明](docs/architecture.md)

配置路径均相对于进程工作目录。`--check` 检查配置结构、字段约束和 Geo 数据，不连接后端、
不验证公网 DNS，也不检查证书文件内容；证书内容和绑定冲突在启动时检查。
`RUST_LOG` 覆盖日志等级，SIGINT/SIGTERM 会关闭监听和当前连接。

## 当前边界

入口支持 HTTP/1.1、HTTP/2 和 HTTP/3，后端支持 HTTP/1.1 和 HTTP/2。
WebSocket 可在三种入口与两种后端之间转换，HTTP/2、HTTP/3 使用扩展 CONNECT。
桥接与 HTTP/HTTPS 可以共用 TCP 端口，QUIC 桥接与 HTTP/3 可以共用 UDP 端口。
SOCKS5 支持 CONNECT、UDP ASSOCIATE 和用户名密码认证，不支持 BIND。
正向代理支持来源 ACL 和目标 IP/域名/正则/GeoIP/GeoSite 策略。

ACME 可用 HTTP-01 或 TLS-ALPN-01，不签发通配证书。前者需要公网 TCP 80 到达 HTTP 监听，
后者需要公网 TCP 443 直达 TLS 监听；
公网 CA 的真实签发仍需在使用者的域名环境验证。账户及证书缓存须保留，staging 与 production
使用不同目录。反代和桥接静态证书默认每 30 秒重载，失败保留旧证书；固定指纹的 agent
需要同步更新配置中的指纹，已有连接不因换证而断开。

流量计数跨 agent 重连保留；配置 `[state]` 后跨服务端重启恢复，正常退出会保存最终快照。
未启用快照时重启清零。配额逐块/报文检查并周期截止空闲连接，存在并发在途流量超额。
管理 API/UI、运行配置持久化和远端配置下发已通过集成测试与真实浏览器验证。
私有共享和 P2P 已通过专项及实际进程验证。真实公网 NAT、公共 CA、Linux/Windows 服务安装、
Docker 与跨平台 CI 需要在对应部署环境继续验收；本机测试不能代替这些环境的结果。

P2P 直连、打洞与中继回退配置见 [P2P 说明](docs/p2p.md)。

## 开发

开发约定和回归命令见 [贡献指南](CONTRIBUTING.md)。项目早期参考了 nps 的功能需求，
相关资料保留在 [历史对照记录](docs/history/nps-comparison.md)；当前设计以 Felucca 的职责边界和使用需求为准，
不兼容 nps/npc 的通信协议。
