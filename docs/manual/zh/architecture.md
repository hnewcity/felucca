# 架构与核心概念 · Architecture and concepts

本页解释 Felucca 由哪些部分组成、数据怎么流动,以及后面每个模块都会用到的术语。
模块内部职责划分见 [架构说明](../../architecture.md)。

This page explains the parts of Felucca, how traffic flows, and the vocabulary used by
every later guide. Internal module ownership is documented separately in
[Architecture](../../architecture.md).

## 部署形态 · Deployment shape

```text
                    公网 / public network
 ┌──────────────────────────────────────────────────────┐
 │  felucca-server                                      │
 │  bridge   14028 (TCP/TLS/WS/WSS 共用一个 socket)     │
 │  quic     14028 (UDP, QUIC 原生流)                   │
 │  kcp      8025 (UDP)                                 │
 │  http     8080 / 8443 (+ http3 UDP)                  │
 │  admin    8088 (管理界面,可选)                        │
 └───────────────▲──────────────────────────────────────┘
                 │ agent 主动外连 / agent dials out
        ┌────────┴────────┐
        │  felucca-agent  │   内网 / private network
        │  本地服务 3000   │
        └─────────────────┘
```

`felucca-agent` 主动连接公网的 `felucca-server`,所以内网不需要公网 IP、不需要在路由器
上做端口映射,也不需要改防火墙。公开入口(端口、域名、代理、文件)全部由 server 持有。

The agent dials out. The private network needs no public address and no router rules; every
public entry point lives on the server.

## 两个程序 · The two programs

| 程序 | 职责 | 需要开放的入站端口 |
| --- | --- | --- |
| `felucca-server` | 接入 agent、持有公开监听、路由、管理、计量 | 是 |
| `felucca-agent` | 在内网发起连接、持有本地监听、执行后端请求 | 否 |

两者必须使用同一个协议版本。当前桥接协议版本为 10,**server 与 agent 必须一起升级**。
用 `--build-info` 可以打印程序名、版本、协议和构建 target:

```sh
target/release/felucca-server --build-info
target/release/felucca-agent --build-info
```

## 术语表 · Key concepts

| 术语 | 说明 |
| --- | --- |
| **桥接 bridge** | server 与 agent 之间的长连接。`bridge.bind` 是 TCP/TLS/WS/WSS 共用的地址,默认 `0.0.0.0:14028`;QUIC 与 KCP 各自使用独立 UDP 端口 |
| **身份 identity** | agent 用 `key_id` + `vkey` 标识;服务端 `[client.<key_id>]` 必须存在同名表项 |
| **传输 transport** | agent 侧可选 `tcp`、`tls`、`ws`、`wss`、`quic`、`kcp`。公网优先 TLS/WSS/QUIC |
| **信任 trust** | agent 验证 server 证书的方式:`webpki`(CA 验证)、`pinned`(固定 SHA-256 指纹)、`insecure`(仅诊断) |
| **隧道 tunnel** | 一个公开入口到一组后端的映射。`mode` 决定入口协议,`backend` 决定目标 |
| **后端 backend** | agent 实际要连接的目标,形如 `127.0.0.1:3000` |
| **监听器 listener** | server 为隧道打开的 socket。同一 `key_id` 重连会替换旧监听 |
| **共享 share** | 服务端把一条已注册隧道授权给指定访客,不占用公开端口 |
| **访客 visitor** | 持有共享密钥的另一个 agent,在自己机器上开监听访问该共享 |
| **本地组 local** | server 自己提供服务的配置组,不需要 agent |
| **mux** | TCP/TLS/WS/WSS/KCP 之上的有界多路复用,提供窗口协商、预算与背压 |
| **集中配置 managed** | server 下发给 agent 的隧道配置,agent 需显式开启 `[remote]` |

## 一条请求的路径 · Life of a request

以 TCP 隧道为例:

1. 访问者连接 server 的公开端口。
2. server 按来源 ACL、配额和连接额度检查,然后分配到对应 agent 的桥接流。
3. agent 收到请求,按负载均衡选择一个健康后端,建立连接。
4. 双向数据通过 mux 流(或 QUIC 原生流)转发,直到任一侧半关闭。
5. server 累加流量;配了 `[state]` 或管理端时也可跨重启保留。

HTTP 隧道多出域名/路径路由和站点认证两步;SOCKS5 多出目标地址解析和目标 ACL 两步。

## 内部模块 · Internal modules

使用者通常不需要关心这些,但它们解释了"哪个模块做什么":

| crate | 用途 |
| --- | --- |
| `felucca-core` | 配置、标识、访问规则、计量类型;不依赖异步运行时 |
| `felucca-mux` | 有界多路复用、窗口与背压 |
| `felucca-proto` | 握手、控制消息、访客与直连协议 |
| `felucca-transport` | TCP/TLS/WS/QUIC/KCP 连接适配与出口网络 |
| `felucca-service` | 两端共用的后端连接、负载均衡、健康检查、受限文件服务 |
| `felucca-server` | agent 接入、入口路由、管理、运行状态 |
| `felucca-client` | agent 连接、重连、配置接收、访客 |
| `felucca-cli` | 两个程序、启动配置、日志、系统服务、更新 |

## 下一步 · Next steps

- [快速开始](get-started.md) 还没跑通的话先回到上一步
- [配置来源与启动参数](configuration.md)
- [felucca-server 模块](server.md) / [felucca-agent 模块](agent.md)
