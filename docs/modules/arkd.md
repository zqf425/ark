# arkd 组合根设计

## 职责与边界

`apps/arkd` 是唯一允许了解全部具体实现的组合根。它创建配置、会话存储、模型客户端、工具、子代理
执行器、基础能力、服务器状态和 shutdown supervisor，并把实现注入端口。

`wiring` 仅做显式装配，不能包含业务规则、平台特例或生命周期状态机。新增实现应先落入所属 crate；
只有装配代码才允许在这里引用多个实现 crate。

## 子代理装配

child executor 在此复用同一个 `ark-agent::Reactor`，并桥接 cancel/steer、工具集和会话 writer。
它不能创建第二套 agent framework，也不能让模型指定任意 workspace。

父回合开始前由组合根读取 `SessionStore` 的父事件，调用 `ark-agent` 的纯 inbox 投影并作为动态 system
source 注入 reactor。父回合成功后再追加 completion acknowledgement；写入失败记录告警并保留下一回合
的至少一次投递，不能把已经完成的模型回合改写为失败。

读取使用 completion 生命周期窄查询而非完整历史回放。组合根的完成处理必须保持“注入 → 正常完成才确认 →
下一回合不再注入；失败或中断不确认”的顺序。

父回合使用 `InteractiveGates`，该实例同时注入 `AppState` 的 HTTP handler 与 `AgentTurnRunner` 的
`ReactorConfig`；不得回退为 `AllowAllGates`，否则审批和用户输入卡片会变成无法影响执行的装饰。

每个父/子回合在开始时读取同一份 provider 配置：自定义 headers、temperature 和 max_tokens 传入
`ModelSeed`，不会被设置页静默忽略。模型目录明确提供 context window 时，组合根将全局 compaction
threshold 收窄到可用输入空间（扣除用户明确设置的 max_tokens）；已运行回合继续使用创建时快照。

工作区由组合根持有 canonical-root/generation 状态。父回合在 runner 入口捕获 lease，动态 prompt 和所有
工具调用都使用同一 root；子回合继承 delegation tool context 的 workspace。切换只改变后续回合，且必须
在没有 parent/child active run 时完成；它不会重启 arkd、sidecar、端口或 data-dir。

Windows 上组合根会在保存 canonical root 前移除 `\\?\` verbatim 前缀，因此 WorkspaceView、模型 prompt
和工具运行器使用同一份可显示、可解析的路径。

启动时组合根连接所有启用的外部 MCP server，并把成功发现的 `<server_id>.<tool>` 原子发布到父工具表；单个
连接失败只反映到 MCP server 状态，不中止 arkd。MCP server 的 upsert/delete 在请求内重新发现并替换该动态
分组，因此下一父回合读取的是最新 schema，而不需要重启。组合根还注册 `load_skill`：它读取 `SkillRuntime`
的已扫描正文快照，避免 `ark-tool` 反向依赖 skills 实现。

## 验证

装配变更至少执行 `cargo check -p arkd`；涉及运行时、存储或并发时执行 workspace 测试，并验证启动/停机
收敛路径。
