# 后台子代理完成通知设计

## 目标

后台子代理创建后立即返回 child id，主回合可继续工作或结束。子代理达到终态后，系统必须可靠地将
结果通知父线程、实时界面和后续主回合，而不是要求用户轮询 child-runs。

## 范围

- 复用 `ark-delegation` 已有的 runtime-owned supervisor、取消、steer、`runs.jsonl` 记录和
  `child_activity` 事件。
- 为终态 child 增加带稳定通知 ID 的父线程完成通知；通知是持久化、幂等、可在启动时补投递的。
- 默认 `notifyOnly`：SSE/UI 显示完成摘要，不产生额外模型调用。
- 预留 `autoContinue`：当父线程没有活动 turn 时，按父线程批量把完成摘要作为系统来源输入交给
  新 continuation turn；启用必须显式配置。

不在本需求中实现 Studio、外部插件、跨会话消息、任务市场或多层子代理。

## 事件与持久化

终态记录仍先追加到父目录的 `children/runs.jsonl`。随后向父线程追加 `child_completed` 事件，包含：
`notificationId`、child id、label、终态、摘要/错误、用量、完成时间。`notificationId` 由 child id 和
该次终态记录唯一决定，消费者按其去重。

两个日志之间不能有跨文件事务，因此采用至少一次投递：启动 hydrate 扫描所有终态 child，发现父线程
缺少相同 `notificationId` 的完成事件时补发。重复事件可出现，但 UI、inbox 和 continuation 必须幂等。

## 主线程收件箱与 continuation

完成事件投影为父线程 completion inbox。新的用户回合开始前读取未确认条目，并以“子代理结果”
系统上下文提供给模型，不能伪造为用户消息。

`autoContinue` 仅在显式启用、父线程无活动 turn、未删除/归档且没有已调度 continuation 时触发。
同一父线程短时间内多个完成通知必须合并为一个 continuation；用户中断、删除或归档父线程时取消待执行
continuation。自动 continuation 仍受模型预算、工具沙箱和审批约束，不能自动获得用户授权。

## 交付阶段

- **阶段 1（本次）**：`child_completed` 作为父线程持久化 SSE 事件发布，带终态摘要和稳定通知 ID；
  hydrate 在事件缺失时补投递，并避免重复投递。
- **阶段 2（后续）**：将事件投影为主线程 completion inbox，在下一次用户回合前以系统来源上下文提供
  给模型。
- **阶段 3（后续）**：在显式 `autoContinue` 配置下，由 per-thread supervisor 合并通知并启动 continuation。

## 影响模块

| 模块 | 变更 |
|---|---|
| `ark-contract` | 完成通知 DTO/事件和 TypeScript 生成 |
| `ark-ports` | completion inbox/调度所需端口 |
| `ark-store` | 父事件查询、通知投影与 hydrate 补投递 |
| `ark-delegation` | 终态后发布完成通知，恢复时幂等补发 |
| `ark-agent` | 将 inbox 结果作为系统来源输入；后续 continuation trigger |
| `ark-server` | SSE/查询接口与错误映射 |
| `web` | child 完成提示、摘要和跳转动作 |
| `arkd` | 配置、supervisor、continuation 装配 |

## 验收

1. `background: true` 在 child 排队后立即返回，父 turn 不等待。
2. child 终态后父 SSE 收到一次可去重的完成通知，含摘要或错误。
3. 重启发生在终态记录与父通知之间时，hydrate 补投递；重复 hydrate 不重复事件。
4. 阶段 2 必须把未确认通知作为系统来源输入，而非伪造用户消息。
5. 阶段 3 的多 child 完成只调度一个合并 continuation；已中断、删除或归档的父线程不得自动 continuation。
6. 默认配置不产生自动模型调用；自动 continuation 仍走原审批和沙箱。
