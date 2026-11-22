# 配置来源与启动参数 · Configuration sources and CLI

本页说明配置文件从哪来、怎么传、能用哪些环境变量,以及怎么在不启动的情况下校验。
字段本身的含义在后续各模块页。

This page covers where configuration comes from, how it is passed, which environment
variables apply, and how to validate without starting. Field meanings live in the module
guides.

## 配置文件 · Configuration files

两个程序都接受 TOML 和 JSON。相对路径以进程工作目录为基准,所以 `--workdir`、
systemd/launchd 的启动目录都会被考虑进来。

```sh
felucca-server --config /srv/felucca/server.toml
felucca-agent  --config /srv/felucca/agent.toml
```

不传 `--config` 时默认读工作目录下的 `felucca-server.toml` / `felucca-agent.toml`。
`-c` 是 `--config` 的短形式;agent 还支持别名 `--launch`。

所有配置都启用 `deny_unknown_fields`:未知字段直接报错,不会静默忽略。

## 配置来源类型 · Accepted sources

`--config` 接受四种形式:

| 形式 | 示例 | 说明 |
| --- | --- | --- |
| 本地文件 | `conf/server.toml` | TOML 或 JSON |
| HTTP(S) URL | `https://cfg.example/edge.json` | 最多等 15 秒,必须 200;不跟随重定向,不接受压缩响应;URL 不接受 userinfo 和 fragment,查询参数可用于签名下载地址 |
| 内联 JSON | `'{"bridge":{"bind":"0.0.0.0:14028"}}'` | 直接传 JSON 对象 |
| base64 | `base64:eyJicmlkZ2Ui...` | 标准 base64 或无 padding 的 base64url |

单份配置上限 4 MiB。`--config-token-env NAME` 从环境变量读取 Bearer token,用于私有
下载地址;凭据不会写进日志。URL、token 和内联正文不会作为配置来源名称打印。

远程配置里的证书、Geo 数据、文件根目录仍然指**agent 本机**的路径。

## agent 启动 URI · Agent launch URI

`felucca-agent` 额外支持简短 URI。默认只用于连接，本机隧道仍需 TOML/JSON；加上 `remote=1` 后隧道和访问范围全部由服务端下发，一条命令即可：

```sh
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&transport=tls'
felucca-agent --config 'felucca://bridge.example.com:14028?key_id=edge&vkey=YOUR_KEY&fingerprint=SHA256_HEX&remote=1'
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| 端口 | 是 | 必须非零 |
| `vkey` | 是 | 共享密钥,值需 URL 百分号编码 |
| `key_id` | 否 | 省略时用默认身份 |
| `transport` | 否 | `tcp`/`tls`/`ws`/`wss`/`quic`/`kcp` |
| `fingerprint` | 否 | 启用固定证书 SHA-256 验证 |
| `ws_path` | 否 | WebSocket 路径 |
| `remote` | 否 | `1` 表示服务和访问范围全部由服务端下发：开启 `remote.enabled` 与 `remote.console_scope`，服务端未给范围前只允许本机地址 |

未知字段和重复字段会报错。

## 多实例 · Multiple agent instances

`--config` 可重复,一个进程最多 32 个实例:

```sh
felucca-agent --config edge-a.toml --config edge-b.json
felucca-agent --config 'https://cfg.example/edge.json' --config-token-env CONFIG_TOKEN
```

每个实例独立维护连接、退避、路由、探针和连接上限。全部配置先校验并初始化,之后才开始
连接;重复的 `server` + `key_id` 组合会报错。某个服务器不可达时其他实例继续工作,
退出信号关闭所有实例。

## 环境变量展开 · Environment expansion

默认关闭。设置 `--expand-env`(或 `FELUCCA_EXPAND_ENV=true`)后,已解析的字符串值会展开:

```toml
server = "${FELUCCA_SERVER:-127.0.0.1:9001}"
key_id = "${FELUCCA_KEY_ID:-agent-1}"
vkey = "${FELUCCA_VKEY}"
```

`${VAR}` 要求变量存在;`${VAR:-default}` 在变量不存在或为空时使用默认值;`$$` 是字面量
`$`。展开只执行一次,不修改字段名,也不把字符串转成数值。展开后超过 4 MiB 会被拒绝。

## 运行配置快照 · Runtime configuration snapshot

```sh
felucca-server --config bootstrap.toml --runtime-config conf/runtime.json
```

也可用 `FELUCCA_RUNTIME_CONFIG` 指定路径。第一次启动成功后原子保存规范化配置;以后启动
优先读取快照。`--check` 和 `--fingerprint` 也读同一快照。文件损坏时启动报错,不回退到
启动文件。**不要**把它和 `[state].path` 的流量文件设成同一个路径。

快照最大 16 MiB 且含实际密钥,Unix 上新文件权限 0600、新目录 0700;文件锁拒绝另一个
daemon 同时使用。

管理界面改配置走的也是这套机制,详见[管理界面与集中配置](management.md)。

## 校验与日志 · Validation and logging

```sh
felucca-server --config conf/server.toml --check
felucca-agent  --config conf/agent.toml  --check
```

`--check` 校验 TOML/JSON 结构、字段约束、端口、ACL 和 Geo 数据,**不**连接后端、**不**
验证公网 DNS、**不**检查证书文件内容。证书内容和绑定冲突在启动时检查。

agent 的日志参数:

```sh
felucca-agent -c agent.toml --log-level info \
  --log-file /data/agent.log --log-json \
  --log-max-bytes 10485760 --log-backups 5 --log-rotate-interval 24h
```

`RUST_LOG` 在两端都能覆盖日志等级。server 的日志写在配置的 `[log]` 表里,改它需要重启。

## 环境变量总表 · Environment variables

| 变量 | 作用 |
| --- | --- |
| `FELUCCA_CONFIG` | 等价于 `--config`(agent 可多次) |
| `FELUCCA_RUNTIME_CONFIG` | 等价于 server `--runtime-config` |
| `FELUCCA_EXPAND_ENV` | 等价于 `--expand-env` |
| `FELUCCA_CONFIG_TOKEN_ENV` | 保存远程配置 Bearer token 的变量名 |
| `RUST_LOG` | 覆盖日志过滤等级 |

## 下一步 · Next steps

- [felucca-server 模块](server.md)
- [felucca-agent 模块](agent.md)
- [系统服务、更新、日志与监控](operations.md)
