# 模块设计索引

每个 workspace 模块在本目录有且只有一份设计文档。文档描述当前实现的职责、边界、
关键实现约束和验证方式；已批准但尚未实现的目标必须明确标为“计划”，不得写成现状。

修改代码前，除阅读 [整体架构](../rust-rewrite-design.md) 外，必须阅读所有受影响模块的
设计文档；跨模块修改还要阅读相邻模块和端口模块的文档。

| 模块 | 设计文档 | 主要职责 |
|---|---|---|
| `ark-common` | [ark-common.md](ark-common.md) | 基础类型、编码、路径与用量 |
| `ark-contract` | [ark-contract.md](ark-contract.md) | 跨进程 wire DTO 与 TS 生成 |
| `ark-ports` | [ark-ports.md](ark-ports.md) | 核心依赖的端口 trait |
| `ark-config` | [ark-config.md](ark-config.md) | 配置快照与热更新分级 |
| `ark-agent` | [ark-agent.md](ark-agent.md) | 唯一回合决策循环 |
| `ark-delegation` | [ark-delegation.md](ark-delegation.md) | 子代理生命周期与并发门 |
| `ark-store` | [ark-store.md](ark-store.md) | 会话权威日志与派生索引 |
| `ark-llm` | [ark-llm.md](ark-llm.md) | Provider 适配与流解析 |
| `ark-provider` | [ark-provider.md](ark-provider.md) | Provider 配置与连通性探测 |
| `ark-tool` | [ark-tool.md](ark-tool.md) | 工具注册、沙箱和内置工具 |
| `ark-http` | [ark-http.md](ark-http.md) | 受治理的出站 HTTP 能力 |
| `ark-mcp` | [ark-mcp.md](ark-mcp.md) | MCP 客户端、统一出口和插件总线 |
| `ark-ui` | [ark-ui.md](ark-ui.md) | 页面/面板注册总线 |
| `ark-plugin` | [ark-plugin.md](ark-plugin.md) | 插件清单、隔离与注册 |
| `ark-skills` | [ark-skills.md](ark-skills.md) | Skill 发现与激活 |
| `ark-workspace` | [ark-workspace.md](ark-workspace.md) | 工作区检查与 Git 状态 |
| `ark-obs` | [ark-obs.md](ark-obs.md) | tracing 与用量聚合 |
| `ark-server` | [ark-server.md](ark-server.md) | HTTP/SSE 边界与优雅停机 |
| `ark-studio` | [ark-studio.md](ark-studio.md) | UI 自动化领域模型（计划中） |
| `arkd` | [arkd.md](arkd.md) | 唯一组合根 |
| `ark-desktop` | [ark-desktop.md](ark-desktop.md) | Tauri 窗口与本地运行时生命周期 |
| `web` | [web.md](web.md) | React Code 模式渲染层 |
