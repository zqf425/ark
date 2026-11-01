# ark-contract 模块设计

## 职责与边界

`ark-contract` 是 HTTP、SSE、MCP 与 React 渲染层之间的 wire DTO 唯一真源。它依赖
`ark-common`，但不得包含端口 trait、持久化实体、内部查询对象或业务实现。

当前公开面包括线程/回合/子代理 DTO、运行时事件、渲染 item、能力目录、配置视图、provider 与
skill 视图，以及插件宿主的只读报告（`PluginHostReport`、`PluginEntry`、`PluginDiagnosticView`）。
后三者的贡献项（pages/panels/tools）由宿主从真正拥有它们的总线汇总，本 crate 只定义形状，不保
平行清单。所有 GUI 字段使用 camelCase；既有查询参数保持既有命名，例如 `since_seq`。

`TurnRequest.text` 的 UTF-8 字节数上限由 `MAX_TURN_TEXT_BYTES` 定义为 1 MiB；HTTP 边界在创建回合前执行该校验。

线程公开面还包括 `RewindRequest`（`turn_id` 或 `seq` 二选一）与 `RewindResponse`。`RuntimeEvent` 的
`thread_rewound` 是持久化的 SSE 重置通知，不是被删除历史的替代投影。
附件当前提供 `AttachmentId`、不可变 `AttachmentEntry`、Base64 JSON 上传请求/响应、关联请求/响应
和诊断 DTO。上传体的 `content` 是标准 Base64，原始字节不会进入附件元数据或任何事件流；下载字节是
独立的 HTTP 响应体。附件 ID 由服务端生成，客户端只能在读取请求中把它作为不透明字符串传回。

`AttachmentEntry.sha256` 是上传时计算的小写十六进制 SHA-256，只用于完整性与后续去重：存储仍以 UUID
寻址。该字段带 serde default，此字段存在前写入的元数据仍可读，空字符串表示未记录。

关联是授权边，而非第二份存储：`POST /v1/threads/{id}/attachments` 接受 `AttachmentAssociationRequest`
（只有 `attachment_id`）并返回 `AttachmentAssociationResponse`；重复调用不追写第二个持久化事件。
`RuntimeEvent` 新增 `attachment_associated`（`attachment_id` 为唯一业务载荷），是授权的唯一真源；
`ThreadDetail.attachment_ids` 与 `TurnStarted.attachment_ids` 都是它的投影或引用，不是额外状态。
`TurnRequest.attachment_ids` 可选且纯增量（默认空、序列化时省略空向量）：运行时在回合启动前逐个校验
该 id 已关联到**本线程**，本阶段只记录本回合引用了哪些附件，不向模型传字节。`TurnStarted` 的
`attachment_ids` 同样带 serde default，使旧日志可读。

`SubagentSettings.auto_continue` 是显式 opt-in 的 wire 配置，serde default 为 `false`，保证旧配置不会
产生额外模型调用；Rust 生成的 TypeScript 使用同名 snake_case 字段。

后台子代理终态使用 `child_completed` 事件传递稳定 `notification_id`、状态、摘要/错误和用量；
消费者按通知 ID 去重，不得把 child 的完整原始转录复制到父线程。父回合正常完成后以
`child_completion_acknowledged` 确认已消费的通知；确认失败保留至少一次重投递语义。
`ThreadDetail.child_completions` 是 `child_completed` 的渲染投影，用于页面重新打开时恢复摘要卡片；它
不取代事件日志，也不包含完整 child 转录。
`ThreadDetail.runtime` 只携带已持久化的每线程覆盖值（作为 PATCH 合并后的结果）；同一存活 `arkd`
进程内的短暂控制快照（active turn 与待处理 gate 的最小展示字段）位于 `ThreadDetail.runtime_snapshot`。
两者以 serde default 保持旧 detail JSON 可读；`runtime_snapshot` 绝不用于重建重启后的 waiter。

## 类型生成

- Rust 类型派生 `serde`、`schemars` 与 `ts-rs`；前端只导入 `web/src/generated/` 生成物。
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

长期 memory wire 包含 `MemoryEntry`、创建/更新请求、列表和诊断响应。它只表达跨会话可复用的
用户偏好或事实，不包含模型推理、检索排名或自动注入策略；这些后续能力仍属待决。
