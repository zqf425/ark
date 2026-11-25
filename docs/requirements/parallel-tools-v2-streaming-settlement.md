# 工具并发执行 v2：流式结算与取消保留（parallel-tools-v2）

状态：已批准计划（待检视）。
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

Qoder 的 STE（流式工具执行器，见逆向分析报告）对同样问题的解法是：逐调用
完成即结算、取消时保留已完成结果、批间保持提交顺序。本设计把它移植到
ark 的状态机与事件模型上，不引入 Qoder 的异步迭代器形态——ark 的 reactor
是单步状态机，不是 pull 流。

## 目标与非目标

- 目标一（流式结算）：批内每个调用完成即写配对的 `ToolCallFinished` 与历史行，
  不等待同批其余调用。
- 目标二（取消保留）：取消时已完成调用的真实结果保留落盘，仅未完成调用写
  合成取消行；回合以 interrupted 语义收敛，已保留的结果进入 `ToolsSettled`。
- 目标三（顺序解耦）：结算循环不得假设输出顺序与输入顺序一致，按 `call_id`
  配对，输入顺序决定历史行顺序。
- 非目标：不改批划分规则（`ParallelPolicy` 不动）；不做跨批调度与乱序交付
  （批间顺序、回合历史按模型给定顺序落盘的语义不变）；不做单调用级取消
  传播（整批取消仍是现状语义）；不改 `max_parallel_tool_calls` 分块。

## 设计

### 1. 逐调用结算（run_calls → settle 通道）

`dispatch::run_calls` 改为返回**完成通道**而非完成向量：每个调用包一层
`mpsc` 发送端，完成一条发送一条；`run_batch_events` 消费端循环 `recv`，
收到一条立即写事件与历史。实现上不需要 `FuturesUnordered`：批内所有
future 仍由一个 `tokio::spawn`（或 `join_all` + 转发任务）并发驱动，
转发任务把完成的 `ToolExecution` 推入通道，消费者按到达顺序结算。
`run_calls` 的现有签名（输入向量 → 输出向量）保留为薄包装，供非 reactor
调用方与既有测试使用。

事件顺序约束：批内 `ToolCallFinished` 的落盘顺序变为完成顺序（乱序），
但每条 Finished 的 `item_id` 与对应 Started 的配对关系由 `call_id` 决定，
与落盘顺序无关（store 投影按 item_id 配对，见 ark-store 设计）。历史行
（模型上下文）仍按输入顺序追加：消费者把乱序完成的结果先归位到
`batch` 对应槽位，整批结束时按槽位顺序统一 `append_tool_history`——
这保证"回合历史按模型给出的顺序落盘"的 v1 语义不变。

### 2. 取消保留（select! 分支重写）

`run_batch_events` 的 `tokio::select!` 改为三分支：

- `results = 转发任务 =>`：正常完成，按到达顺序逐条结算；
- `_ = self.cancel.cancelled() =>`：取消分支。此时**不再立即返回**：
  先给转发任务一个短暂 drain 窗口（`tokio::time::timeout` 100ms），把
  已完成但尚未结算的结果收完并按真实结果结算；窗口关闭后仍未完成的
  调用写合成取消行（现状文案）。随后回合按现状返回
  `interrupt_input`，已保留的真实结果随 `ToolsSettled.executions` 上报，
  step 不感知差异。
- 转发任务自身 panic/意外结束：等价于剩余调用全部失败，走合成失败行，
  不吞错误。

drain 窗口的存在理由：`cancel.cancelled()` 与"最后一个 future 完成"存在
竞态，取消瞬间可能恰有调用刚完成但结果还在通道里。100ms 是经验值——
覆盖 tokio 调度延迟，远小于任何真实工具耗时；窗口内新完成的调用同样
保留，窗口外的不等待。不做无限等待：取消的语义是尽快终止，不能被
一个不肯退出的工具（`bash` 无超时）拖住。

### 3. 配对与补偿（open 列表重写）

`open: Vec<&ToolCallRequest>` 改为 `HashMap<call_id, &ToolCallRequest>`：
结算按 `call_id` 移除，不再依赖顺序。`close_open_tool_calls`（journal
失败补偿）与 `converge_pending_calls_on_cancel`（gate 取消兜底）按
map 剩余键工作，语义不变：只对已 Started 未 Finished 的调用补合成行。

### 4. 不变的部分

- `ParallelPolicy`、`ConcurrencyTable`、`max_parallel_tool_calls` 分块：
  v1 语义原样保留。
- 批间顺序：reactor 仍逐批 `await`，批 N 不开始于批 N-1 全部结算完成前。
- `ToolCallStarted` 仍整批先行落盘（v1 已确立的审计与投影约束）。
- 合成结果（storm、gate 拒绝）路径不动。
- 取消文案 `cancelled before the call completed` 仅用于真未完成的调用，
  保留结果的行不写 detail，行为同正常完成。

## 数据与兼容

- 无 wire 契约变更：`ToolExecution`、`RuntimeEvent`、store item 结构不变。
- `ToolsSettled.executions` 在取消回合可能包含真实结果（此前全为合成取消），
  `step` 对 `InputKind::Interrupt` 的处理在 dispatch 之后，不经此路径，无影响。
- store 投影按 item_id 配对 Started/Finished，Finished 乱序落盘不影响
  Changes/File/Terminal 投影与会话重放（ark-store 侧无改动）。

## 验收

- 单测（`tool_batch` / `dispatch`）：批内快调用先于慢调用落 Finished 事件
  （用 `MapToolRunner` + 延迟桩验证事件顺序）；取消时已完成调用保留真实
  内容、未完成调用写合成取消行；历史行顺序与输入顺序一致；输出乱序时
  call_id 配对正确。
- 回归：`sleep 3` x3 并发总耗时 ≪ 9s（v1 已有断言保持）；`cargo test
  --workspace --exclude ark-desktop`、`cargo clippy --workspace
  --exclude ark-desktop --all-targets -- -D warnings` 通过。
- 文档：ark-agent 模块设计的关键约束小节同步本语义（流式结算、取消保留、
  按 call_id 配对）。
