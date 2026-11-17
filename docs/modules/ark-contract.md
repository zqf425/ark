# ark-contract 模块设计

## 职责与边界

`ark-contract` 是 HTTP、SSE、MCP 与 React 渲染层之间的 wire DTO 唯一真源。它依赖
`ark-common`，但不得包含端口 trait、持久化实体、内部查询对象或业务实现。

当前公开面包括线程/回合/子代理 DTO、运行时事件、渲染 item、能力目录、配置视图、provider 与
skill 视图，以及插件宿主的只读报告（`PluginHostReport`、`PluginEntry`、`PluginDiagnosticView`）。
后三者的贡献项（pages/panels/tools）由宿主从真正拥有它们的总线汇总，本 crate 只定义形状，不保
平行清单。wire 字段全量使用 snake_case 命名（生成的 TypeScript 同名，如 `thread_id`、`prompt_tokens`），
child-runs 系列曾有的 camelCase 例外已随生成类型管线移除；子代理路由的查询参数名同为 `parent_thread_id`/`thread_id`/`since_seq`（无 camelCase 别名）。新增字段不得引入其他命名风格。

`ThreadItem.marker`（`ItemMarker::ContextCheckpoint`）是系统生成条目的结构化标识，取代按文本
前缀识别；判定入口是 `ThreadItem::is_context_checkpoint`（对历史无 marker 条目按前缀兜底，且
gated 在 `Notice` kind 上）。取消原因词表与自治权词表不在本 crate：它们是纯共享词汇，宿主为
`ark-common`（`CancelReason`、`Autonomy`，见其模块文档）。

`TurnRequest.text` 的 UTF-8 字节数上限由 `MAX_TURN_TEXT_BYTES` 定义为 1 MiB；HTTP 边界在创建回合前执行该校验。

线程公开面还包括 `RewindRequest`（`turn_id` 或 `seq` 二选一）与 `RewindResponse`。`RuntimeEvent` 的
`thread_rewound` 是持久化的 SSE 重置通知，不是被删除历史的替代投影。
附件当前提供 `AttachmentId`、不可变 `AttachmentEntry`、Base64 JSON 上传请求/响应、关联请求/响应
和诊断 DTO。上传体的 `content` 是标准 Base64，原始字节不会进入附件元数据或任何事件流；下载字节是
独立的 HTTP 响应体。附件 ID 由服务端生成，客户端只能在读取请求中把它作为不透明字符串传回。

`AttachmentEntry.sha256` 是上传时计算的小写十六进制 SHA-256，只用于完整性与后续去重：存储仍以 UUID
寻址。该字段带 serde default，此字段存在前写入的元数据仍可读，空字符串表示未记录。

附件投递结果 `TurnAttachmentOutcome`（`attachment_id`、`original_filename` 可选、`delivered`
词表、`detail` 可选原因）同时出现在 `TurnStarted.attachment_outcomes` 与
`ThreadItem.attachments`：事件是单回合审计记录，item 是渲染数据源，两者内容相同、消费方不同
（见 `arkd` 与 `web` 模块文档）。`ThreadItem.attachments` 带 serde default 且省略空值，旧 item
与无附件 item 均不受影响；`TurnAttachmentDelivery` / `TurnAttachmentOutcome` 已进 TS 生成 barrel。

关联是授权边，而非第二份存储：`POST /v1/threads/{id}/attachments` 接受 `AttachmentAssociationRequest`
（只有 `attachment_id`）并返回 `AttachmentAssociationResponse`；重复调用不追写第二个持久化事件。
`RuntimeEvent` 新增 `attachment_associated`（`attachment_id` 为唯一业务载荷），是授权的唯一真源；
`ThreadDetail.attachment_ids` 与 `TurnStarted.attachment_ids` 都是它的投影或引用，不是额外状态。
`TurnRequest.attachment_ids` 可选且纯增量（默认空、序列化时省略空向量）：运行时在回合启动前逐个校验
该 id 已关联到**本线程**，本阶段只记录本回合引用了哪些附件，不向模型传字节。`TurnStarted` 的
`attachment_ids` 同样带 serde default，使旧日志可读。

`ToolStormSuppressed` 除 `tool`/`count` 外还携带 `arguments`（serde default，旧行省略时读为 null）：
被熔断抑制的调用必须留下"重复的是什么"的原始参数记录，且不要求审计方把抑制事件与配对的
`ToolCallStarted` 做跨事件 join。`with_seq` 的逐变体重写集中在 `event/with_seq.rs`；每行拼写一个
变体的完整字段表，新增变体或载荷字段必须在此同步一行，编译器的穷尽检查保证不漏。

`ThreadItem.model`（serde default，省略空值）记录创建该 item 的回合实际使用的 `provider:model`
标识：reactor 在回合开始时从注入的 `ModelClient` 取快照盖在开场的 `UserMessage` item（及 steer
补写的用户 item）上。标签随 item 走而不是读线程当前覆盖值，因为模型切换只对**下一回合**生效——
读当前值会给所有历史行贴错标签；web 在用户气泡时间旁渲染该标签。

`Usage.duration_ms`（serde default，省略空值）是该样本关闭的那次模型调用的实测墙钟耗时：只由
runtime 每轮结算的 usage 事件写入（`ark-llm` 解析的 provider payload 永远不带它，聚合不发明它）；
`merge` 按 `sum_opt` 语义折叠，任一样本未计时则聚合保持 None（"未知"与 "0" 不混淆）。

`TurnStarted.auto_recovery: u32`（serde default `0`，序列化时省略零值）标记 harness 自动恢复
回合及其尝试序号：前一个回合因瞬态模型请求失败（`model_request_failed = true`）终结后，恢复调度器
以同一请求文本重开回合并把 1-based 尝试号写进该字段；普通用户/IM/自动续跑回合保持 `0` 且不出现在
JSON 里。消费方据此区分"用户发的消息"与"harness 重放的上一条消息"，并审计一个请求消耗了几次自动恢复。

`TuningSettings` 的首次启动默认值是 `max_rounds=1000`、`max_tokens=0`（不限）和
`max_tool_calls=1000`；它是设置页面与运行时配置的共享 wire 基线，其余字段保持类型定义的默认值。

`SubagentSettings.auto_continue` 是显式 opt-in 的 wire 配置，serde default 为 `false`，保证旧配置不会
产生额外模型调用；Rust 生成的 TypeScript 使用同名 snake_case 字段。
`SubagentSettings.default_background` 的 serde default 为 `true`：缺省旧配置保持"委派不阻塞主会话"
的默认行为，而不是回落到阻塞等待的前台派发。

后台子代理终态使用 `child_completed` 事件传递稳定 `notification_id`、状态、摘要/错误和用量；
消费者按通知 ID 去重，不得把 child 的完整原始转录复制到父线程。父回合正常完成后以
`child_completion_acknowledged` 确认已消费的通知；确认失败保留至少一次重投递语义。
`ThreadDetail.child_completions` 是 `child_completed` 的渲染投影，用于页面重新打开时恢复摘要卡片；它
不取代事件日志，也不包含完整 child 转录。
`ThreadDetail.runtime` 只携带已持久化的每线程覆盖值（作为 PATCH 合并后的结果）；同一存活 `arkd`
进程内的短暂控制快照（active turn 与待处理 gate 的最小展示字段）位于 `ThreadDetail.runtime_snapshot`。
两者以 serde default 保持旧 detail JSON 可读；`runtime_snapshot` 绝不用于重建重启后的 waiter。
`ThreadRuntimeSettings` 的覆盖字段为 workspace、provider_id、model、autonomy、reasoning_effort，全部
可选；`reasoning_effort` 词表由 `REASONING_EFFORTS`（`off|low|medium|high`）定义，空串视为未设置。
PATCH 端校验与回合侧解析分别见 [ark-server](ark-server.md) 与 [arkd](arkd.md)，协议映射见
[ark-llm](ark-llm.md)。

线程创建与批量删除 wire：`CreateThreadRequest` 除 `title` 外接受可选 `workspace`（服务端校验后作为
线程覆盖种子，见 ark-server）；`BulkDeleteThreadsResponse`/`BulkDeleteSkipped` 是
`DELETE /v1/threads?workspace=…` 的按项目批量删除响应，`skipped[].code` 直接复用 `ArkErrorCode`
枚举（`conflict` 即运行中跳过），二者均已进 TS 生成 barrel。

## 类型生成

- Rust 类型派生 `serde` 与 `ts-rs`（JSON Schema 导出所需的 `schemars` 派生属计划，尚未引入）；
  前端只导入 `web/src/generated/` 生成物。
- 修改 wire 类型后运行 `cargo test -p ark-contract --lib ts_export`，再检查
  `git diff --exit-code web/src/generated`。
- 新增根类型必须同时登记 `ts_export.rs` 的 `export_roots()` 与 `ROOT_NAMES`。

## 注意事项

错误码的通用定义当前位于 `ark-common::ArkErrorCode`，而非本 crate；这是当前实现的明确例外。
若未来迁移错误码，必须一次性更新 HTTP、SSE、MCP 和生成的 TypeScript，不能保留双源。
附件生命周期新增的 `mime_mismatch`、`attachment_in_use`、`attachment_not_associated` 同样先落在
`ark-common`，再经同一生成链路进入 `web/src/generated/ArkErrorCode.ts`；新增 code 时必须同时在 web
侧提供 `error.hint.<code>` 文案。

工作区 wire 包含 `WorkspaceView`、`SwitchWorkspaceRequest` 和 `WorkspaceDiff`。后者独立于线程
Changes 投影，明确提供 staged/unstaged/untracked、rename、二进制、patch 上限和 truncation；所有
generation 均以 JSON number 输出。

定时任务 wire 由 `schedule.rs` 提供：`ScheduleKind`、`RunStatus`、`TaskSchedule`、
`ScheduledRunRecord`、`ScheduledTask`、`ScheduledTaskListResponse`、`CreateScheduleRequest`、
`UpdateScheduleRequest`，以及 `MAX_SCHEDULED_TASKS`(100)、`SCHEDULE_TITLE_MAX_BYTES`(200)、
`SCHEDULE_RUN_HISTORY_LIMIT`(10)、`MIN_EVERY_MINUTES`(1)、`MAX_EVERY_MINUTES`(525600)、
`DEFAULT_TIME_OF_DAY`("09:00")、`DEFAULT_SCHEDULE_AUTONOMY`("unattended")。

关键形状约定：`UpdateScheduleRequest` 的每个字段都是合并语义（缺省即保留），清空可选文本靠发空串，
由归一化折回"未设置"；`ScheduledTask.enabled` 带 serde default 且缺省为 `false`，使损坏或旧文档
不会开火无人值守回合；`next_run_at` 用空串表示不调度（manual、禁用、已触发的一次性 `at`）；
`history` 最旧在前、上限 10 条。`RunStatus` 刻意不派生 `Default`："默认运行结果"没有语义。归一化与
`next_run_at` 计算不在本 crate，而在 `ark-schedule`；本 crate 只定义形状与上限常量。

IM 渠道 wire 由 `im.rs` 提供（[IM 渠道接入需求](../requirements/im-channels.md)）：
`ImChannelId`（newtype，序列化为字符串）、`ImChannelKind`、`FeishuDomain`、
`ImCredentials`（写模型，按 kind 判别）、`ImCredentialsView`（脱敏读模型，参照
`ProviderView`）、`ImChannelPolicy`、`ImChannel`（落盘与服务端内部形状）、
`CreateImChannelRequest`/`UpdateImChannelRequest`（载荷对，与 schedules 同构）、
`ImChannelView`、`ImChannelState`/`ImChannelRuntime`/`ImChannelListItem`/
`ImChannelListResponse`、`ImVerifyResponse`、`ImBinding`/`ImBindingListResponse`，以及
`MAX_IM_CHANNELS`(20)、`IM_TITLE_MAX_BYTES`(200)、`DEFAULT_IM_AUTONOMY`("unattended")。
关键形状约定：`UpdateImChannelRequest` 每字段合并语义，凭证密文字段为空/缺省视为保留原值
（与 provider `api_key` 空值回填一致），否则不回显密文的编辑表单会在保存时清空密钥；
`ImChannel.enabled` 缺省 `false`，损坏或旧文档不得自动开长连接；凭证必填校验与归一化在
`ark-im`，本 crate 只定义形状。落盘记录 `ImChannelRecord`（channel + 服务端游标）是端口
类型，居 `ark-ports`，**不登记** ts_export——游标是服务端记账，任何客户端都不可读写。

长期 memory wire 包含 `MemoryEntry`、创建/更新请求、列表和诊断响应。它只表达跨会话可复用的
用户偏好或事实，不包含模型推理、检索排名或自动注入策略；这些后续能力仍属待决。
