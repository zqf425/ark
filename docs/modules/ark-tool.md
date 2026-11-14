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
- 审批规则表（[需求设计](../requirements/approval-rules.md)）的匹配与裁决宿主是
  `policy/approvals.rs`（纯函数，唯一实现）：规则按顺序取首条命中，`pattern` 为空匹配一切、
  尾随 `*` 为前缀匹配、其余等值；`bash` 匹配 `command`，其余工具匹配按键序规范化的 JSON。
  `Verdict::Allow` 只能把 `NeedsApproval` 放宽为 `Unrestricted`，`Deny` 直接改判
  `SandboxDecision::Deny`，两者都不改变只读/写基线。`ApprovalPolicy` 是包装既有
  `BuiltinSandboxPolicy` 的策略实现，由组合根每回合用配置快照构造（规则属 Hot 配置）。
  runner 对 `Deny` 决策与 `NeedsApproval` 同点防御性拒绝，事件配对不受影响。
- `Strict` 与 `WorkspaceWrite` 的路径先经 `ark_common::path::normalize`，再按真实路径和工作区边界校验；必须拒绝 symlink/junction 逃逸。`Unattended` 派发不可变的 `Unrestricted` 决策，保留畸形路径校验但有意跳过该次调用的工作区包含性检查。
- 真实路径比较在两侧统一移除 Windows verbatim 前缀、规范分隔符并忽略大小写，因此 `\\?\` 根与普通 Windows 路径是同一工作区。
- 工具并发由 `ark-agent` 的 `ParallelPolicy` 统一规划（设计刻意保守）：任何变更类调用独占一个批次，
  即使不同路径也不并行；只有**不同资源**的只读调用可共享批次，同资源的只读调用按资源串行。
  未知工具名一律按变更类处理。所有输出走统一 UTF-8 安全截断。
- `delegate_task` 只消费 `SubagentRunner` 端口，子任务工具集不含它。它另支持后台派发
  （`background: true`，立即返回 queued，完成经完成收件箱投递）与恢复（`resumeChildId`，优先级高于
  `background`）；两者均复用同一端口边界。
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
- 持久 shell 会话（[需求设计](../requirements/shell-sessions.md)）由
  `exec/tools/shell_session.rs` 承载：`bash_session`（前台复用 `bash` 语义、leading `cd`
  推进会话 cwd；`run_in_background` 以 std Command 派生独立进程组并立即返回 task_id）、
  `bash_output`（按读者 offset 增量返回输出与 running/finished 状态）、`bash_kill`
  （组信号 + direct kill 兜底，幂等）。`ShellSessionManager` 是进程内运行态（不入盘、不进
  契约）：环境变量**不**跨调用保持（Windows 无可移植 shell 协议，需求 §2 显式记录）；
  输出环形缓冲 64 KiB 超限丢最旧并打标记；任务数上限 16、仅淘汰已完成任务。
  组合根以 `builtins_with_shell_session()` 装配父/子两个注册表（各自独立 manager 实例）。
- `find`/`grep` 的递归遍历跳过固定忽略名单（`target`、`node_modules`、`dist`、`.git`、`.tmp`、
  `.worktrees`，见 `exec::common::IGNORED_DIR_NAMES`）。名单在搜索根之下的每一层生效；把 `path`
  显式指进被忽略目录仍会扫描。该名单既是防噪（minified bundle 命中会灌爆上下文）也是防量：忽略前
  一次 grep 遍历约 41 万个文件（实测 11.9 秒纯遍历），忽略后约 931 个（19 毫秒）。
- `find`/`grep` 的同步目录扫描经 `tokio::task::spawn_blocking` 执行并受共享扫描预算约束
  （`exec::common::BlockingScan`，复用 bash 的 clamp 超时）。直接在 async 执行器里扫描曾把 tokio
  worker 阻塞十余分钟，回合取消信号无法被 poll，"停止"按钮杀不掉 grep；扫描函数在目录与文件粒度
  检查 `ScanDeadline::is_expired`，超时返回带预算毫秒数的结构化错误。
- `grep` 是无正则引擎的纯子串扫描。pattern 含正则元字符（`. * + ? ( ) [ ] { } | ^ $ \`）时返回
  结构化错误并指向 `bash` 的 `grep -E`/`-F`，不再静默按字面量匹配——那会产出"no matches"假阴性
  （实测一次 686 秒扫描换来错误结论）。命中的单行按匹配位置截断到 400 字符（固定前缀会把匹配本身
  截掉），防止单行 minified 文件绕过行数上限灌爆模型上下文（实测一次拉入 254k 字符）。

## 验证

内置工具覆盖权限拒绝、工作区越界、链接逃逸、截断、多字节文本、并发同路径写入和批准后执行。

`bash` 另有 Unix 进程树回归：命令派生孙进程（`sleep 30 & echo $! > …; wait`）并记录其 pid，超时后
孙进程必须退出；外层 `tokio::time::timeout` 取消调用时同样必须退出。Windows 的 `taskkill /T /F`
分支只有编译覆盖，需在 Windows 上单独验证。

`find`/`grep` 另有：忽略名单回归（六个目录不进、显式 `path` 指进被忽略目录仍可搜）、正则元字符
pattern 拒绝（含交替 `|` 场景）、单行 bundle 命中按匹配位置截断（输出长度有界且仍含 needle）。
`grep` 的 blocking 执行正确性由并发测试与 `bash` 树回归共同覆盖；扫描超时路径依赖 `BlockingScan`
的共享预算，与 bash 超时同一 `clamp_timeout` 常量，不单测墙钟。
