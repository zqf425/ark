# 后台子代理完成通知设计

> 状态：**当前实现，待真实交互验收**。默认后台委派、完成通知、completion inbox 与默认开启的自动续跑均已实现；真实浏览器和模型调用路径的最终验收由 [发布收尾与真实环境验收](release-readiness.md) 承接。
> 派发默认已改为**不阻塞主会话**：`subagents.default_background`（Hot，默认 `true`）使 `delegate_task`
> 在模型未显式传 `background` 时立即返回 queued（见 modules/ark-tool.md 与 modules/ark-delegation.md）。
> 显式用户输入还会收到动态委派策略：实质编码、调查和多步骤执行默认拆给后台子代理，主会话负责计划、
> 用户沟通与最终汇总。该策略是模型行为约束，不是绕过模型的强制派发器。

## 目标

后台子代理创建后立即返回 child id，主回合可继续工作或结束。子代理达到终态后，系统必须可靠地将
结果通知父线程、实时界面和后续主回合，而不是要求用户轮询 child-runs。

## 范围

- 复用 `ark-delegation` 已有的 runtime-owned supervisor、取消、steer、`runs.jsonl` 记录和
  `child_activity` 事件。
- 为后台派发的终态 child 增加带稳定通知 ID 的父线程完成通知；通知是持久化、幂等、可在启动时补投递的。
- 默认 `autoContinue`：父线程没有活动 turn 时，按父线程批量把完成摘要作为系统来源输入交给
  新 continuation turn，无需用户再次提示。`subagents.auto_continue` 默认开启，显式关闭后回到仅通知模式。

不在本需求中实现 Studio、外部插件、跨会话消息、任务市场或多层子代理。

## 默认委派策略

`serve` 的显式 User、IM 与 AutoRecovery 父回合在子代理启用且 `default_background=true` 时注入一条动态
system 策略：实质任务默认后台委派，独立工作可并行拆分；主会话只做计划、沟通和汇总，不重复执行 child
工作。简单对话、澄清与微小查询直接处理，用户明确要求阻塞结果时才允许前台委派。AutoRecovery 重发的是
已有用户请求，因此沿用同一策略；它不创建新的委派幂等键，也不能保证模型跨恢复尝试不重复派发。

该消息位于动态 seed 尾部，不进入父/子共享的静态 prompt-cache 前缀。headless、Scheduled 与
AutoContinuation 不注入：headless 单回合结束即 shutdown，不能遗弃晚到的 child；completion 是待汇总结果，
不得被当成新任务再次委派。child 工具集仍不含 `delegate_task`，最大深度保持 1。

## 事件与持久化

终态记录仍先追加到父目录的 `children/runs.jsonl`。记录中的 `notify_parent` 持久化派发模式：后台派发与
HTTP resume 为 `true`，同步 `delegate_task` 为 `false`。前台 child 的最终答案已经由工具结果返回给当前
父模型，不再发布 `child_completed`，避免自动 continuation 重复消费和重复汇总。旧记录缺少该字段时按
`true` 读取，以保留既有后台通知的恢复语义。

后台终态记录写入后，运行时向父线程追加 `child_completed` 事件，包含：`notificationId`、child id、label、
终态、摘要/错误、用量、完成时间。`notificationId` 由 child id 和该次终态记录唯一决定，消费者按其去重。

两个日志之间不能有跨文件事务，因此采用至少一次投递：启动 hydrate 扫描所有 `notify_parent=true` 的终态
child，发现父线程缺少相同 `notificationId` 的完成事件时补发。重复事件可出现，但 UI、inbox 和 continuation
必须幂等。

阶段 2 在父事件日志中以 `child_completion_acknowledged` 记录某个通知已被一个**正常完成**的父回合
消费。确认写入失败或父回合失败/中断时不丢弃通知，下一个回合会再次投递；因此语义为至少一次而非跨
模型调用的伪“精确一次”。

事件压缩不得越过最早的未确认 `child_completed`：该记录及其后的事件保留，直到对应确认落盘。若保留的
重复完成事件依赖更早的确认记录，也必须一并保留该确认。这样压缩不破坏至少一次投递；已经确认且不再
被保留事件引用的旧通知仍可随普通历史一起折叠。

## 主线程收件箱与 continuation

完成事件投影为父线程 completion inbox。新的用户回合开始前读取未确认条目，并以“子代理结果”
系统上下文提供给模型，不能伪造为用户消息。每次最多投递 8 条，超过部分保持未确认并留待后续回合，
以免完成风暴突破上下文预算；每条摘要或错误最多 1,200 UTF-8 字节。只传递 child 的摘要或错误，
不复制原始转录。

父线程详情只返回最新 50 条不同 `notification_id` 的完成摘要，作为有界 wire/read model。当前 web 不把
该字段折叠为主画布摘要卡片，也不在 reducer 中保留 50 条副本。实时 `child_completed` 只显示一次 toast；
持久查看由 `ChildRunsPanel` 重新读取 child-runs API。

`autoContinue` 仅在 `subagents.enabled=true` 且 `subagents.auto_continue=true`、父线程无活动 turn、
未删除/归档且没有已调度 continuation 时触发。关闭 subagents 或 auto-continue 会立即取消进程内待调度
continuation，不影响已开始回合。`auto_continue` 的默认值及旧配置缺失值均为 `true`：默认配置在后台 child
完成后会产生额外的父模型调用，用于自动汇总；显式 `false` 才恢复仅通知、不自动调用模型的模式。
关闭期间到达的通知不被丢弃，也不静默：server 在该线程确有未确认通知时记一条带原因的 `INFO`
（`auto_continue_disabled` / `subagents_disabled` / `automatic_followups_disabled`），重新打开后由
策略扫描补投。同一父线程
250 ms 内的多个完成通知合并为一个 continuation；活动父回合期间到达的通知保留一个待调度槽，等回合
结束后再复核持久 inbox。用户中断、删除或归档父线程时取消待执行 continuation，并抑制迟到 child
completion，直到下一次显式用户回合（或取消归档）开启新的生命周期。该抑制从父线程持久终态恢复，
因此重启不会让一次用户中断失效；运行时重启时合成的 abort 不视为用户意图。shutdown 先拒绝新 wakeup
再取消待调度任务。

自动 continuation 使用普通父 `TurnRunner` 和同一个 `Reactor`，只把 completion inbox 作为 system
source 注入，不创建伪用户消息；模型预算、provider、workspace、工具沙箱、审批/用户输入 gate 与正常父
回合完全相同。仅正常完成后确认本次注入的 notification；失败、中断或确认写入失败仍保留至少一次投递。

每个实际启动的 continuation 回合都必须在父线程事件日志中留下完整的生命周期：一条 `turn_started`
（保留 `auto_continuation` 标记）和一条终态记录（`turn_completed`/`turn_failed`/`turn_aborted`）。
supervisor 负责关闭 reactor 覆盖不到的路径（panic、任务被丢弃、`prepare()` 之前的 runner 错误）：
若日志中缺少该回合的起始记录则先补写再写终态，若已有终态则不再重复写入。`turn_failed` 始终带可读
reason，供熔断与审计共用。
若本批最多 8 条全部确认成功且仍有未确认通知，调度下一批；确认失败不自行链式重试——该句约束的是
**确认写入失败的补偿**（不靠 supervisor 链式重试写确认），而非续跑调度上限。自动续跑另有两条有界
终止条件（当前实现）：

1. **同通知尝试上限**：同一 `notification_id` 的自动续跑投递达到 2 次后，自动路径不再投递该通知，
   也不再为它调度 continuation；通知保持未确认，真实用户回合仍可消费。尝试计数从父线程事件日志
   推导（自动 `turn_started` 事件上的 `auto_continuation` 加法字段），不新增存储文件，跨重启恢复；
   用户回合开启新生命周期后计数归零。
2. **模型失败熔断**：连续 2 次自动续跑因模型请求失败（`turn_failed` 上的 `model_request_failed`
   加法字段，网关 5xx 等模型请求错误）而终止时，暂停该线程的自动续跑并记录诊断日志，不产生新模型
   调用；用户回合或其他非模型终态事件重置连续计数。

同时，completion inbox 的注入文案带明确的边界语义：通知是已完成工作的上下文，不得因通知重新发起
子代理或重复执行任务。

## 交付阶段

当前实现保留 `child_completed` 的父线程持久事件、completion inbox 与自动续跑，但 web 展示边界是
“实时 toast + 右侧 `ChildRunsPanel`”。以下阶段记录按当前展示宿主描述，不把 wire read model 等同于
主画布卡片。

- **阶段 1（当前实现）**：`child_completed` 作为父线程持久化 SSE 事件发布，带终态摘要和稳定通知 ID；
  hydrate 在事件缺失时补投递，并避免重复投递。
- **阶段 2（当前实现）**：将事件投影为主线程 completion inbox，在下一次用户回合前以系统来源上下文提供
  给模型；正常完成后写入确认事件。`ThreadDetail.child_completions` 保持有界 read model；web 对实时
  完成事件只显示一次 toast，持久查看走 `ChildRunsPanel`，不恢复主画布卡片。
- **阶段 3（当前实现）**：由 per-thread supervisor 合并通知并启动普通父 continuation；
  `subagents.auto_continue` 默认开启，可显式关闭；生命周期取消与 shutdown 收敛已实现。

## 影响模块

| 模块 | 变更 |
|---|---|
| `ark-contract` | 完成通知、消费确认事件、`SubagentSettings.auto_continue` 和 TypeScript 生成 |
| `ark-ports` | `SessionStore::replay_child_completion_events` 提供窄查询；`ChildRunRecord.notify_parent` 持久化结果回传模式 |
| `ark-store` | 权威父事件查询与确认事件持久化；压缩保护未确认通知 |
| `ark-delegation` | 终态后发布完成通知，恢复时幂等补发 |
| `ark-agent` | 将 inbox 结果作为系统来源输入 |
| `ark-server` | SSE/查询接口、详情摘要上限 50 条、per-thread debounce 与生命周期取消 |
| `web` | `child_completed` 实时 toast；`ChildRunsPanel` 持久摘要与跳转，不在主画布保留 50 卡片 |
| `arkd` | 配置、默认委派动态提示词、supervisor、continuation 装配 |

## 验收

1. 显式 User、IM、AutoRecovery 在子代理启用且默认后台时收到动态委派策略；headless、Scheduled、
   AutoContinuation、禁用子代理和关闭默认后台时不注入，静态 prompt-cache 前缀不变。
2. `delegate_task` 未传 `background` 时按默认 `true` 在 child 排队后立即返回，父 turn 不等待。
3. `background: false` 等待最终答案但不发布 `child_completed`，不触发额外 continuation；该模式在重启后
   仍不补发通知。后台派发与 HTTP resume 仍发布通知。
4. 后台 child 终态后父 SSE 收到一次可去重的完成通知，含摘要或错误。
5. 重启发生在终态记录与父通知之间时，hydrate 补投递；重复 hydrate 不重复事件；旧记录缺少
   `notify_parent` 时保持后台通知语义。
6. 未确认通知作为系统来源输入，而非伪造用户消息；只有父回合正常完成后才确认，确认失败必须允许下一回合
   重投递。
7. 多 child 完成只调度一个合并 continuation；超过 8 条时仅在前批确认成功后有界排空；已中断、删除或归档
   的父线程不得自动 continuation，且中断抑制跨重启保持。
8. 同一通知的自动续跑投递达到 2 次后停止为它调度 continuation，通知保持未确认供用户回合消费；连续 2 次因
   模型请求失败终止的自动续跑暂停该线程的自动续跑并记录诊断，不产生新模型调用；用户回合仍可消费未确认
   通知，且重置两条计数。与「确认失败不自行链式重试」正交：后者指确认写入失败的补偿。
9. 默认配置下后台完成会产生自动 continuation，但仍走原审批和沙箱；显式 `auto_continue=false` 时不产生
   自动模型调用，通知保留到下一次输入。设置页中英文默认说明与运行时值一致。
