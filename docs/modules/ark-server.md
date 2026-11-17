# ark-server 模块设计

## 职责与边界

`ark-server` 是 HTTP/SSE transport adapter：路由、鉴权、中间件、请求解包、SSE 编码和优雅停机。
业务决策委派给注入的 agent、store、配置、provider、skill 与工作区服务；handler 不得直接写业务分支。

`AppState::new` 默认让 `config_dir` 等于 `data_dir`，保持测试和其他调用方兼容；生产组合根通过
`with_config_dir` 注入 `<root>/config`。运行时配置和子代理配置的保存 handler 都使用该字段，确保
保存路径与 arkd 启动加载的 `<root>/config/config.json` 一致。

## SSE 与生命周期

- `GET /v1/mcp-info` 是原子能力的自描述发现文档（[mcp-server-surface 需求](../requirements/mcp-server-surface.md)）：
  静态工具目录（`service/mcp_tools.rs`，每个工具带 name/description/inputSchema/http.method+http.path）
  经 `service/mcp_info.rs` 组装，`auth.required` 反映运行时是否配置了 token。它挂在免鉴权 open 路由组
  （与 `/health` 同层）——调用方必须先读到文档才知道如何鉴权。schema 只写路径参数与关键请求体字段，
  契约 DTO 建模的请求体引用类型名而非逐字段抄写（`ark-contract` 是唯一真源）；ark-server 路由
  内部定义的个别请求体（如 `UpsertProviderBody`）如实标注为 server 本地类型，不指向契约生成物。集成测试对目录中每个
  `method+path` 发起真实请求，断言不落入路由级裸 404，防止目录与路由表漂移。
- SSE 订阅调用 `SessionStore::subscribe`，取得同一临界区内的 backlog、水位和 live receiver。
- 首帧：`GET /v1/threads/{id}/events` 在 backlog 为空时立即发送一个 `heartbeat` 控制帧，`seq` 取该次订阅的
  watermark。它不新增 wire 类型，也不写入 store：既不能声称 writer 尚未发放的序号，也不能让重连的
  `since_seq` 跳过事件。backlog 非空时不发送该帧，因为首个回放事件已经产生字节，且该帧会与回放末条
  重复同一个 `seq`。因此正文是否产生首字节不再由 keep-alive 间隔决定。
- 连接有界；慢客户端断开后从最后 `seq` 重连，绝不阻塞线程 writer。
- `GET /v1/threads/{id}/turns/{turn_id}` 必须校验 `TurnSummary.thread_id` 与路径 `{id}` 一致，
  不一致按 404 处理（与 steer、child-run 单路由的归属校验一致）；turn id 不是跨线程可读的全局句柄。
- steer 与回合起点共用同一 `TurnRequest` 信封与同一附件授权裁决：`enqueue_steering` 前先
  `authorize_turn_attachments`（按本线程关联事件，未关联统一 404），id 随文本进入同一条
  `SteerEntry`，不存在第二条附件通道。
- `workspace_gate` 是全进程串行启动门，锁内只做注册表变更与必要的轻量存在性检查，不得 await
  磁盘 I/O 或全量事件回放。附件授权（`authorize_turn_attachments`，回放整段事件日志）在取锁前
  完成；`get_thread` 单文件存在性检查与删除路由共用同一 gate，保留在锁内以避免删除竞态。
- `/v1/shutdown`、Ctrl-C 和任务 panic 走同一收敛流程：先阻止新的 completion wakeup，再取消待调度
  continuation 与活动回合，等待有限时间、落终态并清理 registry。
- 后台 child 的持久 `child_completed` 成功写入后会唤醒 per-thread continuation supervisor。
  `subagents.auto_continue` 默认关闭；仅当它与 `subagents.enabled` 同时开启时，同一父线程 250 ms 内的
  通知共享一个可取消 debounce，关闭任一开关会立即取消待调度 continuation。
  `turn.pre_start` 钩子在取 `workspace_gate` 之前调用（回调可能回打受保护路由，持 gate await 会自死锁）；
  钩子拒绝则该自动续跑不排程。取 gate 后复核 active turn、归档/删除、shutdown、配置和未确认 inbox。中断、删除、
  归档会取消并抑制迟到 wakeup；下一次显式用户回合或取消归档解除抑制，中断抑制还会从父线程持久终态
  跨重启恢复（启动修复合成的 abort 除外）。每批最多消费 8 条；仅当前批全部确认成功且仍有待消费项时
  调度下一批，确认失败不会自行循环。自动作业仍调用注入的普通 `TurnRunner`，不会绕过 reactor、gate
  或 sandbox，也不会写入伪用户消息。
- 回合因模型请求失败（`model_request_failed`，网关抖动/断流/流超时）终结时，`service::recovery`
  调度自动恢复：以同一请求文本（`TurnSource::AutoRecovery`）重开回合，退避 5s/30s/120s，最多 3 次；
  尝试号写入 `turn_started.auto_recovery`，恢复回合先写一条 Notice 说明自动重试原因。连续模型失败达
  `RECOVERY_MODEL_FAILURE_STREAK_LIMIT`（与 continuation 熔断同值）即暂停该线程恢复，任何用户回合或
  非模型终态重置计数；日志不可读时 fail-closed。工具/策略失败与预算耗尽的失败保持终态，仍由用户决定
  是否重发——恢复只覆盖"请求是好的、通道是坏的"这一种形态。
- 路径、HTTP 方法、字段名和兼容别名均是对外契约；变更前必须检查 `ark-contract`、`web` 与集成方。
- `GET /v1/threads` 支持分页与筛选：`limit` 显式给出时 clamp 到 `[1, THREAD_PAGE_MAX_LIMIT]`，
  `offset` 按确定顺序（active 优先、`last_event_at ?? updated_at` 降序、`thread_id` 兜底）取页，空页即
  最后一页；`archived` 与 `q`（标题大小写不敏感子串）可与分页组合。`limit` 缺省保持旧行为返回全部线程
  （响应仍为裸 `ThreadSummary` 数组），不得暗中套用默认页大小截断既有客户端。每行携带 `running`
  （`TurnRegistry.active_thread_ids` 一次读取，表示该线程有 in-flight 回合，重启后恒为 false）与
  `last_event_at`（索引投影的最新事件时间，列表排序与行内活动时间显示的真源）。
- `POST /v1/threads` 接受可选 `workspace`：经 `WorkspaceService::validate` 校验（与 composer 的
  新增项目同一条规则）后作为该线程的 workspace 覆盖种子，校验失败返回结构化 400 而不是写入死路径；
  缺省时保持既有行为（回落配置的 default workspace）。编排宿主 `service::threads::create_thread` 的
  显式值优先于 default 种子，并统一经 `ark_common::path::normalize` 规范化；scheduler/IM 等其他调用方
  传 `None` 语义不变。
- `DELETE /v1/threads?workspace=…`（`service::threads::delete_settled_by_workspace`）按项目批量删除
  **已结束**的线程：匹配遵循渲染层项目分组规则（显式 override 与路径规范化后比对，无 override 回退
  运行时 active root；默认组不被此动词服务），"已结束"复用列表 busy 标记的同一持久折叠
  （`durable_running_ids`，注册表与日志同源，两种判定永不漂移）。整段 sweep 持 `workspace_gate`，
  与回合注册、单线程删除互斥；运行中/删除失败的线程逐行进入响应 `skipped`（结构化 `ArkErrorCode`，
  运行中为 `conflict`），不令整个请求失败。响应 `BulkDeleteThreadsResponse`（`deleted`/`skipped`）。
- `GET /v1/threads/{id}` 的 `child_completions` 是最新 50 个不同通知 ID 的有界视图；它从 store 的窄
  completion 查询投影，不能全量回放普通会话事件。
- `InteractiveGates` 只做 HTTP 决议与 reactor gate future 的一次性 rendezvous；`prepare_*` 同步插入
  registry 后才返回 future，resolved 事件仍由 reactor 在实际消费回复后写入。future 自带 Drop 守卫，
  按（id, generation）移除自己的注册：取消/丢弃清理自身，且同 id 重新注册后旧守卫不会误删新注册。
- `GET /v1/threads/{id}` 把 active `TurnRegistry` entry、interaction 窄事件投影和 live gate id 三者取交集
  写入 `ThreadDetail.runtime_snapshot`；`ThreadDetail.runtime` 只携带已持久化的每线程覆盖值。历史 gate 或
  已移除 waiter 不得出现在可操作快照中。
- workbench 的 Plan、Changes、File、Terminal 投影均在 `service/workbench.rs`，handler 只读取权威事件并
  调用该纯函数；File 不直接访问磁盘，而是返回已持久化的 `read` 工具输出。workbench 的 goal/todos
  读写端点（`GET/POST/DELETE /v1/threads/{id}/goal` 与 `/todos`）不经回合注册，直接向线程事件流追加
  `GoalUpdated`/`GoalCleared`/`TodosUpdated`/`TodosCleared` 权威事件（todos 经 `validate_todos` 校验），
  空文本 goal 与非法 todos 返回结构化 400；它们是 workbench 唯一的写路径，读侧 Plan 投影消费同一事件流。
  `validate_todos`（上限 200、id/title 非空且 id 唯一）与 `todo_marker`（`[ ]`/`[~]`/`[x]` 渲染）作为
  共享纯函数宿主在 `service/workbench.rs`：HTTP 路由与组合根的 `todo_write` 工具（见 arkd 设计）是
  仅有的两个调用方，保证持久计划不会因写入路径不同而形状漂移。
- `POST /v1/threads/{id}/fork` 以当前持久水位为切点物理复制前缀线程（不接受任意历史 seq：选择历史
  前缀需要明确的历史 UI，裸露数字会成为一个无法解释选中范围的半成品功能）。
- `/v1/child-runs` 管理族（list/get/delete/abort/steer/resume/thread/events）只解包 HTTP 并转发
  `SubagentRunner`/`SubagentRegistry` 端口；行为语义（预算、终态收敛、resume 继承）以
  [ark-delegation 模块设计](ark-delegation.md) 与 [background-subagent-completion 需求](../requirements/background-subagent-completion.md) 为准，handler 不复制规则。
- provider model catalog 路由只转发 `ProviderService::models`，不读取、记录或回显 api key/header；具体
  限时、限体积和元数据规范化留在 `ark-llm` / `ark-provider`。
- `GET /v1/runtime/plugins` 只转发 `BaseServices::plugins`（`PluginHostService::report`）的
  `PluginHostReport`；handler 不读插件 registry、不重新解释字段。插件服务未装配时返回
  `runtime_unavailable`/503，因为空列表会与“确实没有插件”混淆。该路由只读：没有
  enable/disable/reload，插件只在启动时装载一次。
- `GET /v1/runtime/capabilities` 返回 `ark-contract` 的 `CapabilitiesResponse`。能力按宿主归属分别派生：
  `tool` 来自工具注册表、`model_provider` 来自 provider 表（handler 合成），`ui_page`/`ui_panel` 来自
  `BaseServices::ui` 的注册结果。handler 与组合根都不得保留平行能力清单或第二份导航表。未装配
  UI 总线时返回空能力与空 catalog，不能宣称 `ui_page`。catalog 按 `required_caps` 过滤，
  不满足条件的页/面板不进入响应。
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
  provider，且非空 model 必须有可解析 provider；校验失败不得写入 store。覆盖字段按
  `merge_runtime` 的 `patch.or(current)` 逐项合并（本端点不支持经 PATCH 单项清空覆盖）；`autonomy`
  经 `Autonomy::parse` 校验，`reasoning_effort` 仅接受 `REASONING_EFFORTS`（`off|low|medium|high`），
  空串视为未设置。覆盖语义在回合侧的解析见 [arkd](arkd.md)。
  `POST /v1/threads/{id}/turns` 在注册回合前拒绝超过 `ark_contract::MAX_TURN_TEXT_BYTES` 的 UTF-8 文本。
- `POST /v1/workspace/validate` 只转发 `WorkspaceService::validate` 的目录存在性校验，并使用
  `ark_common::path::normalize` 返回规范化路径；handler 不得复制工作区解析规则。
- `GET /v1/workspace/file?path=[&thread_id=]` 在 `spawn_blocking` 中调用
  `WorkspaceService::read_file`（磁盘 I/O + canonicalize；root 在阻塞调用前捕获，并发切换只影响
  下一次预览的快照语义），返回工作区边界内的 `WorkspaceFilePreview`；无 active workspace 是结构化 404。
  **授权 root 按回合同一顺序解析**：`thread_id` 指向带 workspace 覆盖的会话时用该覆盖，否则用进程
  lease；`thread_id` 不存在或非法直接报错而不回退（回退会把已删会话的请求答成另一个项目的同名文件）。
  containment 与大小上限语义归 [ark-workspace](ark-workspace.md)，见
  [文件预览需求](../requirements/workspace-file-preview.md)。
- `POST /v1/workspace/pick` 在 `spawn_blocking` 中调用 `FolderDialogService::pick_folder`（原生对话框是
  模态 UI，请求取消不关闭已弹出的对话框）；端口未装配时 `runtime_unavailable`/503，返回
  `FolderPickResponse`，`path` 省略即用户取消。见 [workspace-switch 需求](../requirements/workspace-switch-and-git-diff.md)。
- `GET /v1/threads/{id}/usage` 与 `GET /v1/debug/llm-rounds` 只从权威 `usage` 事件投影（每轮模型调用
  恰好一条），不维护第二套用量真源。llm-rounds 每轮的 `usage.duration_ms` 优先采用 agent 循环实测
  的模型调用耗时（不含工具执行），样本未计时（旧日志）才回退到回合边界窗口；该字段是轮次耗时的唯一
  wire 载体（随扁平化的 usage 序列化）。llm-rounds 以 `limit`（缺省
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

## 会话标题：首轮开始时命名

`service/thread_titles.rs` 在首个显式用户回合**启动时**（回合已注册、worker 尚未运行），把占位标题替换为
首条消息开头截断的短标题——改名与它的 `ThreadUpdated` 事件在第一个模型 token 流出前就已落盘，因此每个
客户端在第一帧事件里看到的就是真实标题，不存在"先显示占位、回复完成后才改名"的窗口。策略纯本地：无模型
调用、无 provider 依赖、无超时预算；标题即消息开头（与 IM 线程从首条消息派生标题的既有策略一致），空白折叠
后按字符边界钳制到 40 字节。占位集合（`New thread`/`Untitled thread`/`新会话`，覆盖 HTTP 创建、store
归一化与 web 本地化三个兜底）也在本模块。

- 只在**显式用户回合**（`TurnSource::User`）的启动路径触发，且在 `start_turn_as` 内于 worker spawn 之前
  同步执行：IM 线程已从首条消息派生标题，自动续跑没有 composer 文本，review/scheduler 的 prompt 是系统
  文本而非用户会话名，均不参与命名；回合已被接受，命名是尽力而为——线程不可读、改名失败或事件写入失败
  一律保留原标题并记 warn，永不使回合失败。
- 线程当前标题必须是占位——用户手动的改名是意图，永不覆盖。
- 改名走 `SessionStore::rename_thread` 加 `PATCH /v1/threads/{id}` 同一款 `ThreadUpdated` 事件，只有
  一条写路径；客户端因此对自动改名与手动改名一视同仁。
- web 侧 `renameThreadRow` 把流上的 `thread_updated` 折叠进列表行（与手动改名共用），列表接口在下次
  刷新时仍是权威。

## 定时任务调度服务

[定时任务需求](../requirements/scheduled-tasks.md) 的「调度与执行」落在 `service/scheduler.rs`,
它只拥有**循环**与**结果折叠**,节奏数学仍在 `ark-schedule`:

- `spawn` 先 `reconcile` 再进入 tick 循环,`TICK_INTERVAL` 为 30 秒;循环由 `serve` 在监听绑定后
  启动,headless `run` 不启动它。总开关只门控触发(`tick`/`run_now`),
  `reconcile` 的修复(`running`→`aborted`、misfire→`skipped`)不受开关影响——否则开关关闭期间
  崩溃遗留的 `running` 会一直在设置页显示运行中。循环退出有两条路径:
  `/v1/shutdown` 的广播,以及 `Ctrl-C`/drain 只置标志位的路径——后者靠每轮循环顶部的
  `is_shutting_down()` 检查收敛,最多晚一个 tick,不需要额外的唤醒通道。
- `tick` 把每个任务分类为 `NotDue`/`Due`/`Misfire`(`MISFIRE_GRACE_SECS` 为 300 秒)。
  `Misfire` 记一条 `skipped` 并推进 `next_run_at`,不补跑;`Due` 按 `next_run_at` 升序串行执行。
  同一任务上一次运行还没结束(`InFlightGuard` 未释放)时,本次到期记 `skipped`
  (`previous run was still in flight`)并推进,绝不排队。
- `begin` 经 `threads::create_thread` 建线程、`set_thread_runtime` 写入任务覆盖(`autonomy`
  缺省为 `unattended`,因此无人值守的回合不会停在 gate 上),然后写 `running` 标记。
- `drive` 用 `start_turn` 启动真实回合,拿到与 HTTP 路由完全相同的文本上限、单回合冲突、
  workspace 租约与附件校验;终态经轮询 `state.turns.get(turn_id)` 取得(注册表没有完成通知,
  轮询不会像丢失唤醒那样漏掉终态),成功时摘要取该线程最后一条非空 assistant 文本。
- **一次运行只有一条历史**。`running` 标记只写 `last_run`,有界历史只存终态:成功后历史里是
  一行 `completed`,而不是 `running` 加终态两行。`reconcile` 关闭上个进程留下的 `running` 时
  也把 `aborted` 折进历史(旧文档里的 `running` 行就地改写,不追加第二条)。
- 节奏从**完成时刻**重算(`apply_next_run_at`),因此耗时 10 分钟的 interval 任务不会在结束后
  5 分钟又触发一次。
- `run_now`(`POST /v1/schedules/{id}/run`)在返回前就写好线程与 `running` 标记,响应即可反映
  客户端将观察到的状态;主开关关闭、任务已停用(与 tick 的 `NotDue` 语义对齐,需先启用)或该
  任务仍在运行时返回 `Conflict`/409,主开关只影响触发,不影响 CRUD。

路由 `/v1/schedules`(CRUD + 立即运行)只解包 HTTP 并调用调度服务;捕获的输入与不存在 id 的错误
沿用端口的结构化错误码。

turn 启动与线程创建的编排宿主是 `service` 层（`service::turns::start_turn`/`spawn_turn`、
`service::threads::create_thread`、`service::compaction::ensure_checkpoint`）：HTTP handler 只做
解析与状态码包装，scheduler/continuation 等其他消费者调用同一编排并直接使用 `ArkError`。
`service` 层不得引用 `api::handlers` 或 `ApiError`（`serve.rs` 为绑定路由表的结构性豁免）；
错误到状态码的映射仍集中在 `api::error`。

IM 渠道编排（[IM 渠道接入需求](../requirements/im-channels.md)）居 `service/im.rs`
（生命周期：sync 对账、渠道启停、游标节流落盘）与 `service/im_inbound.rs`（消息管线：
过滤/去重/每会话串行队列/`start_turn_as(TurnSource::Im)`/回复等待与投影）。回合经
`TurnSource::Im` 走与 HTTP 同一 `start_turn` 校验路径；回复文本在终态后按本回合
`turn_id` 从线程投影取（同 scheduler 摘要先例），不用 delta 累积。路由
`/v1/im/channels`(CRUD)、`/v1/im/channels/{id}/verify`、`/v1/im/bindings`、
`/v1/im/weixin/login/{start,wait}`(确认后端直接建渠道,经容量校验、默认停用)只解包
HTTP；
总开关 `config.im.enabled` 只门控连接不门控编辑。

## 路由清单

当前路由以 `src/api/routes.rs` 为准。新增或修改路由必须同步更新 wire DTO、接口测试、渲染层 client
和对应需求设计；不在本文维护第二份易漂移的完整路由表。`protected_routes()` 按资源域拆为一组私有
builder（threads/turns/diagnostics/platform services/settings/workbench/gates/memory/
child-runs/sessions/schedules/im/subagents/pending），每个函数一个域、不超过函数行数上限；合并
结果仍是本文件拥有的唯一可审计路由面。线程接口测试按主题拆在 `api/handlers/threads/tests/` 下的
三个套件文件（list/projection、runtime override、workspace/bulk-delete），经 `#[path]` 挂回同一
`tests` 模块共享 doubles。

附件路由为 `POST /v1/attachments`、`GET /v1/attachments/{id}`、`DELETE /v1/attachments/{id}`、
`GET /v1/attachments/{id}/content`、`GET /v1/attachments/diagnostics`，以及线程侧的关联入口
`POST /v1/threads/{id}/attachments`。上传采用 JSON Base64；
handler 在解码前按存储端口的最大字节数限制编码长度，超限或非法 Base64 返回结构化 400。仅上传路由
以 `DefaultBodyLimit` 放宽至 Base64 编码上限加 64 KiB JSON 余量，并把提取阶段的超限映射为同一结构化
校验错误；其它 API 路由仍保留框架默认限制。字节签名与声明 MIME 不符时存储层返回
`mime_mismatch`/422；未声明或无法识别的 MIME 不被拒绝。内容下载设置持久化 MIME（无效或缺失时为
`application/octet-stream`）、精确 `content-length`、`X-Content-Type-Options: nosniff`，以及采用仅
ASCII 安全文件名的 `Content-Disposition: attachment`；非 UUID、缺元数据或缺字节文件统一为 404。

关联路由挂在线程路径下是刻意设计：URL 本身就声明了是哪个线程获得授权，handler 无法被诱导去授权别的
线程。关联先确认线程与附件都存在；同一对已关联时不追加事件，仍返回 201。关联结果只由持久化的
`attachment_associated` 事件表达，不新增关联表。`DELETE /v1/attachments/{id}` 在该 id 被任一线程关联时
返回 `attachment_in_use`/409 且不删字节；未关联时删除字节并返回 200。本期不提供解绑路由。

`service/attachment_auth.rs` 是三个读方共用的唯一折叠点：`thread_attachment_ids`（线程详情与回合起点
授权）、`associated_attachment_ids`（删除守卫与 `orphan_count`）和 `authorize_turn_attachments`（回合
引用校验）。最后者只在附件已关联到**本线程**时放行；未关联（包括存在但属于其它线程）统一返回
`attachment_not_associated`/404，不用错误码泄露 id 是否存在，也不因重复传参写入重复事件。

工作区路由只经 `WorkspaceService` 读取 active view、执行 switch 和取得 diff。切换时将 parent
`TurnRegistry` 与 active child 数相加；非零即返回 `Conflict`/409，不能隐式取消。diff 响应带
generation，客户端必须丢弃 generation 已变的旧响应。

`/v1/memory` 的 GET、POST、PATCH、DELETE 及 `/v1/memory/diagnostics` 仅解包 HTTP 并调用
`MemoryService`。服务未装配时返回 `runtime_unavailable`/503；输入和不存在 id 的错误保持端口的
结构化错误码，handler 不复制持久化或校验规则。
