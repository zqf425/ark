# ark-agent 模块设计

## 职责与边界

`ark-agent` 是全进程唯一的模型决策循环。它只依赖 `ark-ports` 定义的模型、工具、事件、
会话和闸门端口；模型调用、文件、网络和持久化实现由 `arkd` 注入。

## 回合状态机

回合使用显式状态而非布尔标记：`Preparing`、`AwaitingModel`、`DispatchingTools`、
`AwaitingApproval`、`AwaitingUserInput`、`Compacting`、`Finalizing` 和终态
`Completed`/`Failed`/`Aborted`。`step(state, input) -> Action` 保持纯逻辑，`Reactor`
是唯一把 action 转为 I/O 的位置。

取消由 `CancelToken`（基于 `tokio::sync::watch`，实现随控制原语下沉在 `ark-ports`、由本 crate 根
re-export，刻意不引入 tokio-util 依赖）与 `select!` 完成；
任何退出路径都必须写终态并清理活动注册，不得留下“磁盘记录 running 但任务已丢失”的状态。

gate broker 面（`exec/gate.rs`）包括 `AllowAllGates`、`DenyAllGates` 与 `AutoDenyGates`
（[审批规则需求](../requirements/approval-rules.md) §4）：后者包装任意内层 broker，
无人值守处置为 `deny`（默认）时不注册 gate、立即以结构化原因拒绝，`allow` 时透传内层；
组合根在每个父回合入口选择 broker：`serve` 的显式 User 回合直接使用 `InteractiveGates`；headless、
Scheduled、IM、`AutoContinuation`、`AutoRecovery` 以 `AllowAllGates` 为内层、按该回合当前配置快照
构造 `AutoDenyGates`；child 保持
`AllowAllGates`。因此规则表与 `on_unattended` 的改动都从下一回合 Hot 生效。
`prepare_user_input` 恒透传（用户输入不是审批面）。规则拒绝（`SandboxDecision::Deny`）
的调用在 reactor 的 `approval_asks` 阶段就不产生审批 ask——拒绝已是终态，开 gate 等于允许
HTTP 侧批准一条策略表明确拒绝的调用。

## 关键约束

- 子代理不是第二个 agent：它是同一 `Reactor` 驱动的独立 child turn。
- 工具沙箱决策只在派发前计算一次，执行器仅消费不可变结果。
- `request_user_input` 在 settle 阶段即被拦截为 `Action::AskUser`（契约参数 `{"prompt": string}`，
  兼容别名 `question` 仍被接受），不进入分派计划；答案经 `UserInputResolved` 回填为该调用的
  tool result。助手轮次的工具调用使用 `ModelMessage::tool_calls` 结构化字段进入历史，不再渲染进正文文本。
- 预算、工具风暴、并发安全和历史裁剪属于独立 policy/context 模块，不得回流为巨型 reactor。设置/配置层的默认回合预算为最多 1000 步、累计 token 不限（`0`）和最多 1000 次工具调用；组合根将 `max_tokens=0` 转换为 agent 内部的 `u64::MAX`，以避免直接使用 `TurnLimits::default()` 时将首轮判定为耗尽。
- 工具风暴按工具名与规范化参数组成的签名计数；跨轮已执行次数与同一 `DispatchPlan` 内先前出现次数共同计入上限。被抑制调用必须以合成结果配对 `ToolCallStarted`/`ToolCallFinished`，并写入 `ToolStormSuppressed` 事件。重复计数默认覆盖整个回合（`tuning.tool_storm_window_size = 0`）；配置正数窗口时，只统计最近 N 个模型轮次内实际执行过的调用。计数键是 storm key：bash 调用先剥掉命令开头的 `sleep N;` 等待前缀再归一化空白（`ToolCallRequest::storm_key`），其他工具用字节级签名——否则改一个 sleep 时长就能绕过熔断（实测 `sleep 300;git push` → `sleep 590;git push` 循环 60 分钟）。被抑制调用的参数必须写入事件（合成结果携带原始 `arguments`），事后审计要能还原重复的是什么。
- 回合在模型请求失败（`model_request_failed = true`）终结时，由 ark-server 的恢复调度器自动重试同一请求：退避计划 5s/10s/20s（`recovery::RECOVERY_DELAYS`），尝试次数写进 `turn_started.auto_recovery`；连续模型失败达到 `RECOVERY_MODEL_FAILURE_STREAK_LIMIT` 时暂停该线程的自动恢复，任何用户回合或非模型终态重置计数。恢复回合在会话流写一条 Notice item 说明自动重试原因；它不为已在历史末尾的同一条用户请求再写 `UserMessage`（由 `arkd` 组合根去重，见 [arkd](arkd.md)），因此模型上下文与用户气泡都不会随重试次数增长。工具/策略类失败与耗尽尝试预算后的失败保持终态，仍需用户介入。
- 只读工具的并发批默认不设上限；`tuning.max_parallel_tool_calls > 0` 时批内调用被切成顺序 chunk，批内并发不超过该值，批间顺序不变。
- 每轮模型可见历史必须已经落盘；每个工具调用必须与终态结果配对。历史修复只按配对合法性裁剪：携带 `tool_calls` 的 assistant 行即使 `content` 为空也必须保留（chat-completions 的标准形态），只有既无文本又无 `tool_calls` 的空行才删除。
- 压缩保留 system 前缀和 `keep_recent_messages` 条末尾消息的原始内容；被折叠消息生成的 checkpoint
  链式继承：折叠区内旧 checkpoint 的四个段整体吸收进新 checkpoint（`[user requests]` 保真迁移、
  结构化行归入 goals、段标题与 `[N earlier messages compacted]` 账目行不再重排）——纯文本用户
  请求不匹配任何结构化前缀，不链式继承则第二次压缩会静默丢掉最早的目标请求（实测会话 cb274b9d
  由此失忆）。吸收必须幂等且不 bump 计数：手动 `/compact` 每次重读全部 items,重复吸收同一条
  失败证据不得虚增复发次数。预算截断的优先级为用户请求 → 结构化事实 → 失败证据 → 标识符：
  标识符是可达碎片，最先被裁；截断渲染为每段重放段标题（否则截断代被再吸收时无段 bullet 全部
  丢失，链条在该代断开）；用户请求段超配额时保最旧 6 条与最新若干条，中段以
  `[N middle user requests folded]` 显式声明丢弃。失败证据只收真实诊断行：词边界匹配
  （`failed:` 接受、`failedValue` 拒绝）叠加代码形态拒绝（序列化 JS 行以 `,`/`;` 结尾且冒号后
  是标识符/字面量，如 `failed: null,`、`Failure: null,`；尾分号/`instanceof`/裸 ` => `；
  rustc 的反引号签名诊断保留），数字变体仅在计数语境归并（数字 run 前最近 token 以
  `count`/`actual`/`got`/`expected`/`实际` 等**后缀**匹配，组合词 `actionCount` 同样触发；
  错误码 `A3/A4`、HTTP `401/500`、路径版本 `v2/v3` 等语义数字保持独立），归并
  计数以 ` (repeated xN)` 后缀呈现（字面 `[xN]` 数据不受影响）。并受 `context_limit_tokens` 约束。
  后续压缩会替换已有 checkpoint，不会把它当作稳定 system 前缀累积。预算不足时 checkpoint 明示已裁剪，且只在 UTF-8 字符边界裁剪；`compaction_completed` 复用
  `removed_items` 报告折叠消息数，并报告 checkpoint 字符数。回合种子尾部的持久计划行
  （`[thread plan - current objective and progress]` 的 `Goal:` 与 `- [ ]`/`- [~]`/`- [x]`
  checkbox 行）同样被识别为结构化事实：checkpoint 摘要整个历史（含种子尾部），把这些行带上
  才能让回合内压缩不丢失未完成待办。折叠区含计划行时，`compact_history` 还会把
  `checkpoint.plan_restate()`（计划行从**整个** entries 过滤后再截独立配额 `MAX_PLAN_LINES`，
  最新目标行最先输出不被配额截断，配额截断显式标注；同一待办的状态变体与前后目标按标题
  **后出现者胜**，标题剥离用精确 marker 前缀而非字符集，避免吃掉标题首字符）作为尾部 system
  消息 push 到保留尾部之后（先按 `PLAN_RESTATE_PREFIX`＝`"[thread plan]\n"` 剔除旧 restate
  再 push——该标记**不匹配**种子标题（种子多 `- current objective and progress` 后缀），
  稳定前缀区的种子行永不误删；hygiene 的相邻重复折叠只是兜底，剔除本身是唯一性保证）。
  这是纯内存行为，不落 journal，下一回合重建种子时由 `plan_seed_message` 在种子尾给出当前计划，
  因此不会跨回合堆叠。"尾部近因"是适配器相对的：OpenAI chat/responses 族保留为最后一条消息，
  Anthropic 族会把 system 行提升进顶层 `system` 字段（仍是高注意力位置，但不是字面尾部）。
  已知限制：来自项目指令或工具输出的任意 markdown checkbox 行同样会被识别为计划行
  （`is_structured_line` 自 master 起如此），带待办清单的 README 可能被提升进 restate——
  状态变体折叠保证不会自相矛盾，但第三方待办可能混入"当前计划"；按来源过滤需行级来源标记，
  留待独立需求评估。
- 压缩判定使用唯一估算器 `TokenEstimator`，它必须计入 `tool_calls` 的 `call_id`/`name`/`arguments`
  与每条消息的图片部分（按固定 `IMAGE_PART_TOKENS` 计，不按 base64 长度计——视觉模型按几何计费）：
  编码会话的体积主要来自工具参数（一次 `write` 携带整个文件、一次 `bash` 携带整个脚本），只计 `content`
  会使估算约为真实值的一半，阈值永远达不到而 provider 早已逼近窗口上限。
- 压缩检查发生在**每个**模型轮次之前，不只在 `Preparing`：`step` 的 `compact_or_call` 是唯一入口，
  `begin`、`after_tools`、`after_user_input`、`after_compaction` 都经它决定先折叠还是调用模型
  （`gate_resolved` 只派发工具，其结果回到 `after_tools` 时同样过这道门）。
  回合内工具循环会持续增长前缀，只在回合开始检查会让单个回合在阈值之上继续增长。
  `dispatch` 在本轮结果进入历史后重新估算 `prompt_estimate`，否则 `ToolsSettled` 上的判定
  会滞后一轮（用的是本轮输出产生之前的数字）。判定用 `>=`：配置值本身就是触发点。
- `max_compactions` 只约束**未能把前缀压回阈值以下**的压缩（无可折叠项，或保留尾部本身就超阈值），
  用于阻断死循环；压回阈值以下的压缩不计入，因此长回合可以随自身增长反复折叠，不会在阈值处被预算判死。
- 压缩切分点按 call/result 配对回退：若 `keep_recent` 落在 `tool` 行上，切分点前移到最后一条
  非 `tool` 行，保证保留尾部不会出现「没有声明者」的孤儿 tool result（provider 会拒收）；
  整段可折叠区只有一组调用时报告 `removed_items = 0`，不落空边界。
- 每个批次在执行器启动前持久化全部 `ToolCallStarted` 已校验参数，`ToolCallFinished` 持久化受工具上限限制的输出和 detail；这既是
  会话重放资料，也是服务端 Changes/File/Terminal 投影的唯一来源，不能由 web 解析自由文本补造。
- 同一回合的正文 delta 与 reasoning delta 使用各自稳定的 item ID；两类内容不得合并为同一个渲染条目，
  以保证默认折叠的 reasoning 不会遮蔽用户可见正文。
- 每个模型轮次创建的正文和 reasoning 条目在该轮结算时写入最终文本并完成；失败、取消和其它回合终态也会完成仍打开的条目，工具循环的下一轮使用新的 item ID。正文条目以 provider 最终响应文本为权威：即使已流式输出过 delta，完成条目也替换为最终文本；provider 不输出 delta 但最终文本非空时，仍创建并完成正文条目。
- checkpoint 的用户请求、结构化事实和标识符去重使用伴随 `Vec` 的 `HashSet`，保持原有顺序与输出语义，避免长历史上反复 `Vec::contains` 的平方级扫描。
- 工具批次在任何执行器启动前持久化全部 `ToolCallStarted`。若后续 Started/Finished 追加失败，reactor 只对已经 durable 的 Started 调用尽力补写配对 Finished，避免重放留下永久孤儿；补偿失败保留原始存储错误。
- reactor 在回合开始时从注入的 `ModelClient` 取 `provider:model` 快照（`model_label_of`），盖在开场的
  `UserMessage` item 与 steer 补写的用户 item 上（`ThreadItem.model`）：标签是回合时快照，模型切换只影响
  下一回合，回放后 UI 才能与 live 流一致地显示"这条消息发给了哪个模型"。
- 每轮模型调用在 `pump()` 内实测墙钟耗时（连接 + 流式全程），随该轮结算的 usage 事件写入
  `Usage.duration_ms`——它只覆盖模型调用本身，不含工具执行；取消/失败的轮次没有 usage 事件，也就没有
  耗时。服务端 llm-rounds 投影与 web 会话统计条都只汇总这个实测值，渲染层从不自行计时。
- 新回合须从权威线程项恢复此前的 user/assistant 对话，并在回合启动时持久化当前用户消息；reasoning 不进入
  后续模型历史。会话级工作空间、模型和权限覆盖只在创建新回合时读取，运行中回合保留其创建快照。
- parent steer 的有界 `SteerQueue` 由 server registry 创建并由 `arkd` 注入同一个 reactor；reactor 只在
  轮次边界 drain：先把 steer 文本写入用户历史（并落一条 `UserMessage` item），随后才写 `turn_steered`
  事件——journal 不能声称一条模型尚未见到的更正。steer 落在最后一个模型轮内时（无工具调用的 settle 直接
  终结回合，队列不再有 drain 机会），reactor 在 `Finish(Completed)` 前把待投递更正补 drain 并转入一个
  零批次 dispatch，驱动下一个轮次消费该文本；该补投以「未取消且预算仍 Allow」为前提，取消或预算不可
  负担时不补投、不写账，终态沿用本次 Finish（journal 不声称模型未收到的更正）。队列条目是
  `SteerEntry { text, attachment_ids }`：steer 可引用附件（路由已按线程授权），reactor 在轮次边界把每条
  steer 暂存为独立的文本、图片与投递结果，并在进入模型调用前重新经过上下文 gate；组合根注入的同步
  resolver 只解析一次，同时返回 `ModelImage` 与 `TurnAttachmentOutcome`，结果随补写的 `UserMessage` item
  持久化。同一回合的初始种子与所有 steer 共享 20 MiB 图片预算，后到的 steer 不能重置预算，也不能串用
  前一条 steer 的图片。取消一轮时整条目 restore，附件与文本不拆散。agent 自身不接触文件系统。
- 子代理完成 inbox 从父线程权威事件投影为带标记的 system source，而非用户消息；按稳定通知 ID 去重，
  跳过已确认项，并且每个父回合最多交付 8 条、每条最多 1,200 UTF-8 字节，避免完成风暴突破上下文
  预算。同一条通知最多自动投递 2 次（`MAX_AUTO_DELIVERY_ATTEMPTS`），超限后只在用户显式回合投递；
  模型请求连续失败达到熔断阈值时暂停自动续跑，避免在 provider 故障时烧空预算。
- 每个实际启动的回合（含自动续跑）必须在父线程事件日志中留下 `turn_started` 与一条终态记录。
  reactor 覆盖不到的退出路径（panic、任务被丢弃、`prepare()` 之前的 runner 错误）由 server supervisor
  补齐：缺起始记录则先补写（保留 `auto_continuation` 标记），已有终态则不再重复写入。

## 验证

状态迁移和 policy 均应脱离 I/O 单测；取消、审批、工具失败、模型流失败和持久化失败须覆盖
收敛到终态的路径。reactor 的内联单测宿主在 `service/reactor/tests/reactor_stream_tests.rs`
（经 `#[path]` 挂回 `reactor.rs` 的 `tests` 模块），gate 与回归套件同目录并列；`reactor.rs`
与测试文件分别遵守单文件行数上限。
