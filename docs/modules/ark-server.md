# ark-server 模块设计

## 职责与边界

`ark-server` 是 HTTP/SSE transport adapter：路由、鉴权、中间件、请求解包、SSE 编码和优雅停机。
业务决策委派给注入的 agent、store、配置、provider、skill 与工作区服务；handler 不得直接写业务分支。

## SSE 与生命周期

- SSE 订阅调用 `SessionStore::subscribe`，取得同一临界区内的 backlog、水位和 live receiver。
- 首帧：`GET /v1/threads/{id}/events` 在 backlog 为空时立即发送一个 `heartbeat` 控制帧，`seq` 取该次订阅的
  watermark。它不新增 wire 类型，也不写入 store：既不能声称 writer 尚未发放的序号，也不能让重连的
  `since_seq` 跳过事件。backlog 非空时不发送该帧，因为首个回放事件已经产生字节，且该帧会与回放末条
  重复同一个 `seq`。因此正文是否产生首字节不再由 keep-alive 间隔决定。
- 连接有界；慢客户端断开后从最后 `seq` 重连，绝不阻塞线程 writer。
- `/v1/shutdown`、Ctrl-C 和任务 panic 走同一收敛流程：先阻止新的 completion wakeup，再取消待调度
  continuation 与活动回合，等待有限时间、落终态并清理 registry。
- 后台 child 的持久 `child_completed` 成功写入后会唤醒 per-thread continuation supervisor。
  `subagents.auto_continue` 默认关闭；仅当它与 `subagents.enabled` 同时开启时，同一父线程 250 ms 内的
  通知共享一个可取消 debounce，关闭任一开关会立即取消待调度 continuation。
  启动前在 `workspace_gate` 下复核 active turn、归档/删除、shutdown、配置和未确认 inbox。中断、删除、
  归档会取消并抑制迟到 wakeup；下一次显式用户回合或取消归档解除抑制，中断抑制还会从父线程持久终态
  跨重启恢复（启动修复合成的 abort 除外）。每批最多消费 8 条；仅当前批全部确认成功且仍有待消费项时
  调度下一批，确认失败不会自行循环。自动作业仍调用注入的普通 `TurnRunner`，不会绕过 reactor、gate
  或 sandbox，也不会写入伪用户消息。
- 路径、HTTP 方法、字段名和兼容别名均是对外契约；变更前必须检查 `ark-contract`、`web` 与集成方。
- `GET /v1/threads` 支持分页与筛选：`limit` 缺省 `THREAD_PAGE_DEFAULT_LIMIT`，clamp 到
  `[1, THREAD_PAGE_MAX_LIMIT]`；`offset` 按确定顺序（active 优先、`updated_at` 降序、
  `thread_id` 兜底）取页，空页即最后一页；`archived` 与 `q`（标题大小写不敏感子串）可与分页
  组合。不带 query 的调用保持旧行为返回全部线程（响应仍为裸 `ThreadSummary` 数组）。
- `GET /v1/threads/{id}` 的 `child_completions` 是最新 50 个不同通知 ID 的有界视图；它从 store 的窄
  completion 查询投影，不能全量回放普通会话事件。
- `InteractiveGates` 只做 HTTP 决议与 reactor gate future 的一次性 rendezvous；`prepare_*` 同步插入
  registry 后才返回 future，resolved 事件仍由 reactor 在实际消费回复后写入。future 自带 Drop 守卫，
  按（id, generation）移除自己的注册：取消/丢弃清理自身，且同 id 重新注册后旧守卫不会误删新注册。
- `GET /v1/threads/{id}` 把 active `TurnRegistry` entry、interaction 窄事件投影和 live gate id 三者取交集
  写入 `ThreadDetail.runtime`；历史 gate 或已移除 waiter 不得出现在可操作快照中。
- workbench 的 Plan、Changes、File、Terminal 投影均在 `service/workbench.rs`，handler 只读取权威事件并
  调用该纯函数；File 不直接访问磁盘，而是返回已持久化的 `read` 工具输出。
- provider model catalog 路由只转发 `ProviderService::models`，不读取、记录或回显 api key/header；具体
  限时、限体积和元数据规范化留在 `ark-llm` / `ark-provider`。
- `GET /v1/runtime/capabilities` 的 `capabilities` 与 `catalog` 都从注入的 `BaseServices::ui` 读取，
  handler 不得保留硬编码能力清单或第二份导航表。未装配 UI 总线时返回空能力与空 catalog，
  不能宣称 `ui_page`。
- loopback API 默认不对浏览器开放跨域访问。为 Tauri 桌面壳，仅精确允许 Windows 的
  `http://tauri.localhost` 与 macOS/Linux 的 `tauri://localhost` origin，且只允许 GET/POST/PATCH/DELETE
  和 `accept`、`authorization`、`content-type` headers；禁止将其放宽为 `*`。
- `POST /v1/threads/{id}/rewind` 仅在没有活动回合时接受 `turn_id` 或 `seq` 之一；目标 turn 从其
  `turn_started` 处开始物理删除。成功响应含新的 `last_seq`，重复请求同一已记录目标幂等返回；SSE 的
  `thread_rewound` 事件要求客户端重新读取线程详情。
- `POST /v1/subagents/config` 是子代理开关和并发上限的专用热更新入口，内部复用
  `ConfigPatch.subagents` 的校验、替换和持久化语义，并与 `/v1/runtime/config` 一样返回
  `ApplyResult`；不得路由到其他 split-config 占位 handler。
- `PATCH /v1/threads/{id}` 写入 runtime 前必须通过 `ProviderService::resolve_endpoint` 校验显式
  provider，且非空 model 必须有可解析 provider；校验失败不得写入 store。`POST /v1/threads/{id}/turns`
  在注册回合前拒绝超过 `ark_contract::MAX_TURN_TEXT_BYTES` 的 UTF-8 文本。
- `POST /v1/workspace/validate` 只转发 `WorkspaceService::validate` 的目录存在性校验，并使用
  `ark_common::path::normalize` 返回规范化路径；handler 不得复制工作区解析规则。
- `GET /v1/threads/{id}/usage` 与 `GET /v1/debug/llm-rounds` 只从权威 `usage` 事件投影（每轮模型调用
  恰好一条），不维护第二套用量真源。llm-rounds 以 `limit`（缺省
  `LLM_ROUNDS_DEFAULT_LIMIT`，clamp 到 `LLM_ROUNDS_MAX_LIMIT`）与 `offset` 分页，无 `thread_id`
  时至多扫描最近 `LLM_ROUNDS_MAX_THREADS` 个线程；诊断字段不包含 prompt、API key、header 或用户敏感正文。
  错误归属依赖回合终态事件闭合：末轮之后、终态之前的 `Error` 事件在 `TurnCompleted/Failed/Aborted`
  到达时归入末轮；仍在运行的回合不产出这部分归属，属预期语义而非丢数据。
- `POST /v1/sessions/{id}/resume-thread` 把 session id 兼容为 thread id（本运行时只有线程一个持久
  会话对象）：返回与 `GET /v1/threads/{id}` 完全一致的 `ThreadDetail`；未知 id 结构化 404，坏 id 400，
  绝不隐式创建会话。
- `POST /v1/threads/{id}/review` 启动真实的代码检视回合：复用 `start_turn` 的注册/冲突/未知线程语义，
  提示词由该线程权威的 workspace change 投影构成（有界，不回显敏感正文）；响应与普通回合相同为
  `202` + `TurnSummary`，进度经事件流按返回的 turn id 追蹤。变更投影在回合注册前计算，是对提示词的
  近似快照：若并发回合刚结束，检视模型可用 read 工具按需复核最新状态。
- `GET /v1/threads/{id}/export` 输出版本化（`THREAD_EXPORT_FORMAT_VERSION`）、确定性、可移植的会话
  导出：仅含摘要元数据与升序 `seq` 的完整权威事件日志，不含导出时间戳、凭证、API key、header 或
  附件载荷；相同 store 状态两次导出字节一致。

## 路由清单

当前路由以 `src/api/routes.rs` 为准。新增或修改路由必须同步更新 wire DTO、接口测试、渲染层 client
和对应需求设计；不在本文维护第二份易漂移的完整路由表。

附件路由为 `POST /v1/attachments`、`GET /v1/attachments/{id}`、
`GET /v1/attachments/{id}/content` 与 `GET /v1/attachments/diagnostics`。上传采用 JSON Base64；
handler 在解码前按存储端口的最大字节数限制编码长度，超限或非法 Base64 返回结构化 400。仅上传路由
以 `DefaultBodyLimit` 放宽至 Base64 编码上限加 64 KiB JSON 余量，并把提取阶段的超限映射为同一结构化
校验错误；其它 API 路由仍保留框架默认限制。内容下载设置持久化 MIME（无效或缺失时为
`application/octet-stream`）、精确 `content-length`、`X-Content-Type-Options: nosniff`，以及采用仅
ASCII 安全文件名的 `Content-Disposition: attachment`；非 UUID、缺元数据或缺字节文件统一为 404。

工作区路由只经 `WorkspaceService` 读取 active view、执行 switch 和取得 diff。切换时将 parent
`TurnRegistry` 与 active child 数相加；非零即返回 `Conflict`/409，不能隐式取消。diff 响应带
generation，客户端必须丢弃 generation 已变的旧响应。

`/v1/memory` 的 GET、POST、PATCH、DELETE 及 `/v1/memory/diagnostics` 仅解包 HTTP 并调用
`MemoryService`。服务未装配时返回 `runtime_unavailable`/503；输入和不存在 id 的错误保持端口的
结构化错误码，handler 不复制持久化或校验规则。
