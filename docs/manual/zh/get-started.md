# 快速开始 · Get started

可以连接自己部署的公网 Server，也可以向服务器提供方获取连接命令。无需先制作证书、编辑配置文件或安装 agent。

## 一行命令接入设备

1. 在 **设备 → 接入设备** 中填写名称，保留默认设置即可访问本机、局域网和互联网。需要限制时，展开 **高级设置：访问权限**，选择本机、局域网或自定义范围。
2. 复制命令，在设备的终端（macOS/Linux）或 PowerShell（Windows）中运行。命令会识别处理器、下载对应客户端并校验摘要。
3. 保持窗口打开。设备在线后选择 **添加服务**，填写本地服务地址，再通过发布地址访问。断线后会自动重连。
4. 私有访问时，在 **私有访问** 中创建接收设备，把生成的命令放到另一台电脑运行。接收者无需登录管理后台。

命令包含设备凭据，只交给预期的使用者。以后可再次运行原命令；请先关闭已经连接的窗口。服务器需要提供与设备系统匹配的客户端包。手动 ZIP 下载、文件目录权限和高级配置保留为可选项。

## 自己启动 Server

使用发行包里的程序，在可写目录首次运行：

```sh
./felucca-server --init
```

命令自动生成配置、显示随机的 root 登录密码并启动服务。打开输出的管理地址即可。升级和备份时保留 `felucca-server.toml` 及整个 `felucca-server.data/` 目录。以后启动去掉 `--init`：

```sh
./felucca-server
```

公网部署可用 `--init --admin-url https://console.example.com`，由 HTTPS 反向代理转发到 `127.0.0.1:8990` 并保留 Host。填写直接 HTTP 地址时，会在对应端口公开监听；公网访问请使用 HTTPS。设备需要能连接桥接 TCP 9001，访问者需要能连接服务发布端口。详见[部署与客户端分发](../../management.md)。

## 前置条件 · Prerequisites

| 要求 Requirement | 说明 |
| --- | --- |
| Rust 1.88 或更新版本 | 用于编译;本仓库在 Rust 1.98.1 上验证 |
| Python 3 + OpenSSL | 仅本地演示脚本 `scripts/demo-local.py` 需要 |
| 一台有公网 IP 的主机 | 运行 `felucca-server` |
| 一台内网主机 | 运行 `felucca-agent`,上面有要暴露的服务 |
| 本机 TCP/UDP 绑定权限 | 测试需要;防火墙规则由部署方管理 |

Felucca 不安装系统服务,也不修改防火墙规则。

## 方式一:一键本地演示 · Option 1 — local demo

只想在本机看效果(管理页面 + 端口转发),不需要公网:

```sh
python3 scripts/demo-local.py
```

脚本会增量编译 debug 程序,生成证书和随机凭据,启动测试网站、server 和 agent,
验证真实转发后自动打开演示页面。终端会显示管理地址、账号和密码;`Ctrl+C` 停止全部
演示服务。

所有监听只绑定 `127.0.0.1`,默认端口被占用时自动选择空闲端口。每次运行的配置、
登录信息和日志保存在独立的 `target/local-demo/run-*` 目录。

| 参数 | 作用 |
| --- | --- |
| `--no-open` | 不自动打开浏览器 |
| `--no-build` | 已编译过时跳过构建 |

## 方式二:编译 · Option 2 — build

```sh
npm --prefix apps/admin ci
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
```

产物是 `target/release/felucca-server` 和 `target/release/felucca-agent`。
`scripts/cargo-external.sh` 把 Cargo 缓存放到工作区所在磁盘;磁盘空间充足时也可以直接
调用 `cargo`。

## 方式三:手动跑通第一条隧道 · Option 3 — first tunnel by hand

### 1. 生成桥接证书 · Generate the bridge certificate

桥接证书只做一次,私钥要保留:

```sh
mkdir -p conf
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -subj '/CN=felucca' -keyout conf/bridge.key -out conf/bridge.pem
cp examples/felucca-server.toml conf/server.local.toml
cp examples/felucca-agent.toml conf/agent.local.toml
```

### 2. 设置共享密钥 · Set the shared key

两份配置的 `vkey` 必须是同一个随机字符串:

```sh
openssl rand -hex 32
```

把它写进 `conf/server.local.toml` 的 `[client.agent-1].vkey` 和
`conf/agent.local.toml` 的 `vkey`。agent 的 `key_id` 必须对应服务端的 `[client.<id>]`。
agent 的 `server` 改成服务器的可达地址。

### 3. 取得证书指纹 · Read the certificate fingerprint

```sh
target/release/felucca-server --config conf/server.local.toml --fingerprint
```

把输出的 SHA-256 填进 agent 的 `[trust].fingerprint`。然后校验两份配置:

```sh
target/release/felucca-server --config conf/server.local.toml --check
target/release/felucca-agent --config conf/agent.local.toml --check
```

### 4. 分别启动 · Start both programs

两个终端各执行一条:

```sh
target/release/felucca-server --config conf/server.local.toml
```

```sh
target/release/felucca-agent --config conf/agent.local.toml
```

### 5. 验证 · Verify

示例配置把服务器的 TCP `8080` 转到 agent 机器的 `127.0.0.1:3000`。在 agent 机器上启动
一个监听 3000 的服务,然后访问服务器的 8080 端口即可。

## 下一步 · Next steps

- [架构与核心概念](architecture.md) — 理解 bridge、transport、tunnel 这些词
- [felucca-server 模块](server.md) — 客户端表、HTTP 监听、端口授权
- [felucca-agent 模块](agent.md) — 重连、出口网络、多实例
- [隧道与端口映射](tunnels.md) — TCP/UDP/HTTP/文件/代理模式
- [参考](reference.md) — 完整命令与字段
