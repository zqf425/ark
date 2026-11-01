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
- `request_user_input` 是无法独立执行的门面工具：schema 为 `{"prompt": string, required}`，
  `execute` 返回结构化错误；生产执行路径永远不会走到它，因为 agent 在派发前拦截该调用并交给用户输入 gate。
- `web_fetch`（需求见 [web-fetch-tool](../requirements/web-fetch-tool.md)）只消费注入的
  `HttpCapability`，以 `ark.builtin.web` 身份走治理出口，自身不持有 HTTP client；SSRF 防线
  （拒绝解析到 loopback/私有/链路本地地址的目标）在工具层，HTTPS-only、超时与大小上限来自
  共享 `HttpPolicy`。父与子工具集都注册它；输出与 bash 同规按 64 KiB 截断，HTML 经内置
  无依赖文本提取，非 2xx/二进制/失败都以结构化工具输出返回，不使回合失败。
- `bash` 执行的是一个进程树而不是一个进程。spawn 前用 `ark_common::process::isolate` 把 shell 设为
  进程组组长，spawn 后立即 `ProcessTree::adopt`；超时、回合取消（future 被 drop）都由该句柄发
  `killpg(SIGKILL)` 杀掉整组，`kill_on_drop(true)` 仅作为直接子进程的兜底保留。这样
  `sh -c 'server &'` 留下的后台进程不会继续持有继承来的 stdout/stderr 管道。成功回收后必须
  `disarm`：pid 可能被复用，不能再发组信号。输出收集与超时语义不变（仍是
  `timeout(wait_with_output)`，超时返回带 `timeout` detail 的结构化错误）。

## 验证

内置工具覆盖权限拒绝、工作区越界、链接逃逸、截断、多字节文本、并发同路径写入和批准后执行。

`bash` 另有 Unix 进程树回归：命令派生孙进程（`sleep 30 & echo $! > …; wait`）并记录其 pid，超时后
孙进程必须退出；外层 `tokio::time::timeout` 取消调用时同样必须退出。Windows 的 `taskkill /T /F`
分支只有编译覆盖，需在 Windows 上单独验证。
