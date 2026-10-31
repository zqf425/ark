# ark-store 模块设计

## 职责与边界

`ark-store` 以会话目录为系统记录，实现 `SessionStore` 和事件写入边界。每个线程是可迁移目录；
`events.jsonl` 是权威日志，`items.jsonl`、`usage.jsonl`、`session.json`、child index 和全局
索引均为可重建投影。

条目投影把一次工具调用的 `ToolCallStarted` 与 `ToolCallFinished` 折叠为**单个** `ToolCall` 条目：
结果（output/ok/duration_ms）内嵌在该条目的 tool payload 中，不会产生独立的 `ToolResult` 条目。
下游（如组合根的回合种子回放）必须按此折叠形状消费，不得假设两条目配对。

## 一致性与并发

- 每个线程由一个 writer actor 分配序号并追加事件；同一数据目录由 `data/.lock` 独占。
- `subscribe` 必须在同一 actor 临界区完成“定水位、读取 backlog、注册 live channel”；慢订阅者
  断开后用 `since_seq` 重连，绝不反压 writer。
- 同父目录的 child append、删除、索引重建共用父级锁；锁顺序固定为锁表、父级锁、locator。
- 启动时修复日志末尾半行并从权威日志重建派生数据；非末尾损坏不得静默掩盖。
- `rewind` 采用**物理删除**：writer actor 在同一序列化点拒绝仍有未终结回合的日志、重写 `events.jsonl`，删除目标序号及之后的事件，随后从保留前缀重建 `items.jsonl` 与 `session.json`。它不使用逻辑截断；为让已连接 SSE 客户端丢弃旧投影，会追加一个序号绝不复用的 `thread_rewound` 通知事件。`rewinds.json` 保留已接受目标的幂等标记，因此后续压缩移除通知事件也不改变重复回退的成功语义。

## 目录与恢复

全局附件存储位于 `<data_dir>/attachments/<attachment-id>`，其不可变元数据表位于
`<data_dir>/attachments.json`。二进制与 JSON 均先写同目录临时文件后 rename；元数据使用
`ark_common::write_json_file`，故为 UTF-8 无 BOM。读取同时要求元数据和最终二进制文件存在，
所以崩溃留下的临时文件或已删除条目不会被暴露。`AttachmentId` 是 UUID newtype，文件路径只由该
服务端生成 ID 组成，绝不拼接客户端路径。

```text
data/threads/<thread>/
├── thread.json                 # 线程元数据
├── events.jsonl                # 权威事件
├── items.jsonl / usage.jsonl   # 事件派生数据
├── session.json                # 最后状态投影
└── children/
    ├── runs.jsonl              # child 权威记录
    ├── index.json              # child 派生缓存
    └── <child>/                # 内联子会话
```

启动 reconciliation 以权威日志最大序号和索引水位决定跳过、增量投影或全量重建。索引写失败不能回滚
已经 fsync 的事件；它必须留下可恢复的 dirty/reconcile 状态。全局 `child_id` locator 从所有父目录的
`runs.jsonl` 重建，授权以记录中的 `parent_thread_id` 判断，绝不从目录路径猜测。

`runs.jsonl` 一行是一次**状态转换**，不是一个 child：`queued`、`running`、终态各占一行，`resume` 也
只追加新行并把 `resume_count` 加一，不产生第二个 child。因此任何投影（`index.json`、线程 detail 的
`child_ids`、`GET /v1/child-runs`）都必须先按 `child_id` 折叠到最新一行再计数或列项；直接映射原始
日志会让同一个 child 按状态数重复出现。折叠规则统一由 `ark_ports::latest_child_runs` 提供。

子代理完成收件箱不另建可变真源：它从父线程 `events.jsonl` 的 `child_completed` 与
`child_completion_acknowledged` 事件投影。后者只有父回合成功结束后追加，因此确认写入失败会自然保留
下一回合的重投递机会。

完成收件箱使用 writer actor 的窄事件查询，只复制两类 completion 生命周期事件，不为普通父回合克隆完整
事件历史。压缩的截断点不能晚于最早未确认 `child_completed`，并须保留仍在 retained suffix 中的重复完成
事件所依赖的确认记录，防止折叠掉仍须至少一次投递的权威记录或让已确认通知重新投递。

线程 detail 的交互恢复也经 writer actor 的窄查询，只读取 turn 与 approval/user-input 生命周期事件；它是
渲染候选而非可操作性真源，不能据此重建 oneshot waiter。

## 索引现状

当前 v1 使用内存索引加原子 `data/index.json` 快照，并由单一 `IndexProjector` 维护
`indexed_seq` 和重建计划；它尚不是早期设计中设想的 SQLite/FTS。两者共享“日志权威、索引可重建、
单写者”的架构约束。是否迁移 SQLite 由实际跨会话检索规模决定，不能把未实现目标写成现状。

## 长期 memory

当前实现使用 `<data_dir>/memory.json` 保存用户显式维护的长期偏好和事实。文件通过
`ark_common::write_json_file` 原子替换，读取使用 `read_json_file_opt`：缺失或损坏文件均以空集合恢复，
不会阻断 arkd 启动。进程内 mutex 将读改写串行化，写失败不会提交内存中的变更。

文本上限为 16 KiB UTF-8 字节；空文本、超限文本和未知 id 是结构化错误。自动抽取、检索、模型 prompt
注入和损坏文件的备份/告警策略尚属待决，当前不会暗中执行。

## 迁移与删除

子会话内联于 `threads/<parent>/children/<child>/`。删除父会话前必须处理活动 child；迁移按父会话、
子会话正文、`runs.jsonl`、重建索引、清理孤儿的顺序执行，任一校验失败不留下半个迁移现场。

导出/导入必须区分可搬移的事件、附件、制品与不可搬移的绝对工作区、凭据；导入缺少工作区时，会话
可以只读但不得继续执行。
