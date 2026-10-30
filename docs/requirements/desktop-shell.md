# 本地桌面应用需求设计

> 状态：**已实现，Windows 交付验证部分完成**。Windows GNU 宿主链接、WebView2 真实窗口、既有 arkd 安全复用和 NSIS 安装包构建已验证；宿主在空闲 18791 上自行冷启动及退出收敛仍待干净环境复验。剩余步骤与证据要求见 [发布收尾与真实环境验收](release-readiness.md)。

## 目标

让根目录的 `npm run dev` 像旧 smart-ark 一样直接启动原生桌面应用，同时保留 Rust 运行时的独立性：
`arkd` 必须仍可被 CLI、测试和未来远程部署单独使用。

## 范围

- 新增 `apps/ark-desktop` Tauri v2 宿主和根 npm 入口。
- 开发期启动 Vite 和本地 `arkd`；发布期将 `arkd` 打包为 sidecar。
- 为打包 WebView 到受限 loopback API 的通信增加精确 CORS 和 CSP。
- 保持 Web 的 typed HTTP/SSE client、`arkd` 组合根和所有 Agent/plugin 边界不变。

不包含托盘、自动更新、多窗口、任意 shell、运行时重启器、远程运行时、Electron 兼容层，或对 `ark-agent`、
`ark-plugin`、Provider 的任何 UI 耦合。

## 设计决策

1. **使用 Tauri 而非恢复 Electron 管理器。** Web UI 沿用 React；桌面生命周期为独立 Rust 外壳，避免把旧
   Electron IPC/运行时分支带回核心。
2. **`arkd` 保持进程隔离。** 外壳不链接或调用 ark crate，只使用固定参数启动本地进程；这样崩溃隔离、CLI
   可用性和组合根唯一性都不变。
3. **桌面专属 loopback 与数据目录。** CLI/直接 Web 开发继续用 18790；桌面使用 18791，并传入应用数据目录。
   已有监听只有 `/health` 确认是 arkd 才复用；未知服务只报冲突、不杀进程、不猜测身份。这既避免旧 smart-ark
   的端口碰撞，也避免两个 arkd 并发写同一会话目录。
4. **CORS 白名单而非通配符。** 仅允许 Tauri 自身的跨平台 origin，外部网页不能通过浏览器跨域读取本地 Agent
   API。应用仍仅绑定 loopback。

## 验收

- 在具备 Windows GNU linker 和 WebView2 的环境，`npm run dev` 打开 ark 原生窗口，并由它启动或复用专属的
  18791 `arkd`。
- 打包命令产出携带同 target triple `arkd` sidecar 的 NSIS 安装包。
- 生产 Web 请求指向 loopback，CORS 预检仅接受 Tauri origin、允许的 HTTP 方法和必要 headers。
- `ark-agent`、`ark-plugin`、Provider、MCP 和现有 HTTP/SSE wire 契约无桌面实现分支。
