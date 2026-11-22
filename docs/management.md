# 管理与集中配置

## 首次启动 · First run

发行包包含程序和同平台客户端。在可写目录运行一次 `./felucca-server --init`，即可生成配置、
随机 root 登录密码并启动管理页面；无需手动生成密码哈希或桥接证书。默认管理地址是
`http://127.0.0.1:8990`。以后用 `./felucca-server` 启动，已有配置不会被 `--init` 覆盖。

公网部署可以指定 `--init --admin-url https://console.example.com`。HTTPS 地址表示使用
现有反向代理，后端监听 `127.0.0.1:8990`，代理须保留公共 Host。直接填写
`http://公网地址:8990` 则监听公网端口；公开使用应配 HTTPS。部署方需让设备能连接桥接
TCP 9001，并让访问者能连接所发布服务的端口。域名入口和 P2P 监听仍按需配置。

保留 `felucca-server.toml` 和完整的 `felucca-server.data/`，包括 `runtime.json`、
`runtime.json.identity.json`、`traffic.json` 及锁文件。自动身份损坏时启动失败，不会偷偷换证。
启动后网页修改保存到运行快照，已有快照优先于初始化文件。自定义 `--config` 时数据目录
位于该配置旁，扩展名改为 `.data`。初始密码只在创建时显示，请妥善保存。

Run `./felucca-server --init` once in a writable directory to create the configuration,
print a random root login and start the console. Restart with `./felucca-server`.
The default console is `http://127.0.0.1:8990`. For a public HTTPS reverse proxy preserving
Host and forwarding to `127.0.0.1:8990`, initialize with
`--admin-url https://console.example.com`. A direct HTTP origin binds publicly on its port;
use HTTPS for public access. Devices need bridge TCP 9001 and visitors need published service
ports. Preserve the configuration and its entire `.data` directory, including the automatic
identity. Existing runtime snapshots override the bootstrap file. Save the initial login;
it is only printed when the configuration is created.

## 手动配置 · Manual configuration

先构建当前源码。管理端通过 `Daemon` 运行，监听独立于转发服务：
设备、服务和访问权限修改不会重启全部转发服务。修改或停用的服务结束自身连接；未改动的服务、设备名称和服务名称修改保持连接。旧版 agent 只重连受影响设备。修改桥接、HTTP、P2P 监听等基础设置仍会重新加载转发服务。

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked
target/release/felucca-server --hash-password
```

`--hash-password` 从标准输入读取一行密码，输出供配置使用的哈希。
密码至少 12 字节；命令不接受明文密码参数。
复制 `examples/admin-server.toml`，替换示例密码哈希、节点密钥和端口，再运行：

```sh
target/release/felucca-server --config conf/server.json --check
target/release/felucca-server --config conf/server.json
```

配置文件可以是 TOML 或 JSON。示例管理地址为 `http://127.0.0.1:8990`，账号 `root`。
示例密码 `change-this-password` 仅用于本机演示，实际配置应替换为自己的密码哈希。

## 管理监听与身份

`[admin]` 指定 `bind`、`state_path`，可选 `tls.cert` / `tls.key`。
非 loopback 监听必须设置 `origin`，值为浏览器访问的 origin：推荐 `https://`；接受明文管理流量时可用
`http://host:port`（密码和会话令牌会以明文传输，Cookie 不带 `Secure`）。设置了 `tls` 时 `origin` 必须是 https。
HTTPS 可以由管理监听自身或前置反向代理提供；前置代理须保留公共 Host，
不要把未加密的后端管理端口直接暴露给浏览器。

`[admin.account.<name>]` 的 `role`：

- `admin`：管理全部资源和账号，不设置 `user`。
- `operator`：管理所属 `user` 的节点和集中配置隧道。
- `viewer`：只读所属 `user` 的资源。

操作员和只读账号必须关联一个已存在的用户。服务端本机服务可能打开本机目录，
因此仅管理员可修改。用户配额、管理账号及完整配置同样仅由管理员修改。
至少保留一个启用的管理员。设置 `disabled = true` 可停用账号。

密码使用带随机盐的 PBKDF2-SHA256 哈希，配置固定使用 600000 次迭代。
会话存储在内存中，Cookie 为 HttpOnly、SameSite=Strict；HTTPS 配置同时设 Secure。
浏览器写操作要求同源 Origin 和 CSRF token。修改密码、停用或删除账号后，
旧会话失效；服务进程重启也会使 Cookie 会话失效。

自动化调用可使用独立 Bearer token：

```sh
target/release/felucca-server --generate-token
```

输出包含 `token` 和 `token_hash`；把哈希写入账号的 `token_hash`，
调用时使用原始 token。不要把原始 token 写入服务端配置。
Bearer 请求不需要 Cookie 的 CSRF token，仍执行相同的账号权限检查。

## 配置事务与重试

`admin.state_path` 保存完整的服务端运行配置。已有快照优先于启动文件；
管理员修改会保存用户、客户端、集中隧道、HTTP 设置和账号。
CLI 的 `--runtime-config` 可显式指定快照路径。
流量快照由独立的 `[state]` 配置，不与运行配置文件共用路径。

修改采用版本比较：先校验并准备新服务及回滚服务，再排空旧监听，启动替换服务，
最后原子提交快照。新监听或提交失败时恢复旧配置和监听。
流量统计及桥接身份在重载中保留。有运行配置存储且未指定 `bridge.tls` 时，自动身份保存到
`<快照路径>.identity.json`，跨进程重启保持不变；备份也应包含此文件。
切换管理绑定、证书文件路径、公共 origin 或状态路径需要重启进程。

每次写操作要求：

```text
If-Match: "当前配置版本"
Idempotency-Key: 本次操作唯一标识
```

版本过期返回 409。操作 ID 按账号隔离；相同 ID、相同请求返回之前的结果，
不会再次重启数据面。相同 ID 对应不同请求返回 400。
最近 256 次成功操作随配置持久化，重启后也可识别 DELETE 等操作的重试。
客户端断开 HTTP 请求不会中止已经开始的配置事务。
浏览器对未收到响应的相同请求保留原操作 ID。

`durable = false` 表示尚未获得目录同步成功的确认，或调用 Daemon 时未配置持久化。
设备、服务和访问权限修改不会重启全部转发服务。修改或停用的服务结束自身连接；未改动的服务、设备名称和服务名称修改保持连接。旧版 agent 只重连受影响设备。修改桥接、HTTP、P2P 监听等基础设置仍会重新加载转发服务。
通过嵌套路由访问管理端时，承载 HTTP 请求的转发连接也会关闭，可用原操作 ID 重试。

## 集中配置下发

服务端使用 `[managed.<client-id>]` 保存需要 agent 执行的配置；
通过 `[[managed.<client-id>.tunnel]]` 定义后端、隧道、HTTP 路由和健康检查。
agent 配置示例见 `examples/managed-agent.toml`。

agent 必须显式设置 `[remote] enabled = true`，并使用已验证证书的
TLS、WSS 或 QUIC。服务端也检查实际桥接是否加密，明文连接不能索取配置。
下发使用带摘要的规范化 JSON，配置内的凭据不会出现在控制消息 Debug 日志。

`remote.target_policy` 是 agent 本机设定的边界，与每条下发隧道的目标规则取交集，
同时约束实际目标连接和健康探针。允许域名不能绕过禁止的解析 IP。
管理端只能修改隧道内容，不能下发新的 agent 身份、出口代理或本机授权边界。

例外：agent 设置 `remote.console_scope = true` 后，`[managed.<client-id>]` 中可选的
`target_policy` 会替换该 agent 的 `remote.target_policy`，并计入下发摘要，在线修改即时生效；
未提供时继续使用本机范围。agent 未开启时拒绝带 `target_policy` 的下发配置，不会静默忽略。
目录能力 `remote.file_root` 始终只由本机配置决定。控制台生成的连接包默认开启此项。

An agent that sets `remote.console_scope = true` lets an optional `target_policy` in
`[managed.<client-id>]` replace its `remote.target_policy`; agents without it refuse such a push.

文件服务必须预先配置命名目录能力，例如：

```toml
[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

服务端下发 `file.root = "documents/shared"`，不能下发任意绝对路径。
目录使用预先打开的能力句柄解析，路径或符号链接不能逃出已授权目录。
只读目录不能通过远端配置升级为可写。

`tunnel.enabled = false` 保留配置和稳定 ID，但不注册监听、不启动后端探针。
保留一个空的 `managed` 项表示停止该节点的全部集中隧道；
删除整个 `managed` 项则恢复 agent 启动时的本地隧道配置。
管理页面会区分“已发布”“等待节点应用”和“已停用”。

## API

API 使用 JSON，请求体最大 2 MiB。浏览器跨源请求不被允许。
查询资源通常返回 `{revision, config}`，配置查询默认隐藏密钥；
编辑时原来的 `<redacted>` 标记会保留该字段的值。
新资源的凭据不能使用这个标记。

| 路径 | 方法 / 用途 |
| --- | --- |
| `/api/login` | POST `{account, password}`；要求 Origin |
| `/api/session` | GET 当前身份和 CSRF token |
| `/api/logout` | POST 撤销 Cookie 会话 |
| `/api/overview` | GET 真实在线节点、连接、流量及已注册隧道 |
| `/api/users`、`/api/users/<id>` | GET 列表/单项；单项 PUT、DELETE |
| `/api/clients`、`/api/clients/<id>` | GET 列表/单项；单项 PUT、DELETE |
| `/api/managed/<client>` | GET、PUT 整组集中配置；DELETE 恢复本地配置 |
| `/api/managed/<client>/tunnels/<id>` | GET、PUT、DELETE 单条隧道 |
| `/api/locals`、`/api/locals/<id>` | GET 列表/单项；单项 PUT、DELETE |
| `/api/locals/<group>/tunnels/<id>` | PUT、DELETE 本机隧道 |
| `/api/accounts`、`/api/accounts/<id>` | GET 列表；单项 PUT、DELETE |
| `/api/config` | GET 脱敏完整配置；PUT 导入完整配置 |
| `/api/config?secrets=1` | 管理员显式导出含凭据的配置 |
| `/api/validate` | POST 校验完整配置，不启动监听 |
| `/api/batch` | POST 原子批量修改，最多 128 项 |
| `/api/events` | GET SSE 配置版本变化，10 秒心跳并重新检查会话 |

PUT 账号时可传 `password`，服务端会生成哈希；未传则保留 `password_hash`。
批量操作格式：

```json
{
  "operations": [
    {
      "method": "PUT",
      "path": "/api/managed/office/tunnels/1",
      "value": {
        "id": 1,
        "enabled": false,
        "mode": "tcp",
        "port": 18080,
        "backend": [{"addr": "127.0.0.1:3000"}]
      }
    }
  ]
}
```

批量操作使用一个版本和一个操作 ID，全部校验后统一应用；
任意项目越权或配置无效，都不会提交前面的项目。

`bridge://web` 本地别名现在连接经过同样鉴权的管理端；
原来的 HTTP 反向代理入口使用 `bridge://http`。
这些别名仍执行本地嵌套路由的来源、租户和资源检查。

## 验证

管理功能的 Rust 集成测试覆盖实际 agent 下发、租户隔离、脱敏、CSRF、批量回滚、
密码和 SSE 会话撤销、重复请求持久化、嵌套管理入口和 HTTP/2 authority。
`scripts/probes/probe-admin.mjs` 使用真实 Chrome、server 和 agent，验证浏览器登录、
隧道增改启停、真实后端响应、脱敏导出和进程重启恢复，并保存截图和二进制散列。
当前完成范围以 `docs/implementation-status.md` 和实际审计结果为准。

`GET /metrics` / `GET /api/metrics` 提供按账号权限过滤的 Prometheus 指标。
开启 `admin.profiling` 后，管理员可 `POST /api/profile` 下载有时限的 CPU 火焰图。
采样不修改配置，也不需要 If-Match / Idempotency-Key；Cookie 调用仍需 Origin 与 CSRF。
详见 [运维文档](operations.md)。

## 外部平台

配置 `node` 后，管理页面增加平台连接、管理平台和事件回调。节点状态、失败队列、
流量导入、持久停用及运行时同步见 [节点与外部平台](node.md)。平台使用独立令牌，
不需要把本地管理账号的登录凭据交给外部平台。

## 结构化管理与诊断 · Structured management and diagnostics

管理端支持结构化的后端、路由、健康检查、认证、ACL、配额、文件服务、本机隧道组和端口段配置。
字段元数据从 core 配置类型生成，Rust 仍负责最终验证。修改配置类型后运行
`python3 scripts/generate-admin-schema.py`；`--check` 用于检查元数据是否过期。
常规操作与高级配置均使用表单控件；管理账号页提供令牌生成、替换及撤销。

| API | 方法及用途 |
| --- | --- |
| `/api/validate` | POST；返回 `valid`、`revision`、`restart_fields`，区分配置有效和可在线应用 |
| `/api/config/export` | POST 完整配置草稿；管理员、CSRF 和 If-Match 校验，返回恢复凭据后的启动配置、运行快照及实际快照路径，不应用配置 |
| `/api/token` | POST；管理员生成尚未保存的令牌及哈希，随后用账号 PUT 保存哈希 |
| `/api/launch` | GET；启动模板所需的桥接监听和当前证书指纹，不返回密钥 |
| `/api/diagnostics` | GET；当前账号可见的逐隧道计量、P2P 活动连接和最近事件 |
| `/api/diagnostics/<client>/<tunnel>` | GET；读取本机或已认证 agent 的后端健康快照，不启动额外探测 |
| `/api/certificates` | GET；管理员读取当前实际使用的桥接/HTTP 证书和证书维护状态 |
| `/api/certificates/retry` | POST；管理员请求现有 ACME 工作器重试失败项，不强制重签有效证书 |
| `/api/certificates/upload` | POST `{cert,key}` PEM；管理员上传不可变文件，返回可填入草稿的路径，不自动切换配置 |
| `/api/logs` | GET；管理员读取配置指定日志文件最近至多 64 KiB 的完整行，对已知配置凭据脱敏 |

重启导出使用与持久化相同的快照结构，版本递增且清除旧操作重放记录。必须先停机、备份，
再替换下次启动实际使用的运行快照和启动配置；`--runtime-config` 覆盖路径也必须考虑。
保留原来的流量和平台状态文件。页面不在线重写活动快照，也不安装服务或修改防火墙。
平台变更记录使用 API 游标分页；窗口过期时回到当前窗口。

后端诊断是已认证桥接上的可选流请求，追加到现有流类型之后，既有类型编码和控制消息不变。
旧 agent 拒绝或关闭该诊断流时，页面显示不可用，不影响已有转发。每次最多返回 256 个后端，
服务端等待最长 5 秒。P2P 事件和活动记录各限 512 项，随转发代际重建；超出活动记录上限会明确标记。
证书诊断读取当前内存证书，不把磁盘上尚未启用的文件当作已生效证书。

The console uses structured forms backed by the core configuration model. Rust remains the validator.
Restart exports preserve credentials and provide a startup configuration plus a runtime snapshot;
replace them only while the process is stopped, with backups and the actual runtime path accounted for.
Diagnostic reads retain tenant boundaries. Backend reports use an optional authenticated stream extension;
older agents may report unavailable while forwarding continues. Host deployment commands are generated
for review and are never executed by the console.

## 面向用户的任务流程 · User task flows

1. 打开 **设备 → 接入设备**，填写名称、确认服务器地址，复制命令到设备的终端（macOS/Linux）或 PowerShell（Windows）运行。命令自动识别处理器、下载并校验客户端，再启动连接。默认允许所有网络目标（本机、局域网和互联网），需要限制时展开 **高级设置：访问权限**。页面根据真实连接显示在线状态；已有设备的显式限制继续生效。
2. 打开 **服务 → 添加服务**，选择设备和服务类型，填写该设备上应用的地址。公网访问生成服务端端口或域名入口；私有访问只提供给获授权的访问设备。设备凭据、服务编号和共享密钥由系统管理。
3. 在服务的 **访问方式** 中复制地址，并查看发布状态与后端状态。私有服务通过 **私有访问 → 在另一台设备访问** 生成访问端命令，在另一台设备运行后使用页面给出的本地地址访问。接收者无需管理账号。

主导航只有概览、设备、服务、私有访问、监控、设置。账号、配额、诊断、服务端监听和外部平台放在设置中。日常任务与高级选项均使用表单控件；下载文件和备份仍采用机器可读格式，不需要手工编辑 JSON。

设置中可开启直连。服务端具备直连条件时，生成的访问端优先尝试直连，失败回退服务端中继；未开启时直接使用中继。这是按服务访问，不是全系统 VPN。文件共享需要在接入设备时授权一个确实存在的目录。连接文件包含凭据，请妥善保管；已有访问设备应保留原文件，丢失后重新创建私有连接。设备、服务和访问权限修改不会重启全部转发服务。修改或停用的服务结束自身连接；未改动的服务、设备名称和服务名称修改保持连接。旧版 agent 只重连受影响设备。修改桥接、HTTP、P2P 监听等基础设置仍会重新加载转发服务。

1. Open **Devices → Connect device**, enter a name, confirm the server address and copy the command into Terminal (macOS/Linux) or PowerShell (Windows) on the device. It detects the processor, downloads and verifies the matching client, and connects. All networks (this device, the LAN and the Internet) are allowed by default; open **Advanced settings: access permissions** to restrict access. Online status follows the real connection. Existing explicit device restrictions remain in effect.
2. Open **Services → Add service**. Choose the device and service type, then enter the address of the application on that device. Choose public access for a server port/domain, or private access for authorized receiving devices. Names, device keys, service IDs and sharing secrets are handled by the console.
3. Open a service's **Access** panel to copy its address and check publication/backend status. For a private service, create a receiving device in **Private access**, run its command there and use the displayed local address. The recipient needs no console account.

The main navigation contains Overview, Devices, Services, Private access, Monitor and Settings. Settings holds accounts, quotas, diagnostics, server listeners and external integrations. Routine tasks and advanced options use controls rather than a JSON editor; downloaded files and backups remain machine-readable configurations.

Direct connections can be enabled in Settings. Receiving devices prefer direct connections when the server is configured for them, with server relay as fallback; otherwise the server uses relay. This does not create a system-wide VPN. File access must be authorized by selecting an existing folder during device connection. The connection file contains credentials: keep it private. Replacing an existing receiving device requires its original file or a new private connection. Device, service and access edits do not restart all forwarding. Changed or disabled services end their own connections. Unchanged services and device/service renaming keep connections. Older agents reconnect only the affected device. Bridge, HTTP, P2P listener and other infrastructure changes still reload forwarding.

## 客户端连接包 · Runnable client packages

一行命令和 ZIP 使用同一套经校验的客户端资源。`/connect.sh`、`/connect.ps1` 是公开启动脚本；
`/api/connect/<device>/<target>/sha256` 和 `/binary` 用对应设备的 Bearer 凭据取包，
不授予管理权限。停用设备后不能继续取包。脚本保存配置并前台运行，断线自动重连；
重复执行会提示先关闭已有窗口。命令含设备凭据，只分享给预期使用者。
Unix 缓存位于 `$XDG_DATA_HOME/felucca`（默认 `~/.local/share/felucca`），Windows 位于
`%LOCALAPPDATA%\Felucca`；可用 `FELUCCA_DATA_DIR` 指定路径。

The command and ZIP use the same verified assets. Public `/connect.sh` and `/connect.ps1`
scripts download through `/api/connect/<device>/<target>/sha256` and `/binary` with that
enabled device's Bearer credential, which grants no administration rights. Keep the command
private. The launcher saves configuration, runs in the foreground and retries connections;
duplicate launches ask the user to close the existing window first. Data lives in
`$XDG_DATA_HOME/felucca` (default `~/.local/share/felucca`) or `%LOCALAPPDATA%\Felucca`,
overridable with `FELUCCA_DATA_DIR`.

发布归档包含同平台的 `agents/<target>/`。管理端只提供已配置、版本和协议匹配的客户端；可把其他平台归档中的对应目录放到服务端二进制旁的 `agents/`，或用 `FELUCCA_AGENT_ASSETS` 指定附加目录。制作本机客户端目录使用：

```sh
python3 scripts/stage-agent.py target/release/felucca-agent --output dist/agents
```

服务器在发送前核对 SHA-256，手动 ZIP 下载要求有效管理会话，连接 ZIP 在浏览器生成并包含独立凭据。默认启动不安装系统服务；自动运行入口由设备用户自行选择。macOS/Linux 使用用户级服务，Windows 使用当前用户的登录启动快捷方式。

Release archives include a native `agents/<target>/` directory. Place other packaged targets beside the server executable under `agents/`, or specify an additional directory with `FELUCCA_AGENT_ASSETS`. Only matching version/protocol packages are listed; the server verifies SHA-256 before authenticated download. The browser assembles a ZIP with its own credentials. The optional startup entry uses per-user services on macOS/Linux and a current-user sign-in shortcut on Windows.

CI 将各平台产物汇集为 `felucca-agents.tar.gz`，包含 Linux x64/ARM64、macOS Intel/Apple Silicon、
Windows x64；Windows ARM64 需另行提供原生包。解压后的 `agents/` 可供任意平台 Server 使用。
也可从可信的同版本原生归档自行汇集，脚本核对版本、协议和二进制摘要，不执行其他平台程序：

```sh
python3 scripts/bundle-agents.py dist/native/*.tar.gz dist/native/*.zip --output dist/agents
FELUCCA_AGENT_ASSETS=/srv/felucca/agents ./felucca-server
```

只传入实际存在的归档；输出目录必须尚不存在。容器可把完整 `agents/` 只读挂载至
`/opt/felucca/agents` 并设置 `FELUCCA_AGENT_ASSETS=/opt/felucca/agents`。仓库构建的 Docker
镜像只内置本机架构客户端。摘要校验不代替可信发行来源，普通 CI 包也不代表完成系统签名。

CI assembles `felucca-agents.tar.gz` for Linux x64/ARM64, macOS Intel/Apple Silicon and
Windows x64; Windows ARM64 needs a separately staged native build. Every Server platform can
serve this directory. The bundler above accepts trusted, matching native archives and validates
version, protocol and digests without executing foreign programs. Pass only existing archives
and use a new output directory. For containers, mount `agents/` read-only at `/opt/felucca/agents`
and set `FELUCCA_AGENT_ASSETS=/opt/felucca/agents`. The Docker build itself includes only its native
agent. Checksums do not authenticate the publisher; ordinary CI builds are not signed releases.

## macOS 发行 · macOS distribution

macOS 的普通开发构建尚未通过 Apple 公证，浏览器下载后可能被 Gatekeeper 拦截。
界面会在选择和下载这类版本时提示。不要让使用者通过删除隔离标记或关闭系统保护完成接入。
只有在目标系统正常下载、解压、启动后，才能记录首次接入通过。

仓库提供手动工作流 `.github/workflows/macos-notarize.yml`。在 GitHub 的 `macos-release`
环境中配置 `MACOS_CERTIFICATE_P12_BASE64`、`MACOS_CERTIFICATE_PASSWORD`、
`MACOS_SIGNING_IDENTITY`（Developer ID Application）、`APPLE_ID`、`APPLE_TEAM_ID`、
`APPLE_APP_PASSWORD`。该流程构建程序，在临时钥匙串中签名，只把两个程序提交给 Apple，
检查 Accepted 结果和在线公证票据，重新计算分发摘要并输出构建产物；不会发布 Release。
连接文件、服务配置及部署凭据不参与公证上传。证书和账号仍需项目维护者提供，工作流未运行
不算公证完成。

已配置本机开发者钥匙串的维护者也可以显式执行：

```sh
python3 scripts/notarize-macos.py --binary-dir target/release --output target/signed \
  --identity 'Developer ID Application: Your Organization (TEAMID)' \
  --keychain-profile felucca-release
python3 scripts/package.py --binary-dir target/signed --output dist
```

独立 Mach-O 程序无法附加离线公证票据，Gatekeeper 需要联网查询票据。
`stage-agent.py` 检查 Developer ID 签名及在线票据之后才写入 `notarized: true`。
该元数据用于界面说明，不替代设备自身的签名与系统检查。

Regular macOS development builds are not notarized and may be blocked after browser download.
The UI identifies these builds. Complete normal first-launch verification on the target OS;
do not count disabling protection or removing quarantine as a successful connection.
The manual `macos-notarize.yml` workflow uses the six secrets above in the `macos-release`
environment, signs copied binaries with hardened runtime and a secure timestamp, submits only
the programs to Apple, verifies acceptance and online tickets, and emits artifacts without
publishing a release. It needs the maintainer's signing identity and account; an unrun workflow
is not proof of notarization. Standalone binaries cannot carry stapled tickets, so Gatekeeper
needs online access. Package metadata does not replace the device's own security checks.

官方依据 / Apple references:
[Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) ·
[Customizing the notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
