# ark-rs 子 agent 实现审查

## 结论摘要

当前实现不宜上线：有 2 条 P0 和 7 条 P1。主要阻塞是取消、异常和进程重启后 child 状态无法可靠收敛或恢复；此外，删除活动 child 的流程、`runs.jsonl` 并发读改写、事件增量查询参数和无 registry 时的 transcript 鉴权均有明确问题。生产启动确实会恢复 `ChildLocator`，但不会恢复 `DelegationRuntime` 的历史记录表，因此这不能解决历史 child-runs 路由返回 404 的问题。审查按只读要求进行，未运行会写入构建目录的 cargo 命令。

## P0（必须修，否则不能上线）

### P0-1 执行 future 被取消或 panic 后，子任务状态会永久卡在 queued/running

- **位置**：`ark-rs/crates/ark-delegation/src/service/runtime.rs:278-339`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:235-239`、`ark-rs/crates/ark-agent/src/service/reactor.rs:562-575`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:317-323`
- **问题**：`prepare()` 先持久化 `queued` 记录（`runtime.rs:183-195`），之后 `execute()` 可在等待槽位期间被取消。父 Reactor 的 `select!` 在中断时会丢弃 `run_calls` future，进而丢弃正在执行的 `delegate()` future；此路径没有写入终态。若取消或 panic 发生在槽位取得后，`SlotGuard` 会释放 gate 槽位，但 `running.remove`、`active.fetch_sub` 和 `finish` 都在 await 之后，无法执行；`relabel(RUNNING)?` 写盘失败也会提前返回并留下已插入的 `running` 项和递增的 `active`。之后 `prepare_resume()` 将 queued/running 一律视为仍活动并拒绝恢复。HTTP resume 的 `tokio::spawn` 未保留 JoinHandle，panic 也不会进入其 `if let Err` 处理分支。
- **为什么是 P0**：常规父回合中断、存储错误或子 Reactor panic 都可能留下永远不能恢复的运行记录；发生在 running 状态时，registry 的 active 数和控制句柄还会残留。gate 自身的槽位转移是正确的，但不能替代运行状态清理。
- **建议修法**：让每次 execute 都有覆盖取消、panic、存储错误的生命周期清理器，确保 running/active 总能收敛；对“父回合取消”明确选择继续后台运行或显式取消并落终态，不能直接丢弃 future。启动时还要把遗留 queued/running 记录恢复成可重试终态，或安全地重新排队。

### P0-2 重启后运行时 registry 为空，所有历史 child-runs 都从 API 中消失

- **位置**：`ark-rs/crates/ark-delegation/src/service/runtime.rs:104-121`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:600-618`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:223-228`、`ark-rs/apps/arkd/src/wiring.rs:66-69`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:180-193`
- **问题**：`DelegationRuntime::new()` 将 `known` 初始化为空表；`SubagentRegistry::list/get` 只读 `known`。arkd 启动调用的 `store.reconcile()` 只恢复存储层的 `ChildLocator` 和 `children/index.json`，没有把 `runs.jsonl` 的最新记录加载进 delegation registry。于是重启后 list 为空，get/ensure_owned 对已存在的 child 返回 404，历史 transcript、events 和 resume 也都无法通过当前 handler 访问。`prepare_resume()` 的查找路径同样只查内存表。
- **为什么是 P0**：§7.4.1 把 `children/runs.jsonl` 定为权威记录，并明确要求启动时扫描构建 child 映射、以记录校验归属。磁盘数据虽仍在，产品层却无法列出、查看或恢复它们；queued/running 状态也没有机会进入 P0-1 所需的恢复流程。
- **建议修法**：在接受 HTTP 请求前，从每个父线程的权威 `runs.jsonl` 装载每个 child 的最新记录到 registry，或让 registry 的 list/get/ownership 查询直接由存储层提供；不得把 `index.json` 当鉴权来源。

## P1（应该修，影响正确性或可维护性）

### P1-1 DELETE 未协调运行时任务，活动 child 可能继续写入并复活记录

- **位置**：`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:339-358`、`ark-rs/crates/ark-store/src/service/thread_ops.rs:250-273`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:385-395`、`ark-rs/crates/ark-store/src/service/store.rs:84-92`
- **问题**：DELETE 只做 ownership 检查，随后直接删 `runs.jsonl` 记录和目录；没有检查 child 是否仍在运行、取消任务、等待 writer 关闭，也没有从 `DelegationRuntime.known/running/prepared` 移除 child。若 executor 之后正常结束，`finish()` 会再次追加记录并写回 `known`；删除期间发生的事件写入还可能因 locator 已移除而落到原始 `threads/<child>` 路径。即使 child 已是终态，`known` 也仍保留已删除记录。
- **为什么是 P1**：删除成功后记录可能重现，活动子会话可能继续产出事件，列表与磁盘也会不一致。§7.4.1 对销毁 child 要求先 cancel、等待 writer 落终态并关闭句柄；旧实现也会先打删除 tombstone 并 abort，阻止晚到的完成回调复活记录（`kun/src/delegation/delegation-runtime.ts:1083-1095`）。
- **建议修法**：把删除纳入 delegation runtime 的单一生命周期入口：先标记 deleting 并禁止新写入，再取消或拒绝删除活动 child、等待任务和 writer 收敛，最后删日志与目录，并同步清理 known/running/prepared/locator；`finish()` 对已删除 child 必须拒绝晚到写入。

### P1-2 重写 runs.jsonl 与并发追加没有互斥，会丢失其他 child 的记录

- **位置**：`ark-rs/crates/ark-store/src/store/child_store.rs:19-27`、`ark-rs/crates/ark-store/src/store/child_store.rs:70-89`、`ark-rs/crates/ark-store/src/service/thread_ops.rs:115-125`、`ark-rs/crates/ark-store/src/service/thread_ops.rs:250-269`
- **问题**：追加路径使用文件尾部追加；删除路径先读完整日志、过滤目标 child，再用 `write_atomic` rename 覆盖。两条路径没有共享的 parent 级锁或 actor。若某个 child 的状态追加发生在删除读取之后、原子替换之前，该追加会被覆盖；受影响的可以是另一个仍在运行的 child。
- **为什么是 P1**：`runs.jsonl` 是 §7.4.1 的权威记录，丢失一条追加记录会在重启后失去 child 归属映射和运行状态，目录则成为孤儿。原子 rename 只避免半写文件，不提供与 append 的并发串行化。
- **建议修法**：将同一父线程的 append/remove 串行化到同一 actor/锁中，或改为 append-only 删除标记并在安全时机压实；索引重建必须在同一串行化边界之后执行。

### P1-3 transcript/events 在 registry 缺失时跳过 ownership，形成 fail-open

- **位置**：`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:365-409`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:427-437`、`ark-rs/crates/ark-server/src/state.rs:177-200`、`ark-rs/crates/ark-server/src/state.rs:220-256`
- **问题**：`thread` 和 `events` 仅在 `state.subagents.registry` 为 `Some` 时调用 `ensure_owned`。registry 为 `None` 时，handler 仍解析 `parentThreadId`，但之后不使用它验证 child 归属，直接按 child UUID 读取 store。`AppState::new()` 默认 registry 为 None，而这两条路由仍可用。
- **为什么是 P1**：调用者可用其他父线程的真实 childId 配合任意有效 parentThreadId 读取其 transcript/events；如果 UUID 对应顶层线程，也会走 store 的顶层路径。§7.4.1 明确规定授权以 `ChildRunRecord.parent_thread_id` 为准，不能从路径推导。旧 TS 在 delegation 不存在时确实跳过检查，但这与新设计的权威归属校验不一致。
- **建议修法**：registry 缺失时对这些端点也返回 `RuntimeUnavailable`；若要支持关闭 delegation 后查看历史 transcript，应由 `SessionStore` 提供基于 `runs.jsonl` 的 `owns_child_run(parent, child)` 查询，并始终验证 parent。

### P1-4 events 的 since_seq 查询参数与旧 API 不兼容，轮询会反复重放全量事件

- **位置**：`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:48-56`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:406-409`、`kun/src/server/routes/index.ts:470-476`、`src/renderer/src/agent/kun-runtime.ts:741-745`
- **问题**：`ChildEventsQuery` 使用 `#[serde(rename_all = "camelCase")]`，因此字段 `since_seq` 的查询名是 `sinceSeq`；旧路由和 renderer 发送的是 `since_seq`。该参数不会填充 `since_seq` 字段，handler 随后用 `unwrap_or(0)` 从 seq 0 重放。
- **为什么是 P1**：每次增量轮询都会返回该 child 的全部事件，长会话会持续产生重复数据和不断增大的响应，破坏 §7.9 的 `since_seq` 重连/补放语义。
- **建议修法**：给字段显式标注 `#[serde(rename = "since_seq")]`，或兼容接收 `since_seq` 与 `sinceSeq`；保留 `parentThreadId` 的 camelCase。

### P1-5 cancel/steer bridge 在正常完成后仍永久轮询

- **位置**：`ark-rs/apps/arkd/src/child.rs:155-158`、`ark-rs/apps/arkd/src/child.rs:335-367`
- **问题**：每次 child turn 都 `tokio::spawn` 两个 bridge。cancel bridge 只在 reactor token 被取消时退出，正常 completed/failed 时 token 仍为 false；steer bridge 没有退出条件。调用方没有保存 JoinHandle，也没有在 `reactor.run()` 返回后通知它们退出。
- **为什么是 P1**：每个正常完成的 child 都留下后台任务和持有的 Arc；子任务数有上限但总运行次数无上限，因此长期运行会线性增加 Tokio task、token 和队列对象。
- **建议修法**：给两个 bridge 共用 child-lifetime CancellationToken，在 reactor 返回后取消并 join/abort；或把桥接改为 reactor 生命周期内可取消的 select 分支。

### P1-6 delegate_task 丢失旧版后台委派能力，所有调用都阻塞父回合

- **位置**：`ark-rs/crates/ark-tool/src/domain/args.rs:97-112`、`ark-rs/crates/ark-tool/src/exec/tools/delegate.rs:89-107`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:343-375`、`kun/src/adapters/tool/delegation-tool-provider.ts:73-75`、`kun/src/adapters/tool/delegation-tool-provider.ts:130-142`
- **问题**：Rust `DelegateArgs` 没有 `background` 字段，`DelegateTaskTool` 始终调用并等待 `runner.delegate()` 的终态；`prepare_child/run_child` 只用于 HTTP resume，没有 `delegate_background` 等价路径。旧 TS 的 `background: true` 会立即返回 childId 并 detached 执行，父回合可以继续其他工作。旧工具还允许 `resumeChildId`，Rust 参数未提供该入口，且显式把 `resume_child_id` 固定为 None（`delegate.rs:101-103`）。
- **为什么是 P1**：这改变了旧实现已有的产品语义；长任务不能后台运行，父回合必须等它完成，模型也不能沿旧工具参数恢复失败 child。简报特别提到的 `delegate_background` 在本次实现中实际不存在。
- **建议修法**：在工具契约和 runner 中恢复后台模式：持久化 queued 后，由 runtime 自己持有可取消任务并立即返回 childId；同时明确模型工具 resume 与 HTTP resume 的职责及参数兼容性。

### P1-7 正在排队的 child 没有 abort handle，HTTP resume 接受后无法取消

- **位置**：`ark-rs/crates/ark-delegation/src/service/runtime.rs:285-321`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:620-626`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:197-213`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:312-324`
- **问题**：`running` 控制句柄要等 `SlotGate::acquire()` 成功后才创建；排队中的 child 不在 `running` 表。`abort()` 只查 `running`，所以 queued child 会被报告为“不在运行”并返回 404。resume handler 已将 `run_child()` 丢给 detached `tokio::spawn`，没有保存 JoinHandle 可供取消。
- **为什么是 P1**：队列满时用户接受的 resume 不能中止，槽位释放后仍会自动开始执行；这与 child-runs 提供 abort 管理能力以及旧实现对 queued resume/background 预先注册 AbortController 的做法不符。
- **建议修法**：在排队之前注册每个 child 的控制句柄；abort 应能移除/取消 waiter、持久化 aborted 终态并阻止其之后获得槽位。

## P2 / 仅供参考

- `prepare_resume()` 的 miss 分支在 `lock_known().get(...).cloned().or_else(|| self.find_in_children(...))` 中再次锁同一个非重入 `known` mutex；`find_in_children()` 也只扫描同一张 `known` 表，未知 resume id 的直接 `SubagentRunner` 调用可能自锁。当前 HTTP resume 会先用 `ensure_owned` 拦截未知 id，因此此路径目前不由该 handler 触发。位置：`ark-rs/crates/ark-delegation/src/service/runtime.rs:223-228`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:422-428`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:270-274`。
- 单 child 删除在 runs 记录移除成功、目录删除失败时会留下没有权威记录的目录；注释称 reconciliation 会报告 orphan，但运行时 `sweep_children()` 只吸收 runs 记录、同步 index，没有调用 `orphan_child_dirs_blocking()`，且对已有 locator 只做 add/replace，不清除日志中已不存在的 child。位置：`ark-rs/crates/ark-store/src/service/thread_ops.rs:253-273`、`ark-rs/crates/ark-store/src/service/reconcile.rs:236-268`、`ark-rs/crates/ark-store/src/store/child_store.rs:152-167`。
- Rust 的 abort/steer/resume/delete 响应 DTO 序列化字段为 `id`，旧 TS 动作响应返回 `childId`。当前 renderer 只检查 HTTP 状态、不读取动作响应体，影响有限，但外部 API 客户端会遇到字段漂移。位置：`ark-rs/crates/ark-contract/src/dto.rs:173-188`、`kun/src/server/routes/child-runs.ts:40-46`、`kun/src/server/routes/child-runs.ts:78`、`kun/src/server/routes/child-runs.ts:100`、`kun/src/server/routes/child-runs.ts:128`。

## 与设计文档的偏差点清单

| 设计条款 | 实现现状 | 是否合规 |
|---|---|---|
| §7.8.0 只保留一种决策循环；§7.14 允许单层子代理 | `ChildTurnExecutorImpl` 用同一个 `Reactor` 实现、为每个 child 构造独立 run；bridge 的 `tokio::spawn` 不是决策循环。child 工具表没有 `delegate_task`，store 另有 depth 校验。位置：`ark-rs/apps/arkd/src/child.rs:195-214`、`ark-rs/apps/arkd/src/wiring.rs:198-206`、`ark-rs/crates/ark-store/src/service/child_service.rs:91-115`。 | 合规；“同一个”是同一 loop 实现，不是共享单个 Reactor 实例 |
| §7.4.1 子目录内联、runs.jsonl 权威、index 派生、启动恢复 locator 与授权 | `ChildStore` 写入父目录下 `children/runs.jsonl`，index 从日志重建；arkd 在装配 handler 前调用 `store.reconcile()` 并由 `absorb_parent_blocking()` 恢复 locator。但 runtime 的 `known` 不从日志加载，ownership 查的是内存 registry。位置：`ark-rs/crates/ark-store/src/store/child_store.rs:19-59`、`ark-rs/apps/arkd/src/wiring.rs:66-69`、`ark-rs/crates/ark-store/src/service/reconcile.rs:236-268`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:600-618`。 | 部分不合规（P0-2） |
| §7.4.1 子 ID 归属只能以记录的 parent_thread_id 校验 | registry 存在时 `ensure_owned` 按记录 parent 比较；registry 缺失时 thread/events 跳过检查。位置：`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:372-404`、`ark-rs/crates/ark-server/src/api/handlers/child_runs.rs:427-437`。 | 不合规（P1-3） |
| §7.8.0/§7.14 v1 max_depth=1 | child schemas 不注册 `delegate_task`；`ChildStore::create_child_dir` 执行 depth 校验，模型直接点名未注册工具无法触发委派。位置：`ark-rs/crates/ark-tool/src/exec/tools/mod.rs:30-51`、`ark-rs/crates/ark-store/src/service/child_service.rs:91-115`。 | 合规 |
| §7.9 child_activity 事件与 SSE | 状态事件发布到父 thread stream；publish 失败只 `warn`，不影响 child 执行。位置：`ark-rs/crates/ark-delegation/src/service/runtime.rs:398-419`。 | 合规 |
| §7.15 subagents 为 Hot 字段 | `FIELD_CLASSES` 将 subagents 标记 Hot；`ConfigSubagentPolicy::policy()` 每次从 ConfigStore snapshot 读取，`prepare()` 每次 spawn 调用 policy。位置：`ark-rs/crates/ark-config/src/domain/classify.rs:10-19`、`ark-rs/apps/arkd/src/child.rs:370-392`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:142-169`。 | 合规；已接受任务继续使用其捕获的 policy |
| §7.8 并发槽位与 max_queued | `release()` 将槽位交给活 waiter 时 active 不变，死 waiter 被跳过；全部 waiter 已死时 active 减一次，`SlotGuard::drop` 覆盖 panic。`max_queued` 限制实际进入 gate 的 waiters；prepare/prepare_child 会先持久化 queued，短暂存在 queued 记录但 gate 还没有 waiter，未启动/崩溃的 prepared 任务也不由 gate 计数。位置：`ark-rs/crates/ark-delegation/src/domain/gate.rs:78-132`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:183-195`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:285-307`、`ark-rs/crates/ark-delegation/src/service/runtime.rs:356-375`。 | gate 转移合规；任务生命周期不合规（P0-1） |
| 附录 A 八条 child-runs 路由及状态码 | GET list/get/thread/events、POST abort/steer/resume、DELETE child 路径齐全；Rust 与旧 router 都要求单 child 请求提供 `parentThreadId`；handler 错误统一走 `ArkErrorCode::http_status()`。但 `since_seq` 被 camelCase 重命名，旧参数无法生效；动作 DTO 的 `id` 也与旧 `childId` 不同。位置：`ark-rs/crates/ark-server/src/api/routes.rs:199-226`、`kun/src/server/routes/index.ts:410-479`、`ark-rs/crates/ark-server/src/api/error.rs:51-56`、`ark-rs/crates/ark-contract/src/dto.rs:173-188`。 | 部分不合规（P1-4、P2） |
| §7.3 工具错误以 ok:false 内容交还模型 | failed child outcome 被 render 成 `ToolOutput { ok: false, content }`；Reactor 转成 failed ToolExecution，并以 tool role 内容加入下一轮 history。位置：`ark-rs/crates/ark-tool/src/exec/tools/delegate.rs:116-150`、`ark-rs/crates/ark-agent/src/service/reactor.rs:640-657`、`ark-rs/crates/ark-agent/src/service/reactor.rs:609-615`。 | 合规 |
| §7.14.3 端口边界与实现成本 | `DelegateTaskTool` 只依赖 `SubagentRunner`；store、registry、executor 分层清晰。未发现偏离端口依赖方向的实现；不过已有后台委派和工具侧 resume 语义未迁移。位置：`ark-rs/crates/ark-tool/src/exec/tools/delegate.rs:74-107`、`ark-rs/crates/ark-ports/src/subagent.rs:91-124`、旧实现 `kun/src/adapters/tool/delegation-tool-provider.ts:73-75`。 | 边界合规，行为不完整（P1-6） |
