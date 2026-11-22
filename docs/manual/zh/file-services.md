# 文件服务与 WebDAV · File services and WebDAV

`mode = "file"` 让 agent 直接提供文件服务,不需要另起 HTTP 服务。可以走独立公开端口,
也可以走 server 的 HTTP/HTTPS/HTTP3 域名入口。支持 WebDAV、Range 下载、目录列表和
多账号认证。

`mode = "file"` serves files directly from the agent, with no separate HTTP server. It can
use a dedicated public port or ride on the server's HTTP domain entry. WebDAV, Range
requests, directory listings and per-account authentication are supported.

## 独立端口方式 · Dedicated port

```toml
[[tunnel]]
id = 100
mode = "file"
port = 9001
bind = "127.0.0.1"

[tunnel.file]
root = "public"
prefix = "/files"
read_only = true
auto_index = true

[tunnel.file.auth.users]
reader = "replace-this-password"
```

| 字段 | 说明 |
| --- | --- |
| `root` | 必填,agent 本机**已存在**的目录,相对进程工作目录;启动时打开目录句柄 |
| `prefix` | URL 前缀 |
| `read_only` | 默认 `true` |
| `auto_index` | 目录列表 |
| `auth.users` | 账号表;留空提供匿名服务 |

请求不能通过 `..`、绝对路径或符号链接越出 `root`,COPY/MOVE 的目标同样受此约束。
`root` 和文件账号配置保留在 agent,**不**放进服务端注册消息。

## 域名入口方式 · HTTP domain entry

配置 `hosts` 时,文件服务走 server 的 HTTP/HTTPS/HTTP3 域名入口,`port` 需匹配该入口:

```toml
[[tunnel]]
mode = "file"
port = 8443
hosts = ["files.example.com"]

[tunnel.file]
root = "public"
read_only = false
```

这种方式下可以设 `tunnel.options.http.backend_http2 = true`。来源 ACL、配额和限速继续
适用。文件模式**不**配置网络 backend、目标网络策略、PROXY 输出或 raw TLS 模式。

## WebDAV 方法 · WebDAV methods

| 模式 | 允许的方法 |
| --- | --- |
| 只读(默认) | `GET`、`HEAD`、`PROPFIND`、`OPTIONS` |
| `read_only = false` | 追加 `PUT`、`DELETE`、`MKCOL`、`COPY`、`MOVE`、`LOCK`、`UNLOCK` |

锁在同一文件隧道的多条连接间共享,agent 重启后释放。
支持 Range、条件请求、目录列表和 `index.html`;大文件按块传输,使用背压不整体缓存。

## 集中配置下的文件服务 · Files under centralized configuration

server 下发文件服务时,只能引用 agent 预先声明的命名目录能力:

```toml
# agent
[remote.file_root.documents]
path = "/srv/documents"
read_only = true
```

```toml
# server: [[managed.<client>.tunnel]] 内
[tunnel.file]
root = "documents/shared"
```

不能下发任意绝对路径;路径或符号链接不能逃出已授权目录;只读目录不能通过远端配置升级为
可写。

## 验证清单 · Verification checklist

1. `felucca-agent --config agent.toml --check` 通过。
2. `curl -i http://127.0.0.1:9001/files/` 能列目录或返回 `index.html`。
3. 带 `Range: bytes=0-99` 请求返回 206。
4. 只读模式下 `PUT` 返回 405。
5. `curl --path-as-is 'http://127.0.0.1:9001/files/../../etc/passwd'` 被拒绝。

## 下一步 · Next steps

- [隧道与端口映射](tunnels.md)
- [HTTP/HTTPS 反向代理](http-services.md)
- [管理界面与集中配置](management.md)
- 示例:[file-server.toml](../../../examples/file-server.toml) ·
[file-agent.toml](../../../examples/file-agent.toml)
