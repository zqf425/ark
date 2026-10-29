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
- `GET /v1/threads/{id}` 把 active `TurnRegistry` entry、interaction 窄事件投影和 live waiter id 三者取交集
  写入 `ThreadDetail.runtime`；历史 gate 或已移除 waiter 不得出现在可操作快照中。
- workbench 的 Plan、Changes、File、Terminal 投影均在 `service/workbench.rs`，handler 只读取权威事件并
  调用该纯函数；File 不直接访问磁盘，而是返回已持久化的 `read` 工具输出。
- provider model catalog 路由只转发 `ProviderService::models`，不读取、记录或回显 api key/header；具体
  限时、限体积和元数据规范化留在 `ark-llm` / `ark-provider`。
- loopback API 默认不对浏览器开放跨域访问。为 Tauri 桌面壳，仅精确允许 Windows 的
  `http://tauri.localhost` 与 macOS/Linux 的 `tauri://localhost` origin，且只允许 GET/POST/PATCH/DELETE
  和 `accept`、`authorization`、`content-type` headers；禁止将其放宽为 `*`。
- `POST /v1/threads/{id}/rewind` 仅在没有活动回合时接受 `turn_id` 或 `seq` 之一；目标 turn 从其
  `turn_started` 处开始物理删除。成功响应含新的 `last_seq`，重复请求同一已记录目标幂等返回；SSE 的
  `thread_rewound` 事件要求客户端重新读取线程详情。
- `POST /v1/subagents/config` 是子代理开关和并发上限的专用热更新入口，内部复用
  `ConfigPatch.subagents` 的校验、替换和持久化语义，并与 `/v1/runtime/config` 一样返回
  `ApplyResult`；不得路由到其他 split-config 占位 handler。

## 路由清单

当前路由以 `src/api/routes.rs` 为准。新增或修改路由必须同步更新 wire DTO、接口测试、渲染层 client
和对应需求设计；不在本文维护第二份易漂移的完整路由表。

附件路由为 `POST /v1/attachments`、`GET /v1/attachments/{id}`、
`GET /v1/attachments/{id}/content` 与 `GET /v1/attachments/diagnostics`。上传采用 JSON Base64；
handler 在解码前按存储端口的最大字节数限制编码长度，超限或非法 Base64 返回结构化 400。内容下载
设置持久化 MIME（无效或缺失时为 `application/octet-stream`）和精确 `content-length`；非 UUID、
缺元数据或缺字节文件统一为 404。

工作区路由只经 `WorkspaceService` 读取 active view、执行 switch 和取得 diff。切换时将 parent
`TurnRegistry` 与 active child 数相加；非零即返回 `Conflict`/409，不能隐式取消。diff 响应带
generation，客户端必须丢弃 generation 已变的旧响应。

`/v1/memory` 的 GET、POST、PATCH、DELETE 及 `/v1/memory/diagnostics` 仅解包 HTTP 并调用
`MemoryService`。服务未装配时返回 `runtime_unavailable`/503；输入和不存在 id 的错误保持端口的
结构化错误码，handler 不复制持久化或校验规则。
