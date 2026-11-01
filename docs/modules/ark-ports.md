# ark-ports 模块设计

## 职责与边界

`ark-ports` 定义核心依赖的抽象边界，依赖仅限 `ark-common` 与 `ark-contract`。这里的 trait
不执行 I/O；具体实现只能由基础能力 crate 或 `arkd` 组合根注入。

端口按领域分组：模型、工具与沙箱、会话存储和事件、子代理、后端集成、HTTP/MCP/UI 总线、
平台服务及工作区/provider/skill 基础服务。

`AttachmentStore` 是附件字节与元数据的专用边界：上传端在交给实现前解码 transport Base64，读取端
只能通过服务端生成的 `AttachmentId` 取得元数据或原始字节。端口不接受路径，因此 HTTP 或 agent
层不能借此访问任意本地文件。存储只拥有字节与元数据，不拥有线程：所以 `diagnostics` 的已关联集合
由调用方（能读事件日志的边界层）传入，`orphan_count` 只统计不在该集合内、且超过孤儿阈值的条目。

## 硬约束

- `ark-agent` 只能依赖到本 crate，不得引用 `ark-store`、`ark-llm`、`ark-tool` 等实现。
- `BackendProvider` 只表达用例、版本、执行、制品和结果等领域概念；不得引入具体平台字段。
- `SessionStore::subscribe` 返回的 `SubscribeTicket` 必须保证 backlog、水位和 live 通道来自同一序列化临界区。
- `SessionStore::replay_child_completion_events` 只返回 `child_completed` 与
  `child_completion_acknowledged`，供收件箱和线程详情投影使用；不得为此克隆完整转录事件历史。
- `SessionStore::replay_interaction_events` 只返回 turn 起止与 approval/user-input 生命周期，供 detail 恢复
  使用；是否仍可操作必须再由进程内 `InteractiveGates` waiter 快照判断。
- `SessionStore::replay_attachment_events` 只返回 `attachment_associated`，是线程详情投影、回合起点授权与
  删除守卫共用的唯一窄查询；关联不进第二张表，重启后回放同一事件流即可恢复同一授权集合。
- `SessionStore::rewind_thread` 是物理截断边界：实现必须在同一 writer 序列化点删去目标及之后的权威事件、
  重建投影并发出可使 SSE 客户端失效旧状态的持久化通知；实现还必须拒绝有未终结持久化回合的线程，且
  `rewind_record` 的幂等标记不得随普通事件压缩丢失。
- 插件只能经 `HttpCapability`、`McpCapability` 和 `UiCapability` 与基础设施或其它插件交互。
- `McpServerService` 的 upsert/delete 是异步边界：实现可以在返回前发现远程工具并原子刷新运行时 catalog，
  但不得在 registry 锁持有期间 await。
- `MemoryService` 管理用户明确维护的跨会话长期偏好/事实。未注入的构建必须让 HTTP 层返回结构化
  `runtime_unavailable`，不得返回伪造的空成功结果。
- `PluginHostService`（`base.rs`）只暴露一个 `report()`，产出 `PluginHostReport`，让 `ark-server` 在不链接
  插件加载 crate 的前提下回答 `GET /v1/runtime/plugins`。端口故意不含 enable/disable 或 reload：宿主只在
  启动时装载一次，卸载是 `ark-plugin` 的服务操作而非平台路由。它随 `BaseServices::plugins`
  （`Option`）注入组合根，未装配时的空列表与“确实没有插件”必须可区分，因此实现未注入时 HTTP 层返回
  `runtime_unavailable`。报告中的贡献项从真正拥有它的总线读取，不得保留平行清单。
- `latest_child_runs`（`store.rs`）是纯函数折叠，不是端口方法：`children/runs.jsonl` 是追加日志，一个 child
  合法拥有多行（`queued`/`running`/终态，以及每次 `resume` 追加的一行），因此 `list_child_runs` 的结果是
  **一行 = 一次状态转换**，而不是一个 child。任何计数或列表投影必须先经该函数折叠：保留每个 id 最后
  看到的那行（日志有序，即最新状态），并按首次出现顺序输出，避免刷新间列表跳动。直接把原始行映射成
  列表会把同一个 child id 重复多份。
- `SteerHandle` 同时提供 `len` 与默认实现的 `is_empty`，调用方经 `dyn SteerHandle` 判断队列是否为空，
  不必自行写 `len() == 0`。

## 变更规则

新增或破坏端口前，先评估它是已有端口的扩展还是新稳定边界；破坏性变更必须同步更新实现 crate、
组合根、契约生成物和受影响模块设计。
