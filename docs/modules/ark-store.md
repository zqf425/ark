# ark-store 模块设计

## 职责与边界

`ark-store` 以会话目录为系统记录，实现 `SessionStore` 和事件写入边界。每个线程是可迁移目录；
`events.jsonl` 是权威日志，`items.jsonl`、`usage.jsonl`、`session.json`、child index 和全局
索引均为可重建投影。

条目投影把一次工具调用的 `ToolCallStarted` 与 `ToolCallFinished` 折叠为**单个** `ToolCall` 条目：
结果（output/ok/duration_ms）内嵌在该条目的 tool payload 中，不会产生独立的 `ToolResult` 条目。
下游（如组合根的回合种子回放）必须按此折叠形状消费，不得假设两条目配对。

终态 `TurnFailed` 事件同样由条目投影折叠：每个失败的回合产出一个 `error` 条目（ID 取事件
`item_id`，旧日志缺字段时回退 `ark_common::keyed_item_id("turn_failed:{turn_id}")`，
`status=failed`，text 为失败原因，归属该回合），这是渲染器在**中断处**
内联展示失败原因的唯一数据源，live 流与重启/压缩后的回放因此渲染一致。工具与用户输入事件同样
优先使用 wire `item_id`，稳定 ID 只在 `ark-common` 生成。`TurnAborted` 不投影条目
（主动中断无失败语义）。`Error` 事件仍按 `error:{seq}` 投影为独立条目，`transient_retry` 只进
事件日志，不投影渲染行。

`items.jsonl` 一行是带水位的条目快照：快照本体加上它已覆盖的事件区间（`first_seq` 决定渲染顺序，
`watermark_seq` 是已折叠进快照的最后一个事件序号）。流式 delta 事件**不追加行**——每行携带累计全文，
逐片段写行会使文件随回复长度呈二次方放大；读路径从行快照起投影，仅重放各条目水位之后的 delta。
遗留形状（无水位字段或水位为 0 的行）的文本已含全部 delta，整体忽略该文件并回退全量事件回放，
避免文本重复计入；重建路径（fork/rewind/reconcile）重写文件时一律产出水位化行，使旧文件被就地升级。
事件日志追加时，流式 delta 事件不直接写派生文件，并跳过 event fsync（崩溃最多丢失撕裂尾部，
投影由权威日志补齐），`fsync_on_append` 限制仍然生效。除 delta 外，凡事件会写 items、usage、
session 任一派生，writer 都先用 `write_json_atomic` 创建线程级 `.derived-dirty.json`；版本化 marker
记录 `planned_seq`、append 前 item projector 的 `current_turn` 与完整 session checkpoint。原子 rename
后还会 fsync 父目录，未确认 marker 目录项持久前不得追加权威事件。事件落盘后，本次实际追加到
`items.jsonl`/`usage.jsonl` 的行必须完成文件 fsync；未追加的文件不做无条件 fsync，session 原子写的
临时文件则已在 rename 前同步。三类中适用的派生写及同步全部成功才删除本次创建的 marker，并再次
fsync 父目录；不适用的类别直接视为成功。任一写、同步、删除或目录同步失败均记录
`DerivedWriteFailed`。若 marker 在本次追加前已存在，后续追加不能清除它，只能由启动恢复删除。

派生文件自动修复只发生在 `FileSessionStore::with_diagnostics` 启动阶段。旧数据没有 dirty marker
时，保留原有顶层安全门：compaction sidecar 路径完全不存在、`events.jsonl` 非空且序号严格连续为
`1..=max_seq`、索引中存在该线程且 `indexed_seq == max_seq`，才校验并按需原子重写
`items.jsonl`、`usage.jsonl`、`session.json`。

marker 路径存在即保守视为 dirty，但只有严格 JSON 可解析、版本受支持、`planned_seq > 0` 且该序号
确实位于连续到日志尾的 suffix 中，才授权增量恢复；损坏或未知版本 marker 原样保留，绝不覆盖派生。
旧版仅含 `planned_seq` 的 marker 只在未压缩的完整 `1..planned_seq-1` 前缀能证明 append 前 session 与
turn context 时兼容；压缩历史无法证明便保留。另一可安全清理情形是 compaction sidecar 路径完全
不存在、日志无撕裂尾部且严格连续为 `1..=max_seq`，同时 `max_seq == planned_seq - 1`：这证明 marker
已持久化但计划事件从未提交，因此无需改写派生即可清除。任何 sidecar、序号间隙、尾部歧义或其他
水位关系都继续保守保留 marker。恢复以现有派生文件为 checkpoint，只处理 `planned_seq..tail`：items
从 marker 的 append 前 `current_turn` 起步，按每个 item 的 `watermark_seq` 幂等补齐；usage 按事件 seq
去重补齐；session 从 marker 的 append 前 checkpoint 重放 suffix。不得使用 dirty 期间可能已推进的
`session.json` 作为起点，也不得用 suffix 全量投影覆盖 checkpoint 前缀；三类适用恢复都成功后才清
marker。顶层 dirty 线程还要求全局索引行
存在且不领先于日志；启动扫描也递归覆盖父子元数据和目录名一致的合法 child thread，并在判断 dirty
状态前收敛其 pending compaction/rewind 事务。child 没有全局索引时不猜测；无 pending/dirty marker 或
marker/suffix 不合法便不写。单线程日志中部坏行只记录跳过诊断，不阻断其余线程启动；
`WriterState::open` 和读取路径都不会触发缓存写。

读路径单独判断 events 是否可信：通常仅 sidecar 不存在且非空事件严格从 1 连续时，items/usage
可直接从权威 events 投影；唯一例外是尚未截断、hash 仍匹配旧 events 的 pending compaction intention，
它不声明 hole，读路径仍把完整 events 当权威历史。其他空、损坏、已提交或 hash 不匹配的 sidecar
一律沿用 checkpoint/cache 加保留后缀的读取语义。无 dirty marker 时，items 从 `session.json`
checkpoint 的 `last_turn_id` 初始化 `current_turn`；有合法版本化 marker 时，同进程读取、启动 repair 与
`WriterState::open` 都从 marker 的 `planned_seq` 截取 suffix，并使用其 `pre_current_turn`/`pre_session`，
绝不采用 dirty 期间可能已推进的 `session.json`。损坏或无法验证的 marker 使 compacted items/usage 读取
保守失败。items 按每个 item 的 watermark 重放 suffix 中适用的 non-delta 与 delta；usage 把缓存记录
和 marker 起始的 suffix Usage 按 seq 去重合并。因此重启前即可看到最新 usage，且 Error 后续即使发生
`TurnStarted(B)` 并推进 session，无 turn id 的 `Error` 仍归属 marker 记录的 append 前回合 A，也不会
拿 suffix 覆盖 checkpoint 前缀。

## 一致性与并发

- 每个线程由一个 writer actor 分配序号并追加事件；同一数据目录由 `data/.lock` 独占。append 的
  部分写或 fsync 失败先按原长度回滚；新建日志还必须 fsync 父目录，失败时删除新文件并再次同步
  父目录。回滚本身不确定时 writer 进入失败态，禁止继续追加。
  child JSONL 的无换行合法尾记录在追加前先分隔，撕裂尾截断，避免两条记录粘行。
- 删除与 writer 生命周期共用父级锁：删除先关闭并排空已注册 writer，再在父级临界区内移除权威
  child 记录和目录；删除中的 child 创建、记录追加与事件打开均被 registry 拒绝，取消删除则保留记录。
  fork 水位超过源日志最大 seq 或源存在 compaction sidecar 时拒绝，child 元数据始终解析到 inlined child 目录。
- `subscribe` 必须在同一 actor 临界区完成“定水位、读取 backlog、注册 live channel”；慢订阅者
  断开后用 `since_seq` 重连，绝不反压 writer。
- 同父目录的 child append、删除、索引重建共用父级锁。锁表 mutex 只用于登记锁的短暂取用，在获取
  父级锁**之前**释放；真正嵌套持有的顺序为父级锁 → locator。
- writer 打开时只修复日志末尾半行；派生文件仅按上述启动安全门校验和重写，非末尾日志损坏只记录跳过诊断，不得静默掩盖。
- compaction 截断前先把当前内存 items/session 固化为 checkpoint，再向 sidecar 原子写入包含旧、新
  `events.jsonl` SHA-256 和目标 marker 的 `compaction_pending` intention；两次 rename 均 fsync
  父目录。只有 intention 持久化成功后才原子替换 events，之后把 pending 行提交为正式 marker。
  sidecar 写失败时 events 保持字节不变。崩溃恢复按当前 events hash 判定：匹配旧 hash 就撤销
  intention（完整 events 绝不算 hole），匹配新 hash 就提交 marker；两者都不匹配则保守报错而不猜测。
  提交失败后的同一 writer 在 append/再次 compact 前必须先完成目录同步和事务恢复。
- `rewind` 采用**物理删除**，但先原子持久化 `.rewind-pending.json` intention，其中包含事务 id、替换前后
  `events.jsonl` SHA-256 与事务完成后的 `rewinds.json`。随后一次性把 events 替换为“保留前缀 + 序号
  绝不复用的 `thread_rewound` reset”，再从该权威结果幂等重建 `items.jsonl`、`usage.jsonl`、
  `session.json`、`rewinds.json`，最后删除并同步 pending。恢复时 events 匹配 before hash 就撤销意图，
  匹配 after hash 就完成全部派生，均不匹配则保守报错；因此崩溃在替换前后都不会猜测。任一步失败
  返回时 writer 保留旧内存；后续 append/compact/rewind 以及 watermark、replay、snapshot subscribe、
  rewind record 和其他基于 actor 内存投影的查询，均先完成 pending transaction 并从磁盘重载。
  `SessionStore::read_items` 的磁盘投影读取也作为 writer 命令执行：先在同一单写者串行点恢复 pending，
  再读取 items，禁止 facade 绕过 actor 并发恢复。生产线程 usage 从 `replay_events` 聚合，因而经过相同
  barrier。只有确认 after 已提交才向既有订阅者恰好补发一次 reset，下一 append 接续 reset 高水位而不复用序号。
  `rewinds.json` 的独立幂等记录
  使后续压缩移除通知事件也不改变重复回退语义。仍拒绝未终结回合、任意 compaction sidecar，以及
  `.derived-dirty.json` 存在时的 rewind；dirty marker 也继续阻止 compaction。
- `fork`（`fork_thread(thread, upto_seq)`）同样物理复制：仅允许未压缩源，按序号复制目标前缀的
  `events.jsonl` 等权威文件到新线程目录，新线程从复制前缀的末尾继续分配序号，并从该前缀重建全部
  投影。存在 compaction sidecar 时拒绝 fork，避免只复制 retained suffix 却丢掉 checkpoint 前缀。
  fork 是新线程而非分支指针，不与原线程共享任何文件。

## 定时任务存储

`<root>/tasks/schedules.json` 由 `ScheduleFileStore`（`service/schedule.rs`）持有：进程内 Mutex 串行读改写，每次重写都经 `store::write_json_atomic`（同目录临时文件 + fsync + rename）原子替换，因此崩溃后磁盘上只可能是完整的旧文档或完整的新文档。内存快照只在写成功后才推进，写失败不会让进程内的状态领先于磁盘。

文件缺失按空任务表处理；**存在但不可读**同样折叠为空表，但会 `tracing::warn!` 记录路径——拒绝启动等于为一个十秒即可重建的设置拖垮整个 runtime，而静默丢失又会让用户的定时任务无故消失，告警是两者之间唯一可接受的位置。

任务与最近 10 条运行历史是本文件的真源，不进事件日志、不进 config。该层只做持久化：节奏修复、`next_run_at` 重算和 100 条任务上限都在 `ark-schedule`，由组合根在调用本存储之前完成——存储若擅自改写任务，返回给客户端的文档就会与刚写入的不一致。`insert` 是创建路径，重复 id 返回 `conflict` 而非覆盖（覆盖会丢掉该任务的历史）；`replace` 是运行记账的唯一写路径，历史追加与 `last_run` 镜像必须是一次任务变更，否则可能与 UI 更新交错。`list` 按 `updated_at` 降序返回，稳定排序保持同一时刻的插入顺序。

## IM 渠道存储

`<root>/IM/im.json` 由 `ImFileStore`（`service/im.rs`）持有，模式与 `ScheduleFileStore`
一致：Mutex 串行读改写 + `write_json_atomic` 原子替换 + 不可读折叠为空表并告警。文档形状
为 `{ channels: [ImChannelRecord], bindings: [ImBinding] }`：渠道记录携带服务端入站游标
（由编排层节流落盘，本层不节流），绑定随首条消息写入；渠道与绑定同文档是因为二者同域变更、
且渠道删除需要原子级联删除其绑定。凭证明文落盘沿用 provider `api_key` 的 v1 简化。

## 目录与恢复

全局附件存储位于 `<root>/data/attachments/<attachment-id>`，其不可变元数据表位于
`<root>/data/attachments.json`。二进制与 JSON 均先写同目录临时文件后 rename；元数据使用
`ark_common::write_json_file`，故为 UTF-8 无 BOM。读取同时要求元数据和最终二进制文件存在，
所以崩溃留下的临时文件或已删除条目不会被暴露。`AttachmentId` 是 UUID newtype，文件路径只由该
服务端生成 ID 组成，绝不拼接客户端路径。

崩溃残留的临时文件由启动清扫回收：`FileSessionStore::with_diagnostics` 在取得数据目录锁之后、
装载索引之前，对整个 `<root>/data` 递归执行 `sweep_temp_files`（`store/fs_util.rs`），删除修改时间
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
├── .derived-dirty.json         # append 与三类派生写之间的崩溃恢复标记
├── .rewind-pending.json        # rewind 多文件事务意图（完成后删除）
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
已经 fsync 的事件；它必须留下可恢复的 dirty/reconcile 状态。全局 `child_id` locator 在
`FileSessionStore::with_diagnostics` 返回前从所有父目录的权威 `children/runs.jsonl` 重建，确保重启后
无需先调用 reconciliation 也能把 child 的读取和追加路由到内联目录。`child_id` 在整个 data dir 内只允许
归属一个 parent；若启动扫描发现不同父日志声明同一 id，会返回结构化 conflict 并 fail-fast，绝不按目录
扫描顺序覆盖。运行时 append 遵循“父日志锁 → locator 锁”的固定顺序，在写权威 `runs.jsonl` 前检查归属，
并持有 locator guard 直到 append 成功及 locator 更新完成，因此并发跨父声明至多一个成功；同一父下的
状态转换仍可持续追加。后续 reconciliation 再校验并刷新该视图。授权以记录中的
`parent_thread_id` 判断，绝不从 child 目录路径猜测。

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

索引行除 `indexed_seq` 水位外还投影 `last_event_at`（最新权威事件的时间戳）。`append_event`
在推进水位的同时写入它，rewind 用截断后日志的尾部事件时间重算，启动 reconcile 用同一次日志解析的
尾部回填——三条路径都取事件自身时间，不取写入时的墙钟，列表排序由此跨重启稳定。`ThreadIndex::sorted`
按（archived、`last_event_at ?? updated_at` 降序、thread_id）排序；重命名等只触碰 `updated_at` 的
元数据操作不会把一个没有新事件的线程顶到正在流式输出的线程前面。
重命名、归档和运行时设置刷新索引时只更新元数据字段；`indexed_seq`、`last_event_at`、状态与
usage 始终由事件投影路径维护，避免并发 append/rewind 后被旧元数据快照覆盖。索引行缺失时才用
日志水位初始化新行。

## 长期 memory

当前实现使用 `<root>/data/memory.json` 保存用户显式维护的长期偏好和事实。文件通过
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
