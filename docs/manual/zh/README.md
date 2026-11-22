# Felucca 文档目录 · 中文 Table of contents

中文 · [English](en/README.md) · [日本語](ja/README.md) · [한국어](ko/README.md) · [Français](fr/README.md) · [Español](es/README.md) · [Русский](ru/README.md)

本目录是面向使用者的 Felucca 文档:从零跑通第一条隧道,到每个模块怎么配怎么用,
最后是命令、字段、接口和术语参考。文档描述**如何使用 Felucca**,不描述内部实现。

## 快速入门 Get Started

| 页面 | 内容 |
| --- | --- |
| [快速开始](get-started.md) | 安装、本地演示、手工跑通第一条隧道 |
| [架构与核心概念](architecture.md) | 组成部分、数据流向、术语表 |
| [配置来源与启动参数](configuration.md) | 配置来源、环境变量、多实例、`--check` |

## 使用指南 Guides

| 页面 | 内容 |
| --- | --- |
| [felucca-server 模块](server.md) | 桥接、客户端表、HTTP 监听、端口授权 |
| [felucca-agent 模块](agent.md) | 连接、信任、重连、出口网络、上游代理 |
| [隧道与端口映射](tunnels.md) | 八种模式、`[[mapping]]`、UDP、PROXY 协议 |
| [后端、负载均衡与健康检查](backends.md) | 后端字段、五种策略、探针摘除与恢复 |
| [HTTP/HTTPS 反向代理](http-services.md) | 域名/路径路由、改写、CORS、SNI、HTTP/2、HTTP/3 |
| [证书、SNI 与 ACME](certificates.md) | 桥接证书、静态证书、自动签发与续期 |
| [正向代理](forward-proxy.md) | SOCKS5 / HTTP / 混合入口、账号、目标 ACL |
| [文件服务与 WebDAV](file-services.md) | 目录能力、只读控制、Range、WebDAV 方法 |
| [服务端本地目标与嵌套路由](local-services.md) | `[local.*]`、后端别名、16 跳环路控制 |
| [私有共享与访客](private-shares.md) | 无公开端口的 TCP/UDP/文件/代理访问 |
| [P2P 直连与中继回退](p2p.md) | 一次性票据、打洞、QUIC 直连、回落 |
| [访问控制、配额与数据流编码](security.md) | 来源/目标 ACL、GeoIP、分层配额、压缩与加密 |
| [管理界面与集中配置](management.md) | 管理 UI、账号角色、API、配置下发 |
| [外部平台接入](platform.md) | 平台令牌、反向 WSS RPC、签名回调、流量导入 |
| [系统服务、更新、日志与监控](operations.md) | systemd/launchd/SCM、更新、日志轮转、指标 |

## 参考 Reference

| 页面 | 内容 |
| --- | --- |
| [参考](reference.md) | CLI、字段速查、管理 API、示例索引、数值上限、术语表 |

## 相关文档 Related

- [架构说明](../../architecture.md) — crate 职责边界
- [配置与部署](../../configuration.md) — 字段级细节
- [功能状态](../../implementation-status.md) · [验证记录](../../validation.md)
