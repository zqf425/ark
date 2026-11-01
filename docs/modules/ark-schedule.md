# ark-schedule 模块设计

> 状态:**已实现**(`crates/ark-schedule` + `ark-server` 的路由与调度循环 + web 设置页)。
> 本文件即实现语义。需求边界见 [定时任务需求](../requirements/scheduled-tasks.md)。

## 职责与边界

`ark-schedule` 是定时任务的纯调度域:任务与触发节奏的类型、归一化、`next_run_at` 计算、
到期判定和运行记录的有界折叠。它不认识文件系统、HTTP、tokio 或回合执行——存储经
`ark_ports::ScheduleStore` 端口,执行由 `ark-server` 的调度服务编排。

依赖:`ark-common`、`ark-contract`、`ark-ports`(外加 `chrono`,因为 `DateTime<Local>` 就是
契约里已有的时间类型,且"现在"是纯函数签名的一部分)。不依赖任何实现 crate,也不被实现 crate
依赖——`ark-server` 只消费其公开 API。

## 类型:不重复定义

`ScheduledTask`、`TaskSchedule`、`ScheduledRunRecord`、`RunStatus`、`ScheduleKind` 以及
`CreateScheduleRequest`/`UpdateScheduleRequest` 都在 `ark-contract` 定义,本 crate 只
`pub use` 转出。原因很直接:落盘的 `schedules.json`、HTTP 载荷和 `web/src/generated` 都由契约
crate 生成,在这里再写一份同形状结构,正是三者漂移的开始。

`RunStatus` 刻意不派生 `Default`:"默认运行结果"没有语义,派生会为一个从未运行过的记录随手挑定
一个终态。取值 `running`/`completed`/`failed`/`aborted`/`skipped` 对齐运行时的
`turn_completed`/`turn_failed`/`turn_aborted` 词汇,`skipped` 是 misfire 标记。

## 归一化:修复优先

`normalize_task` / `normalize_schedule` 是创建与更新路径的唯一入口,策略是"修复"而非"拒绝":
clamp `every_minutes` 到 `[1, 525600]`、非法 `HH:mm` 回退 `09:00`、weekly 空星期回退 `[1]`、
monthly 空日期回退 `[1]`、标题与提示词截断到契约上限。拒绝整条任务只会留给用户一个被禁用的
功能和零线索。

只有三处返回结构化错误(而不是静默修复):空提示词、`at` 节奏缺少可用时间、未知 autonomy 取值
(`AUTONOMY_VALUES = ["strict", "workspace_write", "unattended"]`)。任务数达到
`MAX_SCHEDULED_TASKS = 100` 时 `ensure_capacity` 返回 `validation_error`,detail 为
`schedule_limit`。

`HH:mm` 解析是严格的:`9:00` 和 `0900` 都是手改文件或半填表单的产物,悄悄接受会让落盘值
与用户看到的不一致。

不属于当前 `kind` 的字段仍然会被归一化,这样在 daily/weekly 之间切换任务时不会翻出陈旧的非法值。

## next_run_at:唯一的触发决策点

`compute_next_run_at(task, from: DateTime<Local>) -> Option<DateTime<Local>>`

- `manual`、禁用的任务、已触发的一次性 `at` 返回 `None`(落盘为空串)。
- `at` 按 RFC3339 解析后存为 UTC;残留偏移不合法即视为不可调度。
- `interval` 从 `from`(即执行完成时刻)起算,而非从上次计划时刻——否则一次长跑会让后续触发
  立刻堆积。
- `daily`/`weekly`/`monthly` 在**本地时区**逐日扫描:weekly 上限 8 天,monthly 上限 366 天。
  逐日扫描而不是月内取整,是因为 `day_of_month = 31` 在二月没有对应日期,需求要求跳到下一个
  有该日期的月份(而不是滑到月末)。

配套:`is_due` 只信任可解析的已存时间戳,`apply_next_run_at` 把结果写回 `next_run_at`,
`format_next_run_at`/`parse_stored_next_run` 负责空串与 RFC3339 之间的双向转换。

## 运行历史:有界环

历史保存在任务文档内部(最近 `SCHEDULE_RUN_HISTORY_LIMIT = 10` 条,最旧在前),不是独立日志:
设置页要回答的是"这个任务最近怎么样",不是审计。`record_run` 同时追加历史并镜像到
`last_run`——两处必须一起写,否则 UI 会显示一个日志里不存在的状态。misfire 的 `skipped` 与
其它结果一样记录。

## 验证

`cargo test -p ark-schedule --lib`(29 个用例)覆盖:全节奏 `next_run`(含同日、跨日、跨月、
weekly 扫描上限、monthly 无该日期时跳过)、归一化与回退、任务上限、历史有界与 `last_run`
一致性。`cargo clippy -p ark-schedule --all-targets` 无告警。与 `ark-contract` 的 ts_export
一致性由契约 crate 的 `regenerate_typescript_bindings` 测试承接。
