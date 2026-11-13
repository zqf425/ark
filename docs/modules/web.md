# web 渲染层设计

## 职责与边界

`web/` 是 React + TypeScript Code 模式渲染层。它使用 `ark-contract` 生成类型和 typed client 渲染线程、
回合、工具、审批与设置；业务语义、路径归一化、脱敏、截断和事件到 item 的映射不应在前端重复实现。

## 约束

- 不手写 Rust 已拥有的 DTO、错误码 union 或类型别名；未建模的请求组合和纯 UI 状态仅放在
  `src/shared/contracts/web-only.ts`。
- 运行时请求集中在 feature API/client 层；组件不直接裸 `fetch('/v1/...')`。
- SSE 消费保存最后 `seq`，断线后使用 `since_seq` 恢复；页面必须能处理压缩产生的显式空洞。
  会话列表对 `thread_updated`（手动改名与后台自动标题共用同一事件）做行内折叠
  （`threads-slice.renameThreadRow`）：只改该行标题、保持行序，列表接口在下次刷新时仍是权威。
  每条成功建立的连接（含重连）都要重新上报 `open`，因为 composer 以 `streamStatus === 'open'`
  为发送闸门，重连后停留在 `reconnecting` 会让线程在刷新页面前无法再发消息。指数退避只在连接
  稳定存活一段时间后重置，不能每收到一帧就清零，否则运行时重启时会形成重连风暴。
  `onError` 写入的传输诊断不能被随后裸 `onStatus('reconnecting')` 抹掉：仅显式传参或恢复到
  `open` 才清除 `streamError`。
- Plan、Changes、File、Terminal 面板订阅 `workbenchRevision` 而非 `lastSeq`。`lastSeq` 每个事件
  （含每个 text delta token）都递增，作为 effect 依赖会让每个 token 触发约 6 次 HTTP 全量重取；
  只有 `goal/todos` 与 `tool_call_started/finished` 这些会改变服务端投影的事件才递增 revision，
  detail 播种、重置与 `thread_rewound` 另做单调推进。
- SSE 空闲看门狗以“是否收到字节”为存活依据，不以“是否解析出事件帧”为依据：`stream()` 在每次
  `reader.read()` 拿到非空字节时上报活动，订阅层据此重置 75s 看门狗。只到达 `: keep-alive` 注释帧的
  空闲线程因此保持同一条连接；只有真正无字节静默超过阈值才 abort 并从最后 `seq` 重连。注释帧不进入
  reducer，也不改变 `lastSeq`。
- `child_completed` 按稳定 `notification_id` 去重后显示持久摘要卡片；实时到达时显示一次性提示。摘要仅作
  父线程状态展示，不能在 web 侧重建或改写模型上下文。
- `child_completions` 详情与 reducer 内存均限制为最新 50 条；完成、失败和中止状态必须本地化为用户文案，
  不直接显示内部枚举值。
- 子代理监督只由 `child_activity` 与 `child_completed` 驱动刷新，不能因文本 token delta 重复请求
  child-runs；排队与运行同样必须显示为用户文案。已决审批和用户输入在当前页面会话中保留为历史
  卡片。刷新后的待处理 gate 只可来自服务端的 runtime 快照，web 不得从历史 item 猜测仍可提交的 gate id。
- detail 返回 `runtime` 时，web 在以 detail watermark 建立 SSE 前将 active turn 和其中的待处理 gate seed
  到本地状态；SSE terminal/resolved 事件仍是后续状态的权威更新。
- Code 右侧的 Plan、Changes、File、Terminal 均调用服务端投影；File 仅预览本线程已成功 `read` 的工具
  输出，浏览器不得直接读取工作区。子代理控制请求始终带父线程 query，不能只凭 child id 操作。
- Changes 同时展示活动工作区的服务端受限 Git 投影（暂存、未暂存、未跟踪）；切换请求仅提交路径给
  `arkd`，运行中存在父回合或子代理时服务端拒绝切换。浏览器不读取目录、Git 或未跟踪文件内容。
  差异文件行默认折叠，展开后渲染服务端 `patch` 字段的行级 diff（解析仅在展开时进行，面板打开
  不全量解析所有 patch）；`patch` 缺失（二进制或超出字节上限）时明确提示，浏览器不自行 diff。
- 会话流 footer 展示线程用量与上下文水位：用量由权威 `usage` 事件逐条累加，打开线程时以
  `GET /v1/threads/{id}/usage` 播种且只填充空通道（避免与实时事件重复计数）；上下文水位取最近一次
  样本的 `prompt_tokens` 对 `GET /v1/runtime/providers` 发布的 `compaction_threshold_tokens`
  （模型窗口的 80%，输出预留后取整；未声明窗口按 256K 默认窗口推导，与运行时压缩行为同源）的占比，
  仅作展示，不参与运行时决策；仅当完全无有效 provider 时阈值缺省，此时隐藏水位。
  限额缓存带 TTL 并定时重取（footer 常驻挂载、跨线程不重挂，一次性获取会在设置页修改后永久陈旧）。
  `compaction_completed` 以摘要形式显示最近一次压缩规模。footer 另展示最近一轮流式速度与会话累计
  缓存命中率：速度由该轮首个到末个 delta 的实测窗口（短于 0.3s 不显示）折 `usage` 的
  `completion_tokens`，窗口在回合/轮边界与 `usage` 折叠后清零，不保留陈旧值；命中率只取
  `cache_hit_tokens / (cache_hit_tokens + cache_miss_tokens)`，两者按 `sum_opt` 语义累加，缺计数时
  不显示，绝不用 `hit / prompt_tokens` 伪造。
- 模型输出的 markdown 渲染集中在 `shared/ui` 的 `Markdown`（marked + DOMPurify 消毒后注入）与
  `code-highlight`（文件预览高亮）：模型输出是不可信输入，任何进入 `dangerouslySetInnerHTML` 的
  HTML 必须先经 DOMPurify；这是展示层能力，不与 Rust 侧业务逻辑双实现。
- 设置页的 tuning、provider 与 MCP 都使用生成契约；MCP env/header 值只写不回读，编辑时的空
  secret 字段表示保留，不表示删除。provider 的 api key 分两路：列表、能力与诊断只显示
  `has_api_key` 与掩码 hint，而点「编辑」时单独请求 `GET /v1/runtime/providers/{id}` 取回完整条目，
  把已存 key 回填到表单（避免编辑时看起来密钥丢了）；该端点仅供设置表单使用。
  表单里的 secret 用共享 `TextField` 的 `revealable` 变体：默认掩码，可显式切换明文/掩码并带
  `aria-pressed`。provider 表单不再有 label 输入框，api_key 占据该位置；已存的 label 仍在草稿中透传，
  保存不会静默清空。
- 新增/编辑 provider 可显式查询模型目录，在用户选择后才把远端明确声明的 context window 与识图能力写入
  待保存表单；传输层对目录未声明的能力仍保持 unknown、不按模型名称猜测，也不自动提交配置。导入对话框在
  目录解析之外做上下文嗅探：优先用目录明确声明的窗口（含 `max_model_len`、`top_provider.context_length`、
  `providers[].context_length` 等别名），识别不出的模型按 256K 兜底并在候选行标注，落表后仍可逐行修改。
- 组件状态按 feature 拆分，避免重建旧 `Workbench.tsx` 巨型组件；UI 变更应进行实际浏览器验证。
- 会话流按回合分组渲染：一回合内的 reasoning、tool_call、tool_result 收进一张可折叠"执行过程"卡
  （对齐 smart-ark 的工作卡），运行中自动展开、结束后自动收起，用户开关即固定；用户消息与助手回复
  留在卡外。回合多于一个时在画布右缘提供逐回合跳转导航条。会话标题行提供「评审改动」
  （`POST /v1/threads/{id}/review`，同一 202 回合信封，进度走线程事件流）；会话列表行提供「导出」
  （`GET /v1/threads/{id}/export` 快照下载，blob 触发浏览器保存）。
- 回合运行中，画布底部（注意力点）渲染一条实时活动行（`LiveActivity`）：镜像活跃回合最新的流式
  item——reasoning 显示"思考中 + 已耗时 + 尾迹（约 140 字符）"，tool_call 显示"运行 {tool}"，其余流式
  item 显示"正在生成"；数据完全来自已折叠的 item 投影（`activeTurnId` + 最新 streaming item），不新增
  wire 契约、不与 Rust 双实现。动机：过程卡位于助手消息上方，画布自动吸底时它在视野外，纯思考阶段
  画布毫无变化，健康流会被误读为卡死；该行让用户能实时感知推理方向并及时 steer。回合结束后随
  `turnState` 离开 running 而消失，历史仍归过程卡。
- 输入框「技能」面板消费线程级技能视图（`GET /v1/threads/{id}/skills`，合并 base/extra 并展示加载
  警告数，「重新扫描」调用 refresh 路由）；未选择线程时回落全局 `GET /v1/skills`。显式 `/id` 提及
  仍是唯一激活方式，不新增技能运行时字段。
- 已知未接入的路由：`attachments/diagnostics`（诊断）、
  `child-runs/{id}/thread|events|cleanup`（子代理完整转录查看器需要独立的轮询/订阅设计，见子代理
  监督约束）、`sessions/{id}/resume-thread`（定时任务结果已直接携带 thread_id 跳转）。
- Code 使用 ARK Console shell（方案 A）：46px 全局顶栏 + 52px 活动栏 + 26px 状态栏，主舞台在
  `code` 场景下是"线程列表 / 会话画布 / 常驻检查器"三列；顶栏承载工作区、Git head、运行态、模型、
  主题切换与 ⌘K 入口，状态栏承载连接、工作区、变更数、上下文占用、tokens 与版本。会话搜索和最近/
  归档切换只过滤已加载的 `ThreadSummary`，不新增搜索 API；会话列表按服务器权威顺序渲染
  （archived 优先级靠后、`last_event_at ?? updated_at` 降序），行内时间显示同一活动键；活跃会话的
  忙碌标记来自实时事件流（`turn_started`/终态事件驱动 `activeTurnId`），其余已加载行由 5 秒轮询
  `GET /v1/threads` 的 `running` 标记驱动，轮询帧在回合边界不得覆盖实时判定（见
  `use-thread-running.ts`）。Plan、Changes、File、Terminal 是常驻
  检查器的四个 Tab（收起用 `hidden` 保挂载，面板草稿与服务端读模型不得因折叠丢失），不再使用带
  遮罩的浮动抽屉。`review`、`new`、`settings` 是同一 shell 下的全屏场景，由 `view-slice` 的 `view`
  选择；`commandOpen` 控制 ⌘K 命令面板。
- 视觉样式只消费设计令牌：浅色"纸白"为默认主题（`preferences.ts` 默认 `theme: 'light'`），色板在
  `styles/tokens.css`，暗色"石墨"覆盖在 `styles/theme.css`，共享原语样式在 `styles/primitives.css`，
  由 `styles/global.css` 聚合引入；圆角与阴影分级都在令牌层定义，组件 CSS 出现字面 hex/rgba 色值
  属于破坏主题对等性的检视阻断项。ARK Console 语言是扁平、高对比的开发者控制台：无渐变玻璃、
  无氛围光晕、无浮动大圆角输入框；1px 硬边框定义层级，控件 5px / 卡片 7px / 面板 10px / 对话框
  12px 圆角，实心主行动按钮，用户消息是左对齐引用卡而非右对齐气泡。shell 专用的 `--console-*`
  令牌与 diff 语法色 `--tk-*` 同样在 tokens/theme 两侧定义。设置页沿用同一 shell：分组导航 +
  分区表单，卡片用 hairline 分隔，嵌套列表用微底色"凹井"。
- 设置页供应商表单用单个「导入模型」入口合并目录查询与批量导入（对齐 smart-ark 的导入对话框、按
  arkd 能力面收敛）：点击即拉取远端目录并弹出选择对话框，候选来自目录与粘贴输入（每行一个 id，服务无
  /models 时使用）；已存在的 id 标记且默认不勾选，确认后并入表单清单。导入时自动嗅探上下文窗口，识别不出
  用 256K 兜底（对齐 smart-ark 的 `DEFAULT_PROVIDER_CONTEXT_WINDOW_TOKENS`），默认 `model` 为空时由首个
  候选行回填，命中默认模型的行把元数据镜像到条目级字段。
- 模型清单字段占满设置网格的一整行：一行需要 id、默认标记、上下文窗口与识图开关，挤在半宽列时识图会
  溢出到相邻的 api_key 提示上；整行宽度加窄窗口换行保证所有控件留在自己的卡片内。
- 默认模型与所有导入模型用**同一套 `ModelRow` 行**展示，默认模型不再单独占一个输入框：每行都是
  id + 上下文窗口输入框 + 识图开关（+ 移除），首行的 id 可编辑、额外标记「默认」且不可移除。**每行只管自己的上下文窗口与识图能力，没有跨行覆盖、也不存在"谁为准"的
  冲突**：首行（默认模型）的值存条目级 `context_window_tokens` / `supports_vision`，其余行的值存各自的
  `models` 清单项（`ProviderModelInventoryItem`）；线程切换到某模型时只读该行元数据。因此「高级选项」只保留
  真正作用于整个供应商的 `headers` / `max_tokens` / `temperature`，不再重复放模型级字段（旧版把条目级窗口/
  识图错放进高级设置，只对默认模型生效却看起来像全局）。保存时整体提交：表单恒发完整列表（省略 = 保留，
  仅限 MCP 等部分编辑客户端；显式空列表 = 清空，与 secret 类"空=保留"语义不同）。目录查询用独立的
  `catalogBusy`，不锁定表单其余字段。
- 会话运行时覆盖停靠在输入框区（`.ark-composer-stack`），不再占据会话标题区：供应商/模型、权限档位与
  推理强度是输入框工具条上的即时选择器（`ComposerRuntimeControls`），打开时重读 `GET /v1/threads/{id}`
  再经 `PATCH /v1/threads/{id}` 的 `runtime` 覆盖立即保存；后端按字段合并、无清除语义，因此不可清除的
  字段不提供"未设置"选项，`reasoning_effort` 空串即"跟随默认"（协议映射见 [ark-llm](ark-llm.md)）。
  工作空间、上下文结构和手动压缩也收进同一排工具条，不再有展开式详细面板：工作空间 pill 直接列出
  `GET /v1/workspace` 的 active / recent 项目，点选即 `POST /v1/workspace/switch` 并把同一路径经
  `PATCH /v1/threads/{id}` 写成本会话覆盖；「新增项目空间」调用 `POST /v1/workspace/pick` 与
  `POST /v1/workspace/validate` 后走同一切换流程，不需要手填路径。活跃回合时禁用切换，服务端 409 是最终裁决。
  上下文环弹层展示工具、系统提示、技能、消息、思考、检查点、其他的**结构占比**（渲染行数与字符数），
  总量仍取最近一次权威 `usage` 事件的 `prompt_tokens` 与 tuning 窗口；token 估算的唯一宿主仍是
  `ark-agent::TokenEstimator`，前端不重算 token。弹层内「压缩上下文」直接调用
  `POST /v1/threads/{id}/compact`。共享 `ModelProviderMenu` 提供「供应商 → 模型」二级菜单与搜索，
  会话输入框用 `variant="flyout"`：供应商列表常驻、hover/focus 浮出独立滚动的模型卡，避免长模型
  清单挤占输入框；推理强度留在菜单 footer；所有选择打开时重读线程覆盖、写入仍走 `PATCH`。
  设置页使用生成契约提供同一全局权限，展示“永远询问”“仅工作区修改”“完全访问”三档。
- 会话标题下方有紧凑统计条（`SessionStatsBar`）：轮次与模型调用数来自 `GET /v1/threads/{id}/usage`，
  墙钟耗时来自 `GET /v1/debug/llm-rounds`（服务端按权威 `usage` 事件折出的 `duration_ms`），并展示
  prompt/completion/total tokens、真实 `cache_hit_rate` 与 `cost_usd`；诊断分页 `truncated` 为真时把调用数
  标为下限，请求失败保留上次已验证数值而不伪造零。运行时版本、SSE 状态、数据目录等诊断信息不能占据
  主任务层级，必须退为状态详情或提示信息。
- 设置页的分类导航是 `web` 内本地状态，只决定既有 section 的可见性；每个 section 仍独立持有既有 hook，
  不把 provider、MCP 或 runtime 配置逻辑回流到 Code 会话状态或 `ark-agent`。
- 运行调优参数按 smart-ark 的术语分组呈现（回合与 token / 工具保护 / 上下文与压缩 / 流式传输 /
  权限档位），wire 键保持 `TuningSettings` 不变；子代理文案同样对齐（启用子 Agent 委派、单会话并行
  子 Agent 上限）。设置页新增只读「插件与能力」section：`GET /v1/runtime/capabilities`（能力 pills 与
  页面/面板目录计数）、`GET /v1/runtime/plugins`（插件贡献与失败/跳过警示）、`GET /v1/runtime/tools`
  （该路由在 Rust 侧仍是未建模 JSON，前端经 `web-only.ts` 的视图类型渲染，Rust 建模后应迁移到生成
  类型）。`/v1/runtime/tuning` 与 `/v1/runtime/model-providers` 是运行时的 501 占位路由，不是缺口。
- 输入框在活跃回合提供两种非中断动作：「追加」（steer，立即注入下一轮）与「排队」（本地暂存为
  chips，回合结束的 live→idle 转变触发按顺序自动发送）；排队消息可逐条移除，纯本地状态不进 wire。
- 未发送草稿按线程保存在 store（`runtime-slice` 的 `drafts: Record<ThreadId, string>`，`setDraft`/
  `clearDraft`）：Composer 因 `key={threadId}` 在切换会话时重挂载，textarea 不持草稿，挂载时从
  store 恢复本线程草稿，输入即写回（受控 textarea）。发送成功仅清除与已发送文本一致的草稿条目，
  发送失败或被闸门拦截（`sending`、SSE 非 `open`）时草稿原样保留；排队动作把文本移入组件本地
  队列并清掉对应 store 草稿。草稿只存内存、不进 wire、不持久化到磁盘，刷新页面即清空。
- 附件经 `POST /v1/attachments`（Base64 JSON）上传后以 id 列表随 `TurnRequest.attachment_ids` 提交；
  上传失败走命令错误提示，已传 id 可在发送前移除（服务端无删除路由，孤儿附件由运行时清理策略负责）。
- 消息画布用 `ThreadItem.attachments`（serde default，可为空）渲染用户消息下的附件 chips：
  每个投递结果一枚 chip（文件名 + 投递通道 i18n 标签），`image`/`preview_text` 为中性样式，
  `degraded`/`skipped`/`unavailable` 用警示色并以 `detail` 作 title 提示。字段由后端持久化，
  历史回放与 live 流渲染一致；运行时未发附件字段的旧 item 保持原样。
- 「技能」面板的消费面见上方"输入框「技能」面板"条目：线程级视图 + 全局回落，显式 `/id` 提及即激活
  是 arkd 的技能指定方式；不新增技能运行时字段。
- 用户可见文本通过 `web/src/shared/i18n/` 的消息键与中英文资源提供；新增 UI 不得硬编码文案。
  该目录已按域拆分为 `catalogs/*`（`common`、`errors`、`session`、`thread-list`、`workbench`、
  `session-canvas`、`settings-*`）：每个 catalog 输出一张
  `键 -> [zh-CN, en-US]` 表，`messages.ts` 只做聚合并导出 `translate` 与由它推导的 `MessageKey`，
  所以拼错的键是编译错误而不是静默英文回落。新文案加到对应域文件，不新建巨型 `messages.ts`。
- 非 React 代码（reducer、formatter、API 结果映射）不能调 `useI18n`，也不能直接写文案：
  `I18nProvider` 在语言变更时把当前 locale 发布到 `current.ts`，这些调用点用 `translateNow` 读取。
  组件仍用 context 的 `t`，以便语言切换时重渲染；`translateNow` 只是非 React 场景的回落。
- 错误文案由 `shared/api/error-describe.ts` 的 `describeError` 统一拼装：本地化的是错误 **code** 的短提示
  （`error.hint.<code>`），运行时返回的 `message` 保持原文，两者用 `common.hintSeparator` 连接。
  调用方不得自行拼 `提示：message` 或另建一份 code→文案表；未知 code 回退到 `HTTP <status>`。
- `catalog-completeness.test.ts` 是静态双语守卫：逐个键检查中英两列非空、英文列不含 CJK、英文列不是
  中文的逐字复制、同键占位符集合一致、`translate` 对每个键都非空，并校验主要前缀与每个错误 code 都有
  本地化提示。
- 语言、字号和主题是 renderer-only 偏好，默认 `zh-CN`/中号/跟随系统，保存于当前浏览器的 localStorage
  并即时更新根 `lang` 属性及字号、主题 token；选择“跟随系统”时监听 `prefers-color-scheme` 的变化并即时
  重新解析浅/深色主题。它们不属于 Rust wire 契约或运行时配置，不能通过设置 API 写入。

## 桌面宿主边界

`apps/ark-desktop` 是已实现的 Tauri 窗口和本地 `arkd` 生命周期宿主。它不会成为 web 的业务 API：生产构建
仅通过 `VITE_ARK_BASE_URL` 将既有 typed HTTP/SSE client 指向受限 loopback 运行时；开发仍使用 Vite proxy。
不得在 React 组件中引入 Tauri command、shell/plugin API 或 provider/Agent 分支。

独立 `plugin-code-ui` 尚未成为当前 workspace 模块。其未来迁移必须先建立需求设计，不能借桌面外壳绕过
`ark-ui` 注册总线。

## 定时任务管理

设置页「定时任务」section(需求见 [scheduled-tasks](../requirements/scheduled-tasks.md)):
只消费 `/v1/schedules` 的生成契约类型;节奏编辑、启停、删除、立即运行、最近结果展示与
"打开会话"跳转运行线程;全部文案走 i18n 消息键。客户端不做 next_run 计算,以后端返回为准。

## 记忆与技能配置

设置页「记忆」section 消费 memory 路由的生成契约类型:列表、新增、编辑(text/tags/source)、删除与
存储诊断。运行时按预算把全部条目注入每回合,UI 不提供逐条启用开关——删除即停止注入
([memory-store](../requirements/memory-store.md));text 的 16KiB 上限在前端按 UTF-8 字节校验。
编辑时清空来源输入提交显式 `null`,这是 source 的唯一清空方式(键省略表示不修改,契约的双层
Option 语义);与 secret 类字段"空=保留"的约定相反,此处"空=清除"。

「Skills」section 在原只读列表之上提供发现配置(`skills.enabled` 总开关与 `extra_dirs` 额外根,
上限与运行时一致);该配置为 Hot 类,保存后在下一次技能扫描时生效,无需重启;被拒绝的 patch
原样展示。
