# ark-tool 模块设计

## 职责与边界

`ark-tool` 提供工具 schema、注册表、内置工具、输出截断与沙箱策略，并通过
`ark-ports::Tool` 对 agent 暴露统一执行面。

## 关键约束

- 参数先经 schema 解析为强类型，禁止在工具内重复手写字符串解析。
- catalog 的 canonical schema、版本与指纹在变更时计算一次；工具集变化必须显式失效前缀。
- `ToolRegistryPort::schemas()` 输出统一的 function 形状 `{name, description?, parameters}`：JSON Schema 本体放在 `parameters` 内，`description` 仅在工具 schema 声明时输出。openai_chat 适配器将其原样包在 `function` 键下，openai_responses 与 anthropic 各自读取 `name`/`description`/`parameters` 组装 wire 形状，任一适配器都不得假设 schema 裸露在顶层。
- registry 可由组合根按命名分组原子替换动态工具；远程发现不持有 registry 锁，名称与 schema 集合变化才递增 version，重复名称仍返回冲突。
- `SandboxDecision` 在派发前计算一次并写入 `ToolContext`，工具不得重新决策。
- `Strict` 与 `WorkspaceWrite` 的路径先经 `ark_common::path::normalize`，再按真实路径和工作区边界校验；必须拒绝 symlink/junction 逃逸。`Unattended` 派发不可变的 `Unrestricted` 决策，保留畸形路径校验但有意跳过该次调用的工作区包含性检查。
- 真实路径比较在两侧统一移除 Windows verbatim 前缀、规范分隔符并忽略大小写，因此 `\\?\` 根与普通 Windows 路径是同一工作区。
- 文件变更按路径串行、不同路径可并行；所有输出走统一 UTF-8 安全截断。
- `delegate_task` 只消费 `SubagentRunner` 端口，子任务工具集不含它。

## 验证

内置工具覆盖权限拒绝、工作区越界、链接逃逸、截断、多字节文本、并发同路径写入和批准后执行。
