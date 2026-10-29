# Code 与设置界面完整功能设计

> 状态：**已实现，待最终全量验证**。本需求由 2026-10-28 的范围确认建立；本次实现范围和有意保留的
> 边界见文末“实现结论”。

## 目标与边界

交付可独立使用的 Code V1 与设置 V1：用户能管理线程、驱动和控制回合、处理真实的审批/输入闸门、
查看子代理和工作区产物，并在不暴露密钥的前提下完整管理运行时配置、provider、MCP 与 skills。

本需求包含：

- Code：线程新建/选择/重命名/归档/删除/fork，回合发送/steer/中断/压缩，审批与用户输入，子代理
  列表及控制，Plan/Todo，Changes/File/Terminal 四个有真实数据源的右侧面板。
- 设置：通用配置、完整的 hot tuning、子代理、provider、MCP、工作区和 skills。
- 支撑上述 UI 所需的 wire DTO、端口、事件投影、持久化、HTTP/SSE 和组合根装配。
- 本需求开始前发现的正确性问题：provider 高级字段编辑丢失、SSE 控制帧序号回退、
  `ChildRunAggregates` 的 JSON 命名漂移，以及 parent steer 未进入运行中 reactor。

本需求不包含 attachment/memory、Studio、Tauri 外壳、旧 Electron 专属功能、任意 shell 交互终端、
云端后端或工作流。Terminal 面板只展示 ark 已记录的 bash 执行转录，不能把浏览器变成绕过工具沙箱的
裸 shell。

## 设计原则

1. **先修正确性，再扩展 UI**：无损配置编辑、单调 SSE 序号和真实 steer 是上线前提，不以视觉功能掩盖。
2. **服务端投影，前端只渲染**：Changes、Plan、Todo、File 和 Terminal 的领域解释在 Rust 一侧完成；
   web 只保存生成 DTO、渲染和本地交互状态。
3. **会话日志仍是真源**：目标、待办、gate 决议和工作区产物以事件追加；详情/面板读模型可由事件重建。
4. **密钥永不回读**：provider API key、MCP env/header 值只作为写入输入。UI 只显示“已配置”，不返回
   值；当前 V1 中空 secret 字段表示保留，替换必须重新填写。显式删除需要独立 patch 语义，不能把空
   表单误解为清空。
5. **真实闸门**：审批和用户输入先持久化请求事件，再由内存 broker 等待有身份的 HTTP 决议；重复决议、
   过期 id、跨线程 id 均拒绝，取消/关闭使等待者收敛而不悬挂。
6. **工作区边界不下放给浏览器**：File 面板只投影同一会话已成功 `read` 的受限工具输出，绝不让 web
   发起直接文件读取或传绝对路径绕过工具沙箱。

## 阶段 A：先行修复

### A.1 Provider 无损编辑

现有编辑页将未回读的高级字段提交为空，`upsert` 会整体替换条目。当前实现保留空的 API key、headers、
`max_tokens`、`temperature`，因此普通编辑不会破坏未回读的配置；用户可显式填写新值覆盖。读取视图不
暴露 key/header 值。删除 secret 的独立 patch 为后续能力，不能由空字段隐式触发。

### A.2 SSE 与 wire 修复

- SSE keep-alive/idle-timeout 控制帧必须复用该订阅已发送的最大 seq，不能使用旧 watermark 倒退。
- `ChildRunAggregates` 使用 camelCase wire 字段并重新生成 TypeScript。
- `compaction_completed` 复用 `removed_items` 表示被折叠消息数，并提供 checkpoint 字符数；压缩后的
  checkpoint 优先保留可识别的短标识符，预算不足时明确标识裁剪，避免模型对不可恢复的早期内容作误导性回答。

### A.3 Parent steer

`TurnRegistry` 持有每个活动 parent turn 的有界 `SteerQueue`；`/steer` 仅在目标活跃且队列未满时入队。
Reactor 在安全轮次边界 drain 后写 `turn_steered` 并把文本作为用户来源历史注入。不能只追加事件而不改变
模型可见上下文。

## 阶段 B：线程、闸门与计划读模型

### B.1 线程与回合

- UI 调用既有 `PATCH`/`DELETE`，提供重命名、归档/恢复与删除确认；仅在服务端允许删除时删除本地选择。
- 实现 `POST /v1/threads/{id}/fork`，复用 `SessionStore::fork_thread`，返回新的 `ThreadSummary`；V1 以当前
  持久化 watermark 为唯一 fork 点，不复制活动 turn。历史指定 seq 需要可解释的时间线选择器，暂不暴露
  半成品 API。
- 线程列表本地搜索只针对已加载列表，归档列表显式切换，避免宣称有尚未实现的全局全文检索。
- composer 增加 steer 入口；发送、steer、interrupt 的按钮状态均由活跃 turn 和 SSE 状态决定。compact 的
  HTTP 标记接口已保留，但未在 V1 主界面增加无明确收益的按钮。

### B.2 审批与用户输入

增加 wire 请求/响应 DTO、`GateBroker` 的交互式实现和 `/v1/approvals/{id}`、`/v1/user-inputs/{id}` handler。
决议后由 reactor 写 `approval_resolved`/`user_input_resolved`；HTTP handler 不直接伪造终态事件。一个 request
只能决议一次，父线程被取消、gate 超时或服务关闭后请求进入明确终态。

### B.3 Goal 与 Todo

新增带稳定 id 的 `PlanView`、`TodoEntry`、请求与快照 DTO；在 `RuntimeEvent` 补齐 `goal_updated`、
`goal_cleared`、`todos_updated`、`todos_cleared` 载荷。GET 从权威事件投影，POST 写新快照事件，DELETE 写
清除事件。Plan 面板和流式 reducer 使用同一份 DTO 与 SSE 事件，不在 TypeScript 重演领域规则。

## 阶段 C：工作区与子代理面板

### C.1 Changes、File、Terminal

- 工具事件持久化结构化参数和已截断输出；服务端从成功的 `write`/`edit` 生命周期投影 workspace change。
  Changes 面板展示相对路径、操作类型和关联 turn，浏览器不解析自由文本。
- File 预览只返回同一线程已成功 `read` 的结构化工具输出。这样预览与模型实际看到的内容一致，也不会新增
  绕过工具策略的读取通道；尚未读过的文件返回结构化 not-found，并提示先通过 agent 读取。
- Terminal 是 bash tool 的只读转录投影（命令、开始/结束、退出状态、截断输出），不提供浏览器直连 shell。

### C.2 子代理

复用现有 child-runs API，在 Code 侧显示父线程 child 列表、状态、摘要、用量、子会话跳转及
abort/steer/resume。终态摘要卡片继续按 `notification_id` 去重，且不与控制面混为一处。

## 阶段 D：设置完整闭环

1. 将 runtime tuning 定义为明确的 Rust wire view/patch，而不是 web 侧猜测 opaque JSON；所有字段沿用
   `ReloadClass::Hot` 和既有范围校验，整批非法 patch 不改变旧快照。
2. provider 表单使用阶段 A 的无损请求；测试请求可测试保存条目或未保存草稿，明文 key 永不出现在列表、
   toast、日志或测试失败消息中。
3. MCP 提供安全 edit view：返回 transport、command/url、args、allow-tools、env/header 是否已配置及
   连接状态；secret 值只接受写入，空 secret 字段保留原值。显式删除和热断开旧 session 需要独立 patch
   协议，不能由当前全量 upsert 猜测。
4. 工作区设置继续使用服务端 validate/status；skills 继续只读诊断，刷新后更新同一份 runtime snapshot。

## 验收与验证

- 每个新行为先写失败的 Rust 或 Vitest 测试，再实现最小代码；涉及 HTTP 的行为用真实 `AppState`/port fake
  覆盖，不只断言组件内部状态。
- 新增/变更 contract 后运行 `cargo test -p ark-contract --lib ts_export`，并确保
  `git diff --exit-code web/src/generated`。
- 必测故障路径：provider/MCP 更新保留 secret，gate 重复/过期/取消，steer 队满，fork 不能携带活动回合，
  文件路径越界，SSE 控制帧 seq 单调，事件重放后 Plan/Todo/Terminal/Changes 与实时结果一致。
- UI 使用真实 `arkd` 测试实例和浏览器验证：线程操作、gate、子代理控制、四个面板、热更新设置及失败提示。
- 完成前运行 `cargo fmt --all -- --check`、`cargo check --workspace`、`cargo test --workspace`、
  `cargo clippy --workspace --all-targets -- -D warnings`（环境具备时）、`npm test`、`npm run build`、
  `npm run check:rules`。

## 实现结论

本次已实现阶段 A 的四项修复、真实 parent steer 与交互 gate、线程管理及当前 watermark fork、目标/待办
事件投影、Changes/File/Terminal/子代理控制面板，以及 typed tuning、provider 与 MCP 的安全编辑闭环。

设计复核后的两项收敛决定：一是 File 不增加浏览器直接读盘能力，而是复用现有 `read` 工具的受限且已
持久化输出；二是 secret 清除不复用空字符串语义，避免无回显表单在普通编辑时破坏凭据。若需要显式删除，
应另行设计带字段级意图的 patch，而不是扩大全量 upsert 的歧义。

## 阶段 E：供应商模型目录与实际生效的模型配置

本阶段补齐 V1 的 provider 配置闭环：设置表单可在保存前或编辑已保存条目时显式查询模型目录，并在用户
选择模型后预填目录明确返回的 context window 与识图能力。查询是服务端代理的有界单次请求（10 秒、1 MiB、
500 条），错误不回显密钥或 base URL；通用目录未提供元数据时字段保持 unknown，绝不根据 model id 猜测。

选择只是修改待保存草稿，避免远端目录变化隐式修改运行配置。保存后的 context window 会约束下一父/子
回合的压缩阈值，并为用户设置的 max_tokens 预留输出空间。此前已保存但未实际使用的 headers、temperature
和 max_tokens 同步进入模型请求；核心协议头仍由 endpoint family 覆盖，防止自定义 header 破坏认证。

## 阶段 F：Code 与设置的信息架构

视觉重构只发生在 `web/`：不增加 wire 字段、不改变 reactor 状态，也不让插件或 provider 实现承担界面职责。
Code 将已加载会话分为最近/归档两个本地范围，并提供大小写无关的本地标题搜索；搜索不承诺全局全文检索。
默认结构是任务侧栏和宽会话画布，中栏以当前会话和真实回合数/执行态作为操作焦点，输入区保持现有
send/steer/interrupt 语义。Changes、Plan、File、Terminal 是按需打开的上下文抽屉，默认收起；通过计划、
变更或文件选择展开，收起不能卸载或改写其数据。运行时诊断退为低优先级状态，不能压过任务与输入区。

设置页将既有表单按工作区、运行时、供应商、集成、Skills 分类导航。导航只控制可见内容，各 section 的 hook
和保存语义不变：Provider/MCP 的 secret 仍只写不回显，模型目录仍需用户显式选择后才进入草稿。

阶段 F 已将 Code 和设置改为面向任务执行的信息结构，并补上已加载线程的本地搜索/归档切换与设置分类导航。
在界面复核后，Code 再收敛为会话优先、上下文按需打开的工作台，移除了固定三栏和控制台式诊断视觉。
这是一层渲染和本地布局状态的变更，未修改 `ark-agent`、HTTP/SSE 契约、插件总线或运行时语义。

## 阶段 G：界面国际化与可读性偏好

> 状态：**当前实现**。

web 在 `shared/i18n/` 提供唯一的消息资源和 `I18nProvider`。当前支持简体中文与英文，默认简体中文；
设置的“外观与语言”区域允许用户切换显示语言、小/中/大字号及浅色/深色/跟随系统主题（默认跟随系统）；跟随系统时监听系统配色变化并立即应用，改动只持久化到本机浏览器
localStorage。该偏好是纯渲染状态，不能写入 runtime 配置、更不能为它增加 Rust DTO 或 HTTP 接口。

设置导航和新增加的外观区域已使用消息键。后续所有用户可见 UI 文案必须接入该资源，新增功能须同时提供
中英文案和相应测试；不得继续增加硬编码用户文案。现有功能将在后续界面迭代中逐步迁移，以避免把无关
UI 重构混入本需求。
