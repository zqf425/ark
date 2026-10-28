# ark-server 模块设计

## 职责与边界

`ark-server` 是 HTTP/SSE transport adapter：路由、鉴权、中间件、请求解包、SSE 编码和优雅停机。
业务决策委派给注入的 agent、store、配置、provider、skill 与工作区服务；handler 不得直接写业务分支。

## SSE 与生命周期

- SSE 订阅调用 `SessionStore::subscribe`，取得同一临界区内的 backlog、水位和 live receiver。
- 连接有界；慢客户端断开后从最后 `seq` 重连，绝不阻塞线程 writer。
- `/v1/shutdown`、Ctrl-C 和任务 panic 走同一收敛流程：取消活动回合、等待有限时间、落终态并清理 registry。
- 路径、HTTP 方法、字段名和兼容别名均是对外契约；变更前必须检查 `ark-contract`、`web` 与集成方。
- `GET /v1/threads/{id}` 的 `child_completions` 是最新 50 个不同通知 ID 的有界视图；它从 store 的窄
  completion 查询投影，不能全量回放普通会话事件。
- `InteractiveGates` 只做 HTTP 决议与 reactor waiter 的一次性 rendezvous；resolved 事件仍由 reactor 在
  实际消费回复后写入，取消时 Drop 守卫必须移除 waiter。
- workbench 的 Plan、Changes、File、Terminal 投影均在 `service/workbench.rs`，handler 只读取权威事件并
  调用该纯函数；File 不直接访问磁盘，而是返回已持久化的 `read` 工具输出。
- provider model catalog 路由只转发 `ProviderService::models`，不读取、记录或回显 api key/header；具体
  限时、限体积和元数据规范化留在 `ark-llm` / `ark-provider`。
- loopback API 默认不对浏览器开放跨域访问。为 Tauri 桌面壳，仅精确允许 Windows 的
  `http://tauri.localhost` 与 macOS/Linux 的 `tauri://localhost` origin，且只允许 GET/POST/PATCH/DELETE
  和 `accept`、`authorization`、`content-type` headers；禁止将其放宽为 `*`。

## 路由清单

当前路由以 `src/api/routes.rs` 为准。新增或修改路由必须同步更新 wire DTO、接口测试、渲染层 client
和对应需求设计；不在本文维护第二份易漂移的完整路由表。
