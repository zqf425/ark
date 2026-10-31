# ark-llm 模块设计

## 职责与边界

`ark-llm` 适配模型 endpoint、流式协议、usage 解析、重试与消息修复，并实现
`ark-ports::ModelClient`。它还规范化显式请求的 provider model catalog；不承载 provider 配置的持久化管理，
该职责属于 `ark-provider`。

## 实现约束

- endpoint format 在入口只路由一次到 provider family；禁止在巨型函数中堆叠格式分支。
- SSE 帧解析、增量拼接、重试、usage 与消息/工具参数修复各有唯一实现。
- 流必须逐块消费，不能为长响应无界缓冲。
- `cache_hit_rate` 只能读取原生 hit/miss 后计算，不能由 prompt token 猜测。
- 传输经注入的受治理 HTTP 能力复用代理、超时、凭据和审计策略。
- model catalog 只接受远端显式返回的 context/modalities 元数据；通用 `/models` 只有 id 时，能力字段必须是
  unknown。GET 请求 10 秒超时、响应最大 1 MiB、模型最多 500 个，错误不得回显可能携带 query secret 的 URL。
- 自定义 provider header 可以补充请求，但不得覆盖 endpoint family 的认证、版本或 content-type 头；POST 的
  `Content-Type: application/json` 仅由 HTTP 请求构造器写入一次，避免严格网关因重复同名头而拒绝解析 JSON。
- assistant 侧 `tool_calls`（`ark_ports::ToolCallRef`）按 endpoint family 序列化为原生 wire 形状：openai chat
  用 `tool_calls` 数组（arguments 为 JSON 字符串），anthropic 用 assistant content 的 `tool_use` block，
  openai responses 在 assistant 项后展开为独立 `function_call` 项。旧 JSON 无该字段时行为不变；
  `repair_messages` 的孤立 tool 结果丢弃规则继续生效。

## 验证

每个协议族以录制 SSE fixture 覆盖正常分块、畸形帧、断流、重试和 usage 缺失；模拟客户端用于
agent 无网络测试。
