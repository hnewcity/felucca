# Felucca 技术文档 · Felucca Documentation

**中文** · [English](en/README.md) · [日本語](ja/README.md) · [한국어](ko/README.md) ·
[Français](fr/README.md) · [Español](es/README.md) · [Русский](ru/README.md)

面向使用者的完整文档:从安装、快速上手,到每一个功能模块怎么配、怎么用,
最后是命令、字段和接口参考。本指南描述**如何使用 Felucca**,不描述内部实现;
模块职责与源码结构见 [架构说明](../architecture.md)。

This is the complete user documentation: installation, a first working tunnel, how to
configure and use every module, and finally the command, field and API references.
It describes **how to use Felucca**, not how it is implemented. For module ownership and
source layout see [Architecture](../architecture.md).

## 语言 · Languages

七个语言的页面结构完全一致:同一批章节、同样的表格和代码块,每页都可以单独阅读。
代码块、字段名、路径和命令在每种语言里保持一致,只有说明文字不同。
Chinese and English are the reference texts; Japanese, Korean, French, Spanish and
Russian are complete translations of the same twenty pages, with identical structure,
code and field names in every language.

| 语言 Language | 文档首页 Index |
| --- | --- |
| 中文 | [zh/README.md](zh/README.md) |
| English | [en/README.md](en/README.md) |
| 日本語 | [ja/README.md](ja/README.md) |
| 한국어 | [ko/README.md](ko/README.md) |
| Français | [fr/README.md](fr/README.md) |
| Español | [es/README.md](es/README.md) |
| Русский | [ru/README.md](ru/README.md) |

## 目录结构 · Navigation

| 章节 Section | 中文 | English |
| --- | --- | --- |
| Get Started | [快速开始](zh/get-started.md) | [Get started](en/get-started.md) |
| Get Started | [架构与核心概念](zh/architecture.md) | [Architecture and concepts](en/architecture.md) |
| Get Started | [配置来源与启动参数](zh/configuration.md) | [Configuration sources and CLI](en/configuration.md) |
| Guides | [felucca-server 模块](zh/server.md) | [The felucca-server module](en/server.md) |
| Guides | [felucca-agent 模块](zh/agent.md) | [The felucca-agent module](en/agent.md) |
| Guides | [隧道与端口映射](zh/tunnels.md) | [Tunnels and port mappings](en/tunnels.md) |
| Guides | [后端、负载均衡与健康检查](zh/backends.md) | [Backends, balancing and health checks](en/backends.md) |
| Guides | [HTTP/HTTPS 反向代理](zh/http-services.md) | [HTTP/HTTPS reverse proxy](en/http-services.md) |
| Guides | [证书、SNI 与 ACME](zh/certificates.md) | [Certificates, SNI and ACME](en/certificates.md) |
| Guides | [正向代理](zh/forward-proxy.md) | [Forward proxy](en/forward-proxy.md) |
| Guides | [文件服务与 WebDAV](zh/file-services.md) | [File services and WebDAV](en/file-services.md) |
| Guides | [服务端本地目标与嵌套路由](zh/local-services.md) | [Server-local targets and nested routes](en/local-services.md) |
| Guides | [私有共享与访客](zh/private-shares.md) | [Private shares and visitors](en/private-shares.md) |
| Guides | [P2P 直连与中继回退](zh/p2p.md) | [P2P direct and relay fallback](en/p2p.md) |
| Guides | [访问控制、配额与数据流编码](zh/security.md) | [Access control, quotas and stream coding](en/security.md) |
| Guides | [管理界面与集中配置](zh/management.md) | [Admin UI and centralized configuration](en/management.md) |
| Guides | [外部平台接入](zh/platform.md) | [External platform integration](en/platform.md) |
| Guides | [系统服务、更新、日志与监控](zh/operations.md) | [Services, updates, logging and monitoring](en/operations.md) |
| Reference | [参考:命令、字段、接口、示例与术语](zh/reference.md) | [Reference: CLI, fields, APIs, examples, glossary](en/reference.md) |

## 阅读约定 · Conventions

- 命令都在项目根目录执行 / Commands are run from the repository root.
- `felucca-server` 和 `felucca-agent` 是同一个发布包里的两个独立程序 /
  `felucca-server` and `felucca-agent` are two separate programs from one release.
- 配置文件中的相对路径以进程工作目录为基准 / Relative paths in configuration files
  resolve against the process working directory.
- 带 `--check` 的命令只校验配置,不绑定任何端口 / Commands with `--check` validate
  configuration only and bind no ports.
