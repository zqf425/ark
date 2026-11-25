# web 渲染层设计

## 职责与边界

`web/` 是 React + TypeScript Code 模式渲染层。它使用 `ark-contract` 生成类型和 typed client 渲染线程、
回合、工具、审批与设置；业务语义、路径归一化、脱敏、截断和事件到 item 的映射不应在前端重复实现。

## 约束

- 不手写 Rust 已拥有的 DTO、错误码 union 或类型别名；未建模的请求组合和纯 UI 状态仅放在
  `src/shared/contracts/web-only.ts`。
- 运行时请求集中在 feature API/client 层；组件不直接裸 `fetch('/v1/...')`。
- SSE 消费保存最后 `seq`，断线后使用 `since_seq` 恢复；页面必须能处理压缩产生的显式空洞。
  会话列表对 `thread_updated`（手动改名与首轮启动时的自动命名共用同一事件）做行内折叠
  （`threads-slice.renameThreadRow`）：只改该行标题、保持行序，列表接口在下次刷新时仍是权威。
  每条成功建立的连接（含重连）都要重新上报 `open`，因为 composer 以 `streamStatus === 'open'`
  为发送闸门，重连后停留在 `reconnecting` 会让线程在刷新页面前无法再发消息。指数退避只在连接
  稳定存活一段时间后重置，不能每收到一帧就清零，否则运行时重启时会形成重连风暴。
  `onError` 写入的传输诊断不能被随后裸 `onStatus('reconnecting')` 抹掉：仅显式传参或恢复到
  `open` 才清除 `streamError`。
- 实时回合的工具行由前端 reducer 折叠:`tool_call_started`/`tool_call_finished` 在 wire 上是
  裸事件(服务端只把它们折叠进**持久** items 投影,实时通道不发 item 快照),渲染器按
  `apply_tool_started/finished` 同语义映射为 `tool_call:{call_id}` 的确定性 item
  (FNV-1a,与 `turn_failed` 同构),否则回合进行中的画布缺所有工具行——工具卡、
  正文路径白名单(prose-path-links)全部落空,直到整页 detail 重载才补齐。
- Plan、Changes、File、Terminal 面板订阅 `workbenchRevision` 而非 `lastSeq`。`lastSeq` 每个事件
  （含每个 text delta token）都递增，作为 effect 依赖会让每个 token 触发约 6 次 HTTP 全量重取；
  只有 `goal/todos` 与 `tool_call_started/finished` 这些会改变服务端投影的事件才递增 revision，
  detail 播种、重置与 `thread_rewound` 另做单调推进。
- SSE 空闲看门狗以“是否收到字节”为存活依据，不以“是否解析出事件帧”为依据：`stream()` 在每次
  `reader.read()` 拿到非空字节时上报活动，订阅层据此重置 75s 看门狗。只到达 `: keep-alive` 注释帧的
  空闲线程因此保持同一条连接；只有真正无字节静默超过阈值才 abort 并从最后 `seq` 重连。注释帧不进入
  reducer，也不改变 `lastSeq`。
- 回合进行中叠加一条“数据帧看门狗”（45s）：字节看门狗只看传输字节，无法发现“订阅者已被 store 按慢客户端丢弃、连接只剩 keep-alive 注释”的僵尸形态——心跳不断喂狗，前端却再也收不到任何事件，正在运行的回合就此在画布上冻结。因此线程处于活跃回合（`turn_started`/`turn_steered` 置位，终态事件清除）时，只有**解析出的数据帧**才重置该看门狗；超时即主动断开并从最后 `seq` 重连，由 replay 补齐缺口。纯空闲线程（无活跃回合）只有心跳，不启用该看门狗，保持原字节判活行为。
- `child_completed` 实时到达时仅按稳定 `notification_id` 去重并显示一次性 toast，同时推进
  `childRunRevision`；reducer 不在主画布 item 投影保留完成摘要副本，也不维护“最近 50 张”卡片。
  持久查看统一由右侧 `ChildRunsPanel` 通过 child-runs API 重新读取。wire 中有界的
  `ThreadDetail.child_completions` 仍可供其他客户端使用，但当前 web 不将其恢复成主画布卡片。
- 子代理监督只由 `child_activity` 与 `child_completed` 驱动刷新，不能因文本 token delta 重复请求
  child-runs；排队与运行同样必须显示为用户文案。已决审批在当前页面会话中保留为历史卡片。用户输入
  gate 分两种形态：待回答的 pending 卡来自 `userInputs` 门控通道，钉在流末尾（这是活着的“请回答”入口）；
  已回答的问答是会话历史，渲染为提问回合内的一行折叠摘要（`AnsweredGateRow`，`ark-answeredgate`），
  锚点是 store 投影里的持久 `user_input` item（`user_input_resolved` 折叠为
  `keyed_item_id("user_input:{request_id}")`，`status: completed`，`text` 为提问、`detail` 为回答），
  因此回合结束后流末尾不再堆叠已回答卡，切换会话或刷新后（detail 重建投影）问答记录仍在原回合可见。
  流末尾只渲染 pending 的用户输入卡（`userInputCards` + pending 过滤）。刷新后的待处理 gate 只可来自
  服务端的 runtime 快照，web 不得从历史 item 猜测仍可提交的 gate id。
- detail 返回 `runtime` 时，web 在以 detail watermark 建立 SSE 前将 active turn 和其中的待处理 gate seed
  到本地状态；SSE terminal/resolved 事件仍是后续状态的权威更新。
- Code 右侧的 Plan、Changes、File、Terminal 均调用服务端 API。`GET /v1/threads/{id}/files` 仍是
  “本线程曾成功 `read` 的内容”历史投影；当前 File 面板使用
  `GET /v1/workspace/file?path=&thread_id=` 预览该线程生效 workspace 内的任意文件。后一个端点由后端
  按 thread workspace 覆盖优先、进程 lease 回落的顺序做真实路径 containment 与大小限制；浏览器不
  直接接触本地文件系统。子代理控制请求始终带父线程 query，不能只凭 child id 操作。
- Changes 同时展示活动工作区的服务端受限 Git 投影（暂存、未暂存、未跟踪）；切换请求仅提交路径给
  `arkd`，运行中存在父回合或子代理时服务端拒绝切换。浏览器不读取目录、Git 或未跟踪文件内容。
  差异文件行默认折叠，展开后渲染服务端 `patch` 字段的行级 diff（解析仅在展开时进行，面板打开
  不全量解析所有 patch）；`patch` 缺失（二进制或超出字节上限）时明确提示，浏览器不自行 diff。
- 会话标题下方的 `SessionStatsBar` 展示线程级用量；会话流不再有统计 footer。统计条由
  `GET /v1/threads/{id}/usage` 的整线程聚合播种，并折叠后续 live `usage` 事件，展示调用数、活跃
  总时长、tokens、缓存命中率、费用、最近速度、压缩摘要与错误。它不读取有界诊断分页
  `GET /v1/debug/llm-rounds`，因此调用数和耗时不会因分页截断变成下限。上下文占用只在输入框工具条的
  context ring 常驻展示：分母取所选模型真实 `context_window_tokens`（未声明时取运行时同源的 256K
  默认窗口），`compaction_threshold_tokens` 仅画为环内压缩刻度/警示线；两者都只供展示，不参与
  前端运行时决策。占用读数由 HTTP 快照的 `last_valid_prompt_tokens` 恢复，并由 live `usage` 事件更新；
  全零样本不覆盖已有读数。provider 限额缓存带 TTL 并定时重取，避免设置修改后永久陈旧。
- 模型输出的 markdown 渲染集中在 `shared/ui` 的 `Markdown`（marked + DOMPurify 消毒后注入）与
  `code-highlight`（文件预览高亮）：模型输出是不可信输入，任何进入 `dangerouslySetInnerHTML` 的
  HTML 必须先经 DOMPurify；这是展示层能力，不与 Rust 侧业务逻辑双实现。markdown 排版对齐主流
  agent 画布的阅读节奏（`markdown.css`）：标题带下划线分隔（h2）、块间距 0.4em（列表 0.6em，
  聊天卡片内的紧凑节奏）、表格
  `display: block` 横向滚动不挤压画布、代码块壳带控制阴影、任务列表勾选框用主题强调色。
  Markdown 宿主（`.ark-md`、`.ark-msg__body--prose`、`.ark-reply__body`）必须是
  `white-space: normal`：marked 在块级元素之间输出换行文本节点，pre-wrap 会把每个换行画成
  真实空行、叠加块 margin 后变成"每段之间一行空行"（纯文本宿主如用户气泡仍是 pre-wrap，
  见 `reply-prose-layout.test.ts` 的钉子）。
- 一键复制由 `shared/ui/copy` 统一提供：`copyText` 封装异步 Clipboard API 与 `execCommand` 回落，
  `CopyButton` 自带 pressed→copied 反馈并阻断祖先点击；消息气泡（用户/助手）、markdown 代码块与
  输入框（草稿、排队消息）均复用该组件，不各自实现复制。代码块的复制按钮由 markdown 渲染器产出，
  原始源码放在经转义的 `data-copy-source` 属性（`data-*` 在 DOMPurify 默认白名单内），`Markdown`
  用单个委托点击处理器服务整条消息的所有代码块——流式整块替换 DOM，逐块监听会泄漏或失效。助手
  气泡流式期间不提供复制（文本仍在增长），完成后才出现；气泡复制按钮默认隐藏，hover/键盘聚焦时
  浮现。所有复制 UI 文案走 i18n 键（`canvas.copy.*`）。
- Toast 是 `shared/ui/Toast` 独占的表面：组件内用 `useToast().push`；SSE 回调、轮询间隔等非 React
  调用方（回合终态通知）用 `pushToastProgrammatic`——`ToastProvider` 挂载时注册推送句柄，卸载时注销，
  provider 未挂载期间的推送静默丢弃（无表面可渲染），不排队。不得在 feature 内自建浮层通知。
- 消息行带身份标识（对齐主流 agent 画布）：助手元信息行前置 18px 圆形头像盘——hairline 圆环内嵌
  "ark" 字标，纯 CSS 无位图（`ark-msg__avatar`）；助手正文 `padding-left: 25px` 与头像后的作者名
  对齐，形成稳定的纵向导轨。用户气泡不渲染头像与「你」作者行：右侧独立气泡卡本身已表明说话者，
  冗余的作者标签是重复信息——气泡 meta 行退化为底部一行（时间 + 模型标签 + 复制按钮；模型标签
  `ark-msg__model` 渲染 `ThreadItem.model`，即该回合实际使用的 `provider:model`，缺失时省略），
  `canvas.author.you` 与 `canvas.author.youInitial` 键已随行为删除；助手字标 "ark" 是产品名，不进 i18n。
- 本回合的模型耗时显示在**过程卡头部行**、紧跟步数之后（`ark-process__duration` 排在
  `ark-process__meta` 之后，故整行读作“执行过程 · 47 步 · 耗时 10分45秒”）：实测耗时是描述"本回合
  做了什么"的那一行的成本，属于摘要行，不贴最右（头部 `::after` 箭头仍由自己的 `margin-left: auto`
  独占右端），也不放进"最终回复"答案卡的右下角操作行。运行中头部标题改读“正在执行中”
  （`canvas.process.running`，参考 Qoder），耗时显示为**纯时长**（`turnClock.span`，如
  “正在执行中 · 5 步 · 2分12秒”）；结束后标题回落“执行过程”、耗时恢复带“耗时”标签的形态
  （`turnClock.labelled`，时长文案契约的钉子不变）。**没有过程卡**的回合（单轮问答、无工具/思考行）
  把同一耗时交给回复卡标题行（`ark-reply__head` 内的 `ark-reply__duration`），耗时不会因为无可折叠
  步骤而消失；两种情况都不出现在 `ark-reply__foot` 操作行。数值与会话统计条总耗时同源（同一
  `state/rounds.ts` 折叠），因此"各轮次之和 = 统计条总耗时"成立；无已记录轮次时不显示。
  **数据源是 `GET /v1/threads/{id}/usage` 的 `turn_usage`，不是 `GET /v1/debug/llm-rounds`**：后者是
  有界诊断分页（`LLM_ROUNDS_MAX_LIMIT`），长会话会把整个回合挤出单页，于是该回合没有任何行可求和，
  头部把真实十几分钟的回合渲染成「122 步 · 耗时 11 秒」。聚合路由整份日志重放并逐回合折叠，
  与线程总计同一次折叠，所以回合耗时不再取决于历史有多长。`rounds.ts` 的 `appendRound` 也按回合
  累加进已有条目（不是一个回合一行地追加），否则实时 `usage` 事件会把同一回合算两遍。
- **回合头与统计条的"耗时"读数是活跃时长（`active_ms`），不是模型调用时间之和**：服务端
  `ActiveClock` 折叠出每个回合的挂钟时间并扣除等审批/用户输入的窗口；工具执行、HTTP 重试、
  恢复退避与子任务运行全部计入——体感时长（"跑了一两小时的会话"）应与该读数一致，旧口径
  （`duration_ms`，只含模型调用墙钟）仍保留为 `RoundTiming.durationMs` 供"模型时间"类读数使用。
  运行中回合由 `liveActiveMs` 按秒延伸：从回合最新事件锚点（`turnLastEventAt`，reducer 维护）加
  挂钟增量，挂起门禁出现后以最早 `requested_at` 为上限（时钟冻结在提问时刻，不倒退、不空转）；
  下一次 HTTP 刷新以服务端 `active_ms` 为准整体替换，两边永不漂移。运行中的时钟每秒 tick 由
  `components/use-clock-tick.ts` 独占（`LiveActivity` 的旧 `setInterval` 仍是独立实现，属另一读数）。
  旧运行时聚合无 `active_ms` 字段时读数为 `null`：不显示，而不是伪造 `0秒`。
- 用时文案是**本地化的单位拼接**（`formatDurationWords`，读作“耗时 1时2分3秒”），不是 `m:ss` 时钟：
  时/分/秒的单位词来自 i18n（`canvas.duration.hours|minutes|seconds`），由调用方经
  `hooks/use-duration-words.ts` 的 `useDurationFormat` 注入，因此 `session-stats.ts` 仍保持无 React、
  无硬编码用户文案（与 `shared/format.ts` 的 `formatRelative` 同构）。该 hook 是单位映射的唯一宿主：
  回合头部、回复卡标题行与会话统计条三处共用它，避免同一组文案键被抄三遍后漂移；它同时给出
  `labelled`（带“耗时”标签）与 `span`（纯时长）两种形态。前导零单位省略（`45秒`、`1时2分3秒`），
  但一旦出现小时就保留分钟位（`1时0分3秒`；折叠成 `1时3秒` 会读成另一段时长）；不足一秒仍显示
  `0秒`，负值与非有限值一律钳为 `0秒`，不漏出 `NaN秒`。
  函数刻意**不叫** `formatElapsed`：`state/live-activity.ts` 已用该名表示运行中 `m:ss` 读数，同名不同
  渲染正是本仓禁止的漂移。旧的 `m:ss` 辅助函数（原 `formatDuration`）在统计条改用同一文案后已无
  调用方，随改动删除，不保留死代码。**会话统计条的总耗时与回合耗时用同一文案**，因此“各轮次之和 =
  统计条总耗时”不仅数值相等、读起来也一致。
- 工具行按主流 agent（参考 Qoder）的**时间线活动行**呈现，不再是卡中卡：过程卡 body 左侧一条
  竖直导轨（`ark-process__body::before`），每行以一枚生命周期状态节点（`ark-tool__status`）落在
  导轨上——成功绿底对勾盘 / 失败红底叉盘 / 运行中强调色旋转弧（`--ok`/`--fail`/`--live`，节点自带
  不透明底盘遮住导轨线，避免线穿过图标）。节点词汇由工具行、实时活动行与运行中过程卡头共用。
  状态节点之后是工具族图标盘（`ark-tool__icon`，read/write/exec/search 四族 + wrench 回落）与
  **本地化族标签**（`ark-tool__family`，"终端命令/读取文件/编辑文件/搜索内容/工具调用"，族判定唯一
  宿主 `toolFamily`），原始 mono 工具名（`ark-tool__name`）弱化随行供精确核对；宽度契约：族标签与
  工具名（`headtoggle` flex:none）、判定词与耗时（nowrap）不被压缩，窄宽度只由 digest 的省略号吸收
  （实测窄画布下 headtoggle 可缩曾让 flex:none 子元素溢出叠印在 digest 上）；存在文件芯片时隐藏原始
  工具名（`head:has(filechip)`），把宽度让给芯片，避免芯片被挤成只剩 padding 的空壳；折叠头部保留单行
  参数摘要（`ark-tool__digest`，取首个参数 `key=value` 截断省略，只读 `formatToolArguments` 已格式化
  文本、不新增第二套序列化）。过程卡内的工具行去掉边框/底色/阴影与 per-row chevron（整行点击即折叠
  开关，状态节点占据导轨槽），call_id 诊断字段在时间线内隐藏、成功/失败判定退化为无 pill 的安静词；
  独立工具卡（子代理查看器）仍保留上方带边框的处理。写文件类工具卡（`write`/`edit`）在头部追加
  文件芯片（`ark-tool__filechip`，需求见
  [tool-file-preview](../requirements/tool-file-preview.md)）：路径提取的唯一宿主是
  `code-session/tool-file.ts` 纯函数（只认结构化 `path` 参数，`bash` 等间接写入不猜），
  点击芯片调用 `selectFile` 把该文件送入检查器 File 面板（`.md` 自动富文本渲染）并阻断
  折叠切换；`selectFile` 是显式"打开该文件"意图，检查器收起时同时展开（只切 Tab 不展开
  曾把收起态下的芯片变成死按钮）；因此头部拆为行容器 + 内层折叠按钮（`ark-tool__headtoggle`），
  避免 button 嵌套 button 的非法 DOM；无路径的工具卡渲染不变。
  思考行（`ReasoningBlock`）同为时间线条目：导轨上一枚强调色火花节点（`ark-reasoning__status`），
  摘要读作"已思考 · <单行预览>"（流式中为"思考中"，`canvas.reasoning.done`），标签单独成 span 并
  nowrap（裸文本节点作匿名 flex item 会在窄画布竖排），预览折叠空白且不改写
  原文，全文仍逐字留在折叠体；过程卡内去掉左侧强调竖线与 chevron，由导轨与火花节点取代。
  助手**正文**里的文件路径同样可点（需求见
  [prose-path-links](../requirements/prose-path-links.md)）：`renderMarkdown` 第二参
  接收本回合 write/edit 的路径白名单（`code-session/tool-file.ts` 的 `threadWritePaths`
  投影，去重保序），DOM 后处理只把**行内 code** 且经三级解析(精确 → 归一 → 唯一 basename,
  同名多文件不升级,解析结果还原白名单原路径)命中的片段升级为
  `ark-md-filechip`（协议前缀/家目录/空格路径不升级，`pre code` 围栏不参与，
  白名单为空零开销直通）；芯片是 renderer 自产 button（`data-md-file`），
  `Markdown` 组件的可选 `onFileChip` prop 经委托点击路由到 `selectFile`
  （同一收起展开语义），`shared/ui` 不反依 feature——白名单与接线都在
  `TurnBlock`（每回合一次 `useMemo`）与 `MessageBubble` 侧；子代理查看器
  传空白名单与 no-op 回调，不产生跨线程导航。
  正文里的链接**不跟随**：渲染层是单页应用（`useViewStore` 切 scene，无 URL 路由），顶层导航会把工作区
  替换成没有任何返回入口的页面，因此 `Markdown` 的委托点击（以及中键 `auxclick`）对 `a[href]` 先
  `preventDefault`，再交 `shared/desktop.ts#openExternal`：相对引用按当前文档解析，只放行 http(s)，
  `#fragment` 仍由浏览器处理。桌面壳经 `open_external` 交系统默认浏览器、并拒绝异源顶层导航
  （见 [ark-desktop](ark-desktop.md)）；纯浏览器回落新标签页。桥接失败静默丢弃，绝不回落到窗口内导航。
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
- 会话流按回合分组渲染：一回合内除最后一条有可见文本的助手消息外的全部行（reasoning、tool_call、
  tool_result 与中间叙述）收进一张可折叠"执行过程"卡，**卡内行保持真实
  事件顺序**——中间叙述与工具调用按发生顺序穿插，不再把所有工具抽到回复之前；运行中自动展开、结束
  后自动收起，用户开关即固定；用户消息与最终回复留在卡外，运行中所有行都在卡内（尚无"最终回复"）。
  开合状态在 `use-echo-safe-toggle.ts`（`用户固定 ?? 回合是否运行`，对齐主流 UI 的
  `userOverride ?? live` 形状）：固定值只从**真实用户 toggle** 写入——原生 `toggle` 事件对任何
  open 状态变化都会异步派发（规范如此），React 18 又给每个 `<details>` 单独挂原生监听，受控渲染
  与挂载本身都会产生回声事件；钩子以"上报状态 == React 上次渲染值"识别回声并忽略（等价于 Radix
  trigger 只报用户交互的契约），否则挂载回声会把运行中的卡钉死在展开态，回合结束永不收起。
  **固定值按阶段生效**：运行中用户可以自由开合，但该固定值属于"运行阶段"，回合一旦结束即失效，
  于是已结束回合必然回落到收起态，画布上留下的是回答而不是过程；结束后的开合仍会固定（回看历史
  轨迹是持久的主动选择）。
  回合结束后，被提升的最终回复渲染为独立答案卡（`FinalReplyCard`，`ark-reply`）：对勾圆环图标 +
  "最终回复"标题行 + 全宽分隔线 + markdown 正文（答案卡样式），卡底右下角的操作行
  （`ark-reply__foot`，hover 显现复制按钮）带生成时间（`formatClock(item.created_at)`，与用户
  气泡 footer 的时钟同款），流式/中断/失败状态与气泡一致；本回合耗时只在无过程卡时由
  `ark-reply__duration` 出现在标题行（见上文），不会进入 footer；过程卡内的中间
  叙述仍是普通助手气泡，避免卡中卡。
  分组规则在 `turn-sections.ts` 纯函数中，可独立单测。回合多于一个时在画布右缘提供逐回合跳转导航条；
  每个按钮取该回合用户输入，先 trim 并把换行折叠为空格，再显示前 15 个字符，超长追加 `...`。
  没有用户文本时回退纯回合号；`title` 与 `aria-label` 共用含该预览的本地化标签。
  过程卡内默认只完整展示最近 3 条活动行（`RECENT_ACTIVITY_ROW_LIMIT`，活动行 =
  `reasoning`/`tool_call`/`tool_result`），更早的步骤折叠为一行"展开较早的 N 步"摘要。折叠
  **运行中即生效**（不只在结束后）：长回合按当前活动扫描，而正在流式的那一行按构造必然是最新活动行，
  所以始终留在尾部可见。折叠是纯展示层：`splitProcessTrace` 把 trace 切成折叠点前后的 head/tail，
  行按事件位置留在原侧、不造假 item、不改 store，用户点摘要行即展开全量 trace。`error` 与
  `notice` 行永不折叠（错误按契约渲染在中断处）；运行中展开的状态属于运行阶段，回合结束随
  `use-echo-safe-toggle` 的阶段重置一并失效，过程卡整体收起、只留最终回答。
  会话标题行提供「评审改动」
  （`POST /v1/threads/{id}/review`，同一 202 回合信封，进度走线程事件流）；会话列表行提供「导出」
  （`GET /v1/threads/{id}/export` 快照下载，blob 触发浏览器保存）。
- 回合运行中，画布底部（注意力点）渲染一条实时活动行（`LiveActivity`）：镜像活跃回合最新的流式
  item——reasoning 显示"思考中 + 已耗时 + 尾迹（约 140 字符）"，tool_call 显示"运行 {tool}"，其余流式
  item 显示"正在生成"；数据完全来自已折叠的 item 投影（`activeTurnId` + 最新 streaming item），不新增
  wire 契约、不与 Rust 双实现。动机：过程卡位于助手消息上方，画布自动吸底时它在视野外，纯思考阶段
  画布毫无变化，健康流会被误读为卡死；该行让用户能实时感知推理方向并及时 steer。回合结束后随
  `turnState` 离开 running 而消失，历史仍归过程卡。行首的旋转弧复用工具行的状态节点
  （`ark-tool__status--live`），不再自持脉冲点。
- 后台子代理在会话列有两处常驻可见表面（参考 Qoder）：画布上方（统计条之下、滚动区之外）的
  `ChildActivityBanner` 列出本线程活跃子代理（脉冲点 + 「后台」tag + label + 实时时长
  “子 Agent 正在执行 2分49秒”，超过 3 个折叠为一条“另有 N 个”），点击任一行
  `openPanel('subagents')` 打开检查器子代理面板（终止/补充/恢复仍在那里）；composer 上方
  `ChildActivityHint` 显示“N 个任务在后台运行 · 你可以继续发消息”。两者共用
  `hooks/use-child-activity.ts` 的模块级缓存（按 thread id + `childRunRevision` 键，与
  `use-thread-skills` 同形）：并发挂载共享一次 `GET /v1/child-runs` 请求，只在
  `child_activity`/`child_completed` 推进 revision 时重读，不随 token delta 刷新；失败不清空
  已有快照，过期响应不得覆盖新 revision 的条目。时长由 `childRunDuration` + `useDurationFormat`
  每秒 tick（仅有活跃子代理时走表）。横幅不新增 wire 契约，也不在 store 维护第二份子代理列表。
- 错误只在**中断处**展示：终态 `turn_failed` 由服务端投影为该回合的持久 `error` 条目
  （见 [ark-store](ark-store.md)），以普通 item 行内联渲染在失败回合内，历史回放一致；渲染器不为
  `turn_failed`/`error`/`transient_retry` 维护独立的"最近错误"页脚通道或常驻画布底部的错误块。
  传输层与诊断类事件（`transient_retry` 等）不产生渲染行，只留事件日志。
- 输入框「技能」面板消费线程级技能视图（`GET /v1/threads/{id}/skills`，合并 base/extra 并展示加载
  警告数，「重新扫描」调用 refresh 路由）；未选择线程时回落全局 `GET /v1/skills`。显式 `/id` 提及
  仍是唯一激活方式，不新增技能运行时字段。面板本体是浮动 palette（`SkillPalette`），有两种模式：
  - **menu 模式**：`+` 菜单打开，自带搜索框、来源/legacy 标签与重扫入口；
  - **token 模式**：在草稿中键入 `/` 唤起（`mentionTokenAt` 识别光标下的 token，词边界开闭，
    路径/URL 不触发），草稿即过滤词、无搜索框；↑/↓ 换行、Enter 整词替换（`applySkill` 光标处
    替换 token 并清 token）、Esc 关闭，空匹配显示"没有匹配的技能"。两种模式互斥。
  线程技能集缓存在 `use-thread-skills.ts`（30s TTL，模块级共享），Composer palette 与消息画布的
  提及芯片共用同一份缓存。
- 已知未接入的路由：`attachments/diagnostics`（诊断）、
  `child-runs/{id}/cleanup`、`sessions/{id}/resume-thread`（定时任务结果已直接携带 thread_id 跳转）。
  `child-runs/{id}/thread|events` 已接入子代理执行过程查看器：展开行先以 `/thread` 快照播种，
  再以 `/events` 按 `since_seq` 增量轮询（活动子代理 1.5s 一跳，终态后停止），行复用父画布的
  StreamRow 组件；该状态是展开行组件的本地状态，不进全局 store，也不随父流 token 重渲染。
  子代理 transcript 与父画布回合卡应用同一折叠策略（`splitProcessTrace` +
  `RECENT_ACTIVITY_ROW_LIMIT`，当前 3）：只显示最近 3 条推理/工具行，更早的折叠进与父画布同款
  的“展开较早的 N 步”摘要行（`FoldedProcessRow`），点击可展开/收起，错误、通知与
  用户输入（委派文本/补充指令）行永不折叠，折叠只针对模型活动与中途叙述；
  折叠展开是展开行组件的本地 UI 状态，行展开（remount）后回到默认折叠视图。
- 子代理面板（`ChildRunsPanel`）按主流 agent 任务卡风格呈现：分组头（分支 glyph + 大写标题 +
  活跃计数）+ 每个子代理一张软卡（`ark-childcard`，与工具卡同视觉体系：chevron + 生命周期状态点
  `ChildRunStatusDot`（running 脉冲 / queued 闪烁 / 终态实色）+ 标签 + mono 元信息 + 状态 chip），
  展开后是带"执行过程"小标题与实时脉冲点的内嵌查看器。每行可展开：执行过程查看器（快照+增量
  轮询）+ 活动子代理的行内补充指令输入框（Enter 发送、Shift+Enter 换行）与终止按钮；终态行提供
  恢复按钮。加载/实时页脚用同一状态点词汇。列表仍由 `child_activity`/`child_completed` 驱动的
  `childRunRevision` 刷新。
- Code 使用 ARK Console shell（方案 A，当前实现）：46px 全局顶栏 + 26px 状态栏，主舞台在
  `code` 场景下是"线程列表 / 会话画布 / 常驻检查器"三列；两根侧列都可用键盘可操作的
  `ark-resizer` 拖宽（同一 `usePanelResizer` 原语按 `anchor` 分左/右锚定，右列拖其左缘、
  向左拖变宽；宽度存 `workbench-slice` 的 `leftWidth`/`rightWidth`，上下限 200–640px，
  默认 272/392px，检查器收起时不渲染 resizer；响应式断点只设 `max-width` 上限不覆盖拖后的
  宽度）；左列展开态右下角有「隐藏会话」收起按钮（`toggleLeft`），收起后经 26px 边轨按钮
  （`console-sidebar-tab`）恢复；顶栏承载工作区、Git head、运行态、
  主题切换、设置与 ⌘K 入口，状态栏承载连接、工作区、变更数与
  版本（上下文占用唯一的常驻展示是输入框工具条上的上下文环，状态栏不重复）。曾有的 52px 活动栏已
  删除：其场景切换全部有就近入口（会话列表新建、会话标题评审、⌘K、检查器 Tab），唯一独占的设置
  入口移入顶栏右侧；顶栏不展示模型（会话模型以输入框 pill 为准，顶栏曾显示的运行时默认模型易
  误读）。会话搜索和最近/
  归档切换只过滤已加载的 `ThreadSummary`，不新增搜索 API；会话列表按服务器权威顺序渲染
  （archived 优先级靠后、`last_event_at ?? updated_at` 降序），行内时间显示同一活动键；活跃会话的
  忙碌标记来自实时事件流（`turn_started`/终态事件驱动 `activeTurnId`），其余已加载行由 5 秒轮询
  `GET /v1/threads` 的 `running` 标记驱动；`applyRunningFlags` 保持纯语义（集合成员=忙碌、缺席=
  空闲），活跃回合在途时由 `use-thread-running.ts` 的 poll 把活跃 id 重新加入集合来防在途旧帧闪灭
  ——slice 内不得再放"缺席保留"分支，否则后台行永远无法靠服务端 idle 清除，转圈只能靠点开详情停。
  回合终态（`turn_completed`/`turn_failed`/`turn_aborted`）弹出一次 toast 通知（`turn-end-toast.ts`，
  文案带会话标题）：被订阅线程由 SSE 终态事件即时触发，后台线程与其它窗口的回合由列表轮询的
  busy→idle 边沿触发；两侧用终态事件的 durable `timestamp`（即行上的 `last_event_at`）做同一键去重，
  同一回合最多提示一次，之后重放/再轮询不再提示。桌面壳内窗口**失焦**时同一终态边沿还经
  `shared/desktop.ts` 的 `postSystemNotification`（`notify_session_event` 命令）发一条系统级通知
  （Windows toast / macOS 横幅），标题用 `thread.notify.os.*` 键、正文复用 toast 文案；焦点在投递时
  采样而非监听器维护，系统投递失败（返回 `false`）静默回退到已弹出的应用内 toast，点击通知不产生
  任何路由。浏览器（非桌面壳）中该调用降级为 no-op，行为与原先只有 toast 时一致。客户端只在**回合边界**推进活动键并随之重排：用户发消息
  （`turn_started`/`turn_steered`）与回合终态（`turn_completed`/`turn_failed`/`turn_aborted`）；
  delta、工具调用、gate 等回合内事件只推进行内 `last_seq` 水位，detail 播种（点击会话触发的
  `GET /v1/threads/{id}` 重载）也不同步行内活动键——否则被订阅（即被点开）的运行中会话会持续
  压过其它行，多会话并行时表现为"点开谁谁置顶"。行内活动键的陈旧由下一次列表刷新（服务器权威）
  纠偏。Plan、Changes、File、Terminal 是常驻
  检查器的四个 Tab（收起用 `hidden` 保挂载，面板草稿与服务端读模型不得因折叠丢失），不再使用带
  遮罩的浮动抽屉。`review`、`new`、`settings` 是同一 shell 下的全屏场景，由 `view-slice` 的 `view`
  选择；`commandOpen` 控制 ⌘K 命令面板。设置场景的返回不依赖记忆 ⌘K：顶栏右侧齿轮按钮按场景
  变义——工作台下是「设置」入口，settings 场景下变为「返回工作台」（`console.top.backToWorkbench`，
  undo 图标）；设置页自身 hero 行右缘还有常驻「返回工作台」按钮（`settings.backToWorkbench`，
  所有 section 可见），两处都直接 `setView('code')`。顶栏左侧品牌区是单个绿底 “Ark”
  词标（`.console-mark--word`），它同时是 home 控件：任意场景下点击都 `setView('code')`
  回到会话列表工作台（复用 `console.top.backToWorkbench` 作为可访问名）。`new` 场景（新任务页）的输入区**就是会话
  输入框本体**（`Composer`，`threadId = null` + `onPendingStart`）：工作空间 / 模型 / 权限 /
  推理强度 pill、附件与技能提及在会话存在前全部可用——pill 的选择写入页面的暂存覆盖
  （`stagedRuntime` / `onStagedChange`），附件仍走 `/v1/attachments` 上传，草稿存于 store 的
  保留键 `NEW_TASK_DRAFT_KEY`（与线程 UUID 命名空间隔离）。页面不再提供 Code/Plan/Review 模式
  卡片（变更审查仍从 ⌘K 命令面板进入）。发送是一次完整投递：
  `POST /v1/threads`（staged workspace 经服务端 validate 种为覆盖）→ `PATCH /v1/threads/{id}`
  （其余覆盖）→ 逐个关联附件 → `POST /v1/threads/{id}/turns`（首回合），投递成功才清草稿与
  暂存覆盖并 `setView('code')` 跳入工作台——新线程在 create 返回时即写入 store 并置为 active，
  因此跳转后左侧会话列表与画布直接打开该会话；失败全保留且**不切场景**（用户留在新任务页重试）；
  该路径不受 `streamStatus` 闸门约束（尚无流可订阅），仅 `sending` 阻塞。
- 会话列表按项目分组渲染（`thread-projects.ts`：先按会话**实际生效**工作空间聚合，
  无 override 会话回退运行时 active root，active 组置顶）。**项目组只在确有归属会话时渲染**：
  active root 组同样如此，删光某项目下的全部会话（🗑 批量清理或逐条删除）后该项目组随之消失，
  不留无法操作的空壳组。项目组头部提供两个随 hover/focus 显现的
  动作：➕ 在该项目下新建会话——把项目路径经 `POST /v1/threads` 的 `workspace` 字段提交，服务端经
  `WorkspaceService::validate` 校验后作为该线程的 workspace 覆盖种子，新会话即落在该组；🗑 批量清理
  该项目下已结束的会话——`DELETE /v1/threads?workspace=…` 一次调用，"已结束"由服务端权威判定
  （turn 注册表 + 持久日志折叠，重启后仍准确），运行中线程按行跳过并在响应 `skipped` 中回报
  `conflict`，前端以确认框 + toast 呈现结果，被删行经 `removeThread` 折叠出列表，下次刷新仍是权威。
  默认项目组（无 override 且无 active root）不提供这两个动作。
- 视觉样式只消费设计令牌：浅色"纸白"为默认主题（`preferences.ts` 默认 `theme: 'light'`），色板在
  `styles/tokens.css`，暗色"石墨"覆盖在 `styles/theme.css`，共享原语样式在 `styles/primitives.css`，
  由 `styles/global.css` 聚合引入；圆角与阴影分级都在令牌层定义，组件 CSS 出现字面 hex/rgba 色值
  属于破坏主题对等性的检视阻断项。ARK Console 语言是扁平、高对比的开发者控制台：无渐变玻璃、
  无氛围光晕、无浮动大圆角输入框；1px 硬边框定义层级，控件 5px / 卡片 7px / 面板 10px / 对话框
  12px 圆角，实心主行动按钮，用户消息是左对齐引用卡而非右对齐气泡。shell 专用的 `--console-*`
  令牌与 diff 语法色 `--tk-*` 同样在 tokens/theme 两侧定义。顶栏的工作区与 Git 信息使用无边框紧凑
  记录，运行态只保留状态点与文字；状态栏用间距分隔连接、工作区、变更、版本与命令入口，不再串联
  点号。会话列按项目 ledger 分组：项目标题只显示名称、计数与 hover/focus 动作，完整路径留在 title；
  会话选中态复用执行时间线的左侧 2px 状态轨，不使用绿色标题、ring 或阴影；行内 seq 不进入视觉层，
  fork/归档保留紧凑标记，kebab 仅在 hover/focus/选中时显现（无 hover 设备常显）。设置页使用带图标的
  分类导航与同样的左侧选中轨；内容区是 max 840px 的 ruled section，不再把每个分区做成浮起卡片，
  只读 key/value 与供应商列表按 hairline 行组织，嵌套编辑表单仍使用微底色凹井。共享字段采用 7px
  圆角、无投影、3px 焦点环；运行/成功/当前统一为绿色状态点，错误与警示只用于对应状态。640px 以下
  顶栏隐藏 Git head 与搜索文案、命令快捷键，只保留图标入口；会话菜单按 184px 高度预算向上翻转，
  避免底部的删除项落到视口外。设置页移动端保留横向导航，480px 以下返回按钮收为图标；二维码保持固定白色衬底，由两侧主题都定义的
  `--ark-im-qr-surface` 提供，避免暗色主题破坏可扫描性。
- 设置页供应商表单用单个「导入模型」入口合并目录查询与批量导入（按
  arkd 能力面收敛）：点击即拉取远端目录并弹出选择对话框，候选来自目录与粘贴输入（每行一个 id，服务无
  /models 时使用）；已存在的 id 标记且默认不勾选，确认后并入表单清单。导入时自动嗅探上下文窗口，识别不出
  用 256K 兜底（`DEFAULT_PROVIDER_CONTEXT_WINDOW_TOKENS`），默认 `model` 为空时由首个
  候选行回填，命中默认模型的行把元数据镜像到条目级字段。目录入口之外，清单区常驻一个手动「新增模型」
  行（输入 id 回车或点按钮落表）：导入对话框只有目录查询成功且非空才可达，没有该入口时，无 /models
  路由或查询失败的供应商无法手工登记任何模型。手动新增不假设远端元数据——新行不带窗口/识图值（unknown
  即 unknown），不套用 256K 导入兜底；默认 `model` 为空时首个手填 id 回填为默认模型，与默认模型或
  清单重复的输入是 no-op 且保留输入文本供修正。
- 保存（新增或编辑）只登记条目，`make_default` 恒为 false，不提供“保存后设为默认”勾选。默认供应商决定所有未显式覆盖的会话实际使用的模型，切默认必须是显式
  动作：列表行的「设为默认」或「设置 → 通用」的默认模型选择器；否则新增一个供应商就会把所有跟默认走
  的会话静默切到新通道。
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
  推理强度是输入框工具条上的即时选择器（`ComposerRuntimeControls`），打开菜单时重读
  `GET /v1/threads/{id}`，改动经 `PATCH /v1/threads/{id}` 的 `runtime` 覆盖立即保存；后端按字段合并、
  无清除语义，因此不可清除的字段不提供"未设置"选项，`reasoning_effort` 空串即"跟随默认"
  （协议映射见 [ark-llm](ark-llm.md)）。
  **pill 的标签一律来自"当前选中的会话"的覆盖，而不是打开菜单时才读的那份**：会话切换会重挂载输入框
  （`key={threadId}`），pill 的局部状态随之销毁，因此覆盖存在会话 store 的
  `threadRuntime[threadId]` 上（`state/threads-slice.ts`），唯一写入方是 `useThreadStream` 的
  detail 播种（`GET /v1/threads/{id}` 已带 `runtime`），pill 通过 `hooks/use-thread-runtime.ts` 只读它。
  这样一次切换只产生一次详情读取，且在菜单打开前标签就显示本会话的模型/权限/推理强度；
  未写入该键（尚未读到详情）时标签回落运行时默认，绝不残留上一个会话的值。
  同一组 pill 在新任务页（无会话）读写页面的暂存覆盖而非线程记录（见上方 `new` 场景条目），
  提交时机是任务启动而不是选择动作。
  工作空间、上下文结构和手动压缩也收进同一排工具条，不再有展开式详细面板：工作空间 pill 直接列出
  `GET /v1/workspace` 的 active / recent 项目，点选只经 `PATCH /v1/threads/{id}` 写成本会话覆盖；
  「新增项目空间」调用 `POST /v1/workspace/pick` 与 `POST /v1/workspace/validate` 后同样只 PATCH 当前
  线程，不调用全局 `POST /v1/workspace/switch`。活跃回合时禁用选择。全局 switch 仅由设置页和 Changes
  面板承载。上下文环弹层展示工具、系统提示、技能、消息、思考、检查点、其他的**结构占比**（渲染行数
  与字符数）；总量取最近一次有效权威 `usage` 事件的正 `prompt_tokens`，全零样本保留前值；无有效读数时
  显示未知而不是 0。环分母取所选模型真实 context window，压缩 threshold 只作环内刻度。
  token 估算的唯一宿主仍是 `ark-agent::TokenEstimator`，前端不重算
  token。弹层内「压缩上下文」直接调用
  `POST /v1/threads/{id}/compact`。共享 `ModelProviderMenu` 提供「供应商 → 模型」二级菜单与搜索，
  会话输入框用 `variant="flyout"`：供应商列表常驻、hover/focus 浮出独立滚动的模型卡，避免长模型
  清单挤占输入框；推理强度留在菜单 footer；所有选择打开时重读线程覆盖、写入仍走 `PATCH`。
  设置页使用生成契约提供同一全局权限，展示“永远询问”“仅工作区修改”“完全访问”三档。
- 会话标题下方有紧凑统计条（`SessionStatsBar`）：轮次与模型调用数来自 `GET /v1/threads/{id}/usage`，
  耗时是 store 的每轮耗时折叠（`state/rounds.ts`：HTTP 快照播种、live `usage` 事件追加，快照权威覆盖
  live 估计），即各次模型调用实测耗时之和，只统计模型回复时间，不含工具执行；并展示
  prompt/completion/total tokens、真实 `cache_hit_rate` 与 `cost_usd`。它不读取诊断分页，调用数不是
  `truncated` 下限；请求失败保留上次已验证数值而不伪造零。运行时版本、SSE 状态、数据目录等诊断信息不能占据
  主任务层级，必须退为状态详情或提示信息。
- 设置页的分类导航是 `web` 内本地状态，只决定既有 section 的可见性；每个 section 仍独立持有既有 hook，
  不把 provider、MCP 或 runtime 配置逻辑回流到 Code 会话状态或 `ark-agent`。
- 运行调优参数按运行时语义分组呈现（回合与 token / 工具保护 / 上下文与压缩 / 流式传输 /
  权限档位），wire 键保持 `TuningSettings` 不变；子代理文案同样对齐（启用子 Agent 委派、单会话并行
  子 Agent 上限）。`SubagentSection` 直接展示运行时 `auto_continue` 与 `default_background` 值，两者默认均为
  `true`；中英文说明明确主会话默认后台委派并自动汇总，关闭 auto-continue 后只保留通知、等待用户再次
  输入，不在前端另算默认策略。设置页新增只读「插件与能力」section：`GET /v1/runtime/capabilities`（能力 pills 与
  页面/面板目录计数）、`GET /v1/runtime/plugins`（插件贡献与失败/跳过警示）、`GET /v1/runtime/tools`
  （该路由在 Rust 侧仍是未建模 JSON，前端经 `web-only.ts` 的视图类型渲染，Rust 建模后应迁移到生成
  类型）。`/v1/runtime/tuning` 与 `/v1/runtime/model-providers` 是运行时的 501 占位路由，不是缺口。
- 输入框在活跃回合提供两种非中断动作：「追加」（steer，立即注入下一轮）与「排队」（本地暂存为
  chips，回合结束的 live→idle 转变触发按顺序自动发送）；排队消息可逐条移除，纯本地状态不进 wire。
  附件在活跃回合同样可添加（+ 按钮、textarea 粘贴图片、拖拽文件均可用）：steer 与发送共用同一
  「先 `POST /v1/threads/{id}/attachments` 关联、后投递」流程，steer 携带 `TurnRequest.attachment_ids`，
  服务端授权失败走命令错误提示且不清空 chips；「排队」只排文本、附件保留待下一次投递。粘贴/拖入的
  文件走与 + 按钮相同的上传管线（20 MiB 前端上限），剪贴板图片不内联进草稿文本。拖放放置目标是整个
  输入框容器（`.ark-composer__box`，不只 textarea）：`dragover` 仅在拖的是文件（`types` 含 `Files`）
  且可编辑（与 + 按钮同一 `canEditComposer` 门槛）时 `preventDefault` 并高亮；`dragleave` 用
  `contains(relatedTarget)` 区分在子元素间移动与真正离开，避免高亮闪烁；无文件的 drop 忽略，
  文本拖拽保留浏览器默认。桌面壳（ark-desktop）建窗口时调用
  `WebviewWindowBuilder::disable_drag_drop_handler()`：Windows 上 Tauri 默认以原生层接管 WebView2 的
  拖放来产生 `DragDropEvent`，会把 drop 事件吞在页面之外，不禁用则 HTML5 拖放（含本输入框）在桌面端
  完全失效；壳内无 `DragDropEvent` 消费者，禁用无副作用。
- 未发送草稿按线程保存在 store（`runtime-slice` 的 `drafts: Record<ThreadId, string>`，`setDraft`/
  `clearDraft`）：Composer 因 `key={threadId}` 在切换会话时重挂载，textarea 不持草稿，挂载时从
  store 恢复本线程草稿，输入即写回（受控 textarea）。发送成功仅清除与已发送文本一致的草稿条目，
  发送失败或被闸门拦截（`sending`、SSE 非 `open`）时草稿原样保留；排队动作把文本移入组件本地
  队列并清掉对应 store 草稿。新任务页的无会话 Composer 把草稿存在同一条 map 的保留键
  `NEW_TASK_DRAFT_KEY`（`'new-task'`，与线程 UUID 命名空间隔离），随同一条 localStorage 镜像
  持久化。草稿只存内存、不进 wire；`state/draft-storage.ts` 把同一映射镜像到
  localStorage（键 `ark.composer.drafts`，≤100 条、单条 ≤100k 字符，读写失败静默降级），`main.tsx`
  渲染前 `hydrateDrafts()` 恢复——渲染器整页重载（arkd 重启拖垮连接后外壳重开窗口、HMR、崩溃）
  对用户即"某个会话完成后其它输入框全被清空"，因此草稿必须跨页面存活；store 仍是运行期唯一真源，
  镜像只服务水合。清空以"删除条目"表示，不会在下次水合时复活旧内容。
- 附件经 `POST /v1/attachments`（Base64 JSON）上传后以 id 列表随 `TurnRequest.attachment_ids` 提交；
  上传失败走命令错误提示，已传 id 可在发送前移除（服务端无删除路由，孤儿附件由运行时清理策略负责）。
- 附件可视化预览由 `AttachmentThumb`（缩略图）与 `AttachmentLightbox`（全屏查看器）统一提供，消息画布
  与输入框复用同一组件，不得各自实现第二套预览。字节经 `GET /v1/attachments/{id}/content` 的认证 fetch
  取回并转 `blob:` URL（api 层按附件 id 缓存，`exportThread` 同款模式）——运行时所有路由在 Bearer
  Token 之后，`<img>` 的裸 URL 无法通过认证，且生产 CSP 允许 `blob:` 不允许跨源运行时 URL 进 `img-src`。
  图片渲染为可点击缩略图（cursor zoom-in），点击打开灯箱（scrim/×/Esc 关闭，z-index 在 toast 之下）；
  非图片与加载失败的记录回落为文件名 pill，不阻塞渲染。图片性判定在 `attachment-preview-model.ts`：
  画布按投递通道推断（只有图片形文件会走 `image`/`degraded`/`skipped`），输入框用 `File.type`，两者都
  缺时回落扩展名。降级/跳过/读取失败的投递记录外层加警示环（`--ark-warn-wash`），投递通道标签移入
  外层 title，不再内联展开。
- 消息画布用 `ThreadItem.attachments`（serde default，可为空）渲染用户消息下的附件预览：
  每个投递结果一枚缩略图或文件名 pill，`image`/`preview_text` 为中性样式，`degraded`/`skipped`/
  `unavailable` 加警示环并以投递通道 i18n 标签作 title 提示。字段由后端持久化，历史回放与 live 流
  渲染一致；运行时未发附件字段的旧 item 保持原样。
- 「技能」面板的消费面见上方"输入框「技能」面板"条目：线程级视图 + 全局回落，显式 `/id` 提及即激活
  是 arkd 的技能指定方式；不新增技能运行时字段。用户消息正文里的 `/id` 提及在渲染层识别
  （`skill-mention.ts`，仅当 token 与已加载技能 id 精确匹配时）并渲染成 `.ark-mention` 芯片，
  未知斜杠词（路径、URL）保持纯文本；识别只影响展示，复制按钮仍复制逐字原文。**UserBubble 必须从
  `StreamRow`/`TurnBlock` 拿到所属 threadId**（芯片依赖该线程的技能缓存），脱离流的独立渲染
  传 `null` 回落纯文本。输入框草稿侧，`DraftHighlight` 镜像层在 textarea 之下按同字体度量
  （`.ark-composer__editor-font`）重绘草稿：完整提及与光标下进行中 token 均渲染为轻量标记，
  镜像层 `pointer-events: none` 且文本透明，不影响输入/选择/IME/复制。skill 提及的视觉语言
  对齐主流 agent 的 `@file`/`/command` 引用：紧凑圆角矩形 chip（4px 圆角、4px 左右内边距、
  1px 清晰细边、淡紫底），文字与正文同字号同字重。额外宽度来自 `padding`，由等量负 `margin`
  抵消，使 token 的 advance 严格等于逐字文本——后续字形不漂移（实测 0.03px），行首 chip
  左边界正好落在内容起点（0px，不裁切）；进行中 token 只保留细边、无底色，表达"未解析"。
  `box-decoration-break: clone` 让跨行 chip 每段都有完整边框。消息气泡与输入框共用同一 chip，
  深浅主题由 `--ark-skill` / `--ark-skill-soft` 切换。
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
不得在 React 组件中引入 Tauri command、shell/plugin API 或 provider/Agent 分支。桌面壳的原生能力只经
`shared/desktop.ts` 的降级桥进入渲染层（当前：文件夹选择、保存文件、系统通知），组件直接 invoke
Tauri 命令属于检视阻断项；系统通知的发送时机与去重留在 web（`turn-end-toast.ts`），外壳只渲染文本。

独立 `plugin-code-ui` 尚未成为当前 workspace 模块。其未来迁移必须先建立需求设计，不能借桌面外壳绕过
`ark-ui` 注册总线。

## 定时任务管理

设置页「定时任务」section(需求见 [scheduled-tasks](../requirements/scheduled-tasks.md)):
只消费 `/v1/schedules` 的生成契约类型;节奏编辑、启停、删除、立即运行、最近结果展示与
"打开会话"跳转运行线程;全部文案走 i18n 消息键。客户端不做 next_run 计算,以后端返回为准。
表单的工作区不手填:复用 `POST /v1/workspace/pick` 弹出宿主文件夹选择,`POST /v1/workspace/validate`
归一化后写入草稿,取消与失败都保持原值;"清除"是回到全局默认的唯一入口。线程轨的「定时任务」
创建弹窗与设置页共用同一 `ScheduleForm`,头部提供"查看全部定时任务"链接,经
`setView('settings', section)` 一次性落到任务列表 section。

## 记忆与技能配置

设置页「记忆」section 消费 memory 路由的生成契约类型:列表、新增、编辑(text/tags/source)、删除与
存储诊断。运行时按预算把全部条目注入每回合,UI 不提供逐条启用开关——删除即停止注入
([memory-store](../requirements/memory-store.md));text 的 16KiB 上限在前端按 UTF-8 字节校验。
编辑时清空来源输入提交显式 `null`,这是 source 的唯一清空方式(键省略表示不修改,契约的双层
Option 语义);与 secret 类字段"空=保留"的约定相反,此处"空=清除"。

「Skills」section 在原只读列表之上提供发现配置(`skills.enabled` 总开关与 `extra_dirs` 额外根,
上限与运行时一致);该配置为 Hot 类,保存后在下一次技能扫描时生效,无需重启;被拒绝的 patch
原样展示。
