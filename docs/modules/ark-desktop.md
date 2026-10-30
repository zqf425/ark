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
  `cargo run -p arkd -- serve --port 18791 --data-dir <app-data>/runtime --workspace <repo-root>`。
- 发布：打包前先编译 `arkd`，按当前 Rust target triple 复制为 Tauri `externalBin`；sidecar 准备脚本从 Cargo 实际使用的 `CARGO_TARGET_DIR` 读取产物，未设置时回退仓库 `target`。仅 `tauri.release.conf.json`
  声明这个发布产物，开发和普通 `cargo check` 不依赖它。外壳从 sidecar 启动它。
  发布构建将 `VITE_ARK_BASE_URL` 固定为受限的 loopback 地址，不能把生产 API 请求误发给 WebView 自身。
- 复用：如果 `127.0.0.1:18791` 已有监听，外壳只在 `/health` 响应确认它是 arkd 时复用；未知服务会显示
  端口冲突，绝不终止或连接它。由外壳启动的 sidecar 由其显式持有，并在退出时通过进程组/Job Object 回收。

桌面端口和数据目录均与 CLI/纯 Web 默认值隔离：CLI 和直接运行 `web/npm run dev` 仍使用 18790 与默认数据
目录，桌面使用 18791 和应用数据目录。这样旧 smart-ark 或独立 arkd 不会被误连，两个进程也不会并发写同一
会话目录。

## 安全边界

打包后的 WebView 与 loopback `arkd` 不同源。`ark-server` 仅为 Tauri 的确定 origin
`http://tauri.localhost`（Windows）和 `tauri://localhost`（macOS/Linux）发送 CORS 响应，绝不使用 `*`；Vite
开发仍经同源 proxy。Tauri CSP 也只允许连接 `http://127.0.0.1:18791`。

外壳不向 JavaScript 暴露 shell 命令。它只从 Rust 调用 Tauri 的 shell plugin 启动固定的 `arkd` 命令，因此
Web UI 不能借桌面能力执行任意程序。

## 验证

- `node --test apps/ark-desktop/scripts/desktop-layout.test.mjs` 验证根 `npm run dev`、Tauri 与 Vite 编排配置。
- `npm --prefix apps/ark-desktop run build:web` 验证生产 Web 构建把 API origin 指向桌面专属 loopback。
- `cargo check -p ark-desktop`、`npm run dev` 验证 Rust 宿主与真实窗口/sidecar；Windows GNU 工具链必须提供
  `dlltool.exe`，否则这是环境失败，不得将其表述为代码通过。
