# 定时任务（Scheduled Tasks）

> 状态:**已批准计划**(2026-11-01)。本文是需求设计;实现落地后按各阶段更新"落地情况"。
> 推翻 [headless-run.md](headless-run.md) 早期"定时任务明确不在 ark 范围"的边界决定:
> 定时任务自本需求起纳入 ark 范围,云端执行仍不在范围。

## 背景与目标

smart-ark 在桌面端主进程实现了定时任务(`src/main/schedule-runtime.ts` + 管理界面),可按
manual/at/interval/weekly/monthly 五种节奏周期性向运行时发起一个无人值守回合。ark 作为重写
版需要等价能力:用户配置一个提示词与触发节奏,arkd 到点自动建会话执行,并把结果留在可打开的
会话线程里。

对标结论(取自 smart-ark,按 ark 架构裁剪):

| smart-ark 概念 | 本需求取舍 |
|---|---|
| 任务存应用设置 JSON | 独立 `<data-dir>/schedules.json`(运行态高频更新,不进 config) |
| 主进程 setInterval tick | arkd 进程内调度循环(ark-server service 层) |
| 经 HTTP 自调用建线程+回合 | 进程内复用 start_turn 全部校验路径(同 continuation 先例) |
| disableUserInput | 线程 runtime `autonomy=unattended` 覆盖(gate 永不等待无人) |
| worktree 池 / dependsOn / 优先级 | v1 不做,见"非目标" |
| agent 经 MCP 创建任务 | v1 不做 |

## 范围

### 数据模型(wire 契约,ark-contract)

```text
ScheduledTask {
  id, title, enabled
  prompt                       # 每次运行发送的用户消息
  workspace                    # 可选;空 = 使用全局默认工作区
  autonomy                     # strict | workspace_write | unattended;默认 unattended
  model, provider_id           # 可选;空 = 运行时默认
  schedule {                   # 五种触发节奏,与 smart-ark 对齐
    kind: manual|at|interval|daily|weekly|monthly
    every_minutes              # interval
    time_of_day "HH:mm"        # daily/weekly/monthly
    at_time ISO8601            # at(一次性)
    day_of_week: [0..6]        # weekly,0=周日
    day_of_month: [1..31]      # monthly
  }
  created_at, updated_at
  last_run { status, message, thread_id, started_at, finished_at }  # 可空
  next_run_at                  # 空串 = 不调度(manual/禁用/一次性已完成)
}

ScheduledRunRecord = last_run 的形状,另存最近 10 条历史(有界,随任务持久化)。
ScheduleSettings { enabled }   # config.schedules,ReloadClass::Hot
```

- 归一化规则与 smart-ark 一致:非法 `HH:mm` 回退 09:00;weekly 空星期回退周一;monthly 空
  日期回退 1 号;`every_minutes` clamp 到 `[1, 525600]`;任务数上限 100,超限拒绝创建。
- `next_run_at` 在本地时区计算;`manual` 与已触发的 `at` 为空串。

### 调度与执行(ark-server service + arkd 装配)

1. 调度循环随 arkd serve 启动,tick 间隔 30s;单次 tick 串行执行所有到期任务(不并发,v1)。
2. 到期判定:`enabled && next_run_at <= now`;执行后立刻按节奏计算下一次 `next_run_at`
   (interval 从本次执行完成时刻起算,与 smart-ark 一致)。
3. **执行 = 新线程 + 真实回合**:复用 `start_turn` 的注册/冲突/文本上限/workspace_gate 全部
   校验,提示词为任务 `prompt`,不伪造消息类型;线程 runtime 覆盖 `autonomy`(任务字段)、
   `provider_id`/`model`(任务字段,空则缺省)。无人值守语义由 `autonomy=unattended` 的
   sandbox/gate 路径保证——审批与 user-input gate 永不阻塞等待。
4. **停机期间错过的触发不补跑**:启动或 tick 恢复时,`next_run_at` 已过去的任务直接推进到
   下一个未来时点,`last_run` 记一条 `skipped`;`at` 一次性任务过期即标记完成不再触发。
5. 结果折叠:回合终态事件到达后写 `last_run` 与有界历史(成功/失败/中止/跳过 + 助手末条文本
   摘要);运行中的任务再次到期时跳过本次 tick 并顺延(不排队并发)。
6. shutdown:调度循环随全局 shutdown 收敛取消,不遗留 in-flight 记账。

### HTTP API(ark-server)

- `GET /v1/schedules` 全量列表;`POST /v1/schedules` 创建;`PATCH /v1/schedules/{id}` 更新
  (含启停);`DELETE /v1/schedules/{id}` 删除。
- `POST /v1/schedules/{id}/run` 立即运行一次(不等 next_run_at;运行中返回 409)。
- 以上全部经 wire DTO 生成前端类型(`ts_export`);错误复用 `ArkErrorCode` 结构化语义
  (不存在 404、超限 422、冲突 409)。

### 配置(web 设置页)

- 设置页新增「定时任务」section:任务列表(标题、节奏、启停开关、最近结果、打开线程)、
  新建/编辑表单(标题、提示词、节奏、autonomy、model/provider、工作区)、立即运行按钮。
- 全局开关 `schedules.enabled` 关闭时调度循环空转,列表只读可编辑不触发。
- 全部文案走 `web/src/shared/i18n/` 消息键与中英文资源。

## 模块落位

| 职责 | 宿主 | 依据 |
|---|---|---|
| 任务/节奏类型、归一化、next_run 纯函数 | 新 crate `ark-schedule` | 纯域逻辑,无 I/O,独立单测;被 server 与 contract 共用 |
| `ScheduleStore` 端口(trait) | `ark-ports` | 实现由组合根注入,域逻辑不认识文件系统 |
| `ScheduleFileStore`(`schedules.json`,Mutex 串行 + tmp+rename 原子写) | `ark-store` | 与 AttachmentStore/MemoryFileStore 同模式 |
| wire DTO + ts_export | `ark-contract` | Rust 类型是 wire 唯一真源 |
| 调度循环、执行编排、路由 | `ark-server`(`service/scheduler.rs` 等) | 与 continuation supervisor 同层,复用注入的 TurnRunner 与 gate 语义 |
| 装配(store、循环启动、shutdown 挂钩) | `apps/arkd` | 唯一组合根 |
| 管理界面 | `web` 设置页 | 只消费生成契约 |

依赖方向新增:`ark-schedule → ark-common, ark-contract, ark-ports`;
`ark-server → ark-schedule`;其余不变,无循环。

## 非目标(后续需求另立项)

- 云端执行/云端下发任务(smart-ark 的云端派发链路)。
- 任务依赖链(dependsOn)、优先级队列、并发执行、worktree 池隔离。
- agent 经 MCP 工具创建/管理任务;自然语言识别"定时做 X"(smart-ark 的 detector)。
- workflow 图(n8n 式多节点编排)。
- 补跑(misfire catch-up)与持久化调度账本。

## 验收

- 单元:next_run 全节奏用例(含 DST 语义按本地时区逐日扫描、非法输入归一化)、到期判定、
  历史有界、store 原子写与损坏恢复。
- 集成:假执行器下 tick 触发→线程创建→prompt 写入→终态回写 last_run;禁用/删除/409/
  上限拒绝;shutdown 收敛。
- 契约:`cargo test -p ark-contract --lib ts_export` 后 `web/src/generated` 零 diff。
- 端到端:真实模型下创建 interval 任务→自动运行→设置页看到最近结果并可打开线程(发布验收)。

## 落地情况

阶段 1(调度域与存储)已实现:`ark-schedule` 的归一化/next_run/到期判定/历史有界纯函数、
`ark-ports::ScheduleStore` 与 `ark-store::ScheduleFileStore`(原子写、损坏恢复)。

阶段 2(调度与执行 + HTTP API)已实现:

- `crates/ark-server/src/service/scheduler.rs`:`reconcile`(崩溃遗留 `running` → `aborted`、停机期间错过的
  触发 → `skipped`)、30 秒 tick 的 `Due`/`Misfire`/`NotDue` 分类、`InFlightGuard` 串行化与
  `previous run was still in flight` 跳过、`begin` 建线程并写 runtime 覆盖(`autonomy` 缺省
  `unattended`)、`drive` 复用 `start_turn` 并以轮询取终态、从完成时刻重算 `next_run_at`。
- 一次运行只有一条历史:`running` 只写 `last_run`,有界历史只存终态;`reconcile` 的 `aborted` 也折进历史。
- `POST /v1/schedules/{id}/run` 在主开关关闭或任务仍在运行时返回 `Conflict`/409;主开关
  (`config.schedules.enabled`,`ReloadClass::Hot`)只影响触发,不影响 CRUD。
- 路由 `/v1/schedules`(GET/POST)、`/v1/schedules/{id}`(PATCH/DELETE)、`/v1/schedules/{id}/run`(POST)。
- 组合根 `apps/arkd/src/wiring/mod.rs` 用同一 `data_dir` 创建 `ScheduleFileStore` 并 `spawn` 调度循环;
  `Ctrl-C`/drain 这类只置标志位的关闭路径由 tick 顶部的 `is_shutting_down()` 收敛。
- 测试:`cargo test -p ark-server --lib` 155 passed。

阶段 3(管理界面)已实现:设置页「定时任务」section(列表/启停/新建编辑/删除/立即运行/
打开对应会话)、全局开关、节奏摘要纯函数、i18n 中英资源与回归测试(web 136 用例全绿,
typecheck 与硬规则通过)。

尚未实现:端到端真实模型验收(发布验收承接)。
