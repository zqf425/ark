# arkd 组合根设计

## 职责与边界

`apps/arkd` 是唯一允许了解全部具体实现的组合根。它创建配置、会话存储、模型客户端、工具、子代理
执行器、基础能力、服务器状态和 shutdown supervisor，并把实现注入端口。

`wiring` 仅做显式装配，不能包含业务规则、平台特例或生命周期状态机。新增实现应先落入所属 crate；
只有装配代码才允许在这里引用多个实现 crate。

入口命令：`serve`（常驻 HTTP/SSE 运行时）与 `run`（[headless 单回合](../requirements/headless-run.md)，
含 `--review` [审查预设](../requirements/review-preset.md)）。
`run` 复用同一 `wiring::build` 装配，但**必须经真实 HTTP/SSE 边界驱动自身**（进程内 reqwest 客户端），
不得为 headless 另开一条绕过授权与事件语义的内部路径；其日志经 `TracingConfig::to_stderr`
走 stderr，stdout 只承载模型输出。

长期 memory 由组合根以同一 `data_dir` 创建 `ark_store::MemoryFileStore`，再用 `MemoryAdapter` 注入
`BaseServices`。业务校验、JSON 恢复及并发控制不位于组合根。
附件由组合根以同一个 `data_dir` 创建 `ark_store::AttachmentStore`，并以 `AttachmentStore` 端口注入
`AppState`；大小上限使用存储 crate 的 `DEFAULT_MAX_ATTACHMENT_BYTES`。组合根不解析上传内容或决定
附件路径。

回合启动时，组合根把 `TurnJob.attachment_ids`（已由边界层按 `attachment_associated` 事件校验过）
交给 `TurnEvents::with_attachments` 写入 `turn_started` 作审计。附件 v2 第二期起，组合根另以
`attachment_seed::resolve_attachments`（纯函数，唯一决策点）在种子构建时解析这些 id：图片仅当
provider 的模型目录显式声明 `supports_vision`（`ProviderStore::resolve_vision`，`None` 视为未声明）
才读字节并以 base64 注入 `ModelMessage.images`；非图片/降级/超预算（每回合 20 MiB）/
读取失败的附件以 `preview_text` 拼入用户消息文本，随持久 item 进入历史回放。每个附件的传递结果
记入 `turn_started.attachment_outcomes`。agent 不读取文件系统；坏附件不得使回合失败。
关联回放不经过组合根——它由 `SessionStore` 的事件投影提供。

## 子代理装配

child executor 在此复用同一个 `ark-agent::Reactor`，并桥接 cancel/steer、工具集和会话 writer。
它不能创建第二套 agent framework，也不能让模型指定任意 workspace。

父回合开始前由组合根读取 `SessionStore` 的父事件，调用 `ark-agent` 的纯 inbox 投影并作为动态 system
source 注入 reactor。父回合成功后再追加 completion acknowledgement；写入失败记录告警并保留下一回合
的至少一次投递，不能把已经完成的模型回合改写为失败。

读取使用 completion 生命周期窄查询而非完整历史回放。组合根的完成处理必须保持“注入 → 正常完成才确认 →
下一回合不再注入；失败或中断不确认”的顺序。

组合根在 delegation hydrate 前订阅 completion wakeup，随后把父 thread id 转发给 `ark-server` 的
per-thread supervisor；lag 时扫描线程并由持久 inbox 去重恢复。启动和 `subagents.auto_continue` 热开启
同样扫描未确认 inbox。自动 continuation 复用 `AgentTurnRunner`，以独立 system-source 作业类型启动，
不调用 `push_user`，因此不会伪造用户消息；其 provider、workspace、预算、sandbox 和 gates 与普通父回合
走同一路径。

父回合使用 `InteractiveGates`，该实例同时注入 `AppState` 的 HTTP handler 与 `AgentTurnRunner` 的
`ReactorConfig`；不得回退为 `AllowAllGates`，否则审批和用户输入卡片会变成无法影响执行的装饰。

每个父/子回合在开始时读取同一份 provider 配置：自定义 headers、temperature 和 max_tokens 传入
`ModelSeed`，不会被设置页静默忽略。模型与目录流量的 `reqwest::Client` 由组合根经
`ark_http::transport_client`（同一 `HttpPolicy` 的代理/TLS 设置；SSE 流式响应不设 client 级总超时）
注入 `HttpModelClient` 与 `ModelCatalogClient`，组合根不得为模型路径另建裸 client。
模型目录明确提供 context window 时，组合根将全局 compaction
threshold 收窄到可用输入空间（扣除用户明确设置的 max_tokens）；已运行回合继续使用创建时快照。

工作区由组合根持有 canonical-root/generation 状态。父回合在 runner 入口捕获 lease，动态 prompt 和所有
工具调用都使用同一 root；子回合继承 delegation tool context 的 workspace。切换只改变后续回合，且必须
在没有 parent/child active run 时完成；它不会重启 arkd、sidecar、端口或 data-dir。

recent workspace descriptors 是可重建的展示数据，以 canonical 路径 best-effort 原子写入
`<data-dir>/workspace-recent.json`。启动时缺失或损坏的文件按空集合处理，失效、非目录、重复及当前 active
条目会被忽略；写入失败只记录告警，不能阻断 arkd 启动或 workspace switch。recent 不参与 lease、路径
containment 或任何工具授权判断。

Windows 上组合根会在保存 canonical root 前移除 `\\?\` verbatim 前缀，因此 WorkspaceView、模型 prompt
和工具运行器使用同一份可显示、可解析的路径。

UI catalog 由组合根创建唯一 `UiBus`：授予本构建实际具备的能力，以 `ark.builtin.code-ui` 注册 Code 页与
四个面板，再以 `BaseServices::with_ui` 注入 server。注册句柄必须 `forget()` 活到进程结束，否则 drop 即
撤销路由。组合根不得再维护第二份页面或能力清单。

本地插件同样走这唯一的 `UiBus`。`plugin_host::assemble` 在 UI 段之后、`tools.replace_group("mcp", ...)`
之前构造一组共享实例：一个 `HttpBus`（默认 `HttpPolicy`、`CredentialVault` 与容量受限的
`MemoryDiagnostics`）、一个独立的 `ark_mcp::McpBus` 和一个 `PluginHost`（根为 `<data-dir>/plugins`）。
装配只做构造与转发，不按 `plugin_id` 分支；装载策略、隔离与 fail-closed 撤权都在 `ark-plugin` 内部。
`HostHandles.events` 恒为 `None`：平台句柄暴露原始事件 sink，交给插件就能向任意 thread 追加事件，
本版不需要这个能力，所以它不存在而不是被隔离模型解释。

插件工具与外部 MCP server 的工具并入同一个 `mcp` 工具组：`mcp_tools` 先扩展 `plugins.tools()`，
再一次性 `replace_group`。因此命名冲突（同名 `<provider>.<tool>`）是配置错误而不是静默覆盖，
且插件注册失败时总线目录里本就不存在它的工具，无需额外过滤。
MCP server 的运行时 upsert/delete 触发重发现时，`McpServerAdapter` 同样先合并插件工具的实时视图
（`plugin_host::plugin_tools`，与启动装配共用同一转换函数）再整体替换该组——`replace_group` 接收
整组列表，漏掉插件工具会把它们从模型工具表中挤掉直到重启。

运行时自有的 `web_fetch` 工具（见 [web-fetch-tool 需求](../requirements/web-fetch-tool.md)）
复用插件的 `HttpBus`：组合根在装配时以 `HostAllowlist::allow_all()` 注册 `ark.builtin.web`
egress 授权（该构造器对插件清单路径不可达），并把同一能力注入父/子两个工具表的
`ark_tool::web_tools`。内置出口的 SSRF 拒绝在工具层，本组合根不为它写任何分支逻辑。

插件只在启动时装载一次（无热重载、无 per-plugin 开关），结果写入启动日志与
`GET /v1/runtime/plugins`；后者通过 `BaseServices::with_plugins` 注入的 `ark_ports::PluginHostService`
只读读取，组合根不缓存或重算报告。`Wiring` 持有 `PluginRuntime` 以保证句柄活到进程退出。

启动时组合根连接所有启用的外部 MCP server，并把成功发现的 `<server_id>.<tool>` 原子发布到父工具表；单个
连接失败只反映到 MCP server 状态，不中止 arkd。MCP server 的 upsert/delete 在请求内重新发现并替换该动态
分组，因此下一父回合读取的是最新 schema，而不需要重启。组合根还注册 `load_skill`：它读取 `SkillRuntime`
的已扫描正文快照，避免 `ark-tool` 反向依赖 skills 实现。
`request_user_input` 属于内置工具集，随 `ark_tool::builtins()` 在父/子两个注册表中可用；
其执行由 `ark-agent` 的用户输入 gate 拦截，组合根不为它写任何特例。

生命周期 hooks（[需求设计](../requirements/hooks.md)）的唯一执行宿主是 `apps/arkd/src/hooks.rs`：
父回合 `ToolRunnerAdapter` 与子回合 `ChildToolRunner` 都经 `run_around` 组合器包住工具派发，
每次调用从 config 快照现读 `hooks`（`Hot`）。exit 2 拦截、其他失败放行、post stdout 回注的
语义只在这一处实现。

回合种子中的 `conversation_history` 把工具行投影回模型上下文。store 投影把 `ToolCallStarted` +
`ToolCallFinished` 折叠为单个内嵌 output 的 `ToolCall` 条目（见 ark-store 设计），因此这里直接消费该条目：
终态（Completed/Failed）条目将调用折叠进上一条 assistant 消息的 `tool_calls`（无文本时新建空内容
assistant 行），后接一条截断到 16 KiB UTF-8 字节的 `ModelRole::Tool` 结果消息（Failed 条目结果内容为空
兜底）；非终态（Streaming 等）条目不回放，投影顺序保持 durable items 的时间顺序。
带 `ContextCheckpoint` 标记（`ThreadItem::marker`，ark-contract 定义）的 `Notice` 条目回放为 system
消息；判定经 `ThreadItem::is_context_checkpoint`，它同时以 `[context checkpoint` 前缀兜底识别
marker 出现之前持久化的历史条目（含 `: no structured facts extracted` 与 `: truncated` 两个变体）。
不得改回裸前缀匹配：用户消息文本可以伪造任意前缀，改渲染格式也会让历史识别静默失效。

用户长期 memory 也走动态尾部：`run_turn` 在 skills 目录之后、completion inbox 之前，每回合重新读取
`<data-dir>/memory.json` 并追加**至多一条** `[user memory]` system 消息（按 `created_at` 升序，拼接后
按 UTF-8 字符数限 4096，超限整条丢弃最新条目并追加 truncated 尾注）。读取或解析失败只记 warn 并跳过注入，
空 store 不注入，均不影响回合结果。

## 定时任务装配

组合根以同一 `data_dir` 创建 `ark_store::ScheduleFileStore`,注入 `ark_ports::ScheduleStore`;
任务表独立于 `config.json`:运行时每次完成都会重写它,高频变更放在 settings 文件里会与配置存储的
版本机制相互干扰。`wiring/mod.rs` 因此:

1. 构造 `Arc<SchedulerService>`,与 `FileSessionStore`、附件存储并列;
2. `AppState::with_schedules(Arc::clone(&schedules))`,让 `/v1/schedules` 路由拿到同一个实例;
3. 在 `state` 定稿之后最后调 `schedules.spawn(state.clone())` —— 循环的第一个动作是
   `reconcile`,它必须看到所有路由将来的那个 state;
4. 调度循环订阅 `shutdown_receiver()`,在 `Ctrl-C`/drain 只置标志位的路径上靠每轮 tick 顶部的
   `is_shutting_down()` 收敛。

执行直接复用注入的 `TurnRunner` 与 `start_turn` 校验路径(与自动 continuation 同一先例),
不新建第二执行通道。见 [定时任务需求](../requirements/scheduled-tasks.md)。

## 验证

装配变更至少执行 `cargo check -p arkd`；涉及运行时、存储或并发时执行 workspace 测试，并验证启动/停机
收敛路径。
