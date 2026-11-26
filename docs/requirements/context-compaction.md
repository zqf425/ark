# 模型上下文压缩

## 状态

- **当前实现**：`ark-agent` 在每个模型轮次前执行确定性 checkpoint；`ark-server` 提供与活动回合互斥的
  手动 `/compact`；`arkd` 从最新持久 checkpoint 重建下一回合历史。
- **当前实现**：完整请求预算保护动态 system 上下文，按 token 与消息条数选择合法尾部，checkpoint 保留
  typed 工具证据与有界用户请求，并以 durable marker 防止来源混淆。
- **待决**：provider usage 估算校准、手动压缩的完整请求规划、压缩可观测性、模型生成的语义摘要、
  跨会话历史检索和计划行来源标记。

## 目标与不变量

1. 压缩只改变模型可见历史，不删除权威事件；存储层物理日志压缩仍由 `ark-store` 独立负责。
2. 项目指令、skill、用户 memory、委派策略、完成收件箱和持久计划等动态系统上下文不得被 checkpoint
   折叠；它们仍不进入 prompt-cache 静态前缀。
3. 自动压缩后的完整请求估算必须低于触发阈值，并为估算误差保留 1024 token 安全余量；工具 schema、
   system 前缀、保护上下文、staged steering、checkpoint、计划重述、最近尾部、消息与请求 framing
   都必须计入同一预算。
4. 最近尾部按 token 预算和消息条数上限共同约束，切分保留完整 tool call/result 配对，不产生孤儿结果。
5. checkpoint 保持零额外模型调用；摘要失败、空摘要和摘要模型费用不属于当前实现的失败路径。
6. 手动压缩不得与活动回合并发；在 `workspace_gate` 内取得 `TurnRegistry` 压缩预留位后即释放 gate，
   预留位同时阻止新回合与第二次压缩，磁盘 I/O 不持 gate。

## 压缩流程

1. 分离静态 system 前缀、动态保护 system 消息、可折叠对话和最近尾部。最新 checkpoint 之前的全部非
   generated system 行和其后的全部普通 system 行保持原样；旧 checkpoint 与计划 seed 进入摘要来源，
   旧计划 restate 不重复保留。
2. 以最新 durable checkpoint 为边界，只把其后新增的原始消息与旧 checkpoint 链式吸收。checkpoint 同时
   摘要当前保留尾部并记录已摘要尾部条数与指纹，仅当原始尾部仍存在且指纹匹配时跳过它们，避免旧尾部
   重复计数，也避免跨回合误跳过新消息。
3. 从完整请求中扣除固定开销，计算尾部与 checkpoint 的可用 token；在消息条数上限内选择最大合法尾部。
   不能以孤立 `Tool` 结果开头，必要时回退到声明调用的 assistant 行。
4. 尾部不足以装下最小 checkpoint 时停止压缩并交由预算状态机返回 `ContextExhausted`，不得继续调用
   已超限的模型请求。
5. checkpoint 摘要可折叠消息，按用户请求 → 结构化事实 → 失败证据 → 工具证据 → 标识符的优先级裁剪；
   旧成功工具结果离开尾部后外化为有界证据，不复制完整输出。
6. 重建历史并重新估算完整请求；只有仍超限的压缩才消耗 `max_compactions`。
7. 自动压缩持久化带 `ContextCheckpoint` 标记的 Notice；手动压缩使用相同历史投影与 checkpoint 提取
   规则，按当前 provider 阈值约束 checkpoint。手动路径不预知下一回合的工具 schema 与动态 seed，因此
   完整请求预算仍由下一回合的自动压缩负责。

## 信息保真

- 用户单行超长时保留有界头尾并标记截断，不整条丢弃；失败诊断同样先稳定截断再去重/计数，
  去重键使用该截断形式，超长错误不会从 checkpoint 消失。
- checkpoint 保留 typed 工具证据：工具名、`path`/`file`/`command`/`query`/`url`/`pattern`/`cwd`
  等标量参数、成功结果的有界首行和输出中提取的标识符；不复制完整大文件或完整工具输出。
- 计划 seed 原样保护，同时进入 checkpoint；回合内生成计划 restate，避免未完成待办随历史折叠消失。
- checkpoint 身份只使用 `ItemMarker::ContextCheckpoint`，普通 Notice 或工具输出中的同前缀文本不能
  伪装 checkpoint。
- 已持久化 checkpoint 中的原始消息不再重复计入失败证据；手动重复压缩不会虚增复发次数。

## 手动压缩并发

1. handler 解析线程 id 后调用 `service::compaction::ensure_checkpoint`，不在 HTTP 层执行历史编排。
2. service 短暂持有 `workspace_gate`，通过 `TurnRegistry::reserve_compaction` 检查活动回合和已有压缩，
   冲突返回 `Conflict`，成功取得 RAII 预留位后立即释放 gate。
3. 读取线程 provider 配置、全部 durable items，生成并持久化 marked checkpoint；成功、失败和取消路径
   都由预留守卫释放线程。

## 验收

- 巨型工具 schema、超长 system 前缀和巨型最近工具结果下，自动压缩要么得到低于阈值的完整请求，
  要么明确返回预算耗尽，不出现压缩后继续超限调用。
- 长回合内多次压缩后，项目指令、skill、memory、委派策略和计划仍原样可见。
- 并行工具调用与连续结果不会产生孤儿 tool result。
- 手动压缩遇到活动回合或另一次压缩返回 conflict；新事件不会在下一轮被静默跳过。
- 同一失败证据跨多次自动压缩不重复计数；工具输出伪造 checkpoint 前缀不会被吸收。
- 仅存在于工具参数中的文件路径和命令可进入 checkpoint；超长单行用户请求不会完全消失。
- 手动 checkpoint 与自动历史投影保持 User/Assistant/Tool 角色，非终态工具调用不回放，工具输出按
  UTF-8 边界有界截断。
