# ark-schedule 模块设计

> 状态:**已批准计划**(尚未实现;落地前本文件即实现语义)。需求边界见
> [定时任务需求](../requirements/scheduled-tasks.md)。

## 职责与边界

`ark-schedule` 是定时任务的纯调度域:任务与触发节奏的类型、归一化、`next_run_at` 计算、
到期判定和运行记录的有界折叠。它不认识文件系统、HTTP、tokio 或回合执行——存储经
`ark_ports::ScheduleStore` 端口,执行由 `ark-server` 的调度服务编排。

依赖:`ark-common`、`ark-contract`、`ark-ports`。不依赖任何实现 crate,也不被实现 crate 依赖
(`ark-server` 只消费其公开 API)。

## 关键约束

- `compute_next_run_at(task, from)` 是唯一下一次触发决策点:本地时区;`manual`、已触发的
  `at`、禁用任务返回空;`interval` 从 `from`(执行完成时刻)起算;`weekly`/`monthly` 逐日
  扫描至多 366/31 天,空列表按需求回退;非法 `HH:mm` 回退 09:00。
- 归一化(`normalize_task`)是创建/更新路径的唯一入口:clamp `every_minutes` 到
  `[1, 525600]`、截断标题与提示词到契约上限、任务数超限返回结构化错误。
- 运行历史有界(最近 10 条),折叠为最新 `last_run` + 历史列表;跳过(misfire)也记录。
- 全部函数为纯函数或对端口 trait 的薄封装;无 `tokio::fs`、无时钟直读——"现在"一律由
  调用方传入,保证可测。

## 验证

`cargo test -p ark-schedule --lib`:全节奏 next_run 用例(含跨日/跨月/回退)、归一化、
历史有界;与 `ark-contract` 的 ts_export 一致性由契约 crate 测试承接。
