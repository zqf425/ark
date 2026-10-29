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

## 验证

覆盖字段分类、跨字段校验、非法 patch 不修改旧快照、热字段可见性以及重启字段整批拒绝。
