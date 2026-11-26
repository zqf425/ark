# 插件事件钩子总线（Hook Bus）

> 状态：**已批准计划（未实施）**。定位裁定为"开放面追平 cordis、治理纪律保留"（见决策记录）；
> 分四期实施，P1 不改变任何现有行为。头脑风暴与调研底稿在 `.tmp/plugin-hook-redesign.md`
> （不随代码提交）。
>
> 决策记录（2026-11-03）：D1=定位 B（开放面追平 cordis + 治理纪律保留）；D2=Decide/Supply 钩子
> 仅对 `ark.builtin.*` 内置插件开放，digital-life 类内核以内置插件形态落地，第三方 Decide 插件
> （wasm / loopback webhook）后续独立立项；D3=保留审计查询面（不砍为纯 tracing）。

## 当前实现

- 插件能力面：工具（`<plugin_id>.<tool>` 经 McpBus）、UI page/panel、受治理 HTTP、config/KV
  （内存态）、files（磁盘持久）、credentials、notify。`Plugin` trait 仅 `manifest` + `register`。
- 插件拿不到：事件流（`HostHandles.events` 恒 `None`）、定时器、回合/模型层钩子。
- 已开放的等效通道（HTTP API）：`POST /v1/threads/{id}/turns`、turn steer、`/v1/schedules`
  CRUD+run、`/v1/threads/{id}/events`（SSE）、`/v1/approvals/{id}`、`/v1/child-runs/{id}/steer`。
  插件经 HttpCapability 可调用 arkd 自身 API；缺口主要是归因、配额与钩子面。
- 与接线相关的事实（实施时必须处理）：
  - 回合入口有三条路径：`start_turn_as`（`ark-server/src/service/turns.rs:48`，唯一带 gate）、
    `continuation.rs:377` 与 `api/handlers/turns.rs:418` 两处直接 `TurnJob` + `try_register`。
  - child run 使用 `DenyAllGates`（未知 MCP/plugin 工具即时 synthetic denial，不注册、不阻塞），
    其工具目录按回合求值并经过与父路径相同的 `tool_catalog.filter` hook；父路径同样每回合求值。
  - `EventSink::subscribe` 是有界 mpsc（满则丢最新、不可重连），不能直接交给插件。

## 需求边界

### 原则

1. 一切皆插件：arkd 只装配与开放基础能力，不为具体插件写特判。
2. 不深植内核：钩子经端口与装饰器开放，`ark-agent` 零改动；`EventKind`/session log/SSE 语义不动。
3. 已有通道优先：开回合、定时、读写数据走 HTTP API 与 continuation 既有机制；插件自持久化
   （遵守插件目录规范），平台不做 KV 托底。
4. 零回归：新能力全部 manifest 显式声明（`serde(default)`）；无声明 = 无句柄 = 行为与现状一致。
5. ark-agent 仍是唯一模型决策循环：插件不得直调模型 API，只能经"开回合"通道驱动 agent。

### 钩子模型

事件与钩子两个命名空间：`EventKind`（封闭枚举 + ts-rs 生成物）继续只承载"已发生的事实"，
**本次改造一字不动**；钩子是独立命名空间 `HookPoint`，带 `hook_abi` 版本号，默认不落 session
log、不进 SSE。

钩子四级：

| 级别 | 语义 | 失败语义 | 投递保证 |
|---|---|---|---|
| Observe | 事后通知 | 无影响 | 可丢；payload 带 `seq`，水位重放补正确性（至少一次，插件需幂等） |
| Decide | 同步窄改写 | 影响本次调用 | 不可丢、不重试、必须给结论 |
| Supply | 宿主取值 | 取不到用缺省 | 可降级 |
| Effect | 插件请宿主做事 | —— | **不设 Effect 总线**；插件"请宿主做事"的通道是封闭清单：注册工具、followup、HTTP API、notify |

### v1 钩子面

- **Observe**：`turn.started/completed/failed/aborted`（`turn.completed` 携带末条 assistant 文本
  尾 ≤2 KiB）；`llm.call.completed/.failed`（usage、latency、finish_reason、截断输出尾 ≤2 KiB）；
  `tool.call.completed/.failed`（截断结果头 ≤1 KiB）；`approval.resolved`。
- **Decide**（同步、硬预算、窄改写）：
  - `llm.request.decide`：仅改 temperature / max_tokens、增删动态尾、Deny；不可改 messages 与
    tools schema。
  - `tool.pre_call.decide`：仅 `Block(reason)` / `RequireApproval` / `NarrowSandbox`；不改参数。
  - `turn.pre_start.decide`：可拒绝新回合；返回独立错误码 `PluginDenied`（不得复用
    `ModelRequestFailed`，否则毒化 auto-continuation 熔断逻辑）。
  - `approval.pre_resolve.decide`：只能收紧。放宽（自动批准）是独立能力，后续立项。
- **Supply**：
  - `prompt.sections`：seed 求值一次、回合内冻结；固定顺序
    `[static][memory][skills][plugin sections 按 plugin id 排序]`；每段带 `[plugin:<id>]` 标签。
  - `tool_catalog.filter`：在组合根组装 `ModelSeed.tools` 时求值（reactor 与模型装饰器不碰）；
    纯函数；带不可过滤名单（`load_skill`、`delegate_task` 等宿主核心工具）；**父与子两个组装
    站点都接**。
  - `approval.policy`：仅 `Abstain | RequireHuman`。
- **明确不做**：`llm.delta`（每 token 回调）、`item.*` 粒度、`tool.post_call` 输出改写、
  `context.pressure`、任何"订阅全部事件"的泛化钩子。

### Decide 调用纪律

- 按点定预算：`turn.pre_start` 500ms、`llm.request.decide` 100ms、`tool.pre_call` 150ms；每回合
  总预算 2s，超预算后本回合所有 Decide 直接 Abstain + 降级诊断。
- 超时/异常 = Abstain（fail-open）+ 响亮诊断 + 连续 N 次超时熔断；熔断 = **撤销句柄**，不是
  跳过调用。仅 manifest 显式 `required: true` 的点 fail-closed。
- 不重试；拓扑序调用、首个 Block 短路；payload 带 `origin: plugin:<id>` + `depth`，同一插件对
  同一回合深度 >1 不再回调。
- 钩子调用点结构性排除全部锁：绝不持有 `workspace_gate`、`TurnRegistry` 内部锁、EventSink
  writer 锁时回调插件。
- 失败可见：Deny/改写必须可审计（插件、钩子点、thread/turn、决策、延迟、diff）。

### Observe 投递

不把 `EventSink::subscribe` 直接给插件。宿主为每个插件懒启动 per-thread 订阅任务，走既有
`subscribe(thread, since_seq)`（backlog+live 原子拼接、无空洞）；插件把水位存进自己的 state
文件，重启后续传。有界、按插件隔离、drop-oldest 由该订阅任务实现。

### 回合与定时

- **followup**：不新增开回合 API。`CompletionInbox` 增加 `plugin` 来源条目，搭 continuation
  supervisor 的车（`wait_until_idle`、去抖、可取消、用户接管即抑制）。**空闲优先、不做抢占**：
  插件回合只在 workspace 空闲时启动，用户回合结构上永不与之并存。新增内部
  `TurnSource::Plugin { plugin_id, reason }`；wire 用 `serde(default)` 可选 `origin` 字段。
- **配额**（每插件）：每小时回合数、并发、单回合 token 上限；准入门在 inbox 投递处。
- **定时**：进程内插件 `on_tick`（宿主 ticker 回调，默认不注册不回调）；声明 autonomy 的插件
  才可开回合。跨重启 cron 语义后续用 `/v1/schedules` + `owner: plugin:<id>` 命名空间。

### 权限与安全

- **按钩子点授权**：manifest 新增独立 `hooks` 段（`#[serde(default)]`）逐点声明；不扩 `Capability`
  枚举。`PluginEntry` 加加法可选字段 `hook_points: Vec<String>`，装插件前 GUI 可见。
- **句柄即授权**：`ctx.hooks()` 返回类型化句柄，每方法对应一个已授权点；无授权 = 无句柄；
  声明与注册不符复用 `plugin_declaration_mismatch` 拒绝。钩子句柄必须进 `Registration`
  （Drop 即反注册，防卸载/熔断后幽灵回调）。
- **信任模型（威胁模型声明）**：v1 的 Decide/Supply 仅 `ark.builtin.*` 内置插件；第三方插件只有
  Observe + 工具 + HTTP 通道。Decide 级插件等同于宿主代码，安装即授予进程内执行权。
  即便内置插件也做调用边界 `catch_unwind`（panic → Abstain + 熔断 + 诊断；Mutex 中毒按既有
  `PoisonError::into_inner` 写法）。已知残余风险：阻塞型占用 tokio worker 无法用 catch_unwind
  防护，超时放弃线程后插件状态可能被幽灵线程继续改——超时不是无副作用的，写死在文档。
- **payload 红线**：一律不带 `messages`；Decide 只给（roles 摘要 + 末条用户消息头 512B）；
  `prompt.sections` 只给动态尾。分级上限：`llm.call.completed` 2 KiB 尾截（manifest 可声明至
  8 KiB）、`tool.call.completed` 1 KiB 头截、`tool.pre_call.decide` args 4 KiB 头截 + digest、
  `turn.*` 512 B、`prompt.sections` 每插件 2 KiB / 总计 8 KiB、`approval.*` 1 KiB（命令摘要）；
  截断必须带 `truncated: bool` + `bytes_total`；每回合总预算 64 KiB（Observe 丢弃计数，Decide
  永不丢、超预算熔断滥用者）。
- **审计**：照 `HttpDiagnosticSink` / `PluginDiagnosticView` 形状放 `ark-plugin` 侧（有界可查询
  内存环 + wire 视图），不新开 ark-obs 能力。
- **hook_abi**：manifest 声明 `hook_abi: "1"`，检查放扫描/计划阶段（复用 `plugin_skipped` 诊断）；
  payload 只许加法；conformance 测试。

### digital-life 类插件的映射验收基准（P4 试点，非本需求实施范围）

`ark.builtin.digital-life`（生理内核 Rust 重写）：扣能量/改 temperature → `llm.request.decide` +
`llm.call.completed`；急停 → `turn.pre_start.decide`（必须走 register_turn 单点，否则漏掉自唤醒
回合）；文件锁/承诺闸门 → `tool.pre_call.decide` + `tool_catalog.filter`；内省段 →
`prompt.sections`；自主唤醒 → followup 条目；自检 → `on_tick`；状态 → 插件 files 自持久化
（state.json 带schema_version，作为文件系统级外部契约写入插件文档）；WebUI → 独立旁路进程读
files，零改造。器官自举（AI 自写 TS 器官热装载）与自动审批不在范围内，后续立项。

## 分期

- **P1 平台口子（零行为变化）**：`ark-ports` 增 `HookBroker` + `NoHooks` no-op（先例
  `AllowAllGates`）；装饰器接线 `HookModel(MeteredModel(client))`（Hook 在外层，Deny 的调用不记
  usage）与 `HookToolRunner` 包 `HookedToolRunner`；turn 级钩子落 supervisor 层；`register_turn`
  单点收敛三条回合路径；child 工具目录改每回合求值并接 filter（审批类钩子标注"仅父回合"）；
  manifest `hooks` 段 + `hook_abi` + `Registration` 挂钩子句柄；`PluginEntry.hook_points`；审计
  查询面。
- **P2 Decide/Supply**：`prompt.sections`（回合内冻结）、`tool_catalog.filter`（父+子两站点）、
  `turn.pre_start.decide`、`PluginDenied` 错误码、配额/准入。
- **P3 生命通道（与可见性同批）**：CompletionInbox plugin 条目、`on_tick`、插件活动可见性
  （runtime_snapshot origin + GUI"插件活动"指示）、水位重放。
- **P4**：digital-life 试点（独立立项）。

## 验收场景（零回归闸门）

1. 零插件时，模型请求字节与改造前相同。
2. 启用单个插件前后，除该插件贡献的 section 外请求字节一致。
3. 动态片段同回合内重复求值字节稳定（prompt cache 稳定性测试）。
4. 子回合受 `tool.pre_call.decide` 管辖（断言）。
5. 插件 Deny 不设置 `model_request_failed`（断言）。
6. "插件 decide → shell pre_tool_use → dispatch"顺序有测试钉死；shell hook 语义一字不动。
7. 钩子路径结构上不碰 `workspace_gate`/registry 锁；"decide 回调回打受保护路由不死锁"有测试。
8. `EventKind` 枚举不动；SSE 现有流不混插件流量。
9. 老插件（无 hooks 段）原样解析，行为与改造前一致。
10. P1 起每期 `cargo test --workspace --exclude ark-desktop` 通过，覆盖故障路径回归。

## 待决

- 第三方 Decide 插件的运行形态（wasm vs loopback webhook 入站面）与时机。
- 器官自举（受限子进程 + 器官 manifest 能力白名单映射）、`approval.auto_approve`（放宽方向）。
- `EventKind::PluginIntervention` 是否引入（GUI 展示干预记录；至多一个 variant，P2+ 再裁）。
- web 前端动态渲染插件 page/panel（服务端 UiCatalog 契约已就绪，前端缺口）。
- 插件 KV 落盘持久化（本需求不依赖；数字生命走 files 自持久化）。
