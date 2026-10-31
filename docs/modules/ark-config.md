# ark-config 模块设计

## 职责与边界

`ark-config` 负责加载、校验、持久化和原子替换运行时配置快照。它不负责让某个具体组件生效；
订阅者在组合根按确定顺序消费已校验的变更。

## 热更新模型

每个字段有 `ReloadClass`：

- `Hot`：替换快照后即时生效；
- `HotRelisten`：重建监听，客户端依靠 `since_seq` 恢复；
- `Restart`：整批 patch 被拒绝并返回字段清单，禁止部分应用。

`ConfigStore` 使用自洽快照，任何失败都保留旧值。已经运行的任务必须捕获并继续使用其创建时的
策略；新任务读取最新快照。

`default_autonomy` 是未设置会话覆盖时使用的 `Hot` 全局默认值。它只接受 `strict`、`workspace_write` 或向后兼容的 `unattended`；非法 patch 会被事务性拒绝，不得静默回退。

`subagents.auto_continue` 是 `Hot` 布尔开关，默认及旧配置缺失值均为 `false`。仅当
`subagents.enabled` 与 `subagents.auto_continue` 同时为 `true` 时，server 才扫描持久 completion inbox
并调度符合条件的父线程；关闭任一开关会立即取消进程内待调度 continuation，不影响已开始回合。

## 加载诊断

`config.json` 的加载区分三种结果：文件缺失属正常首次启动（取默认值，不打日志）；JSON 非法或
字段值越界仍是错误（拒绝启动）；文档可解析但与 schema 不完全匹配时**行为不变**，但
`load_or_default` 会打一条 `tracing::warn!`，用 `defaulted=[...]` / `ignored=[...]` 列出回落默认值的
缺失键和被忽略的未知键（含 `subagents.*`、`tuning.*` 等嵌套键）。`load_report` 以数据形式返回
同一份诊断，便于调用方在测试中断言。之所以不启用 `deny_unknown_fields`，是因为 GUI 与 `arkd`
共用同一数据目录，未知键必须继续被容忍；报告取代了拒绝。

## 验证

覆盖字段分类、跨字段校验、非法 patch 不修改旧快照、热字段可见性以及重启字段整批拒绝。
