# ark-agent 模块设计

## 职责与边界

`ark-agent` 是全进程唯一的模型决策循环。它只依赖 `ark-ports` 定义的模型、工具、事件、
会话和闸门端口；模型调用、文件、网络和持久化实现由 `arkd` 注入。

## 回合状态机

回合使用显式状态而非布尔标记：`Preparing`、`AwaitingModel`、`DispatchingTools`、
`AwaitingApproval`、`AwaitingUserInput`、`Compacting`、`Finalizing` 和终态
`Completed`/`Failed`/`Aborted`。`step(state, input) -> Action` 保持纯逻辑，`Reactor`
是唯一把 action 转为 I/O 的位置。

取消由 `CancellationToken` 与 `select!` 完成；任何退出路径都必须写终态并清理活动注册，
不得留下“磁盘记录 running 但任务已丢失”的状态。

## 关键约束

- 子代理不是第二个 agent：它是同一 `Reactor` 驱动的独立 child turn。
- 工具沙箱决策只在派发前计算一次，执行器仅消费不可变结果。
- 预算、工具风暴、并发安全和历史裁剪属于独立 policy/context 模块，不得回流为巨型 reactor。
- 每轮模型可见历史必须已经落盘；每个工具调用必须与终态结果配对。

## 验证

状态迁移和 policy 均应脱离 I/O 单测；取消、审批、工具失败、模型流失败和持久化失败须覆盖
收敛到终态的路径。
