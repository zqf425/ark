# ark-mcp 模块设计

## 职责与边界

`ark-mcp` 同时提供外部 MCP 客户端传输、唯一服务端出口和插件间进程内总线。插件注册工具或调用
其它插件只能经 `McpCapability`，不得自建 MCP server 或直连其它插件。

## 核心规则

- 工具名为 `<plugin_id>.<tool>`；重复注册失败，目录是能力自描述的单一来源。
- cross-plugin 调用必须由 caller 的 `mcp.consume` 显式声明；调用链携带 trace，限制深度为 3，
  并拒绝循环和等待图死锁。
- stdio、SSE 与 streamable HTTP 是 transport adapter；领域服务不因协议不同而复制实现。
- 外部写/敏感调用须执行 scope、grant 和宿主审批，调用方传入的 `confirm` 不能自证。

## 双向适配

客户端方向的 stdio、SSE、streamable HTTP transport 统一由 registry 管理连接、重连和关闭；HTTP 传输
复用 `ark-http`，不能另建客户端。服务端方向只做 JSON-RPC、鉴权和作用域适配；应用能力的领域实现
必须与 HTTP handler 共用 service/port，而不是再直接访问 store。

内部 agent 调用本应用能力时走进程内 adapter，不经 JSON-RPC 回环；外部客户端才经 MCP transport。
stdio server 必须独立子进程，避免与主服务争用标准输入输出；远程 HTTP 默认仅 loopback，并要求
运行时 token 和显式远程配置。

## 错误与验证

区分深度超限、调用环、等待图死锁、未声明消费、重复工具名和 grant 拒绝，便于调用方恢复。测试至少
覆盖三种 transport、catalog 同源、跨插件授权、敏感审批、递归拒绝和 handler/HTTP 行为一致性。

## 当前状态

总线、传输和命名空间已是当前模块职责；完整对外 server 路由、全套 grant UI 与所有应用工具
仍应以实际实现为准，不能因早期方案存在而宣称已交付。
