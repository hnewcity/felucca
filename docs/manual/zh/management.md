# 管理界面与集中配置 · Admin UI and centralized configuration

`felucca-server` 内置设备、服务、私有访问和配置管理界面。设备、服务和访问权限的日常修改在线同步，未改动的服务继续运行。修改全局监听等基础设置仍会重新加载转发服务，管理 API 保持可用。

## 不编辑配置也能完成首次连接

1. 打开 **设备 → 接入设备**，填写名称、确认服务器地址，并选择目标设备的系统和处理器。下载客户端连接包，解压后打开 `Connect.command`（macOS）或 `Connect.cmd`（Windows）；Linux 在解压后的文件夹运行 `sh connect.sh`。程序和连接信息已配好，无需另找客户端或编辑文件。页面根据真实连接显示在线状态。
2. 打开 **服务 → 添加服务**，选择设备和服务类型，填写该设备上应用的地址。公网访问生成服务端端口或域名入口；私有访问只提供给获授权的访问设备。设备凭据、服务编号和共享密钥由系统管理。
3. 私有访问的另一台设备也下载独立客户端连接包，启动后使用页面提供的本地地址访问。要登录后自动运行，先关闭已有连接窗口，再运行包内 `Install.command`、`Install.cmd` 或 `sh install.sh`。它将客户端存入个人目录；无需手工编写服务定义。

主导航只有概览、设备、服务、私有访问、监控、设置。账号、配额、诊断、服务端监听和外部平台放在设置中。日常任务与高级选项均使用表单控件；下载文件和备份仍采用机器可读格式，不需要手工编辑 JSON。

设置中可开启直连。服务端具备直连条件时，生成的访问端优先尝试直连，失败回退服务端中继；未开启时直接使用中继。这是按服务访问，不是全系统 VPN。文件共享需要在接入设备时授权一个确实存在的目录。连接文件包含凭据，请妥善保管；已有访问设备应保留原文件，丢失后重新创建私有连接。设备、服务和访问权限修改不会重启全部转发服务。修改或停用的服务结束自身连接；未改动的服务、设备名称和服务名称修改保持连接。旧版 agent 只重连受影响设备。修改桥接、HTTP、P2P 监听等基础设置仍会重新加载转发服务。

页面只提供服务器上已有的客户端版本。未完成 Apple 公证的 macOS 版本会明确提示，正常下载后可能被系统拦截；请联系管理员提供正式发行版，不要关闭系统保护。

## 启用 · Enabling

```sh
target/release/felucca-server --hash-password      # 从 stdin 读一行密码,≥12 字节
target/release/felucca-server --generate-token     # Bearer token 及其存储哈希
```

```toml
[admin]
bind = "127.0.0.1:8088"
state_path = "state/runtime-config.json"
profiling = true
# 非 loopback 监听必须设置浏览器访问的 origin：推荐 https://；
# 接受明文管理流量时可用 http://host:port
# origin = "https://admin.example.com"

[admin.account.root]
role = "admin"
password_hash = "pbkdf2-sha256$600000$..."
```

配置文件可以是 TOML 或 JSON。示例管理地址 `http://127.0.0.1:8088`,账号 `root`。
示例密码只用于本机演示,实际必须替换成自己的哈希。

## 账号与角色 · Accounts and roles

| `role` | 权限 |
| --- | --- |
| `admin` | 管理全部资源和账号,不设置 `user` |
| `operator` | 管理所属 `user` 的节点和集中配置隧道 |
| `viewer` | 只读所属 `user` 的资源 |

操作员和只读账号必须关联一个已存在的用户。服务端本机服务可能打开本机目录,因此仅管理员
可修改;用户配额、管理账号及完整配置同样仅由管理员修改。至少保留一个启用的管理员。
`disabled = true` 停用账号。

密码用带随机盐的 PBKDF2-SHA256,固定 600000 次迭代。会话在内存中,Cookie 为
HttpOnly、SameSite=Strict,HTTPS 配置同时设 Secure。浏览器写操作要求同源 Origin 和
CSRF token。修改密码、停用或删除账号后旧会话失效;进程重启也会使 Cookie 会话失效。

自动化调用用 Bearer token:把 `--generate-token` 输出的 `token_hash` 写进账号的
`token_hash`,调用时用原始 token。Bearer 请求不需要 Cookie 的 CSRF token,仍执行相同的
账号权限检查。**不要把原始 token 写进服务端配置。**

## 写入事务 · Write transactions

每次写操作要求两个头:

```text
If-Match: "当前配置版本"
Idempotency-Key: 本次操作唯一标识
```

- 版本过期返回 409。
- 相同 ID + 相同请求返回之前的结果,**不会**再次重启数据面;相同 ID + 不同请求返回 400。
- 最近 256 次成功操作随配置持久化,重启后也能识别 DELETE 等操作的重试。
- `durable = false` 表示尚未获得目录同步成功的确认。
- 设备、服务和访问权限修改不会重启全部转发服务。修改或停用的服务结束自身连接；未改动的服务、设备名称和服务名称修改保持连接。旧版 agent 只重连受影响设备。修改桥接、HTTP、P2P 监听等基础设置仍会重新加载转发服务。

切换管理绑定、证书文件路径、公共 origin 或状态路径需要重启进程,不能在线改。

## 界面页面 · UI pages

| 页面 | 内容 |
| --- | --- |
| 概览 | 当前账号可见的设备、服务和连接状态。 |
| 设备 | 查看已接入的电脑和服务器，选择设备即可管理它的服务。 |
| 服务 | 选择设备和需要访问的应用，由 Felucca 将它连接到你选择的访问入口。 |
| 私有访问 | 选择一个私有服务，为另一台设备生成连接文件，无需开放服务的公网端口。 |
| 监控 | 查看主机指标、连接数和累计流量。 |
| 设置 | 管理服务器连接、访问权限和外部集成。日常设备与服务操作在主导航完成。 |

## API · API

JSON,请求体最大 2 MiB,浏览器跨源请求不被允许。查询资源通常返回 `{revision, config}`,
配置查询默认隐藏密钥;编辑时原来的 `<redacted>` 标记会保留该字段的值,新资源的凭据不能
使用这个标记。

| 路径 | 方法 / 用途 |
| --- | --- |
| `/api/login` | POST `{account, password}`;要求 Origin |
| `/api/session` | GET 当前身份和 CSRF token |
| `/api/logout` | POST 撤销 Cookie 会话 |
| `/api/overview` | GET 在线节点、连接、流量及已注册隧道 |
| `/api/users`、`/api/users/<id>` | GET 列表/单项;单项 PUT、DELETE |
| `/api/clients`、`/api/clients/<id>` | GET 列表/单项;单项 PUT、DELETE |
| `/api/managed/<client>` | GET、PUT 整组集中配置;DELETE 恢复本地配置 |
| `/api/managed/<client>/tunnels/<id>` | GET、PUT、DELETE 单条隧道 |
| `/api/locals`、`/api/locals/<id>` | GET 列表/单项;单项 PUT、DELETE |
| `/api/locals/<group>/tunnels/<id>` | PUT、DELETE 本机隧道 |
| `/api/shares`、`/api/shares/<name>` | GET、PUT、DELETE 私有共享 |
| `/api/accounts`、`/api/accounts/<id>` | GET 列表;单项 PUT、DELETE |
| `/api/config` | GET 脱敏完整配置;PUT 导入完整配置 |
| `/api/config?secrets=1` | 管理员显式导出含凭据的配置 |
| `/api/validate` | POST 校验完整配置,不启动监听 |
| `/api/batch` | POST 原子批量修改,最多 128 项 |
| `/api/events` | GET SSE 配置版本变化,10 秒心跳并重新检查会话 |
| `/metrics`、`/api/metrics` | GET Prometheus 指标,按账号权限过滤 |
| `/api/profile` | POST 下载有时限的 CPU 火焰图 |

批量操作用一个版本和一个操作 ID,全部校验后统一应用;任意项目越权或配置无效,
都不会提交前面的项目:

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

`PUT` 账号时可传 `password`,服务端会生成哈希;未传则保留 `password_hash`。

## 集中配置下发 · Centralized configuration

服务端用 `[managed.<client-id>]` 保存需要 agent 执行的配置:

```toml
[[managed.office.tunnel]]
id = 1
mode = "tcp"
port = 18080
bind = "127.0.0.1"
backend = [{ addr = "127.0.0.1:3000" }]
```

agent 必须显式开启 `[remote] enabled = true`,且主连接使用已验证证书的 TLS/WSS/QUIC;
服务端也检查实际桥接是否加密,明文连接不能索取配置。下发使用带摘要的规范化 JSON,
配置内的凭据不会出现在控制消息 Debug 日志。

管理端只能修改隧道内容,**不能下发新的 agent 身份、出口代理或本机授权边界**。
文件服务必须预先配置命名目录能力,见 [文件服务与 WebDAV](file-services.md)。

例外：agent 设置 `remote.console_scope = true` 后，`[managed.<client-id>]` 的 `target_policy` 会替换它的目标范围。

| 操作 | 效果 |
| --- | --- |
| `tunnel.enabled = false` | 保留配置和稳定 ID,但不注册监听、不启动后端探针 |
| 保留一个空的 `managed` 项 | 停止该节点的全部集中隧道 |
| 删除整个 `managed` 项 | 恢复 agent 启动时的本地隧道配置 |

管理页面会区分"已发布"、"等待节点应用"和"已停用"。

## 验证清单 · Verification checklist

1. `felucca-server --config server.json --check` 通过。
2. 浏览器打开管理地址,能登录,无 JS 异常。
3. 建一条 TCP 隧道,后端真实返回内容。
4. `GET /api/config` 看不到明文密钥。
5. 重启 server 后配置和隧道仍在;用相同 `Idempotency-Key` 重试不重复生效。

## 下一步 · Next steps

- [外部平台接入](platform.md) — 让别的系统管理这个节点
- [felucca-agent 模块](agent.md) 的 `[remote]` 一节
- [系统服务、更新、日志与监控](operations.md)
- 示例:[admin-server.toml](../../../examples/admin-server.toml) ·
[managed-agent.toml](../../../examples/managed-agent.toml)

管理端现已提供结构化配置表单，覆盖后端权重与 TLS、健康检查、HTTP 路由、服务账号、ACL 与配额、文件服务、本机隧道组和端口映射。管理账号页支持生成、替换和撤销令牌，新令牌在保存成功后下载。

配置校验会列出需要重启的设置。重启导出提供含凭据的启动配置、运行快照和离线替换说明。请先停止进程并备份原文件。运行快照优先于启动配置文件，包括通过 `--runtime-config` 指定的路径；保留流量和平台日志快照，不要覆盖仍在运行的快照。

运行诊断展示逐隧道流量、已有后端探测结果、活动 P2P 会话和最近的中继事件。管理员可以查看当前证书、重试失败的 ACME 申请、上传 PEM 证书和私钥并填入配置草稿，以及查看有界、凭据脱敏的日志尾部。不支持可选诊断流的旧 agent 会显示暂不可用，转发不受影响。读取诊断不会触发额外探测。Agent 和访客配置生成保留本机目录及目标访问授权边界：请在 agent 上检查并加载生成文件，先执行 `--check`。服务和更新命令仅供检查，管理端不会执行。
