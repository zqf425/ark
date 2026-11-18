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
- `edit` 的锚点匹配前把文件内容与 `old_string` 都归一到 LF，写入时再按文件原有行尾恢复
  （文件是 CRLF 就写回 CRLF，是 LF 就保持 LF）。模型按 `\n` 组织锚点，而 Windows 检出
  （`core.autocrlf=true`）的工作区是 CRLF，字节级精确匹配会让每一个多行锚点都失败：实测一天
  21 次 `edit` 仅因该原因报 `old_string not found`，模型只能改用显式 `\r` 重发。
- `write`/`edit` 的成功回执在字节数之外附一行级变更摘要（相对写入/替换前内容的行数差，
  `+N lines`/`-N lines`/`line count unchanged`，fresh write 无摘要）：模型上下文里已有全文，
  回执唯一能补充的信号是"这次操作改动了什么形状"——缺失时模型对自己刚撤销上一步毫无感知
  （实测同一 import 块改→撤销→再改→再撤销翻转 4 次）。
- `delegate_task` 只消费 `SubagentRunner` 端口，子任务工具集不含它。派发模式按优先级决定：模型显式
  `background` 参数优先；缺省时读取注入的 `SubagentPolicySource` 的 `default_background`（Hot，
  对应 `subagents.default_background`，默认开启=后台派发）；未注入策略源时保持前台默认。
  恢复（`resumeChildId`，优先级高于 `background`）始终走前台结果契约；两者均复用同一端口边界。
- `request_user_input` 是无法独立执行的门面工具：schema 为 `{"prompt": string, required}`，
  `execute` 返回结构化错误；生产执行路径永远不会走到它，因为 agent 在派发前拦截该调用并交给用户输入 gate。
- `web_fetch`（需求见 [web-fetch-tool](../requirements/web-fetch-tool.md)）只消费注入的
  `HttpCapability`，以 `ark.builtin.web` 身份走治理出口，自身不持有 HTTP client；SSRF 防线
  （拒绝解析到 loopback/私有/链路本地地址的目标）在工具层，HTTPS-only、超时与大小上限来自
  共享 `HttpPolicy`。父与子工具集都注册它；输出与 bash 同规按 64 KiB 截断，HTML 经内置
  无依赖文本提取，非 2xx/二进制/失败都以结构化工具输出返回，不使回合失败。
- `web_fetch` **自己走 3xx 跳转**：向出口请求时始终带 `follow_redirects: Some(false)`，然后逐跳
  重新解析主机并重做 SSRF 校验（预算 5 跳，与 `HttpPolicy::max_redirects` 对齐）。出口自身的
  跳转只校验 scheme 与 allowlist，而本工具的 egress 身份是 `allow_all()`，交给它跟随会让
  `https://169.254.169.254/` 直达；把 3xx 原样返回又会让普通 `http`→`https`、尾斜杠跳转全部
  变成失败（实测一天 12 次 `web_fetch` 死在 `request returned status 301`，模型随后用整轮手工
  推导最终 URL）。3xx 缺可用 `Location` 时按终态渲染状态码，超过预算返回结构化错误。
- `bash` 在所有支持的平台执行 POSIX shell 命令：Unix 使用 `/bin/sh -c`，Windows 从 `PATH` 中的
  Git for Windows `cmd/git.exe` 反查同一安装目录下的 `bin/bash.exe` 并使用 `-c`。不得直接选择
  `bash.exe`，因为 `C:\Windows\System32\bash.exe` 是 WSL 启动器，既不能保证可用也不能执行
  Git Bash 命令。这样模型可在各平台使用相同的 `ls`、`grep` 等命令，Git Bash 的 UTF-8 输出也不会
  被 `cmd` 的本地代码页破坏。找不到 Git Bash 时返回明确的结构化工具错误。
- `bash` 与 `bash_session` 的子进程输出经 `exec::common::decode_shell_output` 解码，不用
  `String::from_utf8_lossy`。子进程输出是字节：中文 Windows 上 `cmd`、`mvn`、`python` 等按
  活动 ANSI 代码页（GBK/CP936）写字节，`from_utf8_lossy` 会把每行变成 U+FFFD，报错原文
  不可恢复——实测一天内 111 次失败 `bash` 中 19 次带替换字符，另有 99 次「成功」输出被静默
  弄乱。解码统一走 `ark_common::decode`（严格 UTF-8 → GB18030/Big5/Shift-JIS/EUC-KR →
  Latin-1），与文件读取共用一条编码策略。
- `bash` 在 spawn 之前校验工作目录（`ensure_cwd`）：`current_dir` 指向不存在的路径时 `spawn`
  只返回裸 OS 错误（Windows 为 267「目录名称无效」），既不含路径也不含原因，实测三次调用仅
  得到该消息。校验只把「目录已消失」这一常见情形换成带路径的结构化错误，不替代 spawn 本身
  的失败路径。
- `bash` 执行的是一个进程树而不是一个进程。spawn 前用 `ark_common::process::isolate` 把 shell 设为
  进程组组长，spawn 后立即 `ProcessTree::adopt`；超时、回合取消（future 被 drop）都由该句柄发
  `killpg(SIGKILL)` 杀掉整组，`kill_on_drop(true)` 仅作为直接子进程的兜底保留。这样
  `sh -c 'server &'` 留下的后台进程不会继续持有继承来的 stdout/stderr 管道。成功回收后必须
  `disarm`：pid 可能被复用，不能再发组信号。输出收集与超时语义不变（仍是
  `timeout(wait_with_output)`，超时返回带 `timeout` detail 的结构化错误）。
- 持久 shell 会话（[需求设计](../requirements/shell-sessions.md)）由
  `exec/tools/shell_session.rs` 承载：`bash_session`（前台复用 `bash` 语义、leading `cd`
  推进会话 cwd；`run_in_background` 以 std Command 派生独立进程组并立即返回 task_id）、
  `bash_output`（按绝对逻辑字节 offset 增量返回输出与 running/finished 状态；环形缓冲丢弃前缀后仍以
  `base_offset + retained_len` 推进，不能把 offset 当当前 `VecDeque` 下标）、`bash_kill`
  （组信号 + direct kill 兜底，幂等）。`ShellSessionManager` 是进程内运行态（不入盘、不进
  契约）：环境变量**不**跨调用保持（Windows 无可移植 shell 协议，需求 §2 显式记录）；
  输出环形缓冲 64 KiB 超限丢最旧并打标记；任务数是 16 的硬上限，满额时先淘汰最旧已完成任务，
  若 16 个任务都仍在运行则新建后台任务返回结构化错误，绝不插入第 17 个。
  组合根以 `builtins_with_shell_session()` 装配父/子两个注册表（各自独立 manager 实例）。
- `find`/`grep` 的递归遍历跳过固定忽略名单（`target`、`node_modules`、`dist`、`.git`、`.tmp`、
  `.worktrees`，见 `exec::common::IGNORED_DIR_NAMES`）。名单在搜索根之下的每一层生效；把 `path`
  显式指进被忽略目录仍会扫描。该名单既是防噪（minified bundle 命中会灌爆上下文）也是防量：忽略前
  一次 grep 遍历约 41 万个文件（实测 11.9 秒纯遍历），忽略后约 931 个（19 毫秒）。
- `find`/`grep` 的同步目录扫描经 `tokio::task::spawn_blocking` 执行并受共享扫描预算约束
  （`exec::common::BlockingScan`，复用 bash 的 clamp 超时）。直接在 async 执行器里扫描曾把 tokio
  worker 阻塞十余分钟，回合取消信号无法被 poll，"停止"按钮杀不掉 grep；扫描函数在目录与文件粒度
  检查 `ScanDeadline::is_expired`，超时返回带预算毫秒数的结构化错误。
- `grep` 是无正则引擎的纯子串扫描。pattern 含正则元字符（`. * + ? ( ) [ ] { } | ^ $ \`）且
  未显式 `literal: true` 时返回结构化错误，错误里同时点明该显式开关与 `bash` 的 `grep -E`，
  不再静默按字面量匹配——那会产出"no matches"假阴性（实测一次 686 秒扫描换来错误结论）。
  显式 `literal: true` 的调用一律按字面量执行：忽略该标志曾让一天内 76 次 `grep` 被拒，
  其中 22 次调用方已经明确要求精确匹配。命中的单行按匹配位置截断到 400 字符（固定前缀会把
  匹配本身截掉），防止单行 minified 文件绕过行数上限灌爆模型上下文（实测一次拉入 254k 字符）。

## 验证

内置工具覆盖权限拒绝、工作区越界、链接逃逸、截断、多字节文本、并发同路径写入和批准后执行。

`bash` 另有 Unix 进程树回归：命令派生孙进程（`sleep 30 & echo $! > …; wait`）并记录其 pid，超时后
孙进程必须退出；外层 `tokio::time::timeout` 取消调用时同样必须退出。Windows 的 `taskkill /T /F`
分支只有编译覆盖，需在 Windows 上单独验证。

`find`/`grep` 另有：忽略名单回归（六个目录不进、显式 `path` 指进被忽略目录仍可搜）、正则元字符
pattern 拒绝（含交替 `|` 场景）、`literal: true` 时含元字符的 pattern 按字面量搜出结果、拒绝消息点明
该开关、单行 bundle 命中按匹配位置截断（输出长度有界且仍含 needle）。`grep` 的 blocking 执行正确性
由并发测试与 `bash` 树回归共同覆盖；扫描超时路径依赖 `BlockingScan` 的共享预算，与 bash 超时同一
`clamp_timeout` 常量，不单测墙钟。

`bash` 另有：工作目录不存在时错误必须含该路径（不再只是裸 OS 267）、CP936 字节经
`decode_shell_output` 解码为原文且不含替换字符。

`web_fetch` 另有跳转回归（假出口，无网络）：301 后跟随并再次请求新 URL、302 指向私有地址时第二跳
被拒且从未发起请求、超过 5 跳预算返回结构化错误、3xx 缺 `Location` 时渲染该状态码。

`edit` 另有行尾回归：LF 锚点匹配 CRLF 文件、CRLF 文件编辑后仍是 CRLF、LF 文件编辑后仍是 LF、
原生 CRLF 锚点同样匹配（不会被算成第二个 needle）。
