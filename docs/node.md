# 节点与外部管理平台

节点仍以本地持久配置为准。配置 `node` 后，外部平台可使用独立令牌管理节点，
或由节点主动建立反向 WebSocket。配置示例见 `examples/node-server.toml`。
这套接口属于 felucca 自有控制协议，不兼容 nps 平台的线协议。

## 权限与连接

`node.platform.<名称>`：

- `scope = "full"`：完整管理权限，不能同时指定 user。
- `scope = "account"`：必须指定已有 user，只能管理该用户的客户端、隧道，并读取本租户回调状态。
- `mode = "direct"`：令牌可访问公开管理 HTTP API。
- `mode = "reverse"`：仅接受来自该平台的反向 WS RPC，该令牌直接访问管理 HTTP 会返回 401。
- `mode = "dual"`：两个方向均可用。
- `enabled = false`：两种入口均失效，已有反向连接被关闭。

令牌至少 16 字节，各平台必须不同。HTTP 使用 `Authorization: Bearer <token>`。
平台身份与本地管理账号分开；更改令牌、权限或连接配置会更换反向连接，
旧连接中的请求仍按当前配置复核权限。普通配置和资源查询隐藏令牌及回调签名密钥。

公开地址必须 HTTPS/WSS，明文 HTTP/WS 仅允许字面回环地址。WSS 默认验证公开 CA，
也可使用 `trust = { mode = "pinned", fingerprint = "..." }`；不允许 insecure。
HTTP callback 验证公开 CA，不跟随重定向。

## 管理 API

以下路径均需认证。写操作使用 JSON、`If-Match: "<配置版本>"` 和唯一
`Idempotency-Key`；网页 Cookie 认证还需 Origin 与 CSRF。

| 接口 | 行为 |
| --- | --- |
| GET `/api/node/status`、`/api/node/registration` | 节点 ID、启动 ID、持久 epoch、版本、事件窗口、可见平台连接状态 |
| GET `/api/node/usage` | 当前权限范围内的客户端在线状态、连接数、累计双向字节 |
| GET `/api/node/changes?after=<epoch>:<sequence>&limit=100` | 持久变更窗口；limit 1–512 |
| GET `/api/node/callbacks?target=webhook%2Fexample&offset=0&limit=100` | 可见回调队列、投递统计和分页条目；target 可省略 |
| POST `/api/node/callbacks/replay` | `{"ids":["投递 UUID"]}`；清除失败状态并再次投递 |
| POST `/api/node/callbacks/delete` | 同上，仅删除指定可见条目 |
| POST `/api/node/kick` | `{"ids":["client-name"]}`；持久停用并断开客户端，阻止其立即重连 |
| POST `/api/node/sync` | `{}`；管理员重新激活本地持久配置，清理旧运行时、缓存及连接 |
| POST `/api/node/traffic` | 导入外部流量增量，原子累计客户端及所属用户计数 |
| GET/PUT/DELETE `/api/platforms/<名称>` | 管理员管理平台；GET `/api/platforms` 列表 |
| GET/PUT/DELETE `/api/webhooks/<名称>` | 管理员创建/修改/删除持久 webhook；读取按所属租户过滤 |

批量配置、完整导出和恢复复用 `/api/batch`、GET/PUT `/api/config`。
同步和 kick 复用同一配置事务及幂等历史；它们会增加配置版本。
回调动作和流量导入不改变配置版本。回调动作和流量导入分别保留最近 256 次幂等记录；
重放需使用原请求体及配置版本。超过保留窗口后，不应再次提交旧增量。

流量导入必须配置 `state.path`，例如：

```json
{"items":[{"client":"office","inlet_bytes":1234,"export_bytes":5678}]}
```

一次最多 128 项；字节必须为无符号整数，同一客户端的多项先合并。
全部权限验证完成后才提交；任一条目越权或非法，整个批次均不写入。
计数和幂等标记在同一原子快照落盘，成功导入立即影响配额。
导入的应是尚未由本节点数据面计量的外部流量，重复导入节点已计量流量会重复累计。
首次导入会将流量快照升级到 version 2；升级前的程序不能读取它。

## 直连与反向 WS 协议 v1

开启 node 后，`GET /api/ws` 接受 HTTP/1.1 WebSocket Upgrade。原生客户端使用 Bearer
令牌；浏览器可使用同源登录 Cookie，也可通过
`["felucca-node-v1", "felucca-token.<令牌的 base64url 无填充编码>"]` 传递令牌。
服务端只回显 `felucca-node-v1`，不接受 URL 参数认证。Cookie 握手必须带正确 Origin，
Cookie 写请求还需在 RPC headers 中传 `x-felucca-csrf`；普通 GET 无需 CSRF。
最多同时保留 16 个直连管理 WS。凭据撤销、过期或权限变化会关闭现有连接。

节点拨出 `reverse_url`，握手带平台 Bearer 令牌和子协议 `felucca-node-v1`；
平台必须在响应中选择该子协议。所有应用消息为 JSON 文本。

节点先发送 `hello`，包含 node、boot、epoch、cursor、max_message、
max_inflight、heartbeat_ms。平台发送：

```json
{"type":"request","id":"request-1","method":"GET","path":"/api/clients"}
```

写请求另带 body 及 headers：

```json
{"type":"request","id":"request-2","method":"POST","path":"/api/node/kick","headers":{"if-match":"\"3\"","idempotency-key":"kick-001"},"body":{"ids":["office"]}}
```

响应为 `{"type":"response","id":"request-1","status":200,"headers":{},"body":{...}}`。
RPC 复用普通管理处理器的认证、租户过滤、CAS 和幂等行为。允许传递 If-Match、
Idempotency-Key、Accept、X-Felucca-CSRF；不能覆盖 Host 或 Authorization。
登录、退出、SSE、CPU 采样及 WS Upgrade 本身不走 RPC。

最大消息 2 MiB，每个连接最多 2 个并发 RPC，全节点最多 16 个；超限返回 429。
重复的进行中请求 ID 关闭连接。请求超时返回 504，写请求应使用相同幂等键重试。

双方可发送 `{"type":"ping","nonce":1}`，对方回复相同 nonce 的 pong。
节点不覆盖未回答的 ping；超过两个 heartbeat 周期未收到对应 pong 会断线重连。
重连退避 1–30 秒。配置变更会发送 event，订阅落后时发送 resync。
设置 `usage_interval = "10s"` 后，节点周期发送 tenant-filtered usage 消息。

`traffic_report_interval`（100ms–1h）和 `traffic_report_step_bytes` 可开启导入流量通知。
配置了流量配额的客户端首次导入，以及后续达到时间或字节阈值时，会发送
`client.traffic.reported`，包含客户端、用户和当前累计计数。WS 与 callback 按租户过滤，
消息带 `ephemeral: true`。通知不进入 changes 历史、失败队列或重试；慢接收端可能跳过通知，
需要完整数据时应读取 `/api/node/usage`。重复幂等请求不会再次产生流量通知。

### 临时订阅与 JSON 模板

直连和反向 WS 均可使用以下 **仅限 WS RPC** 的路径：

- GET `/api/realtime/subscriptions`：列出当前连接的订阅。
- GET/PUT/DELETE `/api/realtime/subscriptions/<名称>`：查询、创建/替换、删除。

订阅不修改持久配置，不需要 If-Match。每个连接最多 32 项，每项最多 32 KiB，
断线后全部释放。普通账号也可订阅自己的事件，选择器不能授予其他租户的数据权限。
命中订阅后，原始 event 仍发送，另发 `callback` 帧。例如 PUT：

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

响应通知包含 `type: "callback"`、subscription、body、headers。
同样的 selector/content 可写在持久 webhook 或平台 callback 的 delivery 配置中。
持久 Webhook 由管理员维护；普通账号使用临时订阅获取推送。

选择器包括 event_names、resources、actions、users、clients、tunnels、hosts，
每字段最多 64 项。字段之间取交集，空列表不限；event_names 支持末尾 `*` 前缀匹配。
资源事件包含 user/client/tunnel/host 元数据，实际配置值和凭据不进入事件。
每次配置事务最多补充 128 条资源事件，完整变化仍通过全局配置通知和当前配置查询获取。

模板是 JSON 值，不执行脚本。支持 `{{node}}`、`{{id}}`、`{{epoch}}`、`{{event}}` 及
`{{event.kind}}`、resource、action、user、client、tunnel、host、target、sequence、
revision、timestamp_ms、traffic。整个字符串仅为一个占位符时保留原 JSON 类型；
其他字符串做一次插值，插入内容不会再次展开，JSON 引号由序列化器转义。
模板配置最多 16 KiB，渲染体最多 64 KiB、16 个头部。不能覆盖认证、Cookie、
HTTP 长度/连接或 `X-Felucca-*` 签名头；非法头值拒绝投递。

operations.updated、management_platforms.updated、callbacks_queue.updated、
webhook.delivery_succeeded/failed 为实时通知，带 `ephemeral: true`；
它们不进入持久窗口或 HTTP callback，避免投递事件引发回调循环。

## 签名回调、重试与持久化

平台可配置 callback，也可单独配置 `node.webhook.<名称>`。后者通过 user 限制
所属租户，无 user 表示全节点订阅。回调只携带变更元数据，不携带配置密钥；
收到事件后使用当前授权 API 获取所需资源。

请求体：

```json
{"version":1,"node":"edge-a","epoch":"...","id":"投递 UUID","event":{"sequence":1,"revision":0,"timestamp_ms":0,"kind":"node.started"}}
```

签名密钥至少 16 字节。请求头：

- `X-Felucca-Event-Id`：投递 UUID；自动重试和手工重放保持不变。
- `X-Felucca-Timestamp`：本次请求的 Unix 毫秒时间。
- `X-Felucca-Signature`：`v1=` 后跟小写十六进制 HMAC-SHA256。

签名输入是以下 UTF-8 前缀与**原始请求体字节**直接拼接：

```text
felucca-callback-v1\n<timestamp>\n<event-id>\n<body>
```

接收端应验证签名、时间窗口，并按 event-id 去重。回调使用系统时钟；
桥接握手的 NTP 偏移不会修改操作系统时钟。只有 2xx 算成功。
`retry_max` 是首次投递之外的额外尝试数，`retry_backoff` 是固定间隔。
投递为至少一次：接收成功后节点若在确认落盘前退出，重启可能再次投递相同 ID。

`queue_max` 限制每个目标的持久待投递/失败条目，超限丢弃最旧条目并累计 dropped。
设为 0 时不保留最终失败条目，首次投递前仍保留最多 32 条有界待办。
全节点最多 4096 条。失败队列分页是实时视图，投递期间页间条目可能变化；
批量动作按 UUID 验证，最多 128 条，越权时整批拒绝。
停用订阅保留已有队列；删除订阅、更换接收地址、签名密钥、筛选器或模板会清理旧队列。

`node.state_path` 使用独占文件锁、0600 临时文件、fsync 与原子替换。
损坏或未知版本会拒绝启动，不会静默清空。重启保留 epoch、历史和失败队列。
事件窗口记录 `node.started`、`config.changed`、`config.resync` 以及客户端、用户、
集中配置隧道和域名的创建/更新/删除元数据，查询按租户过滤。
游标过期、超前或 epoch 不匹配返回 410/reset_required；此时先获取当前配置，
再从返回的当前位置恢复订阅。该有界窗口不保存完整配置差异，不能作为长期审计日志。

## 验证范围

真实 HTTP、直连 WS 和反向 WSS 集成测试覆盖租户隔离、指纹、令牌权限、Cookie/CSRF、禁用连接、
心跳超时、回调签名/重试/重启重放、流量幂等累计和持久 kick/sync。
临时订阅另验证数量上限、断线释放、模板与资源过滤、实时投递通知。
持久化单测覆盖损坏文件、文件锁、写入失败、请求取消和停机等待。
公网平台接入及跨操作系统部署需在对应环境验收。
