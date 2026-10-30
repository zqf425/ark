# 后台子代理完成通知设计

## 目标

后台子代理创建后立即返回 child id，主回合可继续工作或结束。子代理达到终态后，系统必须可靠地将
结果通知父线程、实时界面和后续主回合，而不是要求用户轮询 child-runs。

## 范围

- 复用 `ark-delegation` 已有的 runtime-owned supervisor、取消、steer、`runs.jsonl` 记录和
  `child_activity` 事件。
- 为终态 child 增加带稳定通知 ID 的父线程完成通知；通知是持久化、幂等、可在启动时补投递的。
- 默认 `notifyOnly`：SSE/UI 显示完成摘要，不产生额外模型调用。
- `autoContinue`：当父线程没有活动 turn 时，按父线程批量把完成摘要作为系统来源输入交给
  新 continuation turn；通过 `subagents.auto_continue` 显式启用，默认关闭。

不在本需求中实现 Studio、外部插件、跨会话消息、任务市场或多层子代理。

## 事件与持久化

终态记录仍先追加到父目录的 `children/runs.jsonl`。随后向父线程追加 `child_completed` 事件，包含：
`notificationId`、child id、label、终态、摘要/错误、用量、完成时间。`notificationId` 由 child id 和
该次终态记录唯一决定，消费者按其去重。

两个日志之间不能有跨文件事务，因此采用至少一次投递：启动 hydrate 扫描所有终态 child，发现父线程
缺少相同 `notificationId` 的完成事件时补发。重复事件可出现，但 UI、inbox 和 continuation 必须幂等。

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

父线程详情只返回最新 50 条不同 `notificationId` 的完成摘要；web 同样只保留 50 个摘要卡片，避免长寿命
线程的响应或渲染状态无界增长。

`autoContinue` 仅在 `subagents.enabled=true` 且 `subagents.auto_continue=true`、父线程无活动 turn、
未删除/归档且没有已调度 continuation 时触发。关闭 subagents 会立即取消待调度 continuation；默认值和
旧配置缺失值均为 `false`，因此升级不得产生额外模型调用。同一父线程
250 ms 内的多个完成通知合并为一个 continuation；活动父回合期间到达的通知保留一个待调度槽，等回合
结束后再复核持久 inbox。用户中断、删除或归档父线程时取消待执行 continuation，并抑制迟到 child
completion，直到下一次显式用户回合（或取消归档）开启新的生命周期。该抑制从父线程持久终态恢复，
因此重启不会让一次用户中断失效；运行时重启时合成的 abort 不视为用户意图。shutdown 先拒绝新 wakeup
再取消待调度任务。

自动 continuation 使用普通父 `TurnRunner` 和同一个 `Reactor`，只把 completion inbox 作为 system
source 注入，不创建伪用户消息；模型预算、provider、workspace、工具沙箱、审批/用户输入 gate 与正常父
回合完全相同。仅正常完成后确认本次注入的 notification；失败、中断或确认写入失败仍保留至少一次投递。
若本批最多 8 条全部确认成功且仍有未确认通知，调度下一批；确认失败不自行链式重试，避免同一通知造成
无界模型调用。

## 交付阶段

- **阶段 1（本次）**：`child_completed` 作为父线程持久化 SSE 事件发布，带终态摘要和稳定通知 ID；
  hydrate 在事件缺失时补投递，并避免重复投递。
- **阶段 2（本次）**：将事件投影为主线程 completion inbox，在下一次用户回合前以系统来源上下文提供
  给模型；正常完成后写入确认事件。`ThreadDetail` 恢复持久摘要卡片，web 对实时完成事件显示一次性提示。
- **阶段 3（本次）**：在显式 `subagents.auto_continue` 配置下，由 per-thread supervisor 合并通知并
  启动普通父 continuation；默认关闭，生命周期取消与 shutdown 收敛已实现。

## 影响模块

| 模块 | 变更 |
|---|---|
| `ark-contract` | 完成通知、消费确认事件、`SubagentSettings.auto_continue` 和 TypeScript 生成 |
| `ark-ports` | `SessionStore::replay_child_completion_events` 提供窄查询，避免回放完整事件历史 |
| `ark-store` | 权威父事件查询与确认事件持久化；压缩保护未确认通知 |
| `ark-delegation` | 终态后发布完成通知，恢复时幂等补发 |
| `ark-agent` | 将 inbox 结果作为系统来源输入 |
| `ark-server` | SSE/查询接口、详情摘要上限 50 条、per-thread debounce 与生命周期取消 |
| `web` | child 完成提示、摘要和跳转动作；摘要卡片上限 50 条 |
| `arkd` | 配置、supervisor、continuation 装配 |

## 验收

1. `background: true` 在 child 排队后立即返回，父 turn 不等待。
2. child 终态后父 SSE 收到一次可去重的完成通知，含摘要或错误。
3. 重启发生在终态记录与父通知之间时，hydrate 补投递；重复 hydrate 不重复事件。
4. 阶段 2 必须把未确认通知作为系统来源输入，而非伪造用户消息；只有父回合正常完成后才确认，确认失败
   必须允许下一回合重投递。
5. 阶段 3 的多 child 完成只调度一个合并 continuation；超过 8 条时仅在前批确认成功后有界排空；已中断、
   删除或归档的父线程不得自动 continuation，且中断抑制跨重启保持。
6. 默认配置不产生自动模型调用；自动 continuation 仍走原审批和沙箱。
