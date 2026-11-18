# 外链交给系统浏览器（桌面窗口导航守卫）

> 状态：**已实现**（2026-11-18）。助手正文里的链接与 `window.open` 不再在 ark 窗口内加载，改由系统默认
> 浏览器打开；桌面壳在主窗口拒绝一切非应用 origin 的顶层导航。单测与编译已过，真实桌面点击未在本会话
> 手工验证（需人工在窗口内点一次外链）。

## 背景

实测：在 ark 桌面窗口里点击助手正文给出的 Google 登录链接，整个 WebView2 原地导航到
`accounts.google.com`。窗口标题仍是 `ark`，但没有地址栏、没有后退按钮；渲染层又是单页应用
（`App.tsx` 用 `useViewStore` 切 scene，没有 URL 路由）。于是导航一旦发生，**应用内没有任何返回入口**，
只能靠 WebView2 的 `Alt+←` 或重启进程，而用户未必知道这两个动作。

根因两处：

1. `renderMarkdown` 产出的 `<a href>` 被 DOMPurify 原样放行（模型正文是不可信输入，但链接本身是合法
   HTML），而 `Markdown` 的委托点击只处理文件芯片与复制按钮，对链接既不 `preventDefault` 也不加
   `target`；
2. `create_main_window` 只设了标题、尺寸与拖放策略，没有 `on_navigation`/`on_new_window` 守卫，Tauri
   不拦顶层导航，也不拦 `window.open`/`target=_blank`。

## 目标与范围

- 桌面窗口只加载应用自身文档：打包态 `tauri://localhost`（macOS/Linux）与
  `http(s)://tauri.localhost`（Windows），开发态 `build.devUrl` 的 origin。
- 其它 http(s) 目标交系统默认浏览器；`javascript:`/`data:`/`file:` 既不导航也不外抛。
- `target=_blank`/`window.open` 不产生第二个 ark webview。
- 纯 Web（浏览器打开 5273）行为不变：外链仍是新标签页。

不包含：内置浏览器面板、OAuth 回调处理、窗口内前进/后退 UI。

## 设计

### 1. 渲染层（唯一入口 `shared/desktop.ts#openExternal`）

- `Markdown` 的委托点击命中 `a[href]` 时 `preventDefault` 并调用 `openExternal(href)`；中键 `auxclick`
  同样转交（否则浏览器语义会开新窗口）。`#fragment` 不拦截：它不替换文档。
- `openExternal` 用 `new URL(href, location.href)` 解析（正文链接可能是相对引用），只放行 `http(s)`；
  桌面壳存在时 invoke `open_external`，纯浏览器用 `window.open(url, '_blank', 'noopener,noreferrer')`。
- 桥接失败静默丢弃：**绝不回落到窗口内导航**，那正是本需求要消除的陷阱。

### 2. 桌面壳（策略宿主在 Rust）

- `open_external(url)`：`tauri::Url::parse` 后重新校验 scheme，只允许 http/https，再交 `shell().open`。
  scheme 策略只在这里执行——渲染层与模型正文都不可信，而 Rust 是唯一能启动进程的一侧。
- `create_main_window` 增加 `.on_navigation(...)`：`is_internal_url` 判定的应用 origin 放行，其余先
  `open_in_browser` 再拒绝。
- `.on_new_window(...)` 返回 `Deny` 并同样转交系统浏览器，避免出现没有 capability 的第二 webview。
- 放行集合由配置推导，不硬编码端口：开发 origin 取 `app.config().build.dev_url`（发布态为 `None`），
  打包 origin 固定三条。改 Vite 端口不需要改 Rust。

## 验收

- `web/src/shared/ui/markdown-link.test.tsx`：外链点击 `defaultPrevented` 且携带原 href 调用
  `openExternal`；`#fragment` 不拦截；中键同样转交。
- `runtime.rs` 单测：`is_internal_url` 接受 dev origin、`tauri://localhost`、`tauri.localhost`；拒绝
  Google、`127.0.0.1:18791` API、异端口、`javascript:`、`file:`；无 `devUrl` 时拒绝 dev origin。
- web：vitest 全绿、`tsc`、`check:rules`、`build`；Rust：`cargo check -p ark-desktop`。
- 真实桌面窗口（手工）：点击正文外链 → 系统浏览器打开，ark 停在原会话；`Alt+←` 不再是唯一退路。
  本会话未执行（沙箱内无法点击真实窗口）。
- `docs/modules/ark-desktop.md`、`docs/modules/web.md` 与本需求同步。
