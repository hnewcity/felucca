# HTTP/HTTPS 反向代理 · HTTP/HTTPS reverse proxy

本页说明怎么把内网 HTTP 服务按域名和路径暴露出去:路由规则、Host/头改写、
重定向、CORS、错误页、SNI、HTTP/2 与 HTTP/3、共用端口和来源头处理。

This page covers exposing internal HTTP services by domain and path: routing, header and
Host rewriting, redirects, CORS, error pages, SNI, HTTP/2 and HTTP/3, shared ports and
forwarded headers.

## 注册一条 HTTP 隧道 · Register an HTTP route

```toml
# server 侧
[http]
bind = "0.0.0.0:8080"
tls_bind = "0.0.0.0:8443"

# agent 侧
[[tunnel]]
mode = "http"
port = 8080                       # 必须匹配 http.bind 或 http.tls_bind
hosts = ["app.example.test", "*.example.test"]
balance = "round_robin"

[tunnel.options.http]
path = "/api/"
scheme = "any"
rewrite_prefix = "/v2/"
host = "backend.internal"

[[tunnel.backend]]
addr = "127.0.0.1:3000"
```

`http` 隧道不创建独立监听;`port` 决定它挂在哪个 HTTP 监听上,HTTP 与 HTTPS 都能访问。

## 路由规则 · Routing rules

```toml
[tunnel.options.http]
path = "/api/"
scheme = "any"        # any | http | https
```

匹配优先级:**精确域名 → 最长路径前缀 → 精确 scheme**。
域名大小写不敏感,末尾的点会归一化。

| 模式 | 匹配 |
| --- | --- |
| 精确 `app.example.test` | 只匹配该域名 |
| 通配 `*.example.test` | 匹配 `a.example.test`,**不**匹配 `example.test` 或 `a.b.example.test` |

相同域名、路径、协议的重复注册会被拒绝。没有匹配路由时返回 404。
同域名可以注册多个路径,分配到不同隧道。

## 请求与响应选项 · Request and response options

```toml
[tunnel.options.http]
rewrite_prefix = "/v2/"
host = "backend.internal"
cors = true
redirect = "https://new.example{path}"
force_https = true
request_headers = { x-source = "{remote_ip}", x-remove = "" }
response_headers = { x-proxy = "felucca" }
error_pages = { "502" = "<h1>Service unavailable</h1>" }
```

| 选项 | 行为 |
| --- | --- |
| `rewrite_prefix` | 前缀改写,保留剩余路径与查询串 |
| `host` | 改写发给后端的 Host |
| `request_headers` | 头模板;值为空表示删除该头 |
| `response_headers` | 响应头模板 |
| `cors` | 返回 `Access-Control-Allow-Origin: *`,合法预检直接 204;预检不要求站点账号,实际请求仍要求认证 |
| `redirect` | 返回 307 |
| `force_https` | 返回 301,目标使用服务端 HTTPS 监听端口(443 省略);注册时必须有 HTTPS 监听 |
| `error_pages` | 按状态码替换响应体,支持 400..599,单页最多 64 KiB |

模板支持 `{host}`、`{path}`(含查询串)、`{scheme}`、`{remote_ip}`、`{remote_port}`,
取值为原请求,替换结果不会再次展开。Content-Length、Transfer-Encoding 等报文边界头和
逐跳头不可改写;Host 用专用字段。

站点认证用 `Authorization: Basic`,失败返回 401;已验证的凭据不会发往后端。

## TLS 模式 · TLS modes

```toml
[tunnel.options.http]
tls_mode = "http"        # http | passthrough | offload
```

| 模式 | 行为 |
| --- | --- |
| `http` | 默认。普通 HTTPS 反向代理 |
| `passthrough` | 按 SNI 把原始 TLS 转给后端,后端证书直接展示给访问者;此时 `backend.tls` 必须关闭 |
| `offload` | 用服务端 SNI 证书终止 TLS,然后把任意 TCP 内容发往后端 |

`passthrough` 和 `offload` 占用完整 SNI,不能同时配置 HTTP 路径/头部选项或站点认证。

## 端口与协议 · Ports and protocols

| 能力 | 说明 |
| --- | --- |
| 入口 | HTTP/1.1、HTTP/2、HTTP/3 |
| 后端 | HTTP/1.1、HTTP/2(`backend_http2 = true`) |
| ALPN | TLS 入口支持 `h2` 和 `http/1.1`;明文入口支持 HTTP/2 prior knowledge |
| WebSocket | 三种入口 × 两种后端自动转换;入口为 HTTP/1.1 返回 101,HTTP/2 或 HTTP/3 使用 `:protocol = websocket` 扩展 CONNECT |
| HTTP/3 | 需要 `http.http3_bind`;每个 QUIC 连接最多 128 个双向流,接收窗口每流 1 MiB、连接 16 MiB,闲置 30 秒关闭,请求头限 32 KiB |

HTTP/3 只是公网入口协议,后端仍选 HTTP/1.1 或 HTTP/2。正常 HTTPS 响应会附加 `Alt-Svc`,
外部映射端口不同时用 `response_headers` 覆盖。

## 共用端口 · Sharing ports

`bridge.bind`、`http.bind`、`http.tls_bind` 可以填**同一个** IP:port,一个 socket 按握手
特征分流:直接连接的 TLS/WSS agent 使用 `felucca/9` ALPN,网站使用 `h2`/`http/1.1` 或无
ALPN;普通 `GET /ws` 仍是站点请求,只有正确的 WebSocket Upgrade 才进入桥接。
QUIC 桥接与 HTTP/3 也可共用同一个 UDP 地址,按 ALPN 分流,桥接证书与网站证书分别选择、
分别刷新。

## 来源地址与 PROXY 头 · Client addresses

默认清除客户端传入的 `Forwarded` / `X-Forwarded-*` / `X-Real-IP`,然后按直连来源或经
`http.proxy_accept` 验证的 PROXY 来源设置 `X-Forwarded-For`、`X-Real-IP`、
`X-Forwarded-Host`、`X-Forwarded-Proto`。设 `forwarded_headers = false` 可关闭这个改写,
适合自行控制完整代理链的部署。逐跳头和 `Connection` 指定的头不会透传到下一跳。

## 下一步 · Next steps

- [证书、SNI 与 ACME](certificates.md)
- [后端、负载均衡与健康检查](backends.md)
- [文件服务与 WebDAV](file-services.md) — 走域名的文件入口
- [访问控制、配额与数据流编码](security.md) — 域名配额与 ACL
