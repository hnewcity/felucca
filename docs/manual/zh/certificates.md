# 证书、SNI 与 ACME · Certificates, SNI and ACME

本页说明三类证书怎么配:桥接证书、网站静态证书、以及用 ACME 自动签发和续期。

This page covers the three kinds of certificates: the bridge certificate, static website
certificates, and certificates issued and renewed automatically by ACME.

## 桥接证书 · Bridge certificate

```toml
[bridge]
bind = "0.0.0.0:14028"
tls = { cert = "conf/bridge.pem", key = "conf/bridge.key" }
cert_reload_interval = "30s"
```

省略 `bridge.tls` 且启用运行配置存储时（包括 `--init` 或管理后台），server 会在运行快照旁保存 `<snapshot>.identity.json`。重启和备份时请保留该文件。未启用运行配置存储时身份仍是临时的。`--fingerprint` 读取显式 PEM；自动身份的指纹可在运行中的管理页面和启动日志查看。

PEM 按 `cert_reload_interval`(默认 30 秒)重载,同时更新 TLS/WSS 和 QUIC 新连接的证书,
**不打断旧连接**。证书或密钥更新不完整时保留旧值。使用 `pinned` 的 agent 需要在下一次
重连前更新指纹;使用正确 CA 验证的 agent 无需固定某张证书。

```sh
target/release/felucca-server --config conf/server.toml --fingerprint
```

agent 侧两种选择:

```toml
[trust]
mode = "pinned"
fingerprint = "<来自 --fingerprint>"

[trust]
mode = "webpki"        # CA 签发的桥接证书
```

## 网站证书 · Website certificates

```toml
[http.certificates."app.example.com"]
cert = "conf/app.pem"
key = "conf/app.key"

[http.certificates."*.example.com"]
cert = "conf/wildcard.pem"
key = "conf/wildcard.key"

[http.default_tls]
cert = "conf/default.pem"
key = "conf/default.key"
```

精确匹配优先于通配,通配优先于 `default_tls`;都没有则拒绝握手。
SNI 与 HTTP Host 冲突返回 421。静态证书默认每 30 秒重读,可用
`http.cert_reload_interval` 调整;证书与私钥匹配后才替换,失败保留旧证书。

HTTP/3 与 HTTP/HTTPS 共用同一套 SNI resolver,包括 ACME 续期和静态 PEM 热重载。

## ACME 自动签发 · ACME issuance

```toml
[http.acme]
email = "admin@example.com"
accept_tos = true
challenge = "http-01"          # http-01 | tls-alpn-01
cache_dir = "state/acme"
staging = false
domains = ["app.example.com"]
```

| 字段 | 说明 |
| --- | --- |
| `accept_tos` | 确认 CA 条款后才为 `true` |
| `challenge` | `http-01` 需要公网 TCP 80 到达 `http.bind`;`tls-alpn-01` 需要公网 TCP 443 直达 `http.tls_bind`,此时 `http.bind` 可关闭 |
| `staging` | 先用 staging 验证连通性;固定选择 Let's Encrypt staging |
| `directory` | 指定其他 CA,默认 Let's Encrypt production |
| `cache_dir` | 账户与证书缓存目录,**请保存并保护** |
| `domains` | 显式约束签发范围;为空时每分钟从已注册的精确域名派生,跳过通配符 |

账户保存在 `account.json`,每个域名的证书链与私钥在同一个 JSON 文件里,原子替换;
Unix 上文件 0600、新缓存目录 0700。启动时从缓存恢复证书。续期以证书实际有效期计算,
提前量不超过有效期的三分之一;每分钟检查,失败一小时后重试,旧证书在续期失败时继续使用。
成功签发后热替换,不打断现有 TLS 连接。

**ACME 只管理 HTTP/HTTPS 反代证书,不修改桥接证书或 agent 的固定指纹。**

### EAB 与 ZeroSSL · EAB and ZeroSSL

```toml
[http.acme.eab]
key_id = "issued-by-your-ca"
hmac_key = "base64url-key-issued-by-your-ca"
```

ZeroSSL 可用手动 EAB,或改用 API key 模式:

```toml
[http.acme]
directory = "https://acme.zerossl.com/v2/DV90"
staging = false
zerossl_api_key = "your-account-key"
```

API key 模式在首次注册账户时获取 EAB,随后通过 ACME 签发;**不能同时配置手动 `eab`**。
账户缓存存在时不会重新申请 EAB。API key 和响应中的秘密不写入错误信息。

## 部署注意事项 · Deployment notes

- 两种 ACME 挑战都**不支持通配证书**;需要通配域名时用外部签发的静态证书。
- 应用监听 8080/8443 时,需要外部把 80/443 映射到对应端口。
- 未成功签发前,该域名的 TLS 握手可能失败,HTTP-01 仍可提供验证。
- 前置 TLS 终止代理必须把挑战 TLS 原样转发,且只对包含 `acme-tls/1` 的 ALPN 返回挑战证书。
- 切换 CA 或 staging/production 时使用新的 `cache_dir`。
- 仓库用本地模拟 CA 驱动真实 CSR 和挑战流程;公网 CA 的真实签发仍需在你的域名环境验证。

## 下一步 · Next steps

- [HTTP/HTTPS 反向代理](http-services.md)
- [felucca-server 模块](server.md)
- [运维](operations.md)
