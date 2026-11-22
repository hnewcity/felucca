# 访问控制、配额与数据流编码 · Access control, quotas and stream coding

本页把安全相关的配置集中说明:来源 ACL、目标 ACL、GeoIP/GeoSite、分层配额与限速、
流量快照,以及每条隧道可选的压缩和独立加密。

This page collects the security-related configuration: source ACLs, target ACLs,
GeoIP/GeoSite, layered quotas and rate limits, traffic snapshots, and the optional
per-tunnel compression and encryption.

## 来源 ACL · Source ACL

```toml
[client.agent-1]
acl_mode = "allow"                 # off | allow | deny
acl_rules = "203.0.113.0/24, 2001:db8::/32"
```

规则是逗号或换行分隔的 IP/CIDR。服务端的顶层 `acl_mode`/`acl_rules`、`[user.<name>]`、
`[client.<id>]` 与隧道规则**取交集**;客户端通过 `user = "<name>"` 关联本机配置中的用户,
agent 无权修改关联。

这些规则控制公开入口的访问者,**不用于拒绝 agent 的桥接地址**——桥接身份由 vkey 校验。
规则作用于访问代理的源 IP,不是目标地址。

来源 ACL 支持 GeoIP。`acl_rules` 也可以写在 `[[tunnel]]`、`[local.<name>]` 和
`[[visitor]]` 上。

## 目标 ACL · Target policy

在 agent 侧执行,适用于固定后端、健康探针和正代目标:

```toml
[tunnel.target_policy]
allow = ["domain:api.example.com", "suffix:example.net", 'regex:^db[0-9]+\.internal$']
deny = ["127.0.0.0/8", "::1", "169.254.0.0/16"]
ports = [80, 443]
```

| 项 | 说明 |
| --- | --- |
| `allow` 为空 | 不限制允许目标 |
| `deny` | 始终优先 |
| `ports` 为空 | 允许任意非零端口 |
| 语法 | IP/CIDR、`domain:`、`suffix:`、`regex:`、`*` |
| Geo | `geoip:CN`、`geosite:category-ads-all`、`geosite:custom:LOCAL@fast@!ads` |

`@属性` 要求存在,`@!属性` 要求不存在,多个条件取交集。`geoip:private` 内置 IPv4/IPv6
私有、回环、链路本地和 CGNAT 网段,不需要文件。名称和解析后的 IP 一起检查;名称不区分
大小写,末尾点会规范化。

缺失文件、分组、错误正则和损坏 protobuf **会报错**,不会退化成空规则放行。
数据在启动时加载,更新数据文件后需重启相应进程。两端各自使用本机声明的数据文件,
远端只能引用分组及别名。

## Geo 数据 · Geo data

```toml
[geo]
geoip = "data/geoip.dat"
geosite = "data/geosite.dat"

[geo.extra]
mycorp = "data/mycorp.dat"
```

指向 V2Ray protobuf `.dat` 文件,路径相对进程工作目录。IP 前缀采用已编译区间查询;
域名支持精确、后缀、关键词和正则。

## 分层配额 · Layered quotas

`[user.<name>.limits]`、`[tunnel.options.limits]`、`[http.host_limit."<域名模式>"]`
接受相同字段:

```toml
max_connections = 20
rate_limit = 1048576
quota = { flow_limit = 1073741824, expires_at = 1893456000 }
```

流量和速率单位是字节,`expires_at` 是 Unix 秒。未填写表示不限;速率 0 表示不限,
连接数 0 明确禁止新连接。同用户多客户端、同隧道多连接、同域名多路径共用各自计数与限制。

多层速率同时计费,等待时间取**严格的一层**;连接名额申请失败会归还此前层的名额。
HTTP 域名限制按原始 Host/SNI 选择,精确域名优先于通配,不受后端 Host 改写影响。
HTTP 连接数统计实际后端连接;HTTP/2 多请求可以复用一个连接。

TCP、HTTP、SNI 和 UDP 都更新各层计数。TCP 以 16 KiB 块检查并限速,UDP 以整个报文处理。
并发中正在转发的数据可能使总量略超配额,**不是精确逐字节截断**。
HTTP 的计数是后端协议字节,含头部及协议开销;UDP 计数仅含载荷。
配额会中止正在传输的连接,计数不用于严格计费。

## 流量快照 · Traffic snapshots

```toml
[state]
path = "data/traffic.json"
save_interval = "5s"
```

启用后按间隔原子替换快照,并在正常退出、连接结束后保存最终计数。快照包括离线资源,
server 重启后仍计入配额;崩溃可能丢失最后一个保存间隔内的数据。同一快照只能由一个
server 写入。损坏或未知版本的快照会拒绝启动,保留原文件。

这是流量快照;管理界面的运行配置另存于 `admin.state_path` / `--runtime-config`,
两者不要用同一路径。启用快照后应固定隧道的 `id`。

## 每流压缩与加密 · Per-stream coding

```toml
[tunnel.options.payload]
compress = true
encrypt = true
```

| 选项 | 实现 | 行为 |
| --- | --- | --- |
| `compress` | Snappy | 以最多 16 KiB 原始数据为一块,仅在压缩后更小时才使用压缩块;解压前检查声明大小 |
| `encrypt` | ChaCha20-Poly1305 | 每条流交换新随机数,用该 agent 的 vkey 验证双方身份及完整路由请求,HKDF 派生两个方向独立的密钥 |

启用压缩时**先压缩后加密**。长度头、序号和结束记录受完整性验证,篡改、重放和缺失结束
记录会关闭该数据流。两端不匹配时拒绝协商,不自动退回明文。每个方向只缓存一个有界记录。

这层编码位于 mux/QUIC 流内,不增加一套多路复用;PROXY 头、HTTP/WebDAV 内容和 UDP 报文
都经过同一编码。流量配额和限速仍按**原始转发字节**计算。

安全边界:

- 独立加密保护路由的数据部分;配置注册和桥接控制消息仍由所选桥接传输保护,
  因此 TLS/WSS/QUIC 仍是默认部署选择。
- 该 PSK 层**不单独提供前向保密**。
- 压缩会改变可观察的记录长度;需要隐藏压缩特征时保持 `compress = false`。

## 密钥处理 · Secret handling

- 原始密钥不进入日志;Debug/Display 输出脱敏。
- 持久化必须保留密钥,展示时才脱敏;新增凭据字段要同步检查脱敏路径。
- 配置快照含实际密钥,Unix 上新文件 0600、新目录 0700。
- 不要把原始管理 Bearer token 写进服务端配置,只写 `token_hash`。

## 下一步 · Next steps

- [隧道与端口映射](tunnels.md)
- [HTTP/HTTPS 反向代理](http-services.md)
- [管理界面与集中配置](management.md)
