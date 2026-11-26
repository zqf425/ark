# 工具并发声明（parallel-tools）

## 背景与问题

一轮模型回复可以携带多个 tool_calls。执行链路是
`step::dispatch → ParallelPolicy::plan → reactor::dispatch → dispatch::run_calls`：
`run_calls` 用 `join_all` 并发跑**同一批内**的调用，批次之间在 reactor 里顺序 `await`。

改变之前，批次划分来自 `ParallelPolicy` 里一份硬编码名字白名单
`["read", "grep", "find", "ls", "web_fetch"]`，不在表里的工具一律按变更类独占一批。
后果是模型同轮发出的 3 条 `bash` 被拆成 3 个各含 1 调用的批次，串行执行：
实测 3 条 `sleep 3` 耗时 9 秒。执行器层没有瓶颈，`BashTool` 每次直接 `spawn` 一个
`sh -c` 子进程，彼此不共享内存。

白名单还有两个结构性缺陷：

- 插件与 MCP 工具永远进不来：核心不可能预知它们的名字，只能一律独占；
- 分类口径错位：`bash` 被判"变更"是因为它可能写工作区，但"可能写工作区"与
  "两个调用会互相破坏"是两件事，前者由沙箱决策管，后者才决定能否并批。

## 目标与非目标

- 目标：不牺牲正确性的前提下，让**确实不会互相破坏**的调用共享一个批次，
  并把分类来源从核心的名字白名单改为工具自述。
- 非目标：不做工作区隔离快照，不做跨批调度，不改批次之间顺序执行的语义
  （回合历史必须按模型给出的顺序落盘）。

## 设计

`ark_ports::ToolConcurrency` 是端口层的自述枚举，与 `SandboxDecision` 正交：

| 取值 | 含义 | 并批 |
| --- | --- | --- |
| `ReadOnly` | 不写共享状态 | 可与资源不同的调用并批 |
| `Isolated` | 写共享状态，但每次调用独占进程/独立回合 | 同上 |
| `Exclusive` | 未知或必须串行（默认） | 独占一批 |

`Tool::concurrency()` 默认 `Exclusive`，所以未表态的工具保持最保守行为；
`ToolRegistryPort::concurrency(name)` 让 agent 不必认识具体实现。
`ark-tool` 的注册表在注册时缓存声明，组合根建回合时调用
`ToolRegistry::concurrency_table()` 取一份快照，经 `ReactorConfig.concurrency` →
`TurnInput.concurrency` → `ParallelPolicy::with_table()` 传入纯策略。快照而不是活视图，
保证回合中途的插件注册不会改变一个已规划批次的形状。

并批规则（`ParallelPolicy::plan`）保持原有的资源判定：两个调用能并批，当且仅当
双方都非 `Exclusive` 且 `resource`（`path`/`file_path`/`filePath`/`target`）不相同。
`bash_session` 虽每次独占进程，但共享 manager 的 cwd 状态，因此 `ToolCallRequest::resource()`
为它返回固定 key `shell_session`，同批调用按批顺序串行，避免 cwd 交错；普通 `bash` 没有资源参数，
仍可彼此并批。`bash` 只与“无冲突”这一侧成立——这正是它必须显式声明 `Isolated` 才可并批的原因：
没有声明就没有并批资格。

内置声明：`read`、`ls`、`find`、`grep`、`web_fetch`、`bash_output` 为 `ReadOnly`；
`bash`、`bash_session`、`bash_kill`、`delegate_task` 为 `Isolated`；`write`、`edit`、
`request_user_input` 之外的未声明工具为 `Exclusive`（`request_user_input` 在派发前被拦截，
声明为 `ReadOnly` 只是让携带它的计划仍可成形）。

## 与沙箱的关系

声明不放宽任何权限。`bash` 在 `WorkspaceWrite` 与 `Strict` 下仍需审批，在 `Unattended` 下
拿到 `Unrestricted`；它在三种情况下都能并批，因为并发安全性来自"每条调用独占一个进程"，
与沙箱决策无关。反过来，`write` 即使在 `Unattended` 也仍是 `Exclusive`：两个并发写入
同一工作区会互相覆盖，沙箱放行不代表竞态消失。

## 限流只有一层

`tuning.max_parallel_tool_calls > 0` 时 `ParallelPolicy` 把批切成顺序 chunk，
这是唯一的批内并发上限。执行器不再额外持有信号量：两层互不感知的限流会取到一个
没有声明的交集（曾经出现配置 `0`（不限）被硬编码 `8` 卡住的情况）。

## 验收

- `ParallelPolicy` 单测：3 条 `bash` 同批；bash 与 read 混排同批；同路径 read 串行；
  `write` 独占；两条 `bash_session` 的 cwd 变更不交错且普通 bash 不被连带串行；插件工具可经表声明为
  `Isolated`；表内缺失的名字仍独占。
- `ark-tool` 回归：3 条 `sleep 1` 的 `bash` 在 `join_all` 下总耗时显著低于串行的 3 秒。
- 全仓 `cargo test --workspace --exclude ark-desktop`、`cargo clippy --workspace
  --exclude ark-desktop --all-targets -- -D warnings` 通过。