# ark-delegation 模块设计

## 职责与边界

`ark-delegation` 管理子代理的队列、并发槽位、运行记录、abort/steer 句柄和父子活动事件。
它不拥有模型循环；`ChildTurnExecutor` 由 `arkd` 实现并复用 `ark-agent::Reactor`。

子代理的运行上下文由父会话继承：工作空间、Provider、模型和权限默认采用父会话该轮生效的设置；
`delegate_task` 明确给出的 Provider、模型或 `autonomy` 权限覆盖优先。工作空间不接受模型指定，
始终由父工具上下文传入。显式权限覆盖会写入 child run 记录，以便恢复同一子代理时仍然生效。

## 生命周期与存储

子任务状态为 `queued`、`running` 或终态。权威记录追加到父会话的
`children/runs.jsonl`；`children/index.json` 仅是可重建投影。v1 最大深度为 1，子任务
工具集不得包含 `delegate_task`。

父回合中断不代表子任务被遗弃：运行时必须显式取消子任务、等待/兜底落终态。队列满、执行失败、
持久化异常都只影响该子任务，不得毒化后续队列。

终态记录写入后，运行时向父线程发布带稳定通知 ID 的 `child_completed` 事件，供 SSE/UI 和后续
主线程收件箱消费。hydrate 会补发缺失通知，但先扫描父事件以避免重复投递。

## 验证

覆盖并发门、队列上限、排队任务删除保护、abort/steer、终态写入失败和进程重启后的 hydrate。
