# 外部平台接入 · External platform integration

`[node]` 让外部的管理平台接管这个节点:平台可以用独立令牌调用管理 API,或者由节点主动
建立反向 WebSocket,把请求收进来。还可以把配置变更事件以签名回调推送出去,并支持流量
导入。这套接口属于 Felucca 自有控制协议,不兼容 nps 平台的线协议。

`[node]` lets an external control plane manage this node: it can call the admin API with a
separate token, or the node can dial out and accept requests over a reverse WebSocket.
Configuration changes can also be pushed as signed callbacks, and external traffic can be
imported. This is Felucca's own control protocol; it is not wire-compatible with nps.

## 启用与权限 · Enabling and scopes

```toml
[node]
id = "edge-a"
state_path = "state/node-events.json"
changes_window = 1024
usage_interval = "10s"

[node.platform.panel]
enabled = true
token = "replace-with-a-random-platform-token"
scope = "account"
user = "office"
mode = "direct"
```

| 字段 | 取值与含义 |
| --- | --- |
| `scope` | `full`(完整管理,不能同时指定 user)或 `account`(必须指定已有 user,只能管理该用户的客户端、隧道并读取本租户回调状态) |
| `mode` | `direct`(令牌可访问公开管理 HTTP API)、`reverse`(仅接受反向 WS RPC,该令牌直接访问管理 HTTP 返回 401)、`dual`(两个方向均可用) |
| `enabled = false` | 两种入口均失效,已有反向连接被关闭 |

令牌至少 16 字节,各平台必须不同。HTTP 用 `Authorization: Bearer <token>`。
平台身份与本地管理账号分开;更改令牌、权限或连接配置会更换反向连接,旧连接中的请求仍按
当前配置复核权限。

公开地址必须 HTTPS/WSS;明文 HTTP/WS 仅允许字面回环地址。WSS 默认验证公开 CA,
也可用 `trust = { mode = "pinned", fingerprint = "..." }`,**不允许 insecure**。
HTTP callback 验证公开 CA,不跟随重定向。

## 直连管理 API · Direct management API

写操作用 JSON、`If-Match: "<配置版本>"` 和唯一 `Idempotency-Key`;网页 Cookie 认证还需
Origin 与 CSRF。

| 接口 | 行为 |
| --- | --- |
| `GET /api/node/status`、`/api/node/registration` | 节点 ID、启动 ID、持久 epoch、版本、事件窗口、可见平台连接状态 |
| `GET /api/node/usage` | 当前权限范围内的客户端在线状态、连接数、累计双向字节 |
| `GET /api/node/changes?after=<epoch>:<sequence>&limit=100` | 持久变更窗口;limit 1–512 |
| `GET /api/node/callbacks?target=...&offset=0&limit=100` | 可见回调队列、投递统计和分页条目 |
| `POST /api/node/callbacks/replay` | `{"ids":["投递 UUID"]}`;清除失败状态并再次投递 |
| `POST /api/node/callbacks/delete` | 仅删除指定可见条目 |
| `POST /api/node/kick` | `{"ids":["client-name"]}`;持久停用并断开客户端,阻止其立即重连 |
| `POST /api/node/sync` | 管理员重新激活本地持久配置,清理旧运行时、缓存及连接 |
| `POST /api/node/traffic` | 导入外部流量增量,原子累计客户端及所属用户计数 |
| `GET/PUT/DELETE /api/platforms/<名称>` | 管理员管理平台 |
| `GET/PUT/DELETE /api/webhooks/<名称>` | 管理员创建/修改/删除持久 webhook |

批量配置、完整导出和恢复复用 `/api/batch`、`GET/PUT /api/config`。同步和 kick 复用同一
配置事务及幂等历史,会增加配置版本;回调动作和流量导入不改变配置版本。

### 流量导入 · Traffic import

必须配置 `state.path`:

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

一次最多 128 项;字节必须为无符号整数,同一客户端的多项先合并。全部权限验证完成后才提交,
任一条目越权或非法则整批不写入。导入的应是**尚未由本节点数据面计量**的外部流量,
重复导入节点已计量流量会重复累计。首次导入会把快照升级到 version 2,升级前的程序不能读。

## 反向 WSS 协议 v1 · Reverse WSS protocol v1

开启 node 后,`GET /api/ws` 接受 HTTP/1.1 WebSocket Upgrade。原生客户端用 Bearer 令牌;
浏览器可用同源登录 Cookie,也可用
`["felucca-node-v1", "felucca-token.<base64url 无填充令牌>"]` 子协议传令牌。
服务端只回显 `felucca-node-v1`,不接受 URL 参数认证。最多同时保留 16 个直连管理 WS。

节点拨出 `reverse_url`,握手带平台 Bearer 令牌和子协议 `felucca-node-v1`,平台必须在响应
中选择该子协议。所有应用消息为 JSON 文本。

节点先发 `hello`(node、boot、epoch、cursor、max_message、max_inflight、heartbeat_ms),
平台再发请求:

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

响应 `{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}`。
RPC 复用普通管理处理器的认证、租户过滤、CAS 和幂等行为。允许传递 If-Match、
Idempotency-Key、Accept、X-Felucca-CSRF;不能覆盖 Host 或 Authorization。
登录、退出、SSE、CPU 采样及 WS Upgrade 本身不走 RPC。

| 限制 | 值 |
| --- | --- |
| 最大消息 | 2 MiB |
| 每连接并发 RPC | 2 |
| 全节点并发 RPC | 16,超限返回 429 |
| 重复进行中请求 ID | 关闭连接 |
| 请求超时 | 504,写请求用相同幂等键重试 |

双方可发 `{"type":"ping","nonce":1}`;节点不覆盖未回答的 ping,超过两个 heartbeat 周期未
收到对应 pong 会断线重连,退避 1–30 秒。配置变更会发 event,订阅落后时发 resync。
设置 `usage_interval` 后节点周期发送 tenant-filtered usage 消息。

## 签名回调 · Signed callbacks

```toml
[node.webhook.events]
user = "office"

[node.webhook.events.delivery]
enabled = true
url = "https://panel.example/felucca/events"
signing_key = "replace-with-a-random-callback-signing-key"
timeout = "5s"
retry_max = 2
retry_backoff = "2s"
queue_max = 100
```

请求头:

| 头 | 内容 |
| --- | --- |
| `X-Felucca-Event-Id` | 投递 UUID;自动重试和手工重放保持不变 |
| `X-Felucca-Timestamp` | 本次请求的 Unix 毫秒时间 |
| `X-Felucca-Signature` | `v1=` 后跟小写十六进制 HMAC-SHA256 |

签名输入是 UTF-8 前缀与**原始请求体字节**直接拼接:

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

接收端应验证签名、时间窗口,并按 event-id 去重。只有 2xx 算成功。
`retry_max` 是首次投递之外的额外尝试数,`retry_backoff` 是固定间隔。
投递为**至少一次**:接收成功后节点若在确认落盘前退出,重启可能再次投递相同 ID。
回调只携带变更元数据,不携带配置密钥;收到事件后用当前授权 API 获取所需资源。

`queue_max` 限制每个目标的持久待投递/失败条目,超限丢弃最旧条目并累计 dropped;0 表示不
保留最终失败条目。全节点最多 4096 条。停用订阅保留已有队列;删除订阅、更换接收地址、
签名密钥、筛选器或模板会清理旧队列。

事件窗口记录 `node.started`、`config.changed`、`config.resync` 以及客户端、用户、
集中配置隧道和域名的创建/更新/删除元数据,查询按租户过滤。游标过期、超前或 epoch 不匹配
返回 410/reset_required;此时先获取当前配置,再从返回的当前位置恢复订阅。该有界窗口不保存
完整配置差异,不能作为长期审计日志。

## 实时订阅与模板 · Realtime subscriptions and templates

直连和反向 WS 都可以用这几个**仅限 WS RPC** 的路径:
`GET /api/realtime/subscriptions`、`GET/PUT/DELETE /api/realtime/subscriptions/<名称>`。
订阅不修改持久配置,不需要 If-Match;每个连接最多 32 项,每项最多 32 KiB,断线全部释放。

```json
{
  "enabled": true,
  "selector": {"event_names":["client.updated"],"clients":["office"]},
  "content": {
    "body_template": {"message":"节点 {{event.client}} 已更新","event":"{{event}}"},
    "header_templates": {"x-event-name":"{{event.kind}}"}
  }
}
```

模板是 JSON 值,不执行脚本。支持 `{{node}}`、`{{id}}`、`{{epoch}}`、`{{event}}` 及
`{{event.kind}}`、resource、action、user、client、tunnel、host、target、sequence、
revision、timestamp_ms、traffic。整个字符串仅为一个占位符时保留原 JSON 类型。
不能覆盖认证、Cookie、HTTP 长度/连接或 `X-Felucca-*` 签名头。

## 流量通知 · Traffic notifications

`traffic_report_interval`(100ms–1h)和 `traffic_report_step_bytes` 可开启导入流量通知:
配置了流量配额的客户端首次导入,以及后续达到时间或字节阈值时发送
`client.traffic.reported`。通知带 `ephemeral: true`,不进入 changes 历史、失败队列或
重试;慢接收端可能跳过,需要完整数据时读 `/api/node/usage`。

## 验证清单 · Verification checklist

1. `GET /api/node/status` 返回节点 ID、epoch 和平台连接状态。
2. 反向 WSS 能完成 `hello` → `request` → `response` 往返。
3. 回调接收端验签通过,重放同一 event-id 不产生重复业务效果。
4. `kick` 后客户端被断开且无法立即重连。
5. 流量导入后 `/api/node/usage` 的累计字节立即变化。

## 下一步 · Next steps

- [管理界面与集中配置](management.md)
- [felucca-server 模块](server.md)
- 示例:[node-server.toml](../../../examples/node-server.toml)
