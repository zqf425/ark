# ark-desktop 桌面外壳设计

## 职责与边界

`apps/ark-desktop/src-tauri` 是 ark 的本地桌面宿主。它只创建原生窗口、启动和回收本地 `arkd`，并将已构建的
`web/` 作为 WebView 渲染内容。它不依赖任何 `ark-*` crate，不能承载 Agent 决策、provider、插件、MCP、存储或
HTTP 业务规则。

`arkd` 仍是唯一组合根，`web` 仍是唯一 Code 渲染层；两者继续以 HTTP/SSE 通信。Tauri 不是 ark 插件容器，
不得将 Tauri plugin 的生命周期、权限或 API 混入 `ark-plugin` 的能力总线。

Tauri 2.12 及其 shell plugin 的最低 Rust 版本为 1.90，因此 `ark-desktop` 单独声明该 MSRV；核心 workspace
其余 crate 维持既有 1.75 基线，不因桌面壳被整体抬升。

## 运行模式

- 开发：根目录 `npm run dev` 调用 `apps/ark-desktop` 的 `tauri dev`；Tauri 启动既有 Vite 服务（以
  `ARK_RUNTIME_PORT=18791` 重定向 proxy），并从仓库根启动
  `cargo run -p arkd -- serve --port 18791 --data-dir ~/.arkd/data --workspace ~/.arkd/workspace`。
  Windows 下 supervisor 用当前 Node 直接执行 `@tauri-apps/cli/tauri.js`；npm 生成的无扩展名 `#!/bin/sh`
  shim 无法由 `CreateProcess` 启动，直接用 `spawn` 会报 `ENOENT`。Unix 继续走 `.bin/tauri`，两种路径
  启动的都是同一个 `tauri dev` 进程树，退出回收语义不变。
- 发布：打包前先编译 `arkd`，按当前 Rust target triple 复制为 Tauri `externalBin`；sidecar 准备脚本从 Cargo 实际使用的 `CARGO_TARGET_DIR` 读取产物，未设置时回退仓库 `target`。仅 `tauri.release.conf.json`
  声明这个发布产物，开发和普通 `cargo check` 不依赖它。外壳从 sidecar 启动它。
  发布构建将 `VITE_ARK_BASE_URL` 固定为受限的 loopback 地址，不能把生产 API 请求误发给 WebView 自身。
- 复用：如果 `127.0.0.1:18791` 已有监听，外壳只在 `/health` 响应确认它是 arkd 时复用；未知服务会显示
  端口冲突，绝不终止或连接它。由外壳启动的 sidecar 由其显式持有，并在退出时通过进程组/Job Object 回收。

Windows 发布构建必须使用 GUI 子系统（`main.rs` 的 `windows_subsystem = "windows"`）：控制台子系统会额外显示
一个黑色控制台窗口，用户关闭该窗口会向挂在其上的进程发送关闭信号，导致桌面壳和 sidecar 一起退出。Debug
构建保留控制台供 `tauri dev` 查看日志。`arkd` 作为 CLI 仍是控制台程序，但桌面壳通过 Tauri shell plugin
启动它时已带 `CREATE_NO_WINDOW`，不会单独弹窗。

桌面仅在端口上与 CLI/纯 Web 默认值隔离：CLI 和直接运行 `web/npm run dev` 使用 18790，桌面使用 18791；
两种构建都把 `~/.arkd/data` 和 `~/.arkd/workspace` 显式传给 arkd，不再使用 Tauri `app_data_dir`、
Documents 或仓库根。已有 18791 监听必须通过健康检查才会复用，从而避免两个 arkd 并发写同一会话目录。

## 安全边界

打包后的 WebView 与 loopback `arkd` 不同源。`ark-server` 仅为 Tauri 的确定 origin
`http://tauri.localhost`（Windows）和 `tauri://localhost`（macOS/Linux）发送 CORS 响应，绝不使用 `*`；Vite
开发仍经同源 proxy。Tauri CSP 也只允许连接 `http://127.0.0.1:18791`。

外壳不向 JavaScript 暴露 shell 命令。它只从 Rust 调用 Tauri 的 shell plugin 启动固定的 `arkd` 命令，因此
Web UI 不能借桌面能力执行任意程序。唯一向渲染器暴露的进程能力是 `open_external`（见下）：它只接收一个
URL，校验 scheme 后交系统默认处理器，不执行程序、不接受 shell 参数。

### 外部链接与导航守卫

渲染层是单页应用（`useViewStore` 切 scene，没有 URL 路由），窗口标题固定、没有地址栏或后退按钮，
**主窗口是唯一能回到工作区的表面**。因此主窗口只允许加载应用自身文档：

- 打包态：`tauri://localhost`（macOS/Linux）与 `http(s)://tauri.localhost`（Windows）；
- 开发态：`build.devUrl` 声明的 origin（`app.config()` 读取，不在 Rust 里硬编码 Vite 端口）。

`create_main_window` 的 `on_navigation` 拒绝其余顶层导航，`on_new_window` 拒绝 `window.open`/`target=_blank`
（否则会得到第二个没有 capability、也回不到应用的 webview），两者都先经 `open_in_browser` 把目标交给系统
默认浏览器再返回拒绝。对外抛出的目标同样只放行 http(s)：`javascript:`/`file:`/`data:` 既不加载也不外抛。
拒绝是终态，窗口停在原页面，不会出现白屏或半加载。

`open_external(url)` 是渲染器侧唯一入口：助手正文里的链接经 `shared/desktop.ts#openExternal` 调用它。
scheme 策略的宿主在 Rust（渲染层不可信：`<a href>` 来自模型正文），`javascript:` 等一律返回错误而不是
交给 OS 处理器。纯浏览器运行没有这个命令，`openExternal` 回落到新标签页；该回落只存在于无壳环境。

### 原生文件夹选择

桌面壳的 `pick_folder` 由 Tauri 命令注入调用窗口（`tauri::Window`），并在创建 `FileDialogBuilder` 时调用
`set_parent(&window)`。Windows 上未绑定 owner 的模态对话框会被当作独立顶层窗口，可能不随主窗口置前、
只显示在任务栏，直到用户手动点击；显式 parent 使对话框直接归属当前窗口。web 通过
`shared/desktop.ts#pickFolder()` 调用该命令，无需传参。

### 窗口拖放

`create_main_window` 显式调用 `WebviewWindowBuilder::disable_drag_drop_handler()`。Tauri 默认在原生层
接管 WebView2 的拖放处理以产生 `DragDropEvent`，Windows 上这会替换 WebView2 的 HTML5 拖放处理器，导致
页面（composer 的文件拖放附件）收不到任何 drop 事件。壳内没有任何 `DragDropEvent` 消费者，禁用后拖放
完全交还页面，桌面与纯 Web 行为一致。

### 系统通知

外壳注册 `tauri-plugin-notification` 并暴露唯一的 `notify_session_event(title, body)` 命令：渲染器
传入**纯展示文本**（标题 + 正文），外壳经插件投递 Windows toast / macOS 横幅；点击通知不做任何路由，
因此不建模任何 IPC 载荷或动作。投递失败（系统关闭通知、无通知后端）返回 `false` 而不是报错——丢失
一条通知绝不能破坏触发它的回合终态流程。是否发送（窗口失焦才发）、去重和文案全部由渲染层
（`turn-end-toast.ts`）决定；外壳不知道"会话"概念，只渲染给定文本。

## 图标资产

`src-tauri/icons/` 下的全部图标由 `npm run icons`（即 `tauri icon src-tauri/icons/ark.svg`）从唯一源
`ark.svg` 生成，生成物整体入库、不走 ignore：它们是构建的必需输入，`tauri-codegen` 在非 Windows
target 会按 `bundle.icon` 找第一个 `.png` 条目（找不到就回退硬编码 `icons/icon.png` 并直接 panic）。
禁止手工编辑或单独替换某个生成图标；换图标时改 `ark.svg` 后重新执行 `npm run icons`。

## 验证

- `node --test apps/ark-desktop/scripts/desktop-layout.test.mjs` 验证根 `npm run dev`、Tauri 与 Vite 编排配置
  （含系统通知命令的桥接断言）。
- `cargo test -p ark-desktop` 覆盖健康探针、就绪等待与导航守卫的 origin 判定
  （`navigation_guard_accepts_only_the_app_origins`、`release_window_refuses_the_dev_origin_when_no_dev_url_is_configured`）。
- `npm --prefix apps/ark-desktop run build:web` 验证生产 Web 构建把 API origin 指向桌面专属 loopback。
- `cargo check -p ark-desktop`、`npm run dev` 验证 Rust 宿主与真实窗口/sidecar；Windows GNU 工具链必须提供
  `dlltool.exe`，否则这是环境失败，不得将其表述为代码通过。
