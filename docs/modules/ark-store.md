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
- 同父目录的 child append、删除、索引重建共用父级锁。锁表 mutex 只用于登记锁的短暂取用，在获取
  父级锁**之前**释放；真正嵌套持有的顺序为父级锁 → locator。
- 启动时修复日志末尾半行并从权威日志重建派生数据；非末尾损坏不得静默掩盖。
- `rewind` 采用**物理删除**：writer actor 在同一序列化点拒绝仍有未终结回合的日志、重写 `events.jsonl`，删除目标序号及之后的事件，随后从保留前缀重建 `items.jsonl` 与 `session.json`。它不使用逻辑截断；为让已连接 SSE 客户端丢弃旧投影，会追加一个序号绝不复用的 `thread_rewound` 通知事件。`rewinds.json` 保留已接受目标的幂等标记，因此后续压缩移除通知事件也不改变重复回退的成功语义。
- `fork`（`fork_thread(thread, upto_seq)`）同样物理复制：按序号复制目标前缀的 `events.jsonl` 等权威
  文件到新线程目录，新线程从复制前缀的末尾继续分配序号，并从该前缀重建全部投影。fork 是新线程
  而非分支指针，不与原线程共享任何文件。

## 定时任务存储

`<data_dir>/schedules.json` 由 `ScheduleFileStore`（`service/schedule.rs`）持有：进程内 Mutex 串行读改写，每次重写都经 `store::write_json_atomic`（同目录临时文件 + fsync + rename）原子替换，因此崩溃后磁盘上只可能是完整的旧文档或完整的新文档。内存快照只在写成功后才推进，写失败不会让进程内的状态领先于磁盘。

文件缺失按空任务表处理；**存在但不可读**同样折叠为空表，但会 `tracing::warn!` 记录路径——拒绝启动等于为一个十秒即可重建的设置拖垮整个 runtime，而静默丢失又会让用户的定时任务无故消失，告警是两者之间唯一可接受的位置。

任务与最近 10 条运行历史是本文件的真源，不进事件日志、不进 config。该层只做持久化：节奏修复、`next_run_at` 重算和 100 条任务上限都在 `ark-schedule`，由组合根在调用本存储之前完成——存储若擅自改写任务，返回给客户端的文档就会与刚写入的不一致。`insert` 是创建路径，重复 id 返回 `conflict` 而非覆盖（覆盖会丢掉该任务的历史）；`replace` 是运行记账的唯一写路径，历史追加与 `last_run` 镜像必须是一次任务变更，否则可能与 UI 更新交错。`list` 按 `updated_at` 降序返回，稳定排序保持同一时刻的插入顺序。

## IM 渠道存储

`<data_dir>/im.json` 由 `ImFileStore`（`service/im.rs`）持有，模式与 `ScheduleFileStore`
一致：Mutex 串行读改写 + `write_json_atomic` 原子替换 + 不可读折叠为空表并告警。文档形状
为 `{ channels: [ImChannelRecord], bindings: [ImBinding] }`：渠道记录携带服务端入站游标
（由编排层节流落盘，本层不节流），绑定随首条消息写入；渠道与绑定同文档是因为二者同域变更、
且渠道删除需要原子级联删除其绑定。凭证明文落盘沿用 provider `api_key` 的 v1 简化。

## 目录与恢复

全局附件存储位于 `<data_dir>/attachments/<attachment-id>`，其不可变元数据表位于
`<data_dir>/attachments.json`。二进制与 JSON 均先写同目录临时文件后 rename；元数据使用
`ark_common::write_json_file`，故为 UTF-8 无 BOM。读取同时要求元数据和最终二进制文件存在，
所以崩溃留下的临时文件或已删除条目不会被暴露。`AttachmentId` 是 UUID newtype，文件路径只由该
服务端生成 ID 组成，绝不拼接客户端路径。

崩溃残留的临时文件由启动清扫回收：`FileSessionStore::with_diagnostics` 在取得数据目录锁之后、
装载索引之前，对整个 `<data_dir>` 递归执行 `sweep_temp_files`（`store/fs_util.rs`），删除修改时间
早于 5 分钟前的隐藏 `*.tmp` 文件（即原子写 `.{name}.{uuid}.tmp` 兄弟文件）。数据目录锁只排除本进程
与其他 arkd 实例，排除不了 `--data-dir` 指向共享目录时其他程序正在写的隐藏 `*.tmp`，时效阈值就是
为此保留的安全距离。可见名的 `*.tmp`（如 ark-config 的 `config.json.tmp`）不是本仓原子写产物，
不在本清扫范围。目录不可读、符号链接与 mtime 不可读的条目静默跳过；单个文件删除失败记入
`tracing::debug!`——都不阻断启动，清扫是卫生动作，不能反过来成为启动失败源。删除数大于零时把
该事实记录到诊断 sink（`DiagnosticKind::TempFilesSwept`）；生产路径 `FileSessionStore::open` 自建
内存 sink，arkd 侧目前没有消费者。

上传时同时计算 SHA-256 并写入元数据的 `sha256` 字段；该字段仅用于完整性与后续去重，路径仍是
UUID（不做内容寻址，避免迁移）。声明了 `mime_type` 的上传会拿前 16 字节做魔法数一致性校验，
覆盖 png/jpeg/gif/webp/pdf，不匹配返回 `mime_mismatch`；未声明或无法识别的类型跳过该校验。

**关联不进存储**: 线程与附件的授权关系**只**由线程事件流中的 `attachment_associated` 事件表达，
不新增第二张关联表。`WriterState::attachment_events` 提供只包含该事件的窄投影，
`SessionStore::replay_attachment_events` 将其暴露给边界层，重启后回放即可恢复同一授权集合。压缩时
这些事件被当作必须保留的锚点（`retention_safe_keep_last`），否则一次容量压缩会静默丢授权。
`diagnostics` 接收调用方投影出的关联集合，带 `orphan_count`（已存活 30 天以上且无关联的条目数，
仅上报，不自动回收）。

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

`thread.json` 除标识、标题与归档标记外，还持久化线程级运行时覆盖（workspace/provider_id/model/
autonomy/reasoning_effort，即 `ThreadRuntimeSettings` 的逐字段平铺；全部 `serde(default)`，旧文件
可读）。覆盖是可移植内容：fork（`forked_from`）把它们随目录复制到新线程，运行时侧如何消费见
[arkd](arkd.md)。

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
`ark_common::write_json_file` 原子替换，进程内 mutex 将读改写串行化，写失败不会提交内存中的变更。
两条读取路径的损坏语义不同：启动时 `open()` 用 `read_json_file_opt`，缺失或损坏文件均以空集合恢复，
不阻断 arkd 启动；每回合 prompt 注入路径用 `read_json_file`，损坏文件上报为错误，由调用方
（组合根的 memory seed）记录告警并跳过注入，不使回合失败。

文本上限为 16 KiB UTF-8 字节；空文本、超限文本和未知 id 是结构化错误。PATCH 的 `source` 字段是
双层 Option：键省略表示不修改，显式 `null` 是唯一的清空方式，字符串为覆盖——普通 `Option` 无法
表达"清除"，会让已写入的 source 永久删不掉。模型 prompt 注入已实现：
组合根每回合把 memory 渲染为**至多一条** `[user memory]` system 消息（见 arkd 设计）。自动抽取与
检索仍属待决，当前不会暗中执行。

## 迁移与删除

子会话内联于 `threads/<parent>/children/<child>/`。删除父会话前必须处理活动 child；迁移按父会话、
子会话正文、`runs.jsonl`、重建索引、清理孤儿的顺序执行，任一校验失败不留下半个迁移现场。

导出/导入必须区分可搬移的事件、附件、制品与不可搬移的绝对工作区、凭据；导入缺少工作区时，会话
可以只读但不得继续执行。
