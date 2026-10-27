# ark 开发规范（AGENTS.md）

本文件是 `ark` 项目的开发约定，由 `docs/rust-rewrite-design.md`（v0.6）提炼而来。
**设计文档是唯一真源**：本文件与设计文档冲突时，以设计文档为准，并修正本文件。
改代码前先读 `docs/rust-rewrite-design.md` 的相关章节（章节号在下面逐条标注）。

---

## 1. 项目范围

- 产品名 `ark`；Rust workspace 位于本仓库根目录，服务端二进制 `arkd`（组合根）。
- 范围内：`ark-agent` 运行时、Code 模式界面、UI 自动化插件、能力总线。
- 范围外：Write / Connect / 工作流（设计 §1.2）。
- 旧 `kun` / `smart-ark` 的 Electron 代码**不在本仓库**，不要在本仓库恢复旧运行时、
  provider 切换器、诊断面板、CodeWhale / Reasonix 适配器、进程管理器或 RPC 桥。

## 2. Workspace 与依赖方向

### 2.1 目录

```
ark/
├── Cargo.toml            # [workspace]：统一 edition / lint / profile / 依赖版本
├── crates/               # 19 个 crate
├── apps/arkd/            # 组合根：唯一允许“知道所有实现”的地方
├── web/                  # React + TS 渲染层
└── docs/                 # 设计文档与审查报告
```

### 2.2 依赖方向（A → B 表示 A 依赖 B，编译期强制，设计 §5.2）

```
ark-common     → 无内部依赖
ark-contract   → ark-common
ark-ports      → ark-common, ark-contract
ark-config     → ark-common, ark-contract
ark-store      → ark-common, ark-contract, ark-ports, ark-config
ark-llm        → ark-common, ark-contract, ark-ports, ark-config
ark-tool       → ark-common, ark-contract, ark-ports, ark-config
ark-http       → ark-common, ark-contract, ark-ports, ark-config
ark-ui         → ark-common, ark-contract, ark-ports, ark-config
ark-mcp        → ark-common, ark-contract, ark-ports, ark-config, ark-http
ark-studio     → ark-common, ark-contract, ark-ports, ark-config
ark-agent      → ark-common, ark-contract, ark-ports   ★ 只到这里
ark-plugin     → ark-common, ark-contract, ark-ports, ark-http, ark-ui, ark-mcp
ark-server     → ark-common, ark-contract, ark-ports, ark-agent, ark-store, ark-config
ark-obs        → ark-common, ark-contract
apps/arkd      → 全部（组合根）
```

硬规则：

- `ark-agent` **只依赖 `ark-ports`**，不得依赖 `ark-llm` / `ark-tool` / `ark-store` 的具体实现。
  模型、工具、存储一律由 `arkd` 注入。
- `ark-contract` **只放跨进程 wire 契约**（HTTP / SSE / MCP / TS 共享 DTO）；
  端口 trait 放 `ark-ports`，不要把异步 trait 和跨进程契约混在一个 crate。
- `ark-studio` 只依赖 `ark-ports` 里的 `BackendProvider`，不得依赖任何具体后端实现。
- 跨 crate 只能 `use ark_contract::X` / `use ark_ports::Y`，
  禁止 `use ark_store::internal::Z` 这类穿透内部模块的引用（内部一律 `pub(crate)`）。

### 2.3 crate 内部分层（设计 §5.1，对齐 QualityDataCenter）

| 层 | 目录 | 约束 |
|---|---|---|
| controller | `src/api/` | 只做参数解包、鉴权、调 service；**禁止写业务分支** |
| service | `src/service/` | 用例编排；依赖 trait，不依赖具体实现 |
| dao | `src/store/` | 只做持久化；IO 细节不向上泄漏 |
| entity | `src/domain/` | 领域实体 + 不变量校验 |
| dto / request | `ark-contract` | 跨模块、跨进程共享结构的唯一定义处 |
| config | `src/config/` | 配置结构体 + 默认值，集中管理 |
| executor | `src/exec/` | 可插拔执行器（工具、Studio 步骤） |
| utils | `ark-common` | 跨 crate 纯函数；**全仓仅一份** |

- `lib.rs` 只做精确 `pub use`（白名单化），**禁止 `pub use *`**，
  避免重新长回“上千个零引用导出”。
- 模块之间禁止循环依赖。
- 纯逻辑（无 IO）必须能脱离文件系统 / 网络单测。

## 3. 代码规模与风格

- 单文件 ≤ 800 行，单函数 ≤ 80 行。触碰超限文件时先拆分，不要继续往里加逻辑。
- 全仓 `#![deny(clippy::all)]`，`cargo clippy -- -D warnings` 必须干净。
- 注释解释“为什么”，不复述代码；每条不显然的约束要写出依据（设计章节号、旧实现行为）。
- 新增内容默认 ASCII；确需中文时只用于面向用户的文案或文档。
- Windows 下写精确字节文件（提交信息、JSON fixture、测试向量）必须 **BOM-free**：
  PowerShell 5.1 的 `Set-Content -Encoding UTF8` 会写 BOM，改用
  `[System.IO.File]::WriteAllText($p, $t, [System.Text.UTF8Encoding]::new($false))`。

## 4. 契约与类型（设计 §6）

- **数据契约**：Rust 类型是唯一真源（`schemars` → JSON Schema，`ts-rs` → TS 声明）；
  生成物有 diff 即失败，前端不得手抄 DTO 或错误码 union。
- **生成管线与 CI 闸门**（P1-9）：

  ```bash
  # 1) 改完 Rust wire 类型后重新生成（这条测试会写盘，是唯一允许写仓的测试）
  cargo test -p ark-contract --lib ts_export
  # 2) CI 断言生成物无漂移
  git diff --exit-code web/src/generated
  ```

  根类型清单在 `crates/ark-contract/src/ts_export.rs` 的 `export_roots()` 与 `ROOT_NAMES`：
  **新增 DTO 必须同时登记两处**，否则 barrel 里没有它，前端无法 import。
  `web/scripts/check-hard-rules.mjs` 双向校验：生成文件必须带 ts-rs 头，
  `web/src/shared/contracts/` 不得再声明任何生成器已拥有的类型名。
- **前端只用生成的类型名**，不要造 web 侧别名：`RuntimeConfigView`（不是 `RuntimeConfig`）、
  `SubagentSettings`（不是 `SubagentConfig`）、`ApplyResult`（不是 `ConfigApplyResult`）、
  `ProviderTestResponse` / `McpServerTestResponse`（不是 `*TestResult`）、
  `SkillSummary` / `SkillDiagnostics` / `SkillLoadWarning`、`McpTransportKind`。
  Rust 未建模的东西（请求体组合、路由裸 JSON、UI 文案、`TurnState` 投影）统一放
  `web/src/shared/contracts/web-only.ts`，并在文件头写明“非 Rust 契约”。
- **可执行逻辑**（路径归一化、脱敏、截断、token 估算）必须选定唯一宿主，禁止双实现：
  - 路径归一化：唯一实现 `ark-common::path::normalize`（前端经 WASM 复用同一份源码）；
  - 脱敏、截断：只在 Rust 侧执行，前端经 HTTP / command 调用。
- **ID 全部用 newtype**（`ThreadId` / `TurnId` / `ItemId` / `ChildId`），
  `#[serde(transparent)]`，禁止裸 `String` / `Uuid` 传参。
- **错误码**用 `ArkErrorCode` 枚举 + `http_status()` 映射，
  禁止再维护第二份字符串列表。
- **用量只有一个 `Usage`** 结构，含 provider 原生 cache 字段：
  - `cache_hit_rate` 只能由 `hit / (hit + miss)` 得出；字段缺失时必须为 `None`，
    **绝不允许用 `hit / prompt_tokens` 推算**，也不允许上报伪造的命中率。
  - 计费走 `PricingTable` trait 注入，不在客户端硬编码价格分支。

## 5. 状态与并发（设计 §3、§7.1、§7.9）

- **状态显式化**：回合生命周期、门禁、审批、子代理状态一律用枚举 + 状态机表达，
  非法状态在类型系统里不可构造，不靠字符串约定。
- **取消是语言级能力**：`CancellationToken` + `select!`，不散落 `aborted: bool`。
- **失败必须隔离**：任何 IO 失败（磁盘、子进程、网络）只影响当前操作，
  不允许毒化队列，不允许 panic 冒泡到进程。
- **资源有主**：每个子进程、临时目录、文件句柄都有明确 Rust 所有者，`Drop` 即回收。
- **每条生命周期路径都要收敛到终态**：异步任务被 drop、线程 panic、
  写盘失败，都必须有明确的兜底（同步 `Drop` 守卫 / supervisor / 启动时 `hydrate`）。
  不允许出现“记录停在 running 但没人跑”或“内存 active 但磁盘已死”的中间态。
- **锁顺序固定**：需要多把锁时按 `锁表 → 父级锁 → locator` 的顺序获取，
  锁表 `Mutex` 只用于取 `Arc`，取到后立刻释放；禁止持有内层锁再去申请外层锁。

## 6. 存储（设计 §7.4）

- **会话目录即系统记录**：`children/runs.jsonl`、`events.jsonl` 是权威，
  `index.json` 只是可重建的派生缓存；**授权与归属判断只能读权威日志**，
  不得用派生索引当鉴权来源。
- 子会话内联：`threads/<parent>/children/<child>/`，
  事件 IO 必须经 `FileSessionStore::resolve_paths()`，不得写进伪造的顶层 thread 目录。
- 子 agent 深度 v1 **上限 = 1**；子 agent 的工具集不得包含 `delegate_task`。
- 追加式日志的状态判定 = “每个 child 的最新一行”，历史行保持不变，
  不要要求历史 `queued` / `running` 行变成终态。
- 同一父目录的 append / delete / 索引重建必须在**同一把父级锁**内完成；
  删除是读全量 → 原子替换 → 重建索引，中间被并发 append 插入会丢记录。
- 需要区分“新增写路径”与“既有写路径”：新增写路径必须复用已有加锁入口，
  不要另起一套裸 `fs` 调用。

## 7. Agent 与子代理（设计 §3、§7.8、§7.14）

- **全进程只有一个决策循环**（`ark-agent` 的 reactor）。
  任何功能只能是工具 / 步骤执行器 / 插件，不得再起第二个 agent。
- 需要并行时用**子代理**：同一套 reactor + 独立会话 + 预算约束，不是第二套框架。
- 子代理运行时负责：并发门、排队上限、记录、abort / steer 句柄、父子活动事件。
  执行器只负责“跑一个回合并如实报告”，不得自己写 run 记录或改预算。
- 父回合被中断 **不等于** 子任务被丢弃：子任务必须继续到终态，
  或显式 abort 并落终态，二选一，不能“丢 future 了之”。
- `delegate_task` 只是 `ark_ports::SubagentRunner` 的消费者，
  不得反向依赖 `ark-delegation` / `ark-store` / 模型实现。

## 8. 插件与能力总线（设计 §7.5、§7.7）

- 新增工具、provider、MCP、UI 自动化步骤、后端平台，**只加插件，不改核心**。
  核心代码不得出现 `if backend == "galaxy"` 这类平台分支。
- 插件不得自建 HTTP 客户端 / MCP server / 界面容器，
  只能调用 `PluginContext` 提供的三组能力（`ark-http` / `ark-mcp` / `ark-ui`）。
- 多插件隔离默认强制：`plugin_id` 由宿主注入、不可自报；
  日志带插件名、配置 `{plugin_id}.{key}`、存储/凭据/缓存/工具名全分区；
  manifest 不提供关闭隔离的开关。
- 安全策略、身份、配额、审计**永远归内核**强制，服务实现只提供能力。

## 9. HTTP / SSE 边界（设计 §7.9、附录 A）

- 路由、路径、字段名是对外契约，修改前先确认渲染层与外部集成的影响面。
- GUI 侧字段用 **camelCase**（`parentThreadId`、`resumeCount`），
  查询参数保持既有命名（如增量拉取用 `since_seq`），不要“顺手统一”成另一种风格。
- SSE 的“快照 + 订阅”必须在同一个序列化临界区内完成：
  先定 watermark、再读 backlog、再注册 live channel，且 live channel 有界。
- `child-runs`、`threads` 等**磁盘目录名与 REST 路径保持不变**，
  避免破坏既有数据与外部集成。

## 10. 配置（设计 §7.15）

- 按“最小重启”分级：端口 / 绑定地址 / 数据目录 / token 等底层项需重启，
  其余全部热更新。**分级写在类型上**，不靠注释约定。
- 热更新字段（如 `subagents`）必须每次使用时读取最新快照，
  不得在启动时拷贝一份长期持有。
- 已在运行的任务沿用其创建时捕获的策略，不被中途改动影响。

## 11. 验证要求

按改动范围选择**最小但足够**的验证，并在提交前真实执行：

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace          # 跨 crate / 运行时 / 存储改动必跑
cargo clippy --workspace --all-targets -- -D warnings
```

- 只改文档：至少 `git diff --check`。
- 改契约、存储、并发或组合根：必须跑 `cargo test --workspace`，
  并补一条针对该行为的回归测试（覆盖“出错路径”，不只覆盖 happy path）。
- 不允许把“测试未跑”表述为“测试通过”；基线已有的 warning 单独说明，不要混算。

## 12. Git 与提交信息

- 提交信息**必须用简体中文**，格式为 `[类型]简短描述`：

  ```
  [feat]设置功能开发
  [fix]修复供应商修改bug
  ```

- 类型取值：`feat`（新功能）、`fix`（缺陷修复）、`refactor`（重构）、
  `docs`（文档）、`test`（测试）、`chore`（构建/杂务）、`perf`（性能）。
- 标题说清“做了什么”，不加句号、不加 `[codex]` 之类工具标记。
- 需要正文时说明**动机**与**影响面**（契约变更、数据迁移、行为变化），仍用中文。
- 一次提交只做一件事；不要混入无关格式化或顺手重构。
- 禁止提交构建产物（`target/`、`web/dist/`、`web/node_modules/`）。

## 13. 常见坑

- PowerShell 5.1 `Set-Content -Encoding UTF8` 写 BOM；
  读取 UTF-8 文本要 `Get-Content -Encoding UTF8`。
- `cargo test ... | Select-String` 会让 `$LASTEXITCODE` 失真，
  判断成败要保留完整输出或直接看 `test result:` 行。
- 多 agent 并行改同一工作区会互相覆盖：一个任务一个目录，合并前先看 diff。
- 修改 `ark-contract` 的枚举 / DTO 会同时影响 HTTP、SSE、TS 声明与 MCP schema，
  改完必须重新生成并检查生成物 diff。
