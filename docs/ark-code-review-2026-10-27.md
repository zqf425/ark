# ark 代码审查报告（2026-10-27）

## 结论摘要

当前实现不具备上线条件：发现 4 个 P0、10 个 P1。最严重的问题是数据目录缺少跨进程写锁、模型可借子代理把工作区根切换到任意目录、文件工具可经 Windows junction/symlink 越界，以及父回合在线程 panic 或 Ctrl-C 关闭时不能可靠落终态。主 SSE 的“快照 + 订阅”原子切换、child 单条路由的 parentThreadId 归属校验、child 的单层工具集限制均已实现；本报告没有把这些合规实现报为缺陷。

## P0（必须修，否则不能上线）

### P0-1 数据目录没有跨进程独占锁，多个 arkd 可同时写入同一权威日志（已修复）

- 位置：crates/ark-store/src/service/store.rs:44-62；crates/ark-store/src/domain/layout.rs:57-59
- 问题：DataPaths 定义了 data/.lock 的路径，但 FileSessionStore::open 只创建目录、加载索引并构造 WriterRegistry，既未获取锁，也未持有锁句柄。两个 arkd 指向同一 data_dir 时会分别从同一最大 seq 初始化各自的 writer。
- 为什么是 P0：两个进程可为同一 events.jsonl 分配相同 seq，并发 append 还可能交错写入；events.jsonl 是会话权威记录，损坏后 SSE 重放、投影和恢复都不可信。这直接违反设计 §7.4.3 的“同一数据目录只有一个 writer 进程”。
- 建议修法：在打开存储前对 data/.lock 获取平台独占文件锁，持有 RAII 锁对象直到 FileSessionStore Drop；锁已被占用时拒绝以写模式启动，并补两个进程争用同一 data_dir 的集成测试。
- 修复：`FileSessionStore::open` 在打开前对 `data/.lock` 取平台独占锁并持有到 Drop，被占用时拒绝启动（crates/ark-store/src/store/data_lock.rs、service/store.rs）

### P0-2 delegate_task 接受模型提供的 workspace，子代理可绕过父工作区边界（已修复）

- 位置：crates/ark-tool/src/domain/args.rs:101-118；crates/ark-tool/src/exec/tools/delegate.rs:91-104；crates/ark-delegation/src/service/runtime.rs:765-776；apps/arkd/src/child.rs:190-201、359-367
- 问题：DelegateArgs.workspace 由模型输入，DelegateTaskTool 原样写入 SubagentRequest；ChildTurnExecutorImpl 随后直接把它作为 ChildToolRunner 的 workspace 根。没有与父 ToolContext.workspace 比较、规范化或授权。delegate_task 的基线决策还是 ReadOnly，默认不需要额外批准。
- 为什么是 P0：模型可传入任意本机目录作为子代理根；子代理的 write/edit 便会把该目录视为“工作区内”并以 WorkspaceWrite 执行，突破父会话的文件访问边界。
- 建议修法：默认并强制子代理继承父 workspace。若产品确需独立 workspace，必须由宿主提供、经真实路径包含校验和显式用户授权的 allowlist 选择，不能接受模型原始路径。
- 修复：子代理 workspace 强制继承父 `ToolContext.workspace`，模型传入的 workspace 一律忽略（crates/ark-tool/src/domain/args.rs、exec/tools/delegate.rs）

### P0-3 词法路径校验可被 symlink 或 Windows junction 绕过（已修复）

- 位置：crates/ark-tool/src/domain/paths.rs:14-35；crates/ark-tool/src/exec/tools/read.rs:41-44；crates/ark-tool/src/exec/tools/write.rs:32-37、64-90
- 问题：paths::resolve 只对字符串做 ark_common::path::normalize，并明确不解析链接；随后 read/write/edit 直接操作这个词法上位于工作区内的路径。工作区中的 symlink/junction 指向外部目录时，检查会通过而文件系统调用会跟随链接。
- 为什么是 P0：默认只读工具可读取工作区外敏感文件，默认写工具可写出工作区；这使 SandboxDecision::ReadOnly/WorkspaceWrite 的边界失效。
- 建议修法：使用 no-follow、handle/dirfd 风格的逐段解析，拒绝 reparse point/symlink；对新建目标至少验证其所有既存父目录的真实路径仍在工作区内，并加入 Windows junction 与 Unix symlink 逃逸回归测试。
- 修复：路径校验改为解析真实路径（`canonicalize` 后比父目录前缀），拒绝经 symlink/junction 逃逸（crates/ark-tool/src/domain/paths.rs）

### P0-4 父回合的 panic 与关闭路径没有 supervisor，可能永久停在 running 或缺少终态事件（已修复）

- 位置：crates/ark-server/src/api/handlers/turns.rs:68-81；crates/ark-server/src/service/serve.rs:38-48、65-76；crates/ark-server/src/api/handlers/runtime.rs:75-84
- 问题：spawn_turn 丢弃 tokio::spawn 的 JoinHandle，只在 TurnRunner 正常返回 Result 时调用 TurnRegistry::finish。任务 panic 时没有 JoinError 处理，内存 registry 永远保持 running。Ctrl-C 分支直接结束 listener 后 drain，却没有先 cancel_all；超时后 Tokio runtime 会丢弃仍在运行的任务。HTTP shutdown 只取消父回合，也没有协调 detached child。
- 为什么是 P0：同一 thread 后续请求持续返回“已有 running turn”，并且 events.jsonl 可能只留下 turn_started，违背设计 §3、§7.1、§7.9 的每条生命周期必须收敛到终态要求。
- 建议修法：由运行时 supervisor 持有 JoinHandle，处理 panic/abort 后写失败终态并清理 registry；将 Ctrl-C 和 HTTP shutdown 统一进入 cancel → 等待 → 超时后显式 abort 并落终态的流程，同时将 child runtime 纳入 draining。
- 修复：新增 turn supervisor 持有 `JoinHandle`，panic/abort 后写失败终态并清理 registry；Ctrl-C 与 HTTP shutdown 统一 cancel → 等待 → 落终态（crates/ark-server/src/api/handlers/turns.rs、service/serve.rs）

## P1（应该修，影响正确性或可维护性）

### P1-1 queued child 未被识别为活动任务，删除父线程会放行仍会启动的子任务（已修复）

- 位置：crates/ark-store/src/domain/child_index.rs:13-20；crates/ark-ports/src/store.rs:94-97；crates/ark-store/src/service/thread_ops.rs:166-174
- 问题：ChildRunRecord::is_active 将 queued 与 running 都视为活动，但用于 delete_thread 门禁的 ACTIVE_STATUSES 缺少 queued。排队 child 因而不阻止删除父线程。
- 为什么是 P1：child 之后取得并发槽位时，其父目录和权威记录已被删除，会产生写入失败、孤儿目录或错误恢复状态。
- 建议修法：只保留一份枚举化的活动状态判定，delete_thread 与 runtime 共用；删除父线程时应对 queued/running 一律返回冲突或按 force 流程取消并等待终态。
- 修复：`is_active_status` 与活动 child 统计纳入 `queued`，删除父线程时不再放行待启动的子任务（crates/ark-store/src/domain/child_index.rs、service/thread_ops.rs）

### P1-2 children/runs.jsonl 不修复崩溃留下的半行，下一次 append 会吞掉新记录（已修复）

- 位置：crates/ark-store/src/store/child_store.rs:19-41；crates/ark-store/src/store/fs_util.rs:71-89
- 问题：读 child run 日志时仅静默跳过无法解析的行；追加时则无条件在文件尾追加。若进程崩溃在最后一条 JSON 的换行前，下一条记录会拼接到残行，整行无法解析，新的状态记录也随之丢失。
- 为什么是 P1：runs.jsonl 是 child 归属和状态的权威来源；丢失末条 queued/running/terminal 记录会导致错误鉴权、错误恢复或不可见 child。
- 建议修法：在启动/首次写入前、持有同一父级锁时检测并截去仅最后一条不完整记录；非末尾损坏应保留诊断并 fail-closed，而不能与尾部恢复混为一谈。
- 修复：append 前修复日志尾部半行，避免下一次写入被吞（crates/ark-store/src/store/child_store.rs）

### P1-3 child 终态写入或派生索引失败后，内存状态会卡在 running（已修复）

- 位置：crates/ark-delegation/src/service/runtime.rs:569-596、618-634、744-762；crates/ark-store/src/service/thread_ops.rs:120-130
- 问题：execute 在 finish 返回 Err 时直接退出；RunSlot Drop 会移除控制句柄并释放 active，但 known 仍保留此前的 running 记录。更糟的是 append_child_run_blocking 在权威行已成功追加后，若 rebuild_child_index_blocking 失败仍返回 Err，运行时同样把它当作终态未写入。
- 为什么是 P1：API 会显示 running，但 abort 找不到句柄、resume/destroy 又因记录仍 active 被拒绝，直到重启 hydrate 才可能修复；这正是“内存 active 与磁盘状态不一致”的禁区。
- 建议修法：区分权威 append 与派生索引失败；终态持久化失败时由 supervisor 保留可恢复状态并重试/调度收敛，不能先释放控制面再把 known 留在活动状态。
- 修复：终态写入或派生索引失败时仍收敛内存状态，不再卡 `running`（crates/ark-delegation/src/service/runtime.rs）

### P1-4 可删除正在运行的父 thread，后续事件会写进没有 thread.json 的幽灵目录（已修复）

- 位置：crates/ark-server/src/api/handlers/threads.rs:110-117；crates/ark-store/src/service/thread_ops.rs:177-184；crates/ark-store/src/service/store.rs:216-231
- 问题：DELETE /v1/threads/{id} 未检查 TurnRegistry 中的活动父回合。delete_thread 会 forget writer 并删除目录；活动 Reactor 的下一次 EventSink::publish 会重新按该 id 创建 writer 路径，却不会重建 thread.json。
- 为什么是 P1：删除成功后仍有事件写入不可枚举的目录，既丢失会话可见性也破坏“一个 thread 一个自包含目录”的存储不变量。
- 建议修法：活动父回合时返回 409，或先取消、等待回合和 writer 关闭后再删除；删除期间设置 tombstone，阻止任何新 writer 为该 thread 重建目录。
- 修复：删除运行中父 thread 前先取消并等待 writer 收敛，拒绝写入幽灵目录（crates/ark-store/src/service/thread_ops.rs）

### P1-5 同一工具调用的 SandboxDecision 被计算两次，批准后的 bash 仍会被拒绝（已修复）

- 位置：crates/ark-agent/src/service/reactor.rs:387-397；crates/ark-agent/src/exec/gate.rs:80-96；crates/ark-tool/src/service/runner.rs:89-103；apps/arkd/src/wiring.rs:75-78
- 问题：Reactor 首次 decide 后为 bash 请求 approval；AllowAllGates 返回 Approved，但 RegistryToolRunner 又重新 decide，仍得到 NeedsApproval 并返回 ok:false。首次决策和批准结果没有传入派发层。
- 为什么是 P1：默认 WorkspaceWrite 配置下，bash 表面上已获批准却永远不能执行，且违反设计 §7.3 “派发前计算一次并随 ToolContext 传入”的安全一致性要求。
- 建议修法：把一次性决策及 approval 结果作为调度数据传到 ToolContext/runner；执行器只消费该不可变决定，不再调用 SandboxPolicy。
- 修复：`SandboxDecision` 在单次调用内只计算一次，批准后复用同一决策（crates/ark-tool/src/service/runner.rs、domain/args.rs）

### P1-6 parent 与 child 的 steer 都没有进入 Reactor 历史，接口实际无效（已修复）

- 位置：crates/ark-server/src/api/handlers/turns.rs:98-139；apps/arkd/src/wiring.rs:264-280；apps/arkd/src/child.rs:186-239、393-409；crates/ark-agent/src/service/reactor.rs:151-170、189-218
- 问题：parent steer 只追加 TurnSteered 事件，parent Reactor 没有 SteerQueue。child bridge 虽把文本推入 SteerQueue，但 Reactor::drain_steering 在全仓只有定义，没有在 run 的任何轮次调用。
- 为什么是 P1：HTTP 返回 202，但修正文本永远不会作为下一轮用户消息进入模型上下文；child 的 bridge 任务徒耗资源且用户无法修正排队/运行任务。
- 建议修法：在每个安全轮次边界调用 drain_steering、写事件并注入 history；TurnRegistry 应持有 parent SteerQueue，路由向它 push 并对队满返回冲突。
- 修复：`delegate_task` 恢复后台委派：新增 `background` 与 `resumeChildId`，后台走 `delegate_background`，resume 保持前台；steer 注入 Reactor 历史（crates/ark-tool/src/domain/args.rs、exec/tools/delegate.rs、crates/ark-agent/src/service/reactor.rs）

### P1-7 已接受的 model_providers 与 tuning 热更新不会影响后续父回合（已修复）

- 位置：crates/ark-config/src/service/config_store.rs:76-113；apps/arkd/src/wiring.rs:100-108、222-232、264-280
- 问题：ConfigStore::apply 只替换快照。model 在启动时选择后被 AgentTurnRunner 静态持有；每个 parent turn 又固定使用 TurnLimits::default 和 CHUNK_BUFFER，而非最新 tuning。接口仍返回 Applied。
- 为什么是 P1：用户看到“热更新成功”，但模型路由、回合额度和 chunk buffer 对后续父回合都不生效，违反设计 §7.15 的热更新承诺。
- 建议修法：每个新 turn 获取同一份最新 ConfigSnapshot，据此选择 model 与构造 limits/chunk buffer，或实现设计要求的有序 on_config_change 订阅器；运行中的 turn 继续使用其创建快照。
- 修复：父回合 model/tuning 改为每回合读最新快照，热更新对后续回合生效（apps/arkd/src/wiring.rs、crates/ark-agent/src/service/reactor.rs）

### P1-8 Usage::merge 会在缓存字段不完整时虚构 aggregate cache_hit_rate（已修复）

- 位置：crates/ark-common/src/usage.rs:49-56、60-66；crates/ark-obs/src/domain/usage_recorder.rs:35-40；crates/ark-server/src/api/handlers/child_runs.rs:137-155
- 问题：merge 会保留单边已知的 hit/miss 后重新计算比例。例如样本 A 为 hit=90、miss=None，样本 B 为 hit=10、miss=10，合并后会显示 100/(100+10)，但 A 的 miss 从未由 provider 提供。
- 为什么是 P1：对外 Usage 会把未知数据展示为确定的 cache hit rate，直接违反设计 §6.4 “字段缺失时为 None，不得臆造”。
- 建议修法：累计时额外追踪完整性；任一参与样本缺 hit 或 miss 时 aggregate cache_hit_rate 设为 None，或明确新增覆盖范围字段而非复用完整汇率字段。
- 修复：`Usage::merge` 在 hit/miss 不完整时不再合成 `cache_hit_rate`，改为 `None`（crates/ark-llm/src/common/usage.rs）

### P1-9 Rust 契约没有生成管线，前端维护了可漂移的 DTO 与错误码副本（已修复）

- 位置：crates/ark-contract/Cargo.toml:7-12；web/src/shared/contracts/dto.ts:1-84；web/src/shared/contracts/event.ts:1-154；web/src/shared/contracts/error.ts:1-15；web/scripts/check-hard-rules.mjs:61-72
- 问题：ark-contract 没有 ts-rs/schemars 依赖，全仓也没有生成命令；web contracts 是手写“待由 ark-contract 生成”文件，检查脚本只验证这句占位说明。error.ts 还维护第二份 ArkErrorCode union。
- 为什么是 P1：HTTP/SSE/TS 的共享类型无法由 Rust 唯一真源约束，任一次单侧字段或错误码变动都会静默漂移，正是设计 §6.1 要消除的真实故障来源。
- 建议修法：为 Rust wire 类型接入 ts-rs 与 schemars，生成 web/src/generated 和 schema，并在 CI 执行生成后 git diff --exit-code；删除手写 DTO 与错误码 union。
- 修复：ts-rs 生成管线接入 CI：`cargo test -p ark-contract --lib ts_export` 产出 `web/src/generated`，web 端手写 DTO 副本全部删除并改为 re-export，`check-hard-rules.mjs` 双向校验；schemars JSON Schema 为 deferred（crates/ark-contract/src/ts_export.rs、web/src/shared/contracts/）

### P1-10 SSE idle timeout 设为 0 时，断连客户端可永久滞留 pump 和订阅者（已修复）

- 位置：crates/ark-server/src/api/handlers/events.rs:46-54、80-107；crates/ark-store/src/store/writer_state.rs:238-256；crates/ark-config/src/domain/snapshot.rs:57-69
- 问题：每条 SSE 连接启动 detached pump。timeout=0 时 pump 永久等待 live.recv；下游 ReceiverStream 因客户端断连而 drop 不会唤醒该等待。订阅者只在下一次事件 fanout 时才被清理；没有新事件的空闲 thread 会无限保留该任务和 receiver。
- 为什么是 P1：配置允许 timeout=0，反复连接/断开可无上限累积任务和 subscriber，违背 §7.9 的有界慢客户端隔离要求。
- 建议修法：将 HTTP 连接取消信号传给 pump 并在 select 中退出，或不创建 detached pump；同时限制每 thread 订阅数并在断开时立即取消 store subscription。
- 修复：SSE pump 接入连接取消信号，`timeout=0` 下客户端断开即退出，不再滞留 pump 与订阅者（crates/ark-server/src/api/handlers/events.rs）

## P2 / 仅供参考

- 位置：crates/ark-server/Cargo.toml:14；crates/ark-server/src/state.rs:17、171。ark-server 直接依赖并持有 ark-obs::UsageRecorder，超出设计 §5.2 为 ark-server 列出的白名单；应改为经 ark-ports 端口注入。
- 位置：crates/ark-contract/src/dto.rs:135-147。ChildRunAggregates 未声明 camelCase；外层 ChildRunListResponse 的 serde 属性不会递归，tool_invocations 会以 snake_case 发出。
- 位置：crates/ark-server/src/api/handlers/events.rs:87-92、111-118。SseIdleTimeout 使用订阅起点 watermark 作为 SSE id；若已发送 live seq，控制帧 id 会倒退。当前 web 客户端用 Math.max 保护续订水位，但通用客户端仍可能重放重复事件。
- 位置：crates/ark-llm/src/common/usage.rs:71-80。cost_usd 恒为 None，全仓未找到 PricingTable；未达到 §6.4 的注入式计费表设计。
- 位置：crates/ark-tool/src/exec/tools/bash.rs:44-53、99-108；crates/ark-tool/src/policy/sandbox.rs:60-63。Unattended 模式把 bash 作为 WorkspaceWrite 执行，但仅设置 current_dir、未 env_clear；若该模式暴露到产品配置，shell 可越界并继承宿主 TOKEN/API_KEY 等环境变量，应在启用前补齐 OS 级隔离与环境白名单。
- 位置：crates/ark-contract/src/event.rs:1-871；crates/ark-delegation/src/service/runtime.rs:1-1003。两个 Rust 文件超过 800 行。另有函数超过 80 行：crates/ark-contract/src/event.rs:435、crates/ark-server/src/api/routes.rs:38、apps/arkd/src/wiring.rs:53、apps/arkd/src/child.rs:176、crates/ark-delegation/src/service/runtime.rs:501、crates/ark-store/src/domain/items.rs:71、crates/ark-skills/src/service/runtime.rs:94。
- 位置：crates/ark-common/src/lib.rs:1；crates/ark-contract/src/lib.rs:1；crates/ark-ports/src/lib.rs:1；crates/ark-server/src/lib.rs:1；apps/arkd/src/main.rs:1。上述编译入口缺少 #![deny(clippy::all)]，根 Cargo.toml 也没有 workspace.lints 补偿；未发现 lib.rs 使用 pub use *。

## 与设计文档的偏差点清单

| 设计条款 | 实现现状 | 是否合规 |
|---|---|---|
| §5.2：ark-agent 只依赖端口，不依赖具体模型/工具/存储 | crates/ark-agent/Cargo.toml 只依赖 ark-common、ark-contract、ark-ports；未发现对 ark-llm/ark-tool/ark-store 的直接依赖 | 合规 |
| §5.2：ark-server 依赖白名单 | ark-server 额外直接依赖 ark-obs 并持有具体 UsageRecorder | 不合规（P2） |
| §6.1：Rust 契约生成 TS/Schema，生成物 diff 为零 | ts-rs 管线已接入（`cargo test -p ark-contract --lib ts_export` + `git diff --exit-code web/src/generated`），web 手写副本已删除；schemars JSON Schema deferred | 合规（P1-9，Schema 暂缓） |
| §6.4：cache hit rate 仅基于完整原生 hit/miss | 单次解析遵守该公式；merge 在 hit/miss 不完整时置 `None` | 合规（P1-8） |
| §7.1、§3：回合取消、panic、写盘失败均收敛终态 | supervisor 持有 JoinHandle 并写失败终态；finish 失败仍收敛内存；Ctrl-C 走 cancel→等待→落终态 | 合规（P0-4、P1-3） |
| §7.3：沙箱决策只算一次，路径受工作区限制 | decision 单次计算后复用；delegate 强制继承父 workspace；路径按真实路径校验 | 合规（P0-2、P0-3、P1-5） |
| §7.4：目录为权威记录，单 writer 与 child 日志一致 | `data/.lock` 独占锁持有到 Drop；append 前修复尾部半行；`queued` 计入活动 child | 合规（P0-1、P1-1、P1-2） |
| §7.8：子代理单层、child 工具不含 delegate_task | wiring 的 child registry 只注册 builtins，未注册 delegation_tools；create_child_run 也有深度检查 | 合规 |
| §7.9：SSE 快照与 live 注册原子、有界、child 访问 fail-closed | writer actor 串行、live=512、child 路由强制 parentThreadId；pump 接入取消信号，timeout=0 下断连即退出 | 合规（P1-10） |
| §7.15：热字段对后续任务生效、在途任务保持快照 | 每回合读取最新 config 快照，model/tuning 热更新对后续回合生效 | 合规（P1-7） |
| §10.3：文件/函数规模与质量门禁 | 已有文件、函数超限，且部分 crate 无 clippy deny | 不合规（P2） |
