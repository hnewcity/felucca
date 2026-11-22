# Felucca Desktop

macOS 与 Windows 的托盘客户端。它在进程内运行 `felucca-client`，不附带也不调用
`felucca-agent`；命令行客户端保持不变，两者读取同一种 `felucca://` 启动 URI
（解析位于 `felucca-core::config::launch`）。

- 粘贴控制台「接入设备」给出的一行命令或 `felucca://` 链接即可添加连接，最多 32 个。
- 控制台的「用桌面版打开」链接会唤起本程序，但只弹出确认，用户核对服务器与设备后才保存。
- 连接保存在系统配置目录的 `connections.json`（Unix 权限 0600），日志写入系统日志目录，
  不记录设备密钥。`FELUCCA_DESKTOP_DIR` 可改用其他目录。
- 关闭窗口后继续在托盘运行；「登录时启动」以 `--hidden` 启动到托盘。
- 界面语言跟随系统，可用 `FELUCCA_LANG` 指定。

## 开发

本目录是独立的 Cargo 工作区（Tauri 需要 Rust 1.90 和系统 WebView），不参与根工作区的构建。

```sh
npm --prefix apps/desktop ci
npm --prefix apps/desktop run lint
npm --prefix apps/desktop test
npm --prefix apps/desktop run tauri dev
npm --prefix apps/desktop run tauri build
```

Rust 检查在 `apps/desktop/src-tauri` 中运行 `fmt --check`、`clippy --locked --all-targets -- -D warnings`
和 `test --locked`。真实转发验证：先构建 `felucca-cli` 与桌面端 debug 二进制，再运行
`python3 scripts/probes/probe-desktop.py`。

CI 产出的 DMG、NSIS、MSI 未签名，Gatekeeper 与 SmartScreen 会提示风险；正式分发需要另行签名和公证。
