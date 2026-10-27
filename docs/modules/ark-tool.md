# ark-tool 模块设计

## 职责与边界

`ark-tool` 提供工具 schema、注册表、内置工具、输出截断与沙箱策略，并通过
`ark-ports::Tool` 对 agent 暴露统一执行面。

## 关键约束

- 参数先经 schema 解析为强类型，禁止在工具内重复手写字符串解析。
- catalog 的 canonical schema、版本与指纹在变更时计算一次；工具集变化必须显式失效前缀。
- `SandboxDecision` 在派发前计算一次并写入 `ToolContext`，工具不得重新决策。
- 所有路径先经 `ark_common::path::normalize`，再按真实路径和工作区边界校验；必须拒绝
  symlink/junction 逃逸。
- 文件变更按路径串行、不同路径可并行；所有输出走统一 UTF-8 安全截断。
- `delegate_task` 只消费 `SubagentRunner` 端口，子任务工具集不含它。

## 验证

内置工具覆盖权限拒绝、工作区越界、链接逃逸、截断、多字节文本、并发同路径写入和批准后执行。
