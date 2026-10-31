# arkd 组合根设计

## 职责与边界

`apps/arkd` 是唯一允许了解全部具体实现的组合根。它创建配置、会话存储、模型客户端、工具、子代理
执行器、基础能力、服务器状态和 shutdown supervisor，并把实现注入端口。

`wiring` 仅做显式装配，不能包含业务规则、平台特例或生命周期状态机。新增实现应先落入所属 crate；
只有装配代码才允许在这里引用多个实现 crate。

长期 memory 由组合根以同一 `data_dir` 创建 `ark_store::MemoryFileStore`，再用 `MemoryAdapter` 注入
`BaseServices`。业务校验、JSON 恢复及并发控制不位于组合根。
附件由组合根以同一个 `data_dir` 创建 `ark_store::AttachmentStore`，并以 `AttachmentStore` 端口注入
`AppState`；大小上限使用存储 crate 的 `DEFAULT_MAX_ATTACHMENT_BYTES`。组合根不解析上传内容或决定
附件路径。

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
`ModelSeed`，不会被设置页静默忽略。模型目录明确提供 context window 时，组合根将全局 compaction
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

启动时组合根连接所有启用的外部 MCP server，并把成功发现的 `<server_id>.<tool>` 原子发布到父工具表；单个
连接失败只反映到 MCP server 状态，不中止 arkd。MCP server 的 upsert/delete 在请求内重新发现并替换该动态
分组，因此下一父回合读取的是最新 schema，而不需要重启。组合根还注册 `load_skill`：它读取 `SkillRuntime`
的已扫描正文快照，避免 `ark-tool` 反向依赖 skills 实现。

回合种子中的 `conversation_history` 将已完成的 ToolCall/ToolResult 行投影回模型上下文：同一 call_id 配对时，
调用折叠进上一条 assistant 消息的 `tool_calls`（无文本时新建空内容 assistant 行），后接一条截断到 16 KiB
UTF-8 字节的 `ModelRole::Tool` 结果消息（失败结果同样回放）；无配对结果的调用与无调用的结果不回放，
投影顺序保持 durable items 的时间顺序。

## 验证

装配变更至少执行 `cargo check -p arkd`；涉及运行时、存储或并发时执行 workspace 测试，并验证启动/停机
收敛路径。
