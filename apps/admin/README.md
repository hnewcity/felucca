# Felucca Admin

React + Vite 管理应用。源代码按 `app`（会话与导航）、`pages`、`components`、
`features`（表单配置映射、监控采样状态）、`lib` 和 `styles` 分层；Rust 的 `admin/` 只负责 HTTP 适配。

## 开发

使用 Node.js 22.12+（推荐 24），在仓库根目录执行：

```sh
npm --prefix apps/admin ci
FELUCCA_ADMIN_TARGET=http://127.0.0.1:18080 npm --prefix apps/admin run dev
```

先启动带有管理账号的 Felucca server，并将 `FELUCCA_ADMIN_TARGET` 指向其本机 HTTP
管理监听。Vite 监听 `127.0.0.1:5173`，代理 API 和 SSE；代理会将 Origin 与 Host 一起适配到
后端，以保留服务端校验。开发服务器仅用于本机，不应对公网开放。HTTPS/Secure Cookie
使用生产构建后的服务端验证。`vite preview` 只用于静态预览，不代理登录 API。

## 构建与验证

```sh
npm --prefix apps/admin run lint
npm --prefix apps/admin test
npm --prefix apps/admin run check:schema
npm --prefix apps/admin run build
sh scripts/cargo-external.sh build --locked -p felucca-cli --bins
FELUCCA_PROBE_PROFILE=debug FELUCCA_PROBE_OUTPUT=target/tmp/react-admin \
  node scripts/probes/probe-admin.mjs
```

`dist/` 与 `node_modules/` 不入库。Vite 输出带内容摘要的 ES modules、CSS 和字体，
随后 `build/stamp.mjs` 写入源码摘要。Rust `build.rs` 拒绝缺失或过期的构建并自动嵌入资源，
因此新增组件无需手工更新资源清单。不要在 Cargo 编译期间修改前端或运行前端构建。
CI 和 Docker 已包含前端构建步骤。
浏览器探针支持 `FELUCCA_PROBE_VITE=1`，用相同操作验证开发代理与 React StrictMode。
`python3 scripts/probes/probe-admin-build.py` 验证源码副本中的过期／缺失产物拒绝行为。

## 设计

采用用户提供的 Revolut 设计系统：近黑 `#191c1f`、白色与 `#f4f4f4` 平面层级，
20px 卡片圆角、胶囊按钮、500 字重标题，无投影。语义色用于运行状态。
桌面侧栏在平板与手机上转为抽屉，数据表只在自身容器内横向滚动。
桌面侧栏还可收起成图标栏：顶栏左侧的按钮切换，选择记在本机存储，平板与手机上仍是抽屉。

页面区块由 `page-content` 统一提供 24px 间距，表单分组由 `form-stack`／`flow-panel`
提供 20px 间距；卡片本身不对相邻卡片添加外边距，以免影响网格对齐。
设置子页在顶栏用“设置 › 当前页面”提供返回入口，并保留侧栏“设置”的选中状态。

配置模型与表单呈现分开维护：`features/config-presentation.js` 负责用途分组、条件显示、
字段说明与上下文相关的枚举标签，不修改序列化值。已知配置字段和选项都必须有七语言文案；
覆盖检查在 `config-presentation.test.js`。常用字段先出现，内部编号及高级策略逐步展开，
未启用功能不要求填写附属参数。简单列表使用逐行输入，复杂条目单独展开。
标签和填写示例保持可见，必填数字的空草稿不能被替换成 0 或旧值；错误显示在字段旁，
提交时会打开错误所在的折叠区域。弹窗正文独立滚动，标题、关闭及底部操作保持可用。

启用／关闭等二元设置统一使用 `Switch`：开启为近黑轨道配白色滑块，关闭为灰色轨道，
同时显示状态文字；启用状态标签也使用黑白色，不使用绿色。列表勾选仍表达选择条目。
开关使用原生输入和 switch 语义，支持空格键、可见焦点、禁用状态及减少动态效果。
配置编辑保持草稿与保存机制，编辑弹窗底部持续说明保存后应用。
展开区域统一使用 `Disclosure`，整行标题可点击，收起时显示配置摘要，最多两层折叠；
更深层参数直接分组展示。收起不卸载输入，出错时展开并把首个错误定位到操作栏上方。

表单浏览器验收使用 `python3 scripts/probes/probe-form-design.py`，可选依赖的安装方式见
仓库 `CONTRIBUTING.md`。先完成前端与 Rust 构建；探针使用本机 Chrome、隔离的真实服务端，
验证保存回读、未改字段和凭据保留、列表排序、输入校验、十个服务器配置分区、资源编辑器及
手机布局，结果写入 `target/tmp/form-design/`。`FELUCCA_PROBE_VITE=1` 可检查开发页面；
开发页面通过不能代替内嵌构建验收。设计依据和范围见 `docs/architecture.md`。

设置采用渐进展开：`SettingGuide` 先展示用途、需要调整的场景、当前配置和改动影响，
`SettingDetails` 收起少用的参数。任务入口优先使用接入／私有访问向导。
设备授权、本地访问入口、设备直连和连接验证时间分别说明作用范围；设备运行页明确编辑的是
待导出的新草稿，不能将其称为设备的实时配置。服务器字段按用户用途选择，NTP 使用明确的
“系统时间／指定时间服务器”选项。上下文相关字段说明与实际默认值集中在
`features/settings-model.js`，文案放入七种语言目录，最终语义校验仍由服务端负责。

界面按运维任务组织：标题使用页面名称，说明交代资源范围、状态含义和操作影响。
不使用宣传口号、英文装饰标题、重复品牌页脚或没有动作的装饰箭头。
配置修订号只出现在配置及变更记录中；正常的配置通知连接不常驻展示，
断开时说明更新提示暂不可用并自动重连。顶栏显示页面数据的读取时间及刷新入口，
主机监控的自动采样与手动刷新独立标明，不将通知通道连通等同于资源正常。

显示字体优先使用本机已有的 Aeonik Pro；未提供其授权文件，因此不分发该字体。
Inter 的可变字体由 `@fontsource-variable/inter` 提供并随应用打包，中文使用系统字体。
字体许可证随应用分发在 `public/inter-license.txt`。无外部字体请求。

## 接口与状态

沿用 `/api/*`、HttpOnly Cookie、CSRF、配置版本及幂等键。
不把凭据存入浏览器持久存储。编辑器绑定打开时的配置版本；SSE 只显示更新提示，
不会覆盖输入。网络结果不确定时保留幂等键，401 清理会话并丢弃旧会话的迟到响应。
Hash 导航支持刷新和前进后退，服务端继续按精确资源路径返回静态文件。

节点、用户、共享访问、管理账号、平台与回调使用结构化表单；
复杂隧道参数在「更多设置」中使用结构化表单，不要求编辑 JSON。本机服务、完整配置和
agent 配置使用同一字段编辑器。`features/config-schema.json` 由
`python3 scripts/generate-admin-schema.py` 根据 Rust 配置模型生成，`check:schema` 检查字段漂移；
最终语义校验仍在 Rust。修改地址时不会重建或丢弃后端权重、TLS 和信任设置。
概览数值全部来自实际 API 快照，不生成示例节点或虚构历史曲线。

“运行诊断”读取逐隧道计量、已连接 agent 的后端采样以及 P2P 事件；管理员另可查看
证书维护和日志。“Agent 与部署”生成带指纹的 agent／visitor 配置及主机命令。
管理账号支持 token 生成和撤销。完整配置校验区分在线切换和重启字段，重启包分别导出
启动配置与运行快照；导出和证书上传不自动激活配置。

监控页通过 `/api/monitor` 读取服务端每 5 秒采集的主机历史，仅管理员可访问。
`MetricChart` 使用 Apache ECharts 6.1.0 的按需模块与 Canvas 渲染，在严格 CSP 下提供
原生浮动提示、四图同步选点/缩放、点击与触摸锁定、可切换网络图例及可见时段统计。
底部范围条支持拖拽缩放，放大/重置按钮与方向键支持键盘操作；缺失值和采样间断不补零。`use-host-metrics` 负责轮询、
取消旧请求、页面重新可见时更新和失败重试，更新时保留已选中的点及绝对时间缩放范围。
历史只保存在服务端内存，最长 1 小时，进程重启后重新积累。

图表通过 React.lazy 单独加载；只有进入监控页才下载图表分包，不把 ECharts 放进登录页的首屏依赖。
本次图表分包约 568 kB / gzip 191 kB，Vite 的 500 kB 提示仍保留，未通过调高阈值隐藏。
主题取自现有设计令牌，配合蓝/青色曲线、轻面积填充、无阴影卡片与暗色提示框。
提示使用 ECharts richText 渲染器，不引入 HTML 提示或放宽 CSP。
ECharts 与 zrender 的许可证及 NOTICE 随静态资源分发。

## 设备、服务与访问

主导航收敛为六个入口。`ConnectDevice` 创建凭据与空集中配置的原子事务，下载固定证书指纹的客户端连接包，并轮询实际在线状态；`ServiceWizard` 将任务模板映射到原有配置，保留未修改的字段；`Sharing` 创建服务访问端及授权并生成本地监听配置。它们共用 `features/product-flow.js`。常规流程不显示内部编号、密钥输入框和配置源码。

`/api/launch` 额外返回服务端直连和 HTTP/HTTPS 监听元数据，帮助向导选择实际可用的连接模式及端口。网站域名入口必要时与规则一起原子创建；已有本地发布的服务不会被新增服务操作隐式接管。高级设置保留权限和资源边界。

构建最新二进制后运行 `node scripts/probes/probe-product-flow.mjs`，验证实际生成文件、转发、私有 TCP/UDP、权限及重启恢复。加 `--interactive` 可启动空白浏览器验收环境，登录详情只保存在任务输出目录中。

高级客户端能力保留在设置中的 `agent` 页面；默认设备接入与服务访问不依赖该页面。连接包使用独立文件名，复制命令引用同一文件，避免多台设备下载文件时混用旧配置。

`/api/agent-packages` 只列出服务器上实际存在的匹配版本。客户端程序通过会话认证下载，
`connection-package.js` 原样封装程序、连接信息、启动入口和可选用户级自启动入口。
没有当前设备系统对应的包时不自动选择其他系统；未公证的 macOS 包在准备和下载阶段提示。
签名、公证及首次启动验收必须独立完成，脚本生成或普通解压测试不能替代系统检查。

普通设备、服务、授权变更在线同步。列表区分等待同步、客户端拒绝、运行中与停用；
名称修改及无关服务修改保持现有连接，基础监听修改仍明确提示重新加载。
