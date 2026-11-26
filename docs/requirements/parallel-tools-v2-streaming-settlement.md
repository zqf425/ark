# 工具并发执行 v2：流式结算与取消保留（parallel-tools-v2）

状态：已批准计划（第一轮检视结论已吸收：修订点见各节标注）。
前置：[parallel-tools.md](parallel-tools.md)（v1 批划分已落地，本设计是其执行层迭代）。

## 背景与问题

v1 之后执行链路是 `step::dispatch → ParallelPolicy::plan → reactor::dispatch →
dispatch::run_calls`：批划分已由工具自述的 `ToolConcurrency` 驱动，批内用
`join_all` 并发执行。但批内执行与结算仍是"全批齐达"模型，有三个实测问题：

1. **事件结算滞后**：`run_batch_events` 先为整批写 `ToolCallStarted`，`join_all`
   全部完成后才逐个写 `ToolCallFinished`。一个 10 分钟 `bash` 与 3 个 50ms `read`
   同批时，3 条 read 的结果要等 10 分钟才可见——web 端 Changes/Terminal 投影
   与会话流在整个批期间停留在"已启动、无结果"。
2. **取消丢弃已完成结果**：`tokio::select!` 选中取消分支时，整批已完成的
   调用结果被丢弃，全部改写为 `cancelled before the call completed` 的合成
   失败行。批内一个 50ms 的 `read` 和一个 10 分钟的 `bash` 同批，用户在第 5
   秒取消时 read 的真实结果（可能已消耗带宽与 IO）被替换为占位错误，模型
   上下文与磁盘都失去这份证据。
3. **输出顺序的隐含耦合**：`run_batch_events` 结果循环里 `open.remove(0)` 假设
   `run_calls` 的输出顺序与 `batch` 输入顺序一致（当前 `join_all` 恰好保证）。
   一旦执行器层改为乱序完成（v2 的目标方向），该假设会静默错配 call 与事件。

Qoder 的 STE（流式工具执行器）对同样问题的解法是：逐调用完成即结算、取消时
保留已完成结果、批间保持提交顺序。本设计把它移植到 ark 的状态机与事件模型上，
不引入 Qoder 的异步迭代器形态——ark 的 reactor 是单步状态机，不是 pull 流。

## 目标与非目标

- 目标一（流式结算）：批内每个调用完成即写配对的 `ToolCallFinished` 事件，
  不等待同批其余调用；**历史行仍按输入顺序在批末一次落盘**（目标一仅覆盖
  事件流式结算；历史行时机见 §1，两者不同步是刻意的——回合历史必须按模型
  给定顺序落盘）。
- 目标二（取消保留）：取消时，取消瞬间**已到达结算通道**的调用结果按真实
  内容落盘（Finished 事件 + 历史行），仅未到达的调用写合成取消行；回合以
  interrupted 语义收敛。保留结果**不经 `ToolsSettled` 上报**（见"数据与
  兼容"）：它经事件与历史行落盘后，由下一回合的 seed 重放进入模型上下文。
- 目标三（顺序解耦）：结算循环不得假设输出顺序与输入顺序一致，按 `call_id`
  配对，输入顺序决定历史行顺序。
- 非目标：不改批划分规则（`ParallelPolicy` 不动）；不做跨批调度与乱序交付
  （批间顺序、回合历史按模型给定顺序落盘的语义不变）；不做单调用级取消
  传播（整批取消仍是现状语义）；不改 `max_parallel_tool_calls` 分块。

## 设计

### 1. 逐调用结算（通道 + 转发任务）

`dispatch::run_calls` 改为单一实现路径：批内所有调用 future 交给一个
`tokio::spawn` 的**转发任务**并发驱动（`join_all` 于任务内部），每个调用
包装一个 `mpsc::Sender`，完成一条立即推送；原有"输入向量 → 输出向量"
签名保留为薄包装，内部消费同一条通道把结果归并成 Vec（不再维护第二套
并发驱动，避免双实现漂移）。

- 通道容量 = `batch.len()`（调用数上界已知）：不存在反压，已完成的
  `send` 永不挂起，转发任务的 `join_all` 轮询不会因通道满而卡住同批
  未完成调用。
- `run_batch_events` 消费端循环 `recv`，收到一条立即写该调用的
  `ToolCallFinished` 事件（含配对 item_id），**成功 append 后**才把该
  call 从 open 映射移除——journal 中途失败时补偿路径仍能覆盖它；仍在
  通道里未消费的结果同样视为 open。
- `ToolCallFinished` 的落盘顺序变为完成顺序（乱序）。store 侧无改动：
  投影按 keyed item_id（`tool_call:{call_id}`）upsert 折叠，与 Finished
  之间的落盘顺序无关（见 [ark-store](../modules/ark-store.md) 的 keyed
  item_id 设计；前置依赖仅"每调用 Started 先于其 Finished"，该约束由
  整批 Started 先行落盘保证）。
- **历史行批末一次落盘**：消费者把乱序完成的结果按 `call_id` 归位到
  `batch` 输入序的槽位（消耗式匹配：每个 call_id 占用其首个未占槽），
  整批结束时（正常完成或取消收尾）按槽位顺序统一 `append_tool_history`。
  同回合重复 call_id 属畸形输入（gate 路径实测发生过，AG-05）：消耗式
  匹配保证每份结果各占一槽、历史行数与 v1 一致（两行都落），但第二份
  与槽位的对应关系不作保证——不为此引入多重映射。

### 2. 取消保留（abort + drain，无定时窗口）

`run_batch_events` 的 `tokio::select!` 保持两分支：`cancel.cancelled()` 与
`转发任务完成`。取消分支的动作序列（**不引入定时窗口**，第一轮检视的
替代方案已被采纳，理由：无超时、无时序 flake，杀进程语义天然保留）：

1. **spawn 前 fast-path**：进入批处理前若 token 已取消，不 spawn 转发
   任务，整批直接走合成取消行（v1 现状语义：取消先于批开始时不执行
   任何工具）。
2. **取消即 `JoinHandle::abort()`**：转发任务被 abort，其内部的
   `join_all` future 树随之 drop，每个工具 future 被 drop——bash 的
   `kill_on_drop(true)` 链路由此恢复（select 丢弃 JoinHandle 本身不会
   kill 子进程，这是 v2 若不处理就会静默破坏的关键语义：用户取消后
   不肯退出的 bash 必须被终止）。
3. **drain 通道存量**：abort 后转发任务的 sender 全部 drop，通道关闭，
   `recv` 返回 `None` 时 drain 自然结束。取消瞬间**已推入通道**的结果
   按 `call_id` 归位、按真实内容结算（Finished 事件 + 槽位）；
   "已完成但尚未推入"的结果随 future drop 丢失，写合成取消行——该
   损耗窗口是 push 与 abort 之间的纳秒级间隙，可接受，语义上定义为
   **以通道为界**：到达通道的结果算数，未到达的作废（有意取舍：保留
   结果与取消行不做标记区分，审计可由事件时间序推断）。
4. **drain 后收敛**：open 映射为空（整批恰好都已完成）时按正常完成
   收敛，不写 interrupt（select! 双就绪竞态下取消分支被随机选中时，
   此探测让行为可预期；drive 循环随后仍观察 cancel token 终止回合）。
   否则对剩余 open 调用逐个写合成取消行（文案沿用
   `cancelled before the call completed`），仅当存在合成行时返回 interrupted——
   interrupted 的判定是 `cancel_hit && had_gaps_before_synthetic`，取消命中与
   实际中断两个事实分开维护，不与 panic 混用。interrupted 提前返回前必须先
   `converge_pending_calls_on_cancel()`：后续批次从未执行，其调用不能在
   journal 与 provider 历史里悬空（AG-03 同类约束）。
5. **转发任务 panic**：`JoinHandle` 返回 `JoinError` 时，剩余未结算调用全部
   写合成失败行，detail 携带 JoinError 摘要（文案
   `tool batch task failed: <panic message>`），`tracing::error` 记录；
   in-flight 子进程依赖 abort drop 的 `kill_on_drop` 收尾，该依赖在此
   显式声明。panic **不是取消**：它不置 interrupted、不终止回合——回合以
   `ToolsSettled` 继续，模型在本回合就能看到全部兄弟调用的真实结果与失败行。
   panic 时通道存量的归属与取消一致（§2.3 的通道边界同样适用）：已推入
   通道的结果按真实内容保留结算，未推入的（含 panic 调用自身的 future，
   被 join_all 树 drop）写合成失败行。
6. **send 失败静默忽略**：消费者消失（理论路径）时 `send` 返回 Err，
   转发任务忽略之并继续驱动剩余调用至自然结束，避免 panics。

### 3. 配对与补偿（open 映射重写）

`open: Vec<&ToolCallRequest>` 改为 `BTreeMap<call_id, &ToolCallRequest>`
（BTreeMap 而非 HashMap：补偿路径按剩余键迭代写 Finished，定序迭代
保证补偿落盘顺序确定，既有回归测试对补偿顺序的断言不 flake）。
结算按 `call_id` 移除，不再依赖顺序。

- `close_open_tool_calls`（journal 失败补偿）：按 map 剩余键工作，
  语义不变，顺序由 BTreeMap 定序保证。
- `converge_pending_calls_on_cancel`（gate 取消兜底，AG-03）：**不改**。
  它按 `self.pending_calls` 与历史 tool 行的差集工作，不持有 open 列表，
  v2 不需要也不应触碰它。

### 4. 不变的部分

- `ParallelPolicy`、`ConcurrencyTable`、`max_parallel_tool_calls` 分块：
  v1 语义原样保留。
- 批间顺序：reactor 仍逐批 `await`，批 N 不开始于批 N-1 全部结算完成前。
- `ToolCallStarted` 仍整批先行落盘（v1 已确立的审计与投影约束）。
- 合成结果（storm、gate 拒绝）路径不动。
- 取消文案 `cancelled before the call completed` 仅用于真未完成（未到达
  通道）的调用；保留结果按正常完成形态落盘（真实 duration、无 detail）。

## 数据与兼容

- 无 wire 契约变更：`ToolExecution`、`RuntimeEvent`、store item 结构不变；
  `InputKind::Interrupt` 不携带 executions。
- **保留结果到达模型的路径**：取消分支下 `dispatch` 仍以
  `interrupt_input` 收敛（现状控制流：`interrupted=true` 时 executions
  不进入 `ToolsSettled`，step 对 `Interrupt` 无条件终态），保留结果只经
  "Finished 事件 + 历史行落盘"持久化，由**下一回合 seed 重放**进入模型
  上下文——这是它到达模型的唯一路径，也是本特性的用户价值所在。
  v1 中取消丢弃结果的等价后果是：磁盘与重放历史里全是合成取消行，
  模型下一回合看不到任何真实产出。
- store 投影按 item_id 配对 Started/Finished，Finished 乱序落盘不影响
  Changes/File/Terminal 投影与会话重放（依赖已由 §1 第 4 点钉死，并
  有 store 侧钉死测试锁定该假设，见验收）。

## 验收

测试基建：`MapToolRunner` 是静态映射，无延迟/挂起/完成通知能力；
新增测试专用 runner（`ark-agent` 测试模块内）：可配置每调用的延迟、
可挂起（永不完成）、可观测完成事件，供乱序/取消用例使用。

- 单测（`dispatch`）：快调用先于慢调用落 `ToolCallFinished`（延迟桩
  验证事件顺序）；输出乱序时按 `call_id` 配对正确、历史行仍按输入
  槽位序；同 call_id 重复时两行历史都落（消耗式槽位）。
- 单测（`tool_batch` 取消路径）：取消时已到达通道的调用保留真实内容、
  未到达的写合成取消行；整批恰好完成时取消不产生 interrupt；spawn 前
  已取消时不执行任何工具；转发任务 panic 时剩余调用合成失败且 detail
  含 JoinError 摘要。
- **回归改写**：`reactor_regression_tests.rs` 现有取消用例断言
  executions 为空且全部为合成行——改为断言"已完成调用保留真实结果、
  未完成调用为合成行"（语义反转点，显式列出）。
- **store 投影钉死测试**（ark-store）：喂乱序 Finished，断言投影折叠
  结果与顺序无关——锁定 §1"store 侧无改动"的核心假设。
- **seed 重放测试**（ark-agent）：取消回合后，下一回合的请求构建包含
  已保留的真实 tool 结果行——锁定"保留结果到达模型"的路径。
- **进程终止回归**（agent 层集成）：取消后 in-flight bash 进程确被终止
  （`kill_on_drop` 链路，防 §2 第 2 点回归）。
- 性能回归：`sleep 3` x3 并发总耗时 ≪ 9s（v1 已有断言保持）。
- 全仓：`cargo test --workspace --exclude ark-desktop`、`cargo clippy
  --workspace --exclude ark-desktop --all-targets -- -D warnings` 通过。
- 文档：ark-agent 模块设计的关键约束同步本语义（流式结算、取消保留、
  按 call_id 配对、保留结果经 seed 重放到达模型）。
