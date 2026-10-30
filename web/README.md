# ark web · Code 模式渲染层

`arkd` 的 Code 模式界面（设计 §7.10）。渲染层只通过本地 HTTP + SSE 与运行时通信，
不做事件语义映射——那部分已经在 Rust 侧完成（设计 §7.10.3）。

## 快速开始

```powershell
cd ark-rs\web
npm install
npm run dev      # http://localhost:5273
```

开发服务器把 `/v1` 与 `/health` 代理到 `http://127.0.0.1:18790`（`arkd` 默认监听地址）。
渲染层默认同源请求，因此生产环境由外壳直接托管页面时无需额外配置；
需要指向别的地址时设置 `VITE_ARK_BASE_URL`。

鉴权：token 从 `location.search` 的 `ark_token` 读取并写入 `localStorage`，
其次读 `localStorage.ark_token`。两者都缺失时不发送 `Authorization` 头，由运行时决定是否拒绝。

## 脚本

| 命令 | 说明 |
|---|---|
| `npm run dev` | Vite dev server（5273） |
| `npm run build` | `tsc --noEmit` + `vite build` |
| `npm run typecheck` | 只做类型检查 |
| `npm run check:rules` | 本地执行下面的硬约束检查 |

## 目录结构（设计 §7.10.2）

```
src/
  features/
    code-session/
      components/   会话流、消息气泡、工具卡片、审批卡片、用户输入、Composer、状态条
      hooks/        use-thread-stream（SSE）、use-thread-list、use-turn、use-runtime-status
      state/        zustand slices：threads / items / runtime / workbench + 纯 reducer + selectors
      api/          只有 HTTP + SSE 客户端，无业务逻辑
    workbench/
      layout/       三栏布局 + 面板 resizer
      panels/       Changes / Plan / File / Terminal
  shared/
    contracts/      wire 类型（grep 可见文件头：待由 ark-contract 生成）
    ui/             Button / Tabs / Toast / EmptyState / ErrorBoundary
    api/            client.ts：唯一的 fetch 出口（get/post/patch/del/stream）
  styles/           设计 token 与基础样式
```

## 运行时契约

| 用途 | 接口 |
|---|---|
| 存活性 / 版本 | `GET /health`、`GET /v1/runtime/info` |
| 线程列表 / 新建 | `GET /v1/threads`、`POST /v1/threads` |
| 线程详情（权威快照） | `GET /v1/threads/{id}` |
| 事件流 | `GET /v1/threads/{id}/events?since_seq=N` |
| 发起回合 | `POST /v1/threads/{id}/turns` |
| 中断回合 | `POST /v1/threads/{id}/turns/{turnId}/interrupt` |
| 审批 / 用户输入 | `POST /v1/approvals/{id}`、`POST /v1/user-inputs/{id}` |

事件流细节：

- 用手写 `fetch` + `ReadableStream` 解析，不用 `EventSource`（需要带 `Authorization` 头）。
- 解析 `id:`（= seq）、`event:`（= kind）、`data:`（JSON）。
- 断线后按指数退避（上限 10s，带抖动）自动重连，并从最后收到的 `seq` 续订。
- `heartbeat` 只推进续订水位，不进入 UI；`sse_idle_timeout` 触发主动重连。
- 75s 内没有收到任何字节（含 `: keep-alive` 注释帧）才本地判定卡死并重连；注释帧不算事件、
  不推进水位。

## 状态约定

- `ThreadItem` 已经是 render-ready 结构，前端只按 `turn_id` 分组、保持服务端顺序，
  不重新推断语义、不改写文本。
- 例外是"少量控制事件"（设计 §7.10.3）：`approval_requested` / `user_input_requested`
  携带用户必须回填的 id，因此由 reducer 记录为待办卡。
- `seq` 的唯一权威是运行时的事件 writer，前端只做续订水位记录。

## CI 硬约束

以下规则进 CI（`npm run check:rules` 是本地等价实现）：

1. **单文件 ≤ 400 行**（对应 eslint `max-lines`）。
2. **`features/*/api` 与 `shared/api` 之外禁止裸 `fetch(`**。所有运行时访问经
   `shared/api/client.ts`，以统一鉴权、错误解析与降级。
3. **运行时路径（`/v1/...`、`/health`）只能出现在 `api/` 层**，页面与组件不得直连。
4. **不得自带主题/弹窗/Toast 实现**，只能用 `shared/ui`
   （等价于禁止 `new EventSource`、`createPortal` 等绕过基础组件的写法）。
5. **`src/shared/contracts` 是生成目录**：文件头必须是
   `待由 ark-contract 生成，CI 校验 diff 为空`，且 CI 校验 `git diff` 为空。

## 当前界面能力

已跑通：线程列表与新建、会话流（用户/助手流式增量/工具卡折叠/错误行）、
Composer（Enter 发送、Shift+Enter 换行、发送中禁用）、中断按钮、审批与用户输入门禁、
运行时状态条与 SSE 状态（含 seq 水位）、右侧四面板 Tab 切换与空态、三栏可拖拽。

未完成（面板只有空态，接口未接入）：Changes 汇总、Plan（todos/goal）、
File 预览、Terminal（bash 会话输出）。这三类数据在运行时侧尚未给出对应读接口。
