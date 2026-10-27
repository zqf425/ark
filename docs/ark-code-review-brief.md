# ark 项目代码审查简报（供 codex）

## 你的角色

你是资深 Rust / 分布式系统审查者。请**只读**检视 `D:\code\github\ark` 仓库的代码，
对照设计文档找出**真实的缺陷**，不要做风格偏好类建议。

## 背景

`D:\code\github\ark` 是一个 Rust workspace，按 `docs/rust-rewrite-design.md`（v0.6）
从旧的 Electron/TypeScript 版 `smart-ark`（原 `kun`）重写而来。
旧实现位于 `D:\code\github\smart-ark`（含 `kun/src`），**只作为行为参考**，不要照搬它的设计。

### 仓库结构

```
crates/            19 个 crate
  ark-common       错误、ID newtype、稳定序列化、UTF-8 截断
  ark-contract     跨进程 wire 契约（HTTP/SSE/MCP/TS 共享 DTO）
  ark-ports        端口 trait（含 SessionStore / EventSink / ModelClient / Tool / SubagentRunner 等）
  ark-config       配置加载 + 热更新分级
  ark-store        会话目录读写（JSONL 权威 + 可重建索引）
  ark-agent        ★唯一决策循环：reactor + 回合状态机
  ark-llm          provider 适配、流解析、usage、重试
  ark-tool         工具注册表、内置工具、沙箱、截断
  ark-http         出站 HTTP 总线
  ark-ui           界面总线
  ark-mcp          MCP 出口与总线
  ark-plugin       插件注册表
  ark-skills       skill 发现与激活
  ark-provider     provider 存储与探测
  ark-workspace    工作区校验与 git 状态
  ark-delegation   子 agent 运行时（并发门、记录、abort/steer）
  ark-studio       UI 自动化插件
  ark-obs          可观测性
  ark-server       axum 路由 / SSE / 鉴权
apps/arkd/         组合根（wiring / child / model / cli / workspace / base）
web/               React + TS 渲染层
docs/              设计文档与历史审查报告
```

## 必读材料（按顺序）

1. `docs/rust-rewrite-design.md` —— 设计唯一真源。重点章节：
   - §3 设计原则（12 条）
   - §5.1 crate 内部分层、§5.2 依赖方向、§5.3 组合根
   - §6 契约层（数据契约 vs 可执行逻辑、错误码、ID newtype、Usage/cache）
   - §7.1 Agent Loop 状态机、§7.3 工具系统、§7.4 存储、§7.8 子 agent、§7.9 HTTP/SSE、§7.15 配置热更新
   - §10.3 代码质量门禁
2. `AGENTS.md` —— 从设计文档提炼的开发规范（依赖方向、分层、并发、存储、验证要求）。

## 审查重点（按严重度）

请对以下维度逐项核查，并明确指出**代码位置（文件:行号）**与**为什么是缺陷**：

### A. 依赖方向与分层（§5.2 / §5.3 / AGENTS.md §2）

- `ark-agent` 是否只依赖 `ark-ports`，有没有偷偷依赖 `ark-llm` / `ark-tool` / `ark-store` 实现。
- `ark-contract` 是否混入了内部 request/query 类型或端口 trait。
- 是否存在跨 crate 穿透内部模块的引用。
- 组合根是否只在 `apps/arkd/src/wiring.rs`，有没有把业务分支写进组合根。

### B. 状态与生命周期收敛（§3 / §7.1 / AGENTS.md §5）

- 是否存在"记录停在 running 但没人跑"或"内存 active 但磁盘已死"的中间态。
- 异步任务被 drop / panic / 写盘失败时，是否有兜底（Drop 守卫、supervisor、启动 hydrate）。
- 取消是否用 `CancellationToken` + `select!`，有无散落的布尔标志。

### C. 存储正确性（§7.4 / AGENTS.md §6）

- `children/runs.jsonl`、`events.jsonl` 是否为唯一权威；`index.json` 是否被误用作鉴权依据。
- 同一父目录的 append / delete / 索引重建是否在同一父级锁内；有无并发丢记录窗口。
- 子会话是否内联在 `threads/<parent>/children/<child>/`，事件 IO 是否经 `resolve_paths()`。
- 子 agent 深度是否严格限制为 1；子工具集是否移除 `delegate_task`。
- 锁顺序是否固定（锁表 → 父级锁 → locator），有无 ABBA 死锁风险。

### D. HTTP / SSE 契约（§7.9 / 附录 A / AGENTS.md §9）

- 路由路径、字段命名（camelCase vs snake_case）是否与旧实现/渲染层一致。
- `since_seq` 等查询参数是否正确解析。
- SSE "快照 + 订阅" 是否在同一临界区完成，live channel 是否有界。
- 单 child 访问是否强制校验 `parentThreadId` 归属；registry 缺失时是否 fail-closed。

### E. Usage 与计费（§6.4 / AGENTS.md §4）

- `cache_hit_rate` 是否只用 `hit / (hit + miss)`；字段缺失时是否为 `None`。
- 有没有用 `hit / prompt_tokens` 推算，或上报伪造命中率。
- 是否有第二份"投影结构"。

### F. 取消 / panic / 并发（§7.8 / AGENTS.md §5）

- 子任务 abort / steer 能否作用到排队中与运行中的 child。
- worker panic 是否被捕获并落终态。
- 后台一轮结束后，cancel/steer bridge 是否确实停止。
- 并发门（max_concurrency / max_queued）的排队与释放是否有泄漏或饥饿。

### G. 工具与沙箱（§7.3）

- 沙箱决策是否只在调度前计算一次并随 `ToolContext` 传入。
- 文件路径是否统一经 `ark_common::path::normalize` 并做 workspace 包含校验。
- 工具错误是否以 `ok: false` 内容交还模型，而不是让回合失败。

### H. 代码规模与质量门禁（§10.3）

- 单文件是否 ≤ 800 行、单函数是否 ≤ 80 行（列出超限文件）。
- `lib.rs` 是否有 `pub use *`。
- 有无明显死代码 / 未使用导出。

## 输出要求

请在仓库中创建文件 `docs/ark-code-review-2026-10-27.md`，用**简体中文**书写，结构如下：

```markdown
# ark 代码审查报告（2026-10-27）

## 结论摘要
（一段话：整体是否可上线，几个 P0，几个 P1）

## P0（必须修，否则不能上线）
### P0-1 <标题>
- 位置：`文件:行号`
- 问题：
- 为什么是 P0：
- 建议修法：

## P1（应该修，影响正确性或可维护性）
### P1-1 <标题>
（同上结构）

## P2 / 仅供参考
（简要列表，不展开）

## 与设计文档的偏差点清单
| 设计条款 | 实现现状 | 是否合规 |
|---|---|---|
| ... | ... | ... |
```

硬性要求：

- 每条缺陷必须给出**具体文件与行号**，不接受"某处"这类模糊表述。
- 必须区分"确实是缺陷"与"与旧实现不同但符合设计"——后者不要报成缺陷。
- 不要修改任何源码（你是只读审查）。只允许创建上面那一个报告文件。
- 如果某个维度没发现问题，明确写"未发现"。
- 报告写完后，在最终回复里用不超过 15 行总结：P0 数量、P1 数量、最严重的 3 条。
