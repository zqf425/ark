# arkd 组合根设计

## 职责与边界

`apps/arkd` 是唯一允许了解全部具体实现的组合根。它创建配置、会话存储、模型客户端、工具、子代理
执行器、基础能力、服务器状态和 shutdown supervisor，并把实现注入端口。

`wiring` 仅做显式装配，不能包含业务规则、平台特例或生命周期状态机。新增实现应先落入所属 crate；
只有装配代码才允许在这里引用多个实现 crate。

入口命令：`serve`（常驻 HTTP/SSE 运行时）与 `run`（[headless 单回合](../requirements/headless-run.md)，
含 `--review` [审查预设](../requirements/review-preset.md)）。

## 持久化目录

默认根为 `~/.arkd`，CLI 默认传入 `--data-dir ~/.arkd/data` 与
`--workspace ~/.arkd/workspace`。组合根通过 `RuntimePaths` 创建 `data`、`config`、`log`、
`skills`、`mcp`、`tasks`、`workspace` 和大小写严格的 `IM`，并按
[持久化目录需求](../requirements/persistence-layout.md)装配各 store。启动不会搬迁或删除旧文件。
显式 `--workspace` 只覆盖实际工具工作区；recent 元数据仍写入分类后的
`workspace/workspace-recent.json`。
`run` 复用同一 `wiring::build` 装配，但**必须经真实 HTTP/SSE 边界驱动自身**（进程内 reqwest 客户端），
不得为 headless 另开一条绕过授权与事件语义的内部路径；其日志经 `TracingConfig::to_stderr`
走 stderr，stdout 只承载模型输出。

长期 memory 由组合根以同一 `data_dir` 创建 `ark_store::MemoryFileStore`，再用 `MemoryAdapter` 注入
`BaseServices`。业务校验、JSON 恢复及并发控制不位于组合根。
附件由组合根以同一个 `data_dir` 创建 `ark_store::AttachmentStore`，并以 `AttachmentStore` 端口注入
`AppState`；大小上限使用存储 crate 的 `DEFAULT_MAX_ATTACHMENT_BYTES`。组合根不解析上传内容或决定
附件路径。原生文件夹选择框同样由组合根提供实现：`NativeFolderDialog`（`base.rs`）在 macOS 走
`osascript choose folder`（rfd 需要 `NSApplication.run()` 事件循环，而 arkd 主线程运行 tokio）、其余
平台走 `rfd`，经 `FolderDialogService` 端口注入 `BaseServices`；取消与启动失败折叠为 `None` 并记 warn。

回合启动时，组合根把 `TurnJob.attachment_ids`（已由边界层按 `attachment_associated` 事件校验过）
交给 `TurnEvents::with_attachments` 写入 `turn_started` 作审计。附件 v2 第二期起，组合根另以
`attachment_seed::resolve_attachments`（纯函数，唯一决策点）在种子构建时解析这些 id：图片仅当
provider 的模型目录显式声明 `supports_vision`（`ProviderStore::resolve_vision`，`None` 视为未声明）
才读字节并以 base64 注入 `ModelMessage.images`；非图片/降级/超预算（每回合 20 MiB）/
读取失败的附件以 `preview_text` 拼入用户消息文本，随持久 item 进入历史回放。每个附件的传递结果
记入 `turn_started.attachment_outcomes`，并且同一记录随 `TurnEvents` 交给 reactor 写到本回合的
`UserMessage` item（`ThreadItem.attachments`，serde default 向后兼容）：item 是渲染数据源，
重载历史后 UI 才能渲染与 live 流一致的附件 chips——`turn_started` 只在本回合可观察。
agent 不读取文件系统；坏附件不得使回合失败。
关联回放不经过组合根——它由 `SessionStore` 的事件投影提供。

`wiring/turn_common.rs` 是父/子两条 turn 路径的**共享规则宿主**，属本模块"仅装配"约束的显式例外：
seed 静态前缀（prompt-cache 字节一致不变量的唯一构造点）、技能动态尾、工作区解析、每回合 limits、
hook 包装的工具执行与 `ReactorSpec` 构造都住在这里。理由是结构性的——两条路径必须共用同一份规则才
不会漂移；除该文件外，`wiring` 仍不得包含业务规则。

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
`ModelSeed`，不会被设置页静默忽略。在此之上，线程级运行时覆盖（`ThreadRuntimeSettings`，经
`PATCH /v1/threads/{id}` 持久化到 thread.json）在回合入口一次性捕获，只影响后续回合：

- `runtime.workspace` 非空时经 `turn_common::resolve_workspace` 覆盖进程 workspace lease；
- provider/model/reasoning_effort 经 `resolve_thread_provider`（`wiring/turns.rs`）从存活 provider
  表解析：显式 `provider_id` 必须可解析，`model` 仅在显式给出时覆盖目录模型，空 `reasoning_effort`
  视为未设置（不发空提示），协议映射见 [ark-llm](ark-llm.md)；
- `runtime.autonomy` 决定该回合的 sandbox 档位（定时任务的 `unattended` 缺省即写这个字段）。

模型与目录流量的 `reqwest::Client` 由组合根经
`ark_http::transport_client`（同一 `HttpPolicy` 的代理/TLS 设置；SSE 流式响应不设 client 级总超时）
注入 `HttpModelClient` 与 `ModelCatalogClient`，组合根不得为模型路径另建裸 client。

回合的 compaction threshold 完全由模型上下文窗口推导，不再是可调配置：`wiring::limits_for_turn`
按 `ark_llm::COMPACTION_TRIGGER_RATIO`（80%，与渲染层上下文环的警示水位一致）乘以窗口并预留条目
`max_tokens`；未声明窗口的模型按默认窗口 `ark_llm::DEFAULT_CONTEXT_WINDOW_TOKENS`（256K，与设置页
模型清单的兜底一致）推导，压缩不被禁用。已运行回合继续使用创建时快照；阈值始终存在，防止 prompt
无界增长。

会话标题自动生成经 `TitleModelAdapter`（`base.rs`）注入 `BaseServices::title_model`：它复用
`resolve(None)` 的存活 provider 与 `select_from_config` 的同一计量 client，构造无工具、低温度、
`max_tokens=512` 的小请求。预算按整段响应计算：reasoning 模型会先消耗不可见的思考 delta，
64 的旧预算被思考耗尽后 content 恒为空（`finish_reason: length`），因此预算必须覆盖思考加短答案；
适配器对空回答记 warn，保证该路径可诊断。组合根只装配端口，占位判定、提示词与超时归 `ark-server`
（[ark-server](ark-server.md)）；未配置 provider 时适配器返回 `None`，标题保持占位。

工作区由组合根持有 canonical-root/generation 状态。父回合在 runner 入口捕获 lease，动态 prompt 和所有
工具调用都使用同一 root；子回合继承 delegation tool context 的 workspace。切换只改变后续回合，且必须
在没有 parent/child active run 时完成；它不会重启 arkd、sidecar、端口或 data-dir。

recent workspace descriptors 是可重建的展示数据，以 canonical 路径 best-effort 原子写入
`<root>/workspace/workspace-recent.json`。启动时缺失或损坏的文件按空集合处理，失效、非目录、重复及当前 active
条目会被忽略；写入失败只记录告警，不能阻断 arkd 启动或 workspace switch。recent 不参与 lease、路径
containment 或任何工具授权判断。

Windows 上组合根会在保存 canonical root 前移除 `\\?\` verbatim 前缀，因此 WorkspaceView、模型 prompt
和工具运行器使用同一份可显示、可解析的路径。

UI catalog 由组合根创建唯一 `UiBus`：授予本构建实际具备的能力，以 `ark.builtin.code-ui` 注册 Code 页与
四个面板，再以 `BaseServices::with_ui` 注入 server。注册句柄必须 `forget()` 活到进程结束，否则 drop 即
撤销路由。组合根不得再维护第二份页面或能力清单。

本地插件同样走这唯一的 `UiBus`。`plugin_host::assemble` 在 UI 段之后、`tools.replace_group("mcp", ...)`
之前构造一组共享实例：一个 `HttpBus`（默认 `HttpPolicy`、`CredentialVault` 与容量受限的
`InMemoryDiagnosticSink`）、一个独立的 `ark_mcp::McpBus` 和一个 `PluginHost`（根为 `<root>/data/plugins`）。
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
组合根另注册**仅父线程可用**的 `todo_write`（`todo_tool.rs`，与 `load_skill` 同一先例）：它持
`Arc<dyn SessionStore>`，把模型给出的完整待办清单以权威 `TodosUpdated` 事件写入当前线程事件流。
校验（条数上限、id/title 非空且 id 唯一、标题字符上限）与 checkbox 渲染（`[ ]`/`[~]`/`[x]`）
分别以 `ark_server::service::workbench` 的 `validate_todos`/`todo_marker` 为唯一宿主，HTTP
`POST /v1/threads/{id}/todos` 与本工具共用；可预期失败（校验不过、线程不存在）以 `ToolOutput::err`
返回给模型，只有意外 I/O 错误才走 `Err` 路径。子注册表刻意不含它：子回合写入的是 child 线程的
事件流，父侧 Plan 投影读不到，注册只会诱导模型做无效写入。

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
消息；判定经 `ThreadItem::is_context_checkpoint`，它同时以 `[context checkpoint` 前缀兜底识别 marker 出现之前持久化的 Notice 条目（含 `: no structured
facts extracted` 与 `: truncated` 两个变体）。判定 gated 在 `Notice` kind 上：用户消息可以打出任意
前缀，永远不算检查点；也不得改回裸前缀匹配，改渲染格式同样会让历史识别静默失效。

审批规则（[需求设计](../requirements/approval-rules.md)）在回合入口接线：父路径与子路径都用
`ark_tool::ApprovalPolicy` 包装既有 autonomy 策略（每回合用配置快照构造，规则属 Hot）；reactor 侧
gate 注入 `AutoDenyGates` 包装 `InteractiveGates`——`approvals.on_unattended=deny`（默认）时无人
值守回合的审批请求立即被结构化拒绝，不再挂死到 `sse_idle_timeout`；`AppState` 仍持裸
`InteractiveGates`（HTTP handler 按 id resolve，从不等待）。

用户长期 memory 也走动态尾部：`run_turn` 在 skills 目录之后、completion inbox 之前，每回合重新读取
`<root>/data/memory.json` 并追加**至多一条** `[user memory]` system 消息（按 `created_at` 升序，拼接后
按 UTF-8 字符数限 4096，超限整条丢弃最新条目并追加 truncated 尾注）。读取或解析失败只记 warn 并跳过注入，
空 store 不注入，均不影响回合结果。

项目指令文件（[需求设计](../requirements/project-instructions.md)）由
`apps/arkd/src/instructions.rs`（唯一发现与截断宿主）在回合生效工作区根按
`AGENTS.md` → `CLAUDE.md` → `README.md` 优先级取第一个命中文件，读为**动态尾部的一条
`[project instructions]` system 消息**（父路径在 skills 之后、memory 之前；子路径在角色行之后、
skill tail 之前）。不进入静态前缀（工作区相关文本会破坏 prompt-cache 字节稳定不变量）；
读取失败、非 UTF-8、空文件按未命中处理并记 warn；UTF-8 字节数限 32 KiB，按字符边界截断。

持久计划（goal + todos）同样走动态尾部：`run_turn` 每回合从线程事件流重投影 `PlanView`
（`ark_server::service::workbench::plan`），经 `history::plan_seed_message` 以
`[thread plan - current objective and progress]` system 消息注入 seed 末尾（最贴近新用户消息）。
todo 行使用共享的 `[ ]`/`[~]`/`[x]` 词汇（`workbench::todo_marker`），与 `todo_write` 工具的
回显一致。这使计划面板内容对上下文压缩免疫：压缩只折叠历史消息，seed 每回合从事件流重建，
非完成待办因此始终留在模型可见的最近尾部；回合内压缩生成的 checkpoint 也将 checkbox 行纳入
结构化事实（见 ark-agent 设计），双重防遗忘。

## 定时任务装配

组合根以 `<root>/tasks` 创建 `ark_store::ScheduleFileStore`,注入 `ark_ports::ScheduleStore`;
任务表独立于 `config.json`:运行时每次完成都会重写它,高频变更放在 settings 文件里会与配置存储的
版本机制相互干扰。`wiring/mod.rs` 因此:

1. 构造 `Arc<SchedulerService>`,与 `FileSessionStore`、附件存储并列;
2. `AppState::with_schedules(Arc::clone(&schedules))`,让 `/v1/schedules` 路由拿到同一个实例;
3. 在 `state` 定稿之后最后调 `schedules.spawn(state.clone())` —— 循环的第一个动作是
   `reconcile`,它必须看到所有路由将来的那个 state;
4. 调度循环订阅 `shutdown_receiver()`,在 `Ctrl-C`/drain 只置标志位的路径上靠每轮 tick 顶部的
   `is_shutting_down()` 收敛。

IM 渠道编排（`ark_server::ImService`）走同一模式：`wiring/mod.rs` 用 `<root>/IM` 创建
`ark_store::ImFileStore`，注入 `ark_ports::ImStore`；适配器的 `reqwest::Client` 由组合根经
`ark_http::transport_client`（与模型流量同一 `HttpPolicy` 代理/TLS 设置）构造，长轮询渠道
以 `client_timeout=None` 传参；`AppState::with_im` 挂到路由后，`serve` 在 `main.rs` 于
`schedules.spawn` 之后调用 `im.spawn(state)`——headless `run` 同样不启动 IM 连接。

执行直接复用注入的 `TurnRunner` 与 `start_turn` 校验路径(与自动 continuation 同一先例),
不新建第二执行通道。见 [定时任务需求](../requirements/scheduled-tasks.md)。

## 验证

装配变更至少执行 `cargo check -p arkd`；涉及运行时、存储或并发时执行 workspace 测试，并验证启动/停机
收敛路径。
