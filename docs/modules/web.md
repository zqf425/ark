# web 渲染层设计

## 职责与边界

`web/` 是 React + TypeScript Code 模式渲染层。它使用 `ark-contract` 生成类型和 typed client 渲染线程、
回合、工具、审批与设置；业务语义、路径归一化、脱敏、截断和事件到 item 的映射不应在前端重复实现。

## 约束

- 不手写 Rust 已拥有的 DTO、错误码 union 或类型别名；未建模的请求组合和纯 UI 状态仅放在
  `src/shared/contracts/web-only.ts`。
- 运行时请求集中在 feature API/client 层；组件不直接裸 `fetch('/v1/...')`。
- SSE 消费保存最后 `seq`，断线后使用 `since_seq` 恢复；页面必须能处理压缩产生的显式空洞。
- `child_completed` 按稳定 `notification_id` 去重后显示持久摘要卡片；实时到达时显示一次性提示。摘要仅作
  父线程状态展示，不能在 web 侧重建或改写模型上下文。
- `child_completions` 详情与 reducer 内存均限制为最新 50 条；完成、失败和中止状态必须本地化为用户文案，
  不直接显示内部枚举值。
- Code 右侧的 Plan、Changes、File、Terminal 均调用服务端投影；File 仅预览本线程已成功 `read` 的工具
  输出，浏览器不得直接读取工作区。子代理控制请求始终带父线程 query，不能只凭 child id 操作。
- 设置页的 tuning、provider 与 MCP 都使用生成契约；API key、MCP env/header 值只写不回读，编辑时的空
  secret 字段表示保留，不表示删除。
- 新增/编辑 provider 可显式查询模型目录，在用户选择后才把远端明确声明的 context window 与识图能力写入
  待保存表单；目录未声明时显示 unknown，不按模型名称猜测，也不自动提交配置。
- 组件状态按 feature 拆分，避免重建旧 `Workbench.tsx` 巨型组件；UI 变更应进行实际浏览器验证。
- Code 使用“任务历史 → 宽会话画布”的默认两栏结构：会话搜索和最近/归档切换只过滤已加载的
  `ThreadSummary`，不新增搜索 API；Plan、Changes、File、Terminal 是按需展开的上下文抽屉，默认不得占据
  空会话的主画布。抽屉视觉收起时组件保持挂载，不能丢失服务端读模型。
- 会话标题、真实回合数和运行态可以作为紧凑上下文显示；运行时版本、SSE 状态、数据目录等诊断信息不能占据
  主任务层级，必须退为状态详情或提示信息。
- 设置页的分类导航是 `web` 内本地状态，只决定既有 section 的可见性；每个 section 仍独立持有既有 hook，
  不把 provider、MCP 或 runtime 配置逻辑回流到 Code 会话状态或 `ark-agent`。
- 用户可见文本通过 `web/src/shared/i18n/` 的消息键与中英文资源提供；新增 UI 不得硬编码文案。语言、字号和
  主题是 renderer-only 偏好，默认 `zh-CN`/中号/跟随系统，保存于当前浏览器的 localStorage 并即时更新根 `lang`
  属性及字号、主题 token；选择“跟随系统”时监听 `prefers-color-scheme` 的变化并即时重新解析浅/深色主题。
  它们不属于 Rust wire 契约或运行时配置，不能通过设置 API 写入。

## 桌面宿主边界

`apps/ark-desktop` 是已实现的 Tauri 窗口和本地 `arkd` 生命周期宿主。它不会成为 web 的业务 API：生产构建
仅通过 `VITE_ARK_BASE_URL` 将既有 typed HTTP/SSE client 指向受限 loopback 运行时；开发仍使用 Vite proxy。
不得在 React 组件中引入 Tauri command、shell/plugin API 或 provider/Agent 分支。

独立 `plugin-code-ui` 尚未成为当前 workspace 模块。其未来迁移必须先建立需求设计，不能借桌面外壳绕过
`ark-ui` 注册总线。
