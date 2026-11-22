# Felucca 架构

Felucca 是独立的 NAT 穿透、代理与远程服务系统。agent 主动连接 server，
server 将入口流量送到 agent 或服务端本机服务。协议、模块和管理能力按实际职责演进。
早期功能对照保存在 [历史记录](history/nps-comparison.md)，不作为目录或接口设计模板。

## 工作区边界

| crate | 职责与依赖边界 |
| --- | --- |
| `felucca-core` | 配置、标识、访问规则与计量基础类型；不依赖异步运行时或上层 crate |
| `felucca-mux` | 有界多路复用、窗口与背压；不依赖应用协议或配置 |
| `felucca-proto` | 双向握手、控制消息、访客与直连协议；依赖 core |
| `felucca-transport` | 连接适配、TCP/TLS/WS/QUIC/KCP 和出口网络；依赖 core/mux |
| `felucca-service` | 两端共用的后端连接、负载均衡、健康检查和受限文件服务 |
| `felucca-server` | agent 接入、入口路由、管理及运行状态；不依赖 client |
| `felucca-client` | agent 连接、重连、配置接收和访客；不依赖 server |
| `felucca-cli` | 两个程序、启动配置、日志、系统服务和更新；组装库与进程生命周期 |

依赖从调用方指向基础模块。新增文件优先归入既有职责；
只有出现独立的复用或依赖边界时才增加 crate。

## 服务端

```text
crates/felucca-server/src/
├── lib.rs                 库入口与稳定导出
├── daemon.rs              运行代际、配置切换、失败回滚
├── control/               身份、资源命令、权限、凭据与配置快照
├── admin/                 HTTP 路由、Cookie/CSRF、SSE、静态资源适配
├── platform/              外部平台 RPC、订阅、回调与队列
├── runtime/               服务组装、监听分流、会话、本机服务
│   └── registration.rs    注册状态、监听替换、过期会话清理
├── proxy/                 入口协议、HTTP 路由、转发与嵌套目标
├── accounting/            连接额度、配额、计量和流量快照
├── certificates/          ACME、ZeroSSL 与证书维护
├── http_support.rs        通用 HTTP body 和连接服务
├── registry.rs            在线客户端与其资源
├── peer.rs                直连协调
└── visitor.rs             私有服务访问
```

`Daemon` 拥有管理监听和当前 `Server`。`control::configuration` 判断修改是否只涉及设备、服务、共享和兼容的管理账号设置。此类修改先准备不可变策略并持久化，再发布 `runtime::live::LivePolicy`；不替换桥接监听和未改动的路由。全局监听等基础设置仍生成替换服务，排空旧服务，失败时启动准备好的回滚服务。

新版 agent 先用旧版认识的 `Ping` 和保留 nonce 探测能力。旧服务端原样回复 `Pong`，客户端保持原来的生命周期，不发送扩展；新版服务端回复 `ConfigChanged` 后客户端才声明 `WatchConfig`。随后用小型心跳检查变更，收到 `ConfigChanged` 才重新协商集中配置。两个枚举成员只追加，旧的序列化编号保持不变。旧版 agent 只重连改动所涉及的设备。客户端保留未改动服务的连接令牌，目录权限等准备失败不影响已有服务；共享权限撤销则取消对应的中继和直连会话。

`Registrations` 拥有注册表和它创建的监听。会话替换、注册、撤销和关闭由同一个组件协调；
旧会话退出不能移除新会话的路由。拆分时必须保留状态所有权和清理顺序。

## 管理业务与入口

`control::Principal` 是业务身份，不包含 Cookie 或 HTTP header。
`Resource`、`Write`、`Command` 表示资源及操作，不携带 URL。
URL 解析、Origin、CSRF、HTTP 状态转换属于 `admin` 适配器。

网页与平台配置操作调用同一个 `Management`，统一处理资源写入、批量操作、停用、同步、
版本检查、幂等与账号密码转换，最终交给 `Daemon` 完成配置事务。
各入口保留原始请求摘要的计算格式，使升级前的幂等记录仍可识别。
平台 HTTP RPC 复用管理适配器的鉴权与请求校验；队列与运行状态直接访问自身状态及 Daemon。

配置查询会脱敏；配置序列化本身必须保留凭据，否则持久化会破坏密钥。
新增凭据字段须同步检查脱敏路径。平台队列、配置和流量快照分别拥有自己的持久化事务。

`control::host_metrics` 拥有主机采样、管理员授权和有界内存历史；`admin::monitor`
只解析历史查询窗口。采样器跟随管理监听启动和取消，不随转发配置重载而重建。
系统查询在阻塞任务中执行，HTTP 请求只读取历史快照，不触发额外 OS 采集。

`control::configuration` 负责重启配置包与 token 生成，`control::operations` 负责受限证书落盘
和日志读取，`control::diagnostics` 负责租户过滤与有界 P2P 观察记录。后端诊断使用已认证
桥接上的可选只读流读取 `service` 的已有探测结果；证书状态来自现有维护任务。
管理适配器只处理 HTTP 参数、会话和响应，不另建配置事务或证书签发器。

## 共享配置与前端

`core/config/agent.rs` 负责 agent 身份、连接与启动配置；`tunnel.rs` 负责共享隧道类型及
规范化；`backend.rs` 负责后端、负载均衡和健康检查。本机服务和集中配置引用共享类型。
模块移动不改变 TOML/JSON 字段或通信编码。

管理端是独立的 React + Vite 应用，源码位于 `apps/admin/`：

```text
apps/admin/
├── src/app/               会话、导航、页面加载、React 上下文与语言选择
├── src/locales/           七种语言的管理端文案目录
├── src/pages/             概览、资源、隧道、监控、配置、平台
├── src/components/        通用组件、表单、确认窗口、指标图表与语言切换
├── src/features/          资源配置映射、监控轮询与时间序列处理
├── src/lib/               HTTP、CSRF、幂等请求、格式化与翻译函数
├── src/styles/            设计令牌与响应式样式
├── public/                图标、字体许可
├── build/                 构建源码摘要
└── dist/                  Vite 生成的 ES modules、CSS、字体（不入库）
```

界面文案集中在 `src/locales/*.json`，由 `useLocale()` 的 `t()` 读取；语言来自
localStorage、浏览器 `navigator.languages`，并随每个请求的 `Accept-Language` 送到服务端，
服务端用同一套语言代码回答自己的消息。服务端文案在 `crates/felucca-i18n/catalogs/`，
键就是英文原文。

页面用 hash 路由，无需服务端 SPA fallback。资源操作绑定页面读取时的配置版本；
编辑窗口保留自己的版本，SSE 提示刷新而不覆盖输入。会话保留在内存，身份由 HttpOnly Cookie
维护；配置写入继续携带 CSRF、If-Match 与幂等键。

先用 npm 构建前端，服务端 `build.rs` 校验源码摘要，再收集产物生成精确路径的静态资源表。
缺少产物或源码摘要不符时拒绝编译，避免交付旧界面。部署时无需 Node.js 或前端源码。
字体随二进制提供；CSP 只允许同源脚本、样式、字体，不开启内联脚本或 eval。

## 桌面客户端

`apps/desktop/` 是 macOS 与 Windows 的 Tauri 托盘客户端，独立 Cargo 工作区，直接链接
`felucca-client`，不依赖 `felucca-server` 或 `felucca-cli`。`felucca://` 启动 URI 的解析在
`felucca-core::config::launch`，命令行与桌面端共用。说明见 [`apps/desktop/README.md`](../apps/desktop/README.md)。

## 测试与文档

- 基础模块单测跟随实现；完整功能测试由 `cli/tests/features.rs` 显式注册。
- `cli/tests/support/` 保存公共网络、管理、平台及访客设施；测试不依赖其他测试模块。
- `scripts/probes/` 保存实际进程和浏览器探针；`docs/audit/` 保存本地验收产物（不纳入版本库）。
- 国际化检查 `scripts/check-locales.py` 核对管理端、服务端和 `docs/manual/` 的语言列表、
  键集合与占位符一致；`cargo test -p felucca-i18n` 核对服务端目录与源码消息一致。
- `docs/history/` 保存历史背景和旧检查点，不提供当前开发指令。
- 当前验证见 [验证记录](validation.md)，开发规则见 [贡献指南](../CONTRIBUTING.md)。
