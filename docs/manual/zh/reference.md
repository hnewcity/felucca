# 参考 · Reference

命令、字段、接口、示例索引和术语表。需要解释"为什么"时看前面的 Guides;这里只列"有什么"。

Commands, fields, endpoints, an example index and a glossary. The guides explain *why*;
this page lists *what*.

## 命令行 · CLI

两个程序公共参数:

| 参数 | 环境变量 | 说明 |
| --- | --- | --- |
| `--build-info` | — | 打印程序名、版本、协议和构建 target |
| `-c, --config <SRC>` | `FELUCCA_CONFIG` | TOML/JSON 文件、HTTP(S) URL、内联 JSON、base64 或 `felucca://` URI;agent 可重复 |
| `--expand-env` | `FELUCCA_EXPAND_ENV` | 展开 `${VAR}` / `${VAR:-default}` |
| `--config-token-env <NAME>` | `FELUCCA_CONFIG_TOKEN_ENV` | 存放远程配置 Bearer token 的变量名 |
| `--check` | — | 校验后退出,不绑定、不连接 |
| `service` 子命令 | — | 系统服务管理 |
| `update` 子命令 | — | 校验并安装独立二进制 |

`felucca-server` 附加:

| 参数 | 环境变量 | 说明 |
| --- | --- | --- |
| `--runtime-config <PATH>` | `FELUCCA_RUNTIME_CONFIG` | 运行配置快照路径 |
| `--fingerprint` | — | 打印 `bridge.tls` 证书 SHA-256 |
| `--hash-password` | — | 从 stdin 读一行密码,输出 PBKDF2 哈希;≥12 字节 |
| `--generate-token` | — | 输出 Bearer token 与 `token_hash` |

`felucca-agent` 附加:

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `--launch` | — | `--config` 的别名 |
| `--log-level <LVL>` | `info` | trace/debug/info/warn/error |
| `--log-file <PATH>` | — | 追加写,同时输出 stderr;父目录须存在 |
| `--log-json` | — | 结构化 JSON 事件 |
| `--log-max-bytes <N>` | 10 MiB | 需 `--log-file` |
| `--log-backups <N>` | 5 | 需 `--log-file` |
| `--log-rotate-interval <D>` | — | 如 `24h`,需 `--log-file` |

`service` 子命令:

| 参数 | 说明 |
| --- | --- |
| `--name <NAME>` | 服务实例名 |
| `--scope <user\|system>` | 默认 `user`;Windows 仅 `system` |
| `--dry-run` | 只打印将执行的动作 |
| `render` | 生成服务定义;`--platform systemd\|launchd\|windows` |
| `install` / `start` / `stop` / `restart` / `status` / `uninstall` | 生命周期 |
| `--workdir <DIR>` | 默认配置所在目录 |
| `--binary <PATH>` | 指定安装的程序 |
| `-- <ARGS>` | 额外 daemon 参数 |

`update` 子命令:

| 参数 | 说明 |
| --- | --- |
| `--source <PATH_OR_URL>` | 独立二进制或 tar.gz/zip |
| `--sha256 <HEX>` | 64 位摘要,信任依据 |
| `--check-only` | 只校验不替换 |
| `--expect-version <V>` | 可选版本断言 |
| `--token-env <NAME>` | 私有下载 Bearer token 所在变量 |

## 配置文件结构 · Configuration layout

| 文件 | 顶层内容 |
| --- | --- |
| server | `allowed_ports`、`max_tunnels`、`max_connections`、`[bridge]`、`[http]`、`[admin]`、`[state]`、`[geo]`、`[ntp]`、`[log]`、`[p2p]`、`[node]`、`[user.*]`、`[client.*]`、`[local.*]`、`[share.*]`、`[managed.*]` |
| agent | `server`、`key_id`、`vkey`、`transport`、`reconnect_min/max`、`dial_timeout`、`max_connections`、`[trust]`、`[network]`、`[mux]`、`[kcp]`、`[ntp]`、`[remote]`、`[geo]`、`[[tunnel]]`、`[[mapping]]`、`[[visitor]]`、`[p2p]`、`[log]`(agent 用命令行) |

`allowed_ports` 必须写在第一个 TOML 表头之前。所有表启用 `deny_unknown_fields`。

## 隧道字段速查 · Tunnel field quick reference

| 字段 | 适用模式 | 说明 |
| --- | --- | --- |
| `id` | 全部 | 稳定 ID,启用流量快照后应固定 |
| `mode` | 全部 | tcp/udp/http/socks5/httpproxy/mixedproxy/file/transparent |
| `port` | 除私有外 | server 端口;`http` 须匹配 HTTP 监听 |
| `bind` | tcp/udp/代理/文件 | 只填 IP |
| `public` | 全部 | `false` 表示私有共享 |
| `hosts` | http、file | 域名列表,支持 `*.example.com` |
| `balance` | 固定后端 | round_robin/weighted/least_connections/source_hash/failover |
| `backend` | 固定后端 | `addr`、`weight`、`tls`、`trust` |
| `health` | 固定后端 | `type`、`path`、`interval`、`timeout`、`failures_to_evict`、`successes_to_restore`、`expect_status` |
| `acl_mode`/`acl_rules` | 全部 | 来源 ACL:off/allow/deny |
| `target_policy` | 固定后端与正代 | `allow`/`deny`/`ports` |
| `options.auth.users` | 正代、HTTP | 账号表 |
| `options.http.*` | http | 路径、改写、头、重定向、CORS、错误页、tls_mode、backend_http2 |
| `options.payload.*` | 全部 | `compress`、`encrypt` |
| `options.limits.*` | 全部 | `max_connections`、`rate_limit`、`quota` |
| `options.proxy_protocol` | tcp/udp/http/tls | off/v1/v2 |
| `options.proxy_accept` | 代理入口 | 接收 PROXY 头 |
| `file` | file | `root`、`prefix`、`read_only`、`auto_index`、`auth` |

## 管理 API 一览 · Admin API summary

认证:Cookie + Origin + CSRF,或 `Authorization: Bearer <token>`。
写操作:`If-Match: "<revision>"` 与唯一 `Idempotency-Key`。请求体上限 2 MiB。

| 分组 | 路径 |
| --- | --- |
| 会话 | `/api/login`、`/api/session`、`/api/logout` |
| 资源 | `/api/users`、`/api/clients`、`/api/locals`、`/api/shares`、`/api/accounts` 及各自 `<id>` |
| 集中配置 | `/api/managed/<client>`、`/api/managed/<client>/tunnels/<id>` |
| 本机隧道 | `/api/locals/<group>/tunnels/<id>` |
| 整配置 | `/api/config`、`/api/config?secrets=1`、`/api/validate`、`/api/batch` |
| 实时 | `/api/events`(SSE)、`/metrics`、`/api/metrics`、`/api/profile` |
| 平台 | `/api/node/*`、`/api/platforms/<name>`、`/api/webhooks/<name>`、`/api/ws`、`/api/realtime/subscriptions` |

完整字段见 [管理界面与集中配置](management.md) 和 [外部平台接入](platform.md)。

## 示例配置索引 · Example configuration index

| 文件 | 内容 |
| --- | --- |
| [felucca-server.toml](../../../examples/felucca-server.toml) | server 最小示例 |
| [felucca-agent.toml](../../../examples/felucca-agent.toml) | agent 最小示例 |
| [features-server.toml](../../../examples/features-server.toml) | QUIC 桥接 + HTTP/HTTPS + 证书 |
| [features-agent.toml](../../../examples/features-agent.toml) | 全部代理模式、负载均衡、健康检查 |
| [shared-server.toml](../../../examples/shared-server.toml) | 共用 TCP/UDP 端口 + HTTP/3 |
| [shared-agent.toml](../../../examples/shared-agent.toml) | agent 侧与批量端口映射 |
| [acme-server.toml](../../../examples/acme-server.toml) | ACME 服务端 |
| [zerossl-server.toml](../../../examples/zerossl-server.toml) | ZeroSSL 与 TLS-ALPN-01 |
| [policy-server.toml](../../../examples/policy-server.toml) / [policy-agent.toml](../../../examples/policy-agent.toml) | 分层 ACL、配额、持久化 |
| [file-server.toml](../../../examples/file-server.toml) / [file-agent.toml](../../../examples/file-agent.toml) | 文件服务 |
| [network-agent.toml](../../../examples/network-agent.toml) | 上游代理、自定义 DNS、NTP |
| [payload-agent.toml](../../../examples/payload-agent.toml) | Snappy 与独立数据流加密 |
| [kcp-server.toml](../../../examples/kcp-server.toml) / [kcp-agent.toml](../../../examples/kcp-agent.toml) | KCP 桥接 |
| [local-server.toml](../../../examples/local-server.toml) | 服务端本地目标与嵌套转发 |
| [visitor-server.toml](../../../examples/visitor-server.toml) / [private-agent.toml](../../../examples/private-agent.toml) / [visitor-agent.toml](../../../examples/visitor-agent.toml) | 私有提供端与访客端 |
| [p2p-server.toml](../../../examples/p2p-server.toml) / [p2p-provider.toml](../../../examples/p2p-provider.toml) / [p2p-visitor.toml](../../../examples/p2p-visitor.toml) | P2P 与回落 |
| [admin-server.toml](../../../examples/admin-server.toml) | 管理服务端 |
| [managed-agent.toml](../../../examples/managed-agent.toml) | 集中配置 agent |
| [node-server.toml](../../../examples/node-server.toml) | 外部平台与节点管理 |

配置路径相对进程工作目录。26 份示例都通过了配置校验。

## 数值上限 · Numeric limits

| 项 | 上限 |
| --- | --- |
| 单份配置 | 4 MiB |
| 运行配置快照 | 16 MiB |
| agent 多实例 | 32 |
| 嵌套路由跳数 | 16 |
| 批量操作 | 128 项 |
| 流量导入 | 128 项/次 |
| 幂等记录保留 | 256 次(回调动作与流量导入各 256) |
| 回调队列 | 每目标 `queue_max`,全节点 4096 |
| 直连管理 WS | 16 |
| WS RPC 并发 | 每连接 2 / 全节点 16 |
| 实时订阅 | 每连接 32 项,每项 32 KiB |
| HTTP 后端连接池 | 每 origin 8 闲置、30 秒空闲,外层 256 项 |
| HTTP/3 | 每连接 128 流,每流 1 MiB / 连接 16 MiB 窗口,头 32 KiB |
| UDP | 每来源 65507 字节;每隧道 256 关联 |
| SOCKS5 UDP | 每会话 64 目标,各排队 8 包 |
| 错误页 | 400..599,单页 64 KiB |
| HTTP 域名限制 | 精确域名优先于通配 |

## 环境与构建 · Environment and build

| 项 | 值 |
| --- | --- |
| 最低 Rust | 1.88 |
| 验证环境 | macOS Intel,Rust/Cargo 1.98.1 |
| 工作区测试 | 581 通过 / 0 失败 / 0 忽略(2026-09-17) |
| 二进制 | `target/release/felucca-server`、`target/release/felucca-agent` |

```sh
sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins
sh scripts/cargo-external.sh test  --workspace --locked
sh scripts/cargo-external.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo-external.sh fmt --all --check
```

同一 target 目录一次只运行一个 Cargo 命令;构建、测试及 rustdoc 完成后再修改源码和
内嵌页面。测试需要本机 TCP/UDP 监听权限。

## 术语表 · Glossary

| 术语 | 含义 |
| --- | --- |
| agent | 运行在内网、主动外连的 `felucca-agent` 进程 |
| bridge | server 与 agent 之间的认证长连接 |
| client / `[client.<id>]` | 服务端为一个 agent 配置的身份表,`key_id` 即表名 |
| key_id | agent 在服务端的名字 |
| vkey | agent 与服务端共享的桥接密钥 |
| transport | 桥接传输:tcp/tls/ws/wss/quic/kcp |
| trust | agent 验证服务端证书的方式:webpki/pinned/insecure |
| tunnel | 公开入口到一组后端的映射 |
| backend | agent 实际连接的目标 |
| mapping | 批量端口一一映射 |
| local group | server 自己的服务组,不需要 agent |
| share | 服务端把一条隧道授权给访客 |
| visitor | 持有共享密钥、在本地开监听的 agent |
| provider | 私有共享或 P2P 中提供服务的一方 |
| managed | 服务端下发给 agent 的配置 |
| mux | TCP 类传输之上的有界多路复用 |
| FEC | KCP 的前向纠错,默认 10 数据 + 3 校验 |
| quota / rate_limit | 流量配额与字节/秒限速 |
| snapshot | 流量或运行配置的一次原子持久化 |

## 相关文档 · Related documentation

- [架构说明](../../architecture.md) — crate 职责边界
- [配置与部署](../../configuration.md) — 字段级细节
- [P2P 说明](../../p2p.md)、[私有隧道和共享访问](../../visitors.md)、
  [管理与集中配置](../../management.md)、[节点与外部平台](../../node.md)、
  [运维与分发](../../operations.md)、[透明 TCP](../../transparent.md)
