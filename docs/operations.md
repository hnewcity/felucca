# 运维与分发

当前源码提供服务管理、显式二进制更新、日志轮转、Prometheus 指标和可选 CPU 采样。
这些命令不要求先加载运行配置；`service render` 只读取路径并生成定义。

## 服务管理

server 和 agent 使用相同子命令，以独立名称支持多实例：

```sh
felucca-server service --name office render --config /srv/felucca/server.toml
felucca-server service --name office --dry-run install --config /srv/felucca/server.toml
felucca-server service --name office install --config /srv/felucca/server.toml
felucca-server service --name office start
felucca-server service --name office status
felucca-server service --name office restart
felucca-server service --name office stop
felucca-server service --name office uninstall
```

Linux 使用 systemd，macOS 使用 launchd，默认安装当前用户的服务；机器级服务增加
`--scope system`，在具备相应权限的终端执行。Windows 使用 SCM，默认且仅支持 system scope。
Windows 的服务进程实现了 SCM dispatcher、启动状态、STOP/SHUTDOWN 和停止状态；不是将普通
控制台程序直接交给 `sc.exe`。

`--workdir` 默认是配置所在目录，配置内的相对路径据此解释。`--binary` 可以指定安装的程序。
额外 daemon 参数放在 `--` 后，例如 agent 的 `--log-file /srv/felucca/agent.log`。
生成定义时会转义 systemd specifier、XML 和 Windows 命令行参数；不通过 shell 拼接运行。
`render --platform systemd|launchd|windows` 可在任意受支持主机查看对应定义。

安装不覆盖已存在的同名服务，也不立即启动。Linux 会 enable；macOS 在下一次登录/启动或
显式 start 时 bootstrap。机器级 Linux/macOS 服务以系统服务管理器默认身份运行，应按部署
需要编辑运行用户；用户服务适合无需特权端口的部署。用户 systemd 服务开机常驻需要部署者
自行配置 lingering；macOS 用户服务依赖图形登录会话。

Windows 安装身份为 `NT AUTHORITY\LocalService`，需要部署者授予配置、日志、状态和文件根目录的
对应权限。配置密钥不会自动放进系统全局环境。日志应配置到可写文件。
卸载保留可执行文件、配置、日志、证书与运行状态。

### 自动重启与状态恢复

生成的服务定义在进程非受管退出后自动拉起：systemd 使用 `Restart=always`、`RestartSec=5` 并关闭
启动次数限制（`StartLimitIntervalSec=0`），launchd 使用 `KeepAlive`，Windows 设置失败操作
（5 s、10 s、之后每 30 s 重启，一天无故障后重新计数，非零退出同样计入）。`service stop` 通过
`systemctl stop` 或 `launchctl bootout` 停止，不会触发重启。进程启动时把打开文件软限制提升到
硬限制（最多 1,048,576），systemd 定义另设 `LimitNOFILE`。panic 同时写入配置的日志文件。

状态文件写入均为临时文件、fsync、rename。启动时若内容损坏：

| 文件 | 处理 |
| --- | --- |
| 运行配置（`admin.state_path` 或 `--runtime-config`） | 改名为 `<文件>.corrupt-<unix 秒>`，用每次成功启动或提交后保存的 `<文件>.last-good` 启动并写回；没有 last-good 时仍拒绝启动 |
| 流量状态（`state.path`） | 改名保留，使用上一次保存前的 `<文件>.previous`；两者都不可用时以空计数启动 |
| 平台节点日志（`node.state_path`） | 改名保留，以新的 epoch 启动，平台据此重新同步 |
| ACME 缓存 | 损坏的证书或账户文件改名保留，然后重新签发或注册 |

版本号高于当前程序的状态文件（通常是降级）不视为损坏，仍拒绝启动且不修改文件。
启动从完好的运行配置读取时不重写它，磁盘已满时也能启动。改名保留的文件不会自动删除。

## 更新

从可信发布渠道取得独立可执行文件的 SHA-256，然后验证或替换当前执行的程序：

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64位摘要> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64位摘要> --expect-version 0.1.0
felucca-agent update --source https://releases.example/felucca-agent --sha256 <64位摘要>
```

摘要是信任依据，应通过可信发布清单获得。输入必须是独立二进制，先解开 tar.gz/zip 再更新。
HTTPS 使用证书校验，不跟随跳转；私有下载可用 `--token-env VARIABLE_NAME`，凭据不写日志。
下载最多 512 MiB、5 分钟，在可执行文件旁创建临时文件。摘要正确后才执行受 10 秒限制的
`--version` 探测，检查 server/agent 类型及可选版本。失败清理临时文件；更新锁防止并发替换。
替换保留原文件权限。完成后需显式 restart 服务，已运行的进程继续使用旧版本。

协议版本发生变化时，server 和 agent 必须一同升级。`--build-info` 输出程序名、版本、协议和
构建 target；打包脚本会实际执行它，拒绝混装不同协议或 target 的二进制。

## 日志

server 配置：

```toml
[log]
level = "info"
file = "/data/felucca.log"
json = true

[log.rotation]
max_bytes = 10485760
max_files = 5
interval = "24h"
```

文件仍与 stderr 同时输出。轮转在完整事件之间执行，所以大小阈值可多出一个事件；归档为
`.1` 到 `.5`，`.1` 最新。interval 从打开当前文件开始计时，重启会重新开始该周期。
独占锁拒绝多个进程同时轮转同一路径。新建日志与锁文件在 Unix 上为 0600。
更改 server 日志配置需要重启；管理端会拒绝在线修改这些设置。

agent 对应参数：

```sh
felucca-agent -c agent.toml --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

agent 设置 log-file 就默认启用 10 MiB、5 份归档。`RUST_LOG` 可覆盖日志过滤等级。

## 监控与 CPU 采样

管理监听的 `GET /metrics` 与 `GET /api/metrics` 返回 Prometheus 文本，使用管理账号 Bearer token
或有效会话；不会提供匿名指标入口。指标遵守账号所属用户的可见范围。

包含服务健康、配置修订号、节点在线/启用状态、连接数、节点累计流量、隧道就绪及累计流量。
流量计数沿用现有持久化 MeterStore；开启 state 后能跨服务端重启恢复。

管理 UI 的“监控”默认展示服务端主机 CPU、物理内存、磁盘使用率与网络收发速率折线图，
不展示 Prometheus 原始文本或接入配置。管理员可通过 `GET /api/monitor?window=900`
读取同一份结构化历史；`window` 支持 300、900、3600 秒，默认 900。
主机采样每 5 秒一次，最多保留 1 小时、721 个点，仅存内存，转发配置重载保留历史，
进程重启清空。主机数据仅管理员可见，operator/viewer 仍仅看所属资源的连接与流量。

图表支持悬停、点选锁定和键盘逐点查看，显示毫秒时间与本地时区。
可拖动底部范围条或使用放大/重置按钮，四图同步缩放；刷新保留选中的绝对时间范围。
卡片统计可见时段有效采样的最低、算术平均及峰值，网络图例可切换接收/发送曲线。
CPU 首次差分与网络首次差分留空；不支持的指标或采样断档不会补零。
磁盘为服务可执行文件所在文件系统，网络汇总非回环接口（含虚拟接口），
速率使用实际采样间隔计算。

设置 `[admin] profiling = true` 后，管理员可调用 `POST /api/profile`，JSON 为
`{"seconds":5}`，或在监控页下载 CPU 火焰图。Linux/macOS 支持，其他平台拒绝开启。
每次 1..30 秒、49 Hz、进程范围内仅允许一个任务，输出上限 8 MiB。
HTTP 请求断开不会提前释放采样锁；daemon 停止会取消采样。空闲期间无样本时返回说明 SVG。
release 保留函数符号用于采样；不保留调试行信息，因此行级定位需要自建带调试信息的版本。

## Docker 与打包

```sh
docker build --target server -t felucca-server:local .
docker build --target agent -t felucca-agent:local .
docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

镜像默认使用 UID/GID 10001，配置目录只读、数据目录需允许其写入。按配置显式开放额外 TCP/UDP
端口；容器内绑定应使用可达地址。更新容器应替换镜像，容器内的 update 命令不适用于不可写的
镜像文件系统。多架构镜像可由 Buildx 按 Dockerfile 原生目标构建；此处不自动发布镜像。

### 本机 OrbStack

启动 OrbStack 后，在项目根目录执行 `sh scripts/run-orbstack.sh`。脚本只构建和部署服务端，
前端在镜像内构建；宿主机需要 Docker Compose、Python 3 和 OpenSSL。管理地址为
`http://127.0.0.1:8088`。初始账号为 `root`，随机密码保存在
`conf/orbstack/credentials.json`，不写入镜像或 Git；该文件只记录初始密码。

`compose.orbstack.yaml` 使用只读配置目录 `conf/orbstack/config/` 和数据卷
`felucca_server-data`，通过 OrbStack 主机网络提供管理端和 TCP/QUIC 桥接端口 14028。
管理监听仅绑定 loopback；桥接证书在初始化时生成并保留；
初始没有客户端，后续通过管理界面配置。已保存的管理配置以数据卷中的快照为准。
容器以 UID/GID 10001 运行，配置重建和流量快照写入 `/data`。

```sh
docker --context orbstack compose -f compose.orbstack.yaml ps
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

再次运行脚本会构建当前代码并更新容器，保留已有凭据、证书和数据卷。停止容器时不要加
`down -v`，该选项会删除数据卷。主机网络共享监听端口，增加隧道无需逐个发布 Docker
端口，但须避开本机其他程序已占用的端口。

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

产物包含两个程序、示例、文档、逐二进制摘要清单及压缩包摘要。默认拒绝无法在打包主机执行的
二进制；跨平台 CI 分别在目标平台打包。GitHub workflow 配置 Linux amd64/arm64、macOS arm64、
Windows amd64 的检查、测试与归档，并单独验证 Rust 1.88 最低版本与 Docker 构建。
workflow 只上传任务附件，不创建 GitHub release，也不推送镜像。

本机已验证定义生成、macOS plist 解析、隔离副本更新和真实 CPU 采样。2026-09-26 在
OrbStack 中实际构建并运行了 Linux arm64 服务端镜像，完成 HTTP 登录、监控接口、
桥接 TLS 和容器重建后的持久化验证，见 部署记录。
**没有在本机安装系统服务，没有运行 Linux/Windows 服务管理器，GitHub workflow 尚未运行。**
这些部署验收不能由生成文件或本机测试代替。
