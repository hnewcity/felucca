# 透明 TCP

`mode = "transparent"`（别名 `tcptrans`）从服务端内核读取 TCP 原始目的地址，
将目的 IP/端口经桥接交给 agent，仍经过来源 ACL、目标 ACL、配额、限速和原始流量计量。
协议为 v9，两端必须同时更新。

```toml
[[tunnel]]
id = 31
mode = "transparent"
port = 15080
bind = "0.0.0.0"

[tunnel.target_policy]
allow = ["10.0.0.0/8"]
ports = [80, 443]
```

不配置 `backend`：目标来自内核。透明入口要求公开监听，不接受私有共享、代理账号
或入口 PROXY 头；也不能通过 `tunnel://` 等嵌套别名模拟原始套接字。
可配置向后端发送 PROXY v1/v2，其中目的地址使用恢复后的原目标。

## 平台

- Linux/Android：默认使用 IPv4 `SO_ORIGINAL_DST` / IPv6 `IP6T_SO_ORIGINAL_DST`。
  需要由部署方配置 NAT REDIRECT。
- Linux/Android TPROXY：设置 `[tunnel.options] transparent_tproxy = true`，
  监听器设置 `IP_TRANSPARENT` / `IPV6_TRANSPARENT`，使用保留的本地目的地址。
  需要相应权限、路由和防火墙规则。
- macOS/FreeBSD：预先打开 `/dev/pf`，通过 `DIOCNATLOOK` 查询 PF 状态，
  按出站和入站方向查找。需要 `/dev/pf` 权限和对应的 PF 重定向规则。
- 其他平台拒绝该入口。agent 可在不同操作系统上运行，因为地址查询发生在 server。

felucca 不安装或更改系统防火墙规则。选择不同于被代理服务的监听端口，并把
agent/server 自己的桥接和后端连接排除在拦截规则外，避免转发环路。
普通直连监听地址不会被当作透明目标；查询失败也不会回退成监听地址。

## 验证范围

原目标地址校验、PF 字节布局/网络序、真实 ioctl 错误传播和 agent 的目标 ACL/半关闭
有专项测试。PF 适配器仅在一个记录安全前提的函数中调用 unsafe ioctl，
其余数据处理使用固定、完全初始化的字节数组；Linux 使用 socket2 的接口。

`scripts/probes/probe-transparent-linux.py` 提供独立进程的 Linux REDIRECT 验收：
它先建立新网络命名空间，并检查与宿主命名空间不同，随后才设置回环和防火墙。
测试仅拦截临时客户端 UID，验证原目标、半关闭以及两层原始字节计量。
需要 Linux root、unshare、iproute2、iptables 和已经构建的 release 程序。

当前开发机为 macOS，没有实际运行这项 Linux 防火墙测试，也未更改本机 PF。
不要把适配器单测、脚本存在或本地编译通过当作已完成真实重定向验收。
