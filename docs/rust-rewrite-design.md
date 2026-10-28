# ark Rust 重写：整体架构设计

> 本文只定义跨模块架构、全局不变量和模块边界。模块内的数据结构、状态机、协议细节、实现路径和
> 验证要求见 [模块设计索引](modules/README.md)；特定功能的范围和验收见
> [需求设计目录](requirements/)。

## 1. 目标与范围

ark 用 Rust 重建原 smart-ark 的 Code 运行时与渲染层，重点解决契约漂移、重复逻辑、巨型装配、
会话持久化和 UI 自动化双决策循环等问题。

范围包括 Agent runtime、模型适配、工具、会话存储、HTTP/SSE、Code UI、子代理、skills、provider、
工作区检查，以及 UI 自动化的领域边界。Write、Connect、工作流、云端后端和 Playwright 内核不在
本仓库重写范围内。

## 2. 当前实现与目标态

所有设计内容都必须标明状态：

- **当前实现**：以 workspace、测试和运行结果为准；模块文档记录已实现约束。
- **已批准计划**：已确认的架构目标，但尚未实现；必须在对应模块或需求文档标注“计划”。
- **待决**：影响产品边界、兼容性或安全模型的事项；未决前不得假定实现存在。

当前 workspace 的唯一运行时组合根是 `apps/arkd`。`apps/ark-desktop/src-tauri` 是已实现的桌面宿主，
但不链接任何 ark crate，也不成为组合根；它只启动/回收 `arkd` 进程。`plugins/`、`xtask/` 和独立
`plugin-code-ui` 等目录尚不是当前 workspace 模块；引用它们时必须标记为计划。

## 3. 架构原则

1. **单一决策循环**：`ark-agent` 的 reactor 是唯一模型决策循环。子代理复用它；Studio、MCP 和
   sidecar 只能是工具、适配器或确定性执行器。
2. **契约与行为分离**：跨进程数据由 Rust 类型生成；路径归一化、脱敏、截断、token 估算等可执行
   逻辑只能有一个宿主实现。
3. **端口隔离实现**：核心业务依赖 `ark-ports`，具体模型、工具、存储与网络能力由 `arkd` 注入。
4. **会话目录为真源**：会话权威状态是追加日志，索引和渲染投影可重建；派生数据不得用于授权。
5. **显式状态与可取消性**：生命周期以枚举和状态机表达；取消使用 `CancellationToken`，所有路径
   都收敛到明确终态。
6. **能力集中治理**：HTTP、MCP、UI 和插件身份/授权由宿主提供，插件不重建通信、鉴权或界面容器。
7. **失败隔离与资源所有权**：I/O 和子进程失败只影响当前操作；临时目录、句柄与子进程都有 owner，
   drop 或 supervisor 负责回收。
8. **最小重启**：进程资源和安全边界变更才重启；其他配置通过一致快照热更新。

## 4. 系统边界

```text
React web ── HTTP/SSE ── ark-server ── ports ── ark-agent
                                  │               │
                                  │          tool / model / store
                                  │               │
                                  └────────── arkd ──────────┐
                                                              │
                     ark-http / ark-mcp / ark-ui / ark-plugin │
                                                              │
                         provider、skills、workspace、obs、Studio（按需）

Tauri desktop ── 管理本地 arkd 进程 ── React web（同一 HTTP/SSE 边界）
```

- `ark-server` 仅是 HTTP/SSE 边界，不包含领域分支。
- `ark-agent` 只认识端口，不认识 axum、文件、SQLite、provider 或具体工具。
- `arkd` 是唯一组合根，允许依赖所有实现，但只负责显式装配。
- `web` 只消费生成契约和 typed client；不重复后端语义。
- Node/Playwright sidecar 是已批准的 UI 自动化执行边界，不包含模型决策。

## 5. 当前依赖方向

下图反映当前生产依赖；新增依赖必须保持无循环，并同步更新所属模块设计。

```text
ark-common     → 无内部依赖
ark-contract   → ark-common
ark-ports      → ark-common, ark-contract
ark-config     → ark-common, ark-contract
ark-store      → ark-common, ark-contract, ark-ports
ark-agent      → ark-common, ark-contract, ark-ports
ark-delegation → ark-common, ark-contract, ark-ports
ark-llm        → ark-common, ark-contract, ark-ports
ark-provider   → ark-common, ark-contract, ark-ports, ark-llm
ark-tool       → ark-common, ark-contract, ark-ports
ark-http       → ark-common, ark-contract, ark-ports
ark-ui         → ark-common, ark-contract, ark-ports
ark-mcp        → ark-common, ark-contract, ark-ports, ark-http
ark-plugin     → ark-common, ark-contract, ark-ports, ark-http, ark-ui, ark-mcp
ark-skills     → ark-common, ark-contract, ark-ports
ark-workspace  → ark-common, ark-contract
ark-obs        → ark-common, ark-contract
ark-server     → ark-common, ark-contract, ark-ports, ark-agent, ark-store, ark-config, ark-obs
ark-studio     → ark-common, ark-contract, ark-ports
arkd           → 所有实现（组合根）
ark-desktop    → 无 ark crate 依赖；以受控进程启动 arkd
```

特别约束：`ark-agent` 不得依赖具体实现；`ark-contract` 不得承载端口 trait 或内部业务对象；
`ark-studio` 不得依赖模型或具体后端；跨 crate 不得穿透 `pub(crate)` 内部模块。

## 6. 跨模块不变量

### 6.1 契约和兼容性

- wire DTO、事件和前端声明由 `ark-contract` 的生成管线统一维护。
- 对外路径、字段和兼容别名变更需要同时审查 server、web 和集成方。
- “不兼容重写”仅指内部架构；用户数据、既有 JSONL、已公开 HTTP/SSE 契约的兼容范围由具体需求
  明确，不能用一句“无需兼容”替代决策。

### 6.2 状态、存储与事件

- 每个回合和子任务都有唯一终态；panic、取消、写入失败和 shutdown 都要收敛。
- `events.jsonl` 与 `children/runs.jsonl` 是权威记录；索引只可辅助查询。
- SSE 快照、水位和 live 订阅在同一序列化点建立；序号严格递增，压缩引起的跳跃必须有持久化标记。
- 同一数据目录只有一个写进程；同父目录的 child 写、删、重建使用相同父级锁。

### 6.3 插件和能力总线

- `plugin_id` 由宿主签发且不可伪造；日志、配置、凭据、KV、文件、缓存、MCP 工具和配额按其隔离。
- 插件不自建 HTTP client、MCP server 或界面容器；跨插件调用经 MCP 总线并接受声明、深度和循环校验。
- 安全策略、授权、审计与资源上限永远由内核强制，服务实现只能提供能力。

### 6.4 配置与变更

- `ReloadClass` 是类型层的字段属性；含 `Restart` 字段的 patch 整批拒绝。
- 新任务读取最新配置快照，已运行任务保留创建时策略。
- 新功能优先扩展现有模块、端口或插件；需要新增边界时先在需求设计中说明职责、依赖和退出条件。

## 7. 模块与需求文档治理

模块文档是模块内部设计的唯一真源。修改前必须阅读整体架构和全部受影响模块的文档；涉及对外行为、
数据迁移、状态机、配置、权限或多模块协作时，必须同时创建或更新独立需求设计。

正式 `docs/` 只存整体架构、模块设计和需求设计。审查报告、交接、调研、brief、计划草稿和提示词
放入 `.tmp/`，不得提交。旧总设计全文已在本次迁移中保留至本地
`.tmp/docs-archive/rust-rewrite-design-pre-split.md`，仅供追溯，不是正式真源。

## 8. 设计审查与演进

拆分时发现的文档/实现偏差与修改建议记录在 `.tmp/design-migration-audit.md`。其中涉及架构决策的
建议必须经确认后才修改正式目标；仅属于事实陈旧的内容可以直接按代码和测试结果修正。

每个需求完成时，更新受影响模块设计、需求设计和必要的架构图；不要以新的 review 或 handoff 文档
替代正式设计更新。
