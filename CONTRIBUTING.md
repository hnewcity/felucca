# 开发指南

先阅读 [架构](docs/architecture.md)，按模块职责定位改动。
Felucca 使用自有协议，跨端协议变更必须一起检查 server 和 agent。

## 常用命令

管理端位于 `apps/admin/`，使用 React + Vite。首次构建需要 Node.js 22.12+（推荐 24）
和 npm；部署时仍只需 Rust 二进制。先安装锁定依赖并构建前端：

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run lint
npm --prefix apps/admin test
npm --prefix apps/admin run check:schema
npm --prefix apps/admin run build
python3 scripts/check-locales.py
```

前端源码变更后需重新运行 `npm --prefix apps/admin run build`。服务端构建脚本校验源码摘要，
产物缺失或过期时明确报错，不会静默嵌入旧界面。不要在 Cargo 运行期间修改源码或构建前端。

结构化配置表单来自 Rust 序列化模型。修改配置字段后运行
`python3 scripts/generate-admin-schema.py` 更新表单描述，再运行 `check:schema`；
服务端仍负责最终语义校验。

```sh
sh scripts/cargo-external.sh check --workspace --all-targets --locked
sh scripts/cargo-external.sh test --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
sh scripts/cargo-external.sh build --locked -p felucca-cli --bins
```

包装脚本将 Cargo 缓存和临时文件放在工作区所在磁盘。也可在有足够空间的环境直接用 Cargo。
同一 target 目录一次运行一个 Cargo 命令；构建、测试及 rustdoc 完成后再修改源码和内嵌页面。
网络测试需要本机 TCP/UDP 监听权限。

```sh
sh scripts/cargo-external.sh test -p felucca-core --locked
sh scripts/cargo-external.sh test -p felucca-cli --test features admin_features --locked
sh scripts/cargo-external.sh test -p felucca-cli --test features node_features --locked
sh scripts/cargo-external.sh test -p felucca-cli --test features peer_features --locked
```

前端开发可通过 `npm --prefix apps/admin run dev` 启动 Vite。默认代理到
`http://127.0.0.1:18080`，用 `FELUCCA_ADMIN_TARGET` 指向实际的本机 HTTP 管理监听；
代理保持 Cookie 会话与服务端 Origin 校验。目录与设计约定见
[`apps/admin/README.md`](apps/admin/README.md)。

修改 `apps/admin/` 后依次构建前端和 Rust 二进制。设备与服务向导的真实进程验证：

```sh
node scripts/probes/probe-product-flow.mjs
node scripts/probes/probe-product-flow.mjs --one-command
node scripts/probes/probe-one-command-browser.mjs
node scripts/probes/probe-server-init.mjs
```

它加载与界面共用的任务转换和 ZIP 打包函数，用真实 server/agent 验证带身份认证的
二进制下载、摘要、标准 ZIP 解压、原样启动脚本、公网与私有转发、域名入口、权限及重启恢复。
自动解压验证不等同于保留下载隔离标记的 macOS 首次启动验收。浏览器验收另行启动空环境：

```sh
FELUCCA_PROBE_OUTPUT=target/tmp/product-flow-browser \
node scripts/probes/probe-product-flow.mjs --interactive
```

根据输出目录的私有 `fixture.json` 登录，完成接入设备、下载连接包并启动包内 agent、
添加服务、生成另一台设备的私有访问包。使用目标操作系统的正常解压和启动方式，
记录 Gatekeeper/SmartScreen 的真实结果，不能通过关闭保护或删除隔离标记计为成功。
确认实时状态自动更新、表单不被刷新清空、
普通流程没有 JSON 编辑器，以及手机宽度无页面溢出。进程探针不能代替浏览器验收。
旧 `probe-admin.mjs` 的隧道/JSON 页面选择器尚未迁移，不作为新向导的通过证据。

`--one-command` 从空缓存运行页面原样命令，覆盖设备凭据下载、重复实例拦截、公开/私有
转发和默认身份跨重启恢复。`probe-one-command-browser.mjs` 在 Chrome 中创建设备和服务，
执行复制出的命令，并用真实在线会话和转发载荷判断成功；步骤标题不作为在线证据。
它们只验证本机平台与 loopback 网络，不能代替 Windows、Linux 或公网 NAT 验收。

在线变更测试在 `tests/features/managed.rs` 和 `peer.rs` 中覆盖 TLS/原生 QUIC、
保持已有流、快速停用再启用、错误配置与持久化失败、凭据轮换、独立私有授权撤销和直连取消。
浏览器操作时可用 `node scripts/probes/probe-held-service.mjs <port> <report-path>`
保持同一 HTTP/TCP 连接，结束时发送 SIGTERM；探针不会自动重连。

桌面托盘客户端位于 `apps/desktop/`，是独立工作区，命令见 [`apps/desktop/README.md`](apps/desktop/README.md)。

普通 CI 产物不代表已完成 macOS 分发验证。正式签名与公证由手动工作流
`.github/workflows/macos-notarize.yml` 执行，配置及边界见
[`docs/management.md`](docs/management.md#macos-发行--macos-distribution)。

构建保护可在源码副本上验证，不修改工作区源码：

```sh
python3 scripts/probes/probe-admin-build.py
```

其他进程探针位于 `scripts/probes/`。平台浏览器探针的可选 Python 依赖单独安装：

```sh
python3 -m venv target/probe-venv
target/probe-venv/bin/python -m pip install -r scripts/probes/requirements.txt
FELUCCA_PROBE_PROFILE=debug \
FELUCCA_PROBE_OUTPUT=target/tmp/node-audit \
target/probe-venv/bin/python scripts/probes/probe-node.py
```

平台探针同样使用本机 Chrome，不需要额外下载浏览器。Python 虚拟环境不参与产品构建。

协议矩阵探针只需标准库之外的两个客户端依赖，同样装进同一个虚拟环境：
`aioquic` 驱动 HTTP/3，`h2` 驱动 HTTP/2 的 WebSocket extended CONNECT。

```sh
python3 scripts/probes/probe-protocol-matrix.py
python3 scripts/probes/probe-protocol-features.py
```

两个探针各自启动真实的 server/agent 进程，只绑定 127.0.0.1，产物默认写在
`target/tmp/protocol-*-audit/`，可用 `FELUCCA_PROBE_OUTPUT` 覆盖。

改动默认桥接端口后，用真实进程复核默认值：不写 `[bridge]` 的 server、TOML 配置的
agent、`felucca://` 启动 URI 的 agent，以及一个仍指向旧端口的 agent：

```sh
python3 scripts/probes/probe-default-port-e2e.py
```

该探针同样只绑定 127.0.0.1，需要 `target/debug` 下的 server/agent 二进制，不依赖
额外 Python 包，产物默认写在 `target/tmp/default-port-e2e/`。

## 国际化 · Internationalisation

产品文案分三层，都以英文原文作为唯一标识：管理端界面文案、服务端返回的消息，以及
`docs/manual/` 下的文档。新增一种语言只需增加目录和目录文件，不需要改代码。

Text an operator reads lives in three places, and the English source is the identity in
each: console copy, server messages and the manual.

### 管理端 · Console

界面文案在 `apps/admin/src/locales/<语言>.json`，键是点分层级（`tunnels.empty.title`）。
`useLocale()` 提供 `t(key, args)`、`tag`、`number`、`dateTime`、`time`、`list`；
`{name}` 占位符由 `t` 填充，带 `{count}` 时按 `Intl.PluralRules` 选择
`key.one` / `key.other`。`en.json` 是基准，其他语言必须与它键集合完全一致。

```jsx
const { t, tag } = useLocale();
<h2>{t("tunnels.empty.title")}</h2>
<span>{bytes(value, tag)}</span>
```

### 服务端 · Server

服务端消息在 `crates/felucca-i18n/catalogs/<语言>.json`，键就是调用点上的英文原文，
因此不需要另造一套编号。动态值写成命名占位符：

```rust
bad(Message::with("user {id}: {reason}", [Value::debug("id", id), Value::nested("reason", e.message())]))
```

`en.json` 与源码中的消息字面量一一对应，`cargo test -p felucca-i18n` 会逐条比对：
源码里出现而目录没有的消息、目录里而源码已不再使用的消息，都会让测试失败。缺译时渲染
英文原文，不会启动失败。语言按 `Accept-Language` 协商，`X-Felucca-Locale` 可显式指定，
命令行用 `--lang` 或 `FELUCCA_LANG`；日志保持英文。

### 文档 · Manual

`docs/manual/<语言>/` 下是同一批 20 个页面，结构、代码块、字段名、链接目标与英文版
一致，只翻译说明文字。

### 检查 · Checks

```sh
python3 scripts/check-locales.py
sh scripts/cargo-external.sh test -p felucca-i18n --locked
npm --prefix apps/admin test
```

该脚本核对三处语言列表一致、每种语言键集合与英文一致、占位符一致、`docs/manual/`
各语言页面齐全，以及管理端源码里没有遗留的硬编码文案。

## 变更约定

- core 不依赖异步运行时，server/client 不互相依赖。
- 通信解析放 proto，传输放 transport，后端执行放 service。
- 管理业务放 control；HTTP/会话和平台传输负责适配。
- 配置保持 `deny_unknown_fields`，激活前完成校验。
- 保留双向认证、限额、任务取消与代际清理语义。
- 原始密钥不进入日志；持久化保留密钥，展示时才脱敏。
- 文件访问使用目录能力句柄；QUIC 使用原生流，不叠加 mux。
- 新测试复用 `tests/support/`，不通过其他测试模块导入设施。
- 源码检查、debug 进程验证与 release 验证分别记录。
- 历史检查点归档到 `docs/history/`，当前状态集中在功能状态和验证文档。
