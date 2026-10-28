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
- 每个批次在执行器启动前持久化全部 `ToolCallStarted` 已校验参数，`ToolCallFinished` 持久化受工具上限限制的输出和 detail；这既是
  会话重放资料，也是服务端 Changes/File/Terminal 投影的唯一来源，不能由 web 解析自由文本补造。
- 同一回合的正文 delta 与 reasoning delta 使用各自稳定的 item ID；两类内容不得合并为同一个渲染条目，
  以保证默认折叠的 reasoning 不会遮蔽用户可见正文。
- 新回合须从权威线程项恢复此前的 user/assistant 对话，并在回合启动时持久化当前用户消息；reasoning 不进入
  后续模型历史。会话级工作空间、模型和权限覆盖只在创建新回合时读取，运行中回合保留其创建快照。
- parent steer 的有界 `SteerQueue` 由 server registry 创建并由 `arkd` 注入同一个 reactor；reactor 只在
  轮次边界 drain、写 `turn_steered` 后把文本加入用户历史。
- 子代理完成 inbox 从父线程权威事件投影为带标记的 system source，而非用户消息；按稳定通知 ID 去重，
  跳过已确认项，并且每个父回合最多交付 8 条、每条最多 1,200 UTF-8 字节，避免完成风暴突破上下文
  预算。

## 验证

状态迁移和 policy 均应脱离 I/O 单测；取消、审批、工具失败、模型流失败和持久化失败须覆盖
收敛到终态的路径。
