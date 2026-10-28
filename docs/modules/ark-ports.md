# ark-ports 模块设计

## 职责与边界

`ark-ports` 定义核心依赖的抽象边界，依赖仅限 `ark-common` 与 `ark-contract`。这里的 trait
不执行 I/O；具体实现只能由基础能力 crate 或 `arkd` 组合根注入。

端口按领域分组：模型、工具与沙箱、会话存储和事件、子代理、后端集成、HTTP/MCP/UI 总线、
平台服务及工作区/provider/skill 基础服务。

## 硬约束

- `ark-agent` 只能依赖到本 crate，不得引用 `ark-store`、`ark-llm`、`ark-tool` 等实现。
- `BackendProvider` 只表达用例、版本、执行、制品和结果等领域概念；不得引入具体平台字段。
- `SessionStore::subscribe` 返回的 `SubscribeTicket` 必须保证 backlog、水位和 live 通道来自同一序列化临界区。
- `SessionStore::replay_child_completion_events` 只返回 `child_completed` 与
  `child_completion_acknowledged`，供收件箱和线程详情投影使用；不得为此克隆完整转录事件历史。
- 插件只能经 `HttpCapability`、`McpCapability` 和 `UiCapability` 与基础设施或其它插件交互。

## 变更规则

新增或破坏端口前，先评估它是已有端口的扩展还是新稳定边界；破坏性变更必须同步更新实现 crate、
组合根、契约生成物和受影响模块设计。
