# ark-delegation 模块设计

## 职责与边界

`ark-delegation` 管理子代理的队列、并发槽位、运行记录、abort/steer 句柄和父子活动事件。
它不拥有模型循环；`ChildTurnExecutor` 由 `arkd` 实现并复用 `ark-agent::Reactor`。

## 生命周期与存储

子任务状态为 `queued`、`running` 或终态。权威记录追加到父会话的
`children/runs.jsonl`；`children/index.json` 仅是可重建投影。v1 最大深度为 1，子任务
工具集不得包含 `delegate_task`。

父回合中断不代表子任务被遗弃：运行时必须显式取消子任务、等待/兜底落终态。队列满、执行失败、
持久化异常都只影响该子任务，不得毒化后续队列。

## 验证

覆盖并发门、队列上限、排队任务删除保护、abort/steer、终态写入失败和进程重启后的 hydrate。
