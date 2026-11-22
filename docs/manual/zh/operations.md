# 系统服务、更新、日志与监控 · Services, updates, logging and monitoring

本页说明怎么把 Felucca 交给系统服务管理器、怎么安全地更新二进制、怎么配日志轮转,
以及怎么看指标和做 CPU 采样。

This page covers registering Felucca with a service manager, updating binaries safely,
log rotation, metrics and CPU profiling.

## 安装为系统服务 · Installing as a system service

两个程序使用相同子命令,`--name` 支持多实例:

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

| 平台 | 管理器 | 默认 scope |
| --- | --- | --- |
| Linux | systemd | `user`;`--scope system` 为机器级 |
| macOS | launchd | `user`;`--scope system` 为机器级 |
| Windows | SCM | 仅 `system` |

`render --platform systemd|launchd|windows` 可以在任意受支持主机查看对应定义,
不通过 shell 拼接运行。`--workdir` 默认是配置所在目录,配置内的相对路径据此解释;
`--binary` 可指定安装的程序。额外 daemon 参数放在 `--` 后,例如
`-- --log-file /srv/felucca/agent.log`。

安装**不覆盖**已存在的同名服务,也不立即启动。Linux 会 enable;macOS 在下一次登录/启动
或显式 `start` 时 bootstrap。用户 systemd 服务开机常驻需要部署者自行配置 lingering;
macOS 用户服务依赖图形登录会话。机器级服务以系统服务管理器默认身份运行,应按部署需要
编辑运行用户。Windows 安装身份为 `NT AUTHORITY\LocalService`,需要部署者授予配置、日志、
状态和文件根目录的对应权限。

卸载保留可执行文件、配置、日志、证书与运行状态。

边界:仓库验证了定义生成、macOS plist 解析、隔离副本更新和真实 CPU 采样。
另已在 OrbStack 中实际构建、运行 Linux arm64 服务端，验证管理登录、监控、桥接 TLS
和容器重建后的持久化，见部署记录。
**没有**在本机安装系统服务或运行 Linux/Windows 服务管理器；这些验收不能由生成文件代替。

## 更新二进制 · Updating binaries

从可信发布渠道取得独立可执行文件的 SHA-256,然后验证或替换当前执行的程序:

```sh
felucca-server update --source /downloads/felucca-server --sha256 <64位摘要> --check-only
felucca-server update --source /downloads/felucca-server --sha256 <64位摘要> --expect-version 0.1.0
felucca-agent  update --source https://releases.example/felucca-agent --sha256 <64位摘要>
```

摘要是**信任依据**,应通过可信发布清单获得。输入必须是独立二进制,先解开 tar.gz/zip。
HTTPS 使用证书校验,不跟随跳转;私有下载用 `--token-env VARIABLE_NAME`,凭据不写日志。
下载最多 512 MiB、5 分钟。摘要正确后才执行受 10 秒限制的 `--version` 探测,检查
server/agent 类型及可选版本。失败清理临时文件;更新锁防止并发替换。替换保留原文件权限。

完成后需显式 `service ... restart`,已运行的进程继续使用旧版本。
容器内更新不适用于不可写的镜像文件系统——应替换镜像。

协议版本变化时 server 和 agent 必须一同升级。打包脚本会实际执行 `--build-info`,
拒绝混装不同协议或 target 的二进制。

## 日志 · Logging

server 侧:

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

文件与 stderr 同时输出。轮转在完整事件之间执行,大小阈值可能多出一个事件;归档为 `.1`
到 `.5`,`.1` 最新。`interval` 从打开当前文件开始计时,重启重新开始该周期。独占锁拒绝多个
进程同时轮转同一路径;新建日志与锁文件在 Unix 上为 0600。

**更改 server 日志配置需要重启;管理端会拒绝在线修改这些设置。** agent 用命令行参数,
见 [felucca-agent 模块](agent.md)。

## 监控 · Monitoring

`GET /metrics` 与 `GET /api/metrics` 返回 Prometheus 文本,使用管理账号 Bearer token 或
有效会话,**不提供匿名指标入口**;指标遵守账号所属用户的可见范围。

包含服务健康、配置修订号、节点在线/启用状态、连接数、节点累计流量、隧道就绪及累计流量。
流量计数沿用现有持久化 MeterStore,开启 `[state]` 后能跨 server 重启恢复。
管理 UI 的“监控”默认提供主机 CPU、物理内存、磁盘和网络速率曲线，不展示 Prometheus 文本和配置。每 5 秒采样，保留最近 1 小时（重启后重新积累）；可选择 5 分钟、15 分钟、1 小时。悬停或点击锁定采样点，查看准确时间、时区与数值，也可用方向键逐点查看。拖动图表底部范围条可同步缩放四图，放大/重置按钮支持键盘操作；自动刷新保留当前缩放范围。卡片显示可见采样的最低、平均与峰值，网络图例可切换接收/发送曲线。主机指标仅管理员可见，其他角色只展示有权访问的 Felucca 资源连接与流量。

## CPU 采样 · CPU profiling

设置 `[admin] profiling = true` 后,管理员可调用 `POST /api/profile`,JSON 为
`{"seconds":5}`,或在监控页下载 CPU 火焰图。Linux/macOS 支持,其他平台拒绝开启。
每次 1..30 秒、49 Hz、进程范围内仅允许一个任务,输出上限 8 MiB。
HTTP 请求断开不会提前释放采样锁;daemon 停止会取消采样。空闲期间无样本时返回说明 SVG。
release 保留函数符号用于采样,不保留调试行信息;行级定位需要自建带调试信息的版本。

## Docker · Docker

```sh
docker build --target server -t felucca-server:local .
docker build --target agent  -t felucca-agent:local .

docker run --rm -p 14028:14028 \
  --mount type=bind,src=/srv/felucca/config,dst=/etc/felucca,readonly \
  --mount type=bind,src=/srv/felucca/data,dst=/data \
  felucca-server:local
```

镜像默认 UID/GID 10001,配置目录只读、数据目录需允许其写入。按配置显式开放额外 TCP/UDP
端口;容器内绑定应使用可达地址。多架构镜像可由 Buildx 按 Dockerfile 原生目标构建。

### 本机 OrbStack

启动 OrbStack 后，在项目根目录运行：

```sh
sh scripts/run-orbstack.sh
```

脚本只部署服务端，并在镜像内构建管理页面；本机需要 Docker Compose、Python 3 和 OpenSSL。
打开 `http://127.0.0.1:8088`，用 `root` 和 `conf/orbstack/credentials.json` 中的随机密码
登录。管理监听仅绑定 loopback。凭据和桥接证书保存在
Git 忽略的 `conf/orbstack/` 下，管理配置和流量快照保存在 `felucca_server-data` 数据卷。
密码文件只记录初始密码；已有管理配置以数据卷中的快照为准。

重复运行脚本会构建当前代码并更新容器，保留凭据、证书和数据。容器会随 Docker 引擎恢复运行，
手动停止后保持停止。初始没有客户端；TCP/QUIC 桥接端口均为 14028。Compose 使用
OrbStack 主机网络，新增隧道无需逐个发布 Docker 端口，但须避开本机已占用的端口。

```sh
docker --context orbstack compose -f compose.orbstack.yaml logs --tail 100 server
docker --context orbstack compose -f compose.orbstack.yaml restart server
docker --context orbstack compose -f compose.orbstack.yaml down
```

`down` 保留数据卷；不要加 `-v`，否则会删除持久化配置和流量数据。
`STOPSIGNAL` 是 SIGTERM,和程序的关闭路径一致。

## 打包与 CI · Packaging and CI

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
python3 scripts/package.py --output dist
```

源码构建需要 Node.js 22.12+（推荐 24）；React + Vite 管理端位于 `apps/admin/`，
构建后嵌入服务端二进制，部署时无需 Node.js。前端变更后需先重建，过期产物会被 Cargo 拒绝。

产物包含两个程序、示例、文档、逐二进制摘要清单及压缩包摘要。默认拒绝无法在打包主机
执行的二进制;跨平台 CI 分别在目标平台打包。GitHub workflow 配置 Linux amd64/arm64、
macOS arm64、Windows amd64 的检查、测试与归档,并单独验证 Rust 1.88 最低版本与 Docker
构建。workflow 只上传任务附件,不创建 GitHub release,也不推送镜像。

## 验证清单 · Verification checklist

```sh
felucca-server service --name office render --config conf/server.toml
felucca-server --config conf/server.toml --fingerprint
felucca-agent  --config conf/agent.toml  --check
```

`--fingerprint` 要求配置固定 PEM 文件。`--check` 不代表后端、DNS、证书文件或公网端口
已经可用。

## 下一步 · Next steps

- [felucca-server 模块](server.md)
- [管理界面与集中配置](management.md)
- [参考](reference.md)
