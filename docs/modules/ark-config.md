# ark-config 模块设计

## 职责与边界

`ark-config` 负责加载、校验、持久化和原子替换运行时配置快照。它不负责让某个具体组件生效；
订阅者在组合根按确定顺序消费已校验的变更。

生产组合根从 `<root>/config/config.json` 加载，并把相同的 `config_dir` 注入 `AppState`；
HTTP 配置与子代理配置保存都回写该路径。`AppState::new` 默认仍令 `config_dir == data_dir`，
以保持测试和非 arkd 调用方兼容。Provider 表同属配置，位于 `<root>/config/providers.json`。

## 热更新模型

每个字段有 `ReloadClass`：

- `Hot`：替换快照后即时生效；
- `HotRelisten`：重建监听，客户端依靠 `since_seq` 恢复；
- `Restart`：整批 patch 被拒绝并返回字段清单，禁止部分应用。

配置文件保存先同步临时文件再原子替换。Windows 保持 `MoveFileExW` 现有路径；Unix 在 rename
后打开 parent directory（空 parent 视为 `.`）并 `sync_all`，确保新目录项持久化。目录打开或同步
失败会明确返回错误，但此时 rename 已提交，不能把该错误解释为旧文档仍未替换。

`ConfigStore` 使用自洽快照；校验或合并失败保留旧值。已经返回 `Applied` 的修改会立即替换内存
快照，即使持久化失败也不回滚，而是告警；此后即使 `changed` 为空的空 patch 重试，也会重新持久化
当前已应用快照，不会因“没有新字段”而再次跳过保存。已经运行的任务必须捕获并继续使用其创建时的
策略；新任务读取最新快照。首次启动或 `tuning` 缺失字段使用的默认调优值为：`max_rounds=1000`、
`max_tokens=0`（不限）、`max_tool_calls=1000`；其余调优字段沿用类型定义的默认值。

`default_autonomy` 是未设置会话覆盖时使用的 `Hot` 全局默认值。它只接受 `strict`、`workspace_write` 或向后兼容的 `unattended`；非法 patch 会被事务性拒绝，不得静默回退。

`subagents.auto_continue` 是 `Hot` 布尔开关，默认及旧配置缺失值均为 `true`。仅当
`subagents.enabled` 与 `subagents.auto_continue` 同时为 `true` 时，server 才扫描持久 completion inbox
并调度符合条件的父线程；关闭任一开关会立即取消进程内待调度 continuation，不影响已开始回合。默认开启意味着
后台 child 完成后会产生额外的父模型调用用于汇总，用户可在设置页显式关闭。

该开关是显式值优先的：文件里写成 `false` 时，即使默认值后来改成 `true` 也不会被覆盖，也不会触发
配置诊断告警（诊断只报告缺失或未知键）。关闭期间到达的完成通知保持在持久 inbox 中，server 以
`auto_continue_disabled` 为原因记录一条 `INFO`（详见 [ark-server](ark-server.md)）；恢复开关后同一通知
由策略扫描补投，不需要重新触发 child。

`approvals` 配置节整体为 `Hot`：规则表和 `on_unattended` 都在每个父回合入口从同一当前快照读取，
只影响后续回合。`serve` 的显式 User 回合使用交互 gate；headless、Scheduled、IM、自动 continuation 与
自动 recovery 按 `on_unattended` 处置，child 不经过审批 gate。校验边界为规则数 ≤128、pattern ≤4 KiB、tool 名非空且
≤64 字符，action 与 on_unattended 只接受契约枚举；非法 patch 事务性拒绝。

`hooks` 配置节（[hooks 需求](../requirements/hooks.md)）整体为 `Hot`：命令清单、匹配规则与
`timeout_ms` 在快照替换后对下一次工具调用即时生效。校验边界：至多 32 条 hook、单条命令编码后
不超过 8 KiB、timeout 不超过 60 s，越界 patch 事务性拒绝。`hooks` 整节缺席是合法的常态（多数
安装不使用 hooks），加载诊断对它**刻意不告警**，不能把它列入 `defaulted`/`ignored`。

`skills` 配置节整体为 `Hot`：`SkillRuntime` 通过组合根注入的配置源在每次扫描/解析时重读快照，
patch 保存后在下一次重扫即生效，无需重启进程。HTTP 层在 apply 结果的 `changed` 含 `skills` 时
还会主动调用 skills 服务的 `reload()`，保证只读快照的回合解析路径不落在陈旧快照上。（此前归为
`Restart` 时 `apply` 整批拒绝，设置界面永远无法保存该节。）

## 加载诊断

`config.json` 的加载区分三种结果：文件缺失属正常首次启动（取默认值，不打日志）；JSON 非法或
字段值越界仍是错误（拒绝启动）；文档可解析但与 schema 不完全匹配时**行为不变**，但
`load_or_default` 会打一条 `tracing::warn!`，用 `defaulted=[...]` / `ignored=[...]` 列出回落默认值的
缺失键和被忽略的未知键（含 `subagents.*`、`tuning.*` 等嵌套键）。`load_report` 以数据形式返回
同一份诊断，便于调用方在测试中断言。之所以不启用 `deny_unknown_fields`，是因为 GUI 与 `arkd`
共用同一数据目录，未知键必须继续被容忍；报告取代了拒绝。

## 验证

覆盖字段分类、跨字段校验、非法 patch 不修改旧快照、热字段可见性以及重启字段整批拒绝；保存失败后
内存快照保持已应用状态，空 patch 重试重新持久化同一快照。
