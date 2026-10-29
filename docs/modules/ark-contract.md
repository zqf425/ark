# ark-contract 模块设计

## 职责与边界

`ark-contract` 是 HTTP、SSE、MCP 与 React 渲染层之间的 wire DTO 唯一真源。它依赖
`ark-common`，但不得包含端口 trait、持久化实体、内部查询对象或业务实现。

当前公开面包括线程/回合/子代理 DTO、运行时事件、渲染 item、能力目录、配置视图、provider 与
skill 视图。所有 GUI 字段使用 camelCase；既有查询参数保持既有命名，例如 `since_seq`。

`TurnRequest.text` 的 UTF-8 字节数上限由 `MAX_TURN_TEXT_BYTES` 定义为 1 MiB；HTTP 边界在创建回合前执行该校验。

线程公开面还包括 `RewindRequest`（`turn_id` 或 `seq` 二选一）与 `RewindResponse`。`RuntimeEvent` 的
`thread_rewound` 是持久化的 SSE 重置通知，不是被删除历史的替代投影。
附件当前提供 `AttachmentId`、不可变 `AttachmentEntry`、Base64 JSON 上传请求/响应和诊断 DTO。
上传体的 `content` 是标准 Base64，原始字节不会进入附件元数据或任何事件流；下载字节是独立的 HTTP
响应体。附件 ID 由服务端生成，客户端只能在读取请求中把它作为不透明字符串传回。

后台子代理终态使用 `child_completed` 事件传递稳定 `notification_id`、状态、摘要/错误和用量；
消费者按通知 ID 去重，不得把 child 的完整原始转录复制到父线程。父回合正常完成后以
`child_completion_acknowledged` 确认已消费的通知；确认失败保留至少一次重投递语义。
`ThreadDetail.child_completions` 是 `child_completed` 的渲染投影，用于页面重新打开时恢复摘要卡片；它
不取代事件日志，也不包含完整 child 转录。
`ThreadDetail.runtime` 是同一存活 `arkd` 进程内的短暂控制快照：包含 active turn 及待处理 gate 的
最小展示字段。它以 serde default 保持旧 detail JSON 可读，绝不用于重建重启后的 waiter。

## 类型生成

- Rust 类型派生 `serde`、`schemars` 与 `ts-rs`；前端只导入 `web/src/generated/` 生成物。
- 修改 wire 类型后运行 `cargo test -p ark-contract --lib ts_export`，再检查
  `git diff --exit-code web/src/generated`。
- 新增根类型必须同时登记 `ts_export.rs` 的 `export_roots()` 与 `ROOT_NAMES`。

## 注意事项

错误码的通用定义当前位于 `ark-common::ArkErrorCode`，而非本 crate；这是当前实现的明确例外。
若未来迁移错误码，必须一次性更新 HTTP、SSE、MCP 和生成的 TypeScript，不能保留双源。

工作区 wire 包含 `WorkspaceView`、`SwitchWorkspaceRequest` 和 `WorkspaceDiff`。后者独立于线程
Changes 投影，明确提供 staged/unstaged/untracked、rename、二进制、patch 上限和 truncation；所有
generation 均以 JSON number 输出。

长期 memory wire 包含 `MemoryEntry`、创建/更新请求、列表和诊断响应。它只表达跨会话可复用的
用户偏好或事实，不包含模型推理、检索排名或自动注入策略；这些后续能力仍属待决。
