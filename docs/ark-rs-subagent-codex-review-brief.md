# 任务：审查 ark-rs 子 agent（delegation）实现，输出 P0/P1 问题清单

## 你的角色

你是资深 Rust + 分布式系统代码审查者。请**只读**审查，不要修改任何文件，不要运行会写盘的命令。

## 仓库与路径

- 仓库根（git worktree）：`D:\code\github\smart-ark-wt-rust-rewrite`
- Rust 工作区：`D:\code\github\smart-ark-wt-rust-rewrite\ark-rs`
- 设计文档（唯一权威，约 2400 行）：`D:\code\github\smart-ark-wt-rust-rewrite\docs\rust-rewrite-design.md`
- 旧 TS 实现（行为参考，产品语义真源）：`D:\code\github\smart-ark-wt-rust-rewrite\kun\src\`
  - 重点：`kun\src\delegation\delegation-runtime.ts`、`kun\src\delegation\child-agent-executor.ts`、`kun\src\server\routes\child-runs.ts`、`kun\src\adapters\tool\delegation-tool-provider.ts`

## 本次实现的功能范围（只审查这些）

新实现"子 agent / delegation"，对应设计文档 §7.8、§7.14、附录 A 的 `child-runs` 路由族与 `delegate_task`。涉及文件：

**新 crate `ark-rs\crates\ark-delegation\`**
- `src\domain\gate.rs` —— 并发槽位 + 等待队列（SlotGate/SlotGuard/acquire_error/ERR_QUEUE_FULL）
- `src\domain\mod.rs` —— ChildTurnContext / ChildTurnReport / ChildTurnStatus / StopReason / finished_record
- `src\service\runtime.rs` —— DelegationRuntime：prepare / prepare_child / run_child / execute / delegate、abort/steer、child_activity 事件、known/running/prepared 三张表
- `src\service\runtime\tests.rs`、`src\domain\gate.rs` 内联测试

**端口 `ark-rs\crates\ark-ports\src\subagent.rs`**
- SubagentRequest / SubagentOutcome / SubagentRunner（delegate / prepare_child / run_child）/ SubagentPolicy / SubagentPolicySource / SubagentRegistry / CancelHandle / SteerHandle

**存储 `ark-rs\crates\ark-ports\src\store.rs` + `ark-rs\crates\ark-store\src\`**
- `ChildRunRecord`（含 prompt/summary/error/usage/tool_invocations/resume_count）
- `SessionStore::create_child_run / append_child_run / list_child_runs / delete_child_run`
- `ark-store\src\store\child_store.rs` 的 `remove_child_run_blocking` / `remove_child_dir_blocking`
- `ark-store\src\service\thread_ops.rs` 的 `delete_child_run` 实现
- `ark-store\src\service\store.rs` 的 `resolve_paths`（子会话寻址）

**工具 `ark-rs\crates\ark-tool\src\exec\tools\delegate.rs`（+ `delegate\tests.rs`）**
- `DelegateTaskTool`：只依赖 `Arc<dyn SubagentRunner>`，不依赖 store/agent
- `ark-tool\src\exec\tools\mod.rs` 的 `delegation_tools()`（子工具集不含 delegate_task）
- `ark-tool\src\domain\{schema,args,meta}.rs` 相关增量

**HTTP `ark-rs\crates\ark-server\src\api\handlers\child_runs.rs`（+ `child_runs\tests.rs`）**
- list / get / abort / steer / resume / remove / thread / events
- `ark-server\src\state.rs` 的 `SubagentServices` + `AppState::with_subagents`
- `ark-server\src\api\routes.rs` 中 child-runs 路由从 pending 切到真实 handler
- `ark-contract\src\dto.rs` 的 ChildRunSummary / ChildRunListResponse / ChildRunAggregates / ChildRunActionResult / ChildRunEventsResponse

**组合根 `ark-rs\apps\arkd\src\child.rs` + `wiring.rs`**
- `ChildTurnExecutorImpl`：用同一个 reactor 跑子会话，cancel/steer 桥接，`observed_counts`、`final_answer`、`limits_of`
- `ConfigSubagentPolicy`：每次 spawn 读实时配置
- `wiring.rs` 里 `build_child_tools()` / `build_tools()` 的构造顺序

## 审查必须回答的问题

请**逐条**核对设计文档原文（引用章节号），不要凭印象。重点：

### A. 是否符合设计文档的硬约束
1. §7.8.0「只保留一个 agent / 只保留一个决策循环」——实现里是否真的没有第二个 agent loop？`ChildTurnExecutorImpl` 是否复用了同一个 `Reactor`？有没有隐藏的 `tokio::spawn` 里跑第二个循环？
2. §7.4.1「会话目录即系统记录」——子会话是否严格内联在 `threads/<parent>/children/<child>/`？`children/runs.jsonl` 是否仍是唯一权威？`index.json` 是否只当派生缓存、未被用于鉴权？
3. v1 `max_depth = 1`——现在靠"子工具集移除 delegate_task"+"store 层 depth 校验"双保险，是否真的不可绕过？如果模型的工具调用直接点名 `delegate_task` 会怎样（子 reactor 的 schema 里没有它，registry 也找不到）？
4. §7.9 事件/SSE——`child_activity` 发到**父** stream 是否与设计一致？失败时是否只 warn 不 fail？
5. §7.15 ReloadClass——`subagents` 是 Hot 字段，`ConfigSubagentPolicy` 每次 spawn 读快照是否正确？有没有被缓存住的地方？
6. 附录 A 路由契约——8 条 child-runs 路由的方法/路径/`parentThreadId` 要求是否与文档和旧实现一致？状态码是否用 `ArkErrorCode::http_status()` 而非手写？

### B. 并发与正确性（最容易出 P0/P1 的地方）
7. `SlotGate::acquire/release` 的槽位转移语义：waiter 被唤醒时"releaser 把槽位转给它、active 不变"，这条不变量在 `release()` 里有 `while let Some(waiter) = pop_front()` 跳过已死 waiter 的逻辑——**跳过死 waiter 后 active 是否仍正确？** 请逐路径推演：正常转移 / waiter future 被 drop / gate 被 drop / panic。
8. `DelegationRuntime::execute` 里如果 `relabel(RUNNING)` 返回 Err（磁盘写失败），`guard`、`running` entry、`active` 计数分别会怎样？会不会泄漏槽位或留下永远 `queued` 的记录？
9. `running` / `known` / `prepared` 三张表的加锁顺序是否可能死锁？有没有在持锁时 await？
10. `prepare_child` → `run_child` 的两段式：如果 HTTP resume 之后进程崩了，`prepared` 表里的条目是否会导致记录永远停在 `queued`？启动对账是否会处理这种情况？
11. `delegate_background` / `prepare_child` 与并发门的关系：`prepare` 阶段不占槽位，`execute` 才占。那么"排队上限 `max_queued`"是否真的生效？会不会出现 N 个 `queued` 记录但 gate 里 0 个 waiter 的情况？

### C. 子会话与数据一致性
12. `resolve_paths` 依赖内存 `ChildLocator`。**进程重启后 locator 是空的**，子会话的 `replay_events` / `read_items` 会不会落到错误的 `threads/<child_id>` 路径？`reconcile_index` 是否真的会 `absorb_parent_blocking` 填满 locator？重启后立刻访问 child-runs/thread 会怎样？（这条我怀疑是 P0，请重点验证并给出结论）
13. `delete_child_run` 的顺序：先移除 runs.jsonl 记录 → 移除目录。如果中途失败，会不会出现"记录已删、目录残留"或反过来？orphan 目录是否有对账清理？
14. `remove_child_run_blocking` 用 `write_atomic` 重写整个 runs.jsonl。**并发安全吗？** 如果同时有 `append_child_run_blocking` 在追加，会不会丢记录？（其他 append 走的是文件尾部追加，没有文件锁）

### D. 工具与边界
15. `DelegateTaskTool::render` 的 `ok` 语义：aborted 算成功（`is_usable`），failed 算失败但返回 `ToolOutput{ok:false}` 而非 `Err`。与 §7.3「工具错误转成 ok:false 内容」是否一致？模型能否从 `ok:false` 正确恢复？
16. `child_runs.rs` 的 `ensure_owned`：**每次请求都调 `registry.get()`，走内存表**。如果该 child 是别的进程/重启前创建的，`get()` 返回 None → 一律 404。这会不会让"历史子会话永远打不开"？（结合第 12 条）
17. `resume` handler：先 `runner.prepare_child()` 再 `tokio::spawn(runner.run_child())`。如果 spawn 的任务在 `run_child` 里 panic，记录会停在 `queued` 吗？谁负责兜底？
18. `events` / `thread` handler 在 `subagents.registry` 为 None 时**跳过 ownership 检查**，直接 `state.store.replay_events(child_thread_id)`。这是否构成越权读取（任意 UUID 都能试）？

### E. 可维护性 / 与既有代码风格一致性
19. 是否有与仓库既有风格不一致的地方（错误处理、命名、注释语言、公共 API 形状、`ark-ports` 冻结约束）？
20. 是否有过度设计（多余抽象层、为不存在的需求预留的字段/方法）？是否有重复实现（同一规则在 store / delegation / server 各写一遍）？

## 输出格式（严格遵守）

把结论写到文件：`D:\code\github\smart-ark-wt-rust-rewrite\docs\ark-rs-subagent-codex-review.md`

> 注意：该目录当前你的沙箱是 `read-only`，如果写盘失败，请改为**直接在最终回复里输出完整 Markdown**，我会自己落盘。

格式：

```markdown
# ark-rs 子 agent 实现审查

## 结论摘要
（3-5 句：整体是否可接受，P0 几条、P1 几条）

## P0（必须修，否则不能上线）
### P0-1 <一句话标题>
- **位置**：`相对路径:行号`
- **问题**：具体描述，引用设计文档章节号
- **为什么是 P0**：会导致什么后果（数据丢失 / 死锁 / 越权 / 记录永远卡住）
- **建议修法**：具体到函数/字段级别

## P1（应该修，影响正确性或可维护性）
（同上格式）

## P2 / 仅供参考
（一句话一条，不展开）

## 与设计文档的偏差点清单
| 设计条款 | 实现现状 | 是否合规 |
|---|---|---|
```

## 硬性要求

- **每条结论必须给出 `文件:行号`**，行号用你实际读到的行号，不要猜。
- **区分"违反设计文档"和"违反工程常识"**：前者要引用章节号，后者要说明为什么。
- **不要罗列 P2 凑数**。如果 P0/P1 各只有 2 条，就只写 4 条。
- 优先看并发、槽位泄漏、重启后状态、鉴权边界这四类，它们最可能是真 P0。
- 可以运行 `cargo check -p ark-delegation` 之类的只读编译命令来确认类型约束，但**不要 `cargo test` 以外的写操作**，也不要改文件。
