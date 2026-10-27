# ark-llm 模块设计

## 职责与边界

`ark-llm` 适配模型 endpoint、流式协议、usage 解析、重试与消息修复，并实现
`ark-ports::ModelClient`。它不承载 provider 配置的持久化管理，该职责属于 `ark-provider`。

## 实现约束

- endpoint format 在入口只路由一次到 provider family；禁止在巨型函数中堆叠格式分支。
- SSE 帧解析、增量拼接、重试、usage 与消息/工具参数修复各有唯一实现。
- 流必须逐块消费，不能为长响应无界缓冲。
- `cache_hit_rate` 只能读取原生 hit/miss 后计算，不能由 prompt token 猜测。
- 传输经注入的受治理 HTTP 能力复用代理、超时、凭据和审计策略。

## 验证

每个协议族以录制 SSE fixture 覆盖正常分块、畸形帧、断流、重试和 usage 缺失；模拟客户端用于
agent 无网络测试。
