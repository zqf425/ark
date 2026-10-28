# 交互 gate 与活跃回合恢复设计

> 状态：**当前实现**。同一存活 arkd 进程通过 detail runtime 快照和 web seed 恢复可控制 gate/回合
> 视图；重启后遗留 parent turn 在启动时收敛为 aborted。本需求不恢复崩溃前的 reactor。

## 实现进度

- 当前已实现：`ThreadDetail.runtime`、interaction 窄事件查询、live waiter 与持久事件的交集投影，及
  web 在 detail 水位建立 SSE 前恢复 active turn 与待处理 gate。
- 当前已实现：arkd 启动时扫描每个 thread 的 turn 生命周期，对遗留非终态 parent turn 幂等追加
  `TurnAborted`，reason 固定为 `runtime restarted before turn could finish`。

## 目标与非目标

页面刷新、切换线程或短暂 SSE 断线后，Code 页面必须重新获得同一存活进程中的活跃回合、待审批和待输入
的权威快照，使用户能继续 Steer、终止或处理 gate。

不包含重启后恢复模型调用、工具执行、SteerQueue 或 oneshot waiter；`InteractiveGates` 和
`TurnRegistry` 是进程内控制对象，重启后不可伪造为仍可操作。

## 当前实现

`ThreadDetail.runtime` 返回当前 `TurnRegistry` 的 active turn，以及由 interaction 窄事件查询和 live
waiter id 交集得到的待处理 gate。web 在 detail 的 `last_seq` 建立 SSE 前 seed 该快照；后续状态仍由 SSE
terminal/resolved 事件推进。事件日志仍是唯一持久化真源，已含 `turn_*`、`approval_*`、`user_input_*`
与 `turn_steered`。

## 已批准方案

### Wire

`ark-contract` 新增并嵌入 `ThreadDetail.runtime`（serde default 的加法字段）：

- `active_turn: Option<TurnSummary>`；仅反映 `TurnRegistry` 中当前存活的回合；
- `pending_approvals: Vec<PendingApprovalView>`；
- `pending_user_inputs: Vec<PendingUserInputView>`。

gate view 必须带稳定 gate id、turn id、请求内容与请求时间；不含 sender、工具参数、凭据或模型上下文。
老 JSON 缺少 `runtime` 时反序列化为空，生成 TypeScript 后由 web 使用生成类型。

### 权威与并发

`SessionStore` 增加窄 `replay_interaction_events(thread)`，只返回回合终态与 gate 生命周期事件，按 seq 排序。
它从 `events.jsonl` 投影，不创建第二份 gate 真源。`ark-server` 用该投影计算“request 后尚未 resolve 且所属
live active turn”的候选 gate，再以 `InteractiveGates` 的 live id 快照交集过滤。HTTP 的 resolve 仍只向
存活 oneshot sender 发送；历史事件绝不能使已取消/重启的 gate 可提交。

detail 与随后 SSE 水位允许有短暂竞态：SSE 的 resolved/terminal 事件补齐状态；POST 若 waiter 已移除返回
既有 404。web 不把 204 当作权威 resolved，仍等待 SSE 或下次 detail。

### 生命周期

- 同一存活 arkd：刷新 detail 恢复 active turn 与仍等待的 gate；Steer 进入该 turn 的既有队列；中断取消
  reactor，Drop 移除 waiter，reactor 持久化唯一终态。
- arkd 重启：reconcile 后扫描每线程的最新非终态 parent turn，一次性追加 `TurnAborted`，reason 为
  `runtime restarted before turn could finish`；不得追加 gate resolved、重放工具/模型或重建 waiter。
- 正常 shutdown 先按既有流程取消并等待；下次扫描必须幂等，已终态回合不再写入。

## 模块责任

| 模块 | 责任 |
|---|---|
| `ark-contract` | runtime/gate wire DTO 与 TS 导出 |
| `ark-ports` / `ark-store` | interaction 窄事件查询、启动收敛所需枚举 |
| `ark-server` | detail 投影、thread 归属校验、HTTP adapter |
| `ark-agent` | 不变；仍是唯一 reactor 与 gate 事件写入者 |
| `arkd` | 启动收敛与 live registry/gate 装配 |
| `web` | detail seed、SSE 增量、只渲染 server 给出的可操作 gate |

## 验收与验证

覆盖旧 detail JSON、DTO/TS 导出、gate request/resolve/terminal 投影、跨线程 interrupt 拒绝、刷新后
Steer/interrupt/gate 操作、resolve 与取消竞争、restart 单次收敛和正常 shutdown 幂等。此变更涉及契约、
存储、并发与组合根，实施完成必须执行 workspace 测试、fmt/check/clippy、契约生成检查及真实浏览器刷新。
