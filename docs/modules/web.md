# web 渲染层设计

## 职责与边界

`web/` 是 React + TypeScript Code 模式渲染层。它使用 `ark-contract` 生成类型和 typed client 渲染线程、
回合、工具、审批与设置；业务语义、路径归一化、脱敏、截断和事件到 item 的映射不应在前端重复实现。

## 约束

- 不手写 Rust 已拥有的 DTO、错误码 union 或类型别名；未建模的请求组合和纯 UI 状态仅放在
  `src/shared/contracts/web-only.ts`。
- 运行时请求集中在 feature API/client 层；组件不直接裸 `fetch('/v1/...')`。
- SSE 消费保存最后 `seq`，断线后使用 `since_seq` 恢复；页面必须能处理压缩产生的显式空洞。
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
  样本的 `prompt_tokens` 对 tuning `context_limit_tokens` 的占比，仅作展示，不参与运行时决策。
  限额缓存带 TTL 并定时重取（footer 常驻挂载、跨线程不重挂，一次性获取会在设置页修改后永久陈旧）。
  `compaction_completed` 以摘要形式显示最近一次压缩规模。
- 模型输出的 markdown 渲染集中在 `shared/ui` 的 `Markdown`（marked + DOMPurify 消毒后注入）与
  `code-highlight`（文件预览高亮）：模型输出是不可信输入，任何进入 `dangerouslySetInnerHTML` 的
  HTML 必须先经 DOMPurify；这是展示层能力，不与 Rust 侧业务逻辑双实现。
- 设置页的 tuning、provider 与 MCP 都使用生成契约；API key、MCP env/header 值只写不回读，编辑时的空
  secret 字段表示保留，不表示删除。
- 新增/编辑 provider 可显式查询模型目录，在用户选择后才把远端明确声明的 context window 与识图能力写入
  待保存表单；目录未声明时显示 unknown，不按模型名称猜测，也不自动提交配置。
- 组件状态按 feature 拆分，避免重建旧 `Workbench.tsx` 巨型组件；UI 变更应进行实际浏览器验证。
- Code 使用"任务历史 → 宽会话画布"的默认两栏结构，且无全局顶栏：侧栏底部的 footer 承载运行时
  状态（在线点、重检按钮）与 Code/设置视图切换，设置视图保留顶栏。会话搜索和最近/归档切换只过滤
  已加载的 `ThreadSummary`，不新增搜索 API；Plan、Changes、File、Terminal 是浮在画布之上的上下文
  抽屉（带遮罩，默认 `hidden` 隐藏、不占据主画布），不是挤压布局的第三列。抽屉收起用 `hidden` 而非
  条件渲染：组件保持挂载，面板草稿与服务端读模型不得因开关抽屉丢失。抽屉打开时移入焦点并支持
  Esc 关闭。
- 视觉样式只消费 `styles/global.css`（含 `theme.css`）的设计令牌；亮/暗两套色板、圆角与阴影分级都
  在令牌层定义，组件 CSS 出现字面 hex/rgba 色值属于破坏主题对等性的检视阻断项。实心主按钮亮色主题
  用近黑（`--ark-primary`）、暗色主题反转，意图色面（ok/warn/error 的 wash）同样由令牌提供。
- 设置页供应商表单支持批量导入模型（对齐 smart-ark 的导入对话框、按 arkd 能力面收敛）：候选来自
  已查询的模型目录与粘贴输入（每行一个 id，服务无 /models 时使用）；已存在的 id 标记且默认不勾选，
  确认后并入表单清单，默认 `model` 为空时由首个候选行回填。清单作为芯片列表展示、可逐项移除，
  保存时整体提交：表单恒发完整列表（省略 = 保留，仅限 MCP 等部分编辑客户端；显式空列表 = 清空，
  与 secret 类"空=保留"语义不同）。
- 会话运行时覆盖（工作空间/供应商/模型/推理强度/权限档位）停靠在输入框上方（`.ark-composer-stack`），
  不再占据会话标题区；模型输入带 datalist，建议项来自所选供应商的默认模型与导入清单；推理强度经
  `runtime.reasoning_effort` 覆盖（`off|low|medium|high`，协议映射见 [ark-llm](ark-llm.md)）。覆盖只影响
  该会话的下一回合，
  运行中的回合不变，未设置字段回退到全局默认值。设置页使用生成契约提供同一全局权限，展示“永远询问”“仅工作区修改”“完全访问”三档。
- 会话标题、真实回合数和运行态可以作为紧凑上下文显示；运行时版本、SSE 状态、数据目录等诊断信息不能占据
  主任务层级，必须退为状态详情或提示信息。
- 设置页的分类导航是 `web` 内本地状态，只决定既有 section 的可见性；每个 section 仍独立持有既有 hook，
  不把 provider、MCP 或 runtime 配置逻辑回流到 Code 会话状态或 `ark-agent`。
- 输入框在活跃回合提供两种非中断动作：「追加」（steer，立即注入下一轮）与「排队」（本地暂存为
  chips，回合结束的 live→idle 转变触发按顺序自动发送）；排队消息可逐条移除，纯本地状态不进 wire。
- 附件经 `POST /v1/attachments`（Base64 JSON）上传后以 id 列表随 `TurnRequest.attachment_ids` 提交；
  上传失败走命令错误提示，已传 id 可在发送前移除（服务端无删除路由，孤儿附件由运行时清理策略负责）。
- 「技能」面板列出 `GET /v1/skills` 的发现结果，点击把 `/id` 插入输入框——显式提及即激活，是
  arkd 的技能指定方式；不新增技能运行时字段。
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
